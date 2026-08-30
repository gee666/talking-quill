#![cfg_attr(not(feature = "windows-native-test-input"), allow(dead_code))]

use std::ptr::null_mut;

#[cfg(not(test))]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SendInput;
use windows_sys::Win32::{
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        KEYEVENTF_SCANCODE, VK_CONTROL, VK_V,
    },
    UI::WindowsAndMessaging::LLKHF_INJECTED,
};

use crate::platform::{PasteFailure, PasteResult};
use talking_quill_keyboard_core::transactional::{
    CleanupBatch, InputSource, JOURNAL_CAPACITY, MenuModifiers, NativeKey, PhysicalPhase,
    ReplayBatch, ReplayRecord,
};

#[cfg(all(feature = "windows-native-test-input", not(debug_assertions)))]
compile_error!("windows-native-test-input cannot be included in a release helper");

// The low-level injected flag is required as well as an unpredictable marker;
// a physical record can never opt into a helper class merely by carrying the
// same integer in KBDLLHOOKSTRUCT. One set is created for each hook install and
// shared by callback classification and every injection path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InjectionMarkers {
    replay: usize,
    paste: usize,
    dummy: usize,
}

impl InjectionMarkers {
    pub(super) fn generate() -> Option<Self> {
        for _ in 0..4 {
            let mut bytes = [0_u8; size_of::<usize>()];
            // SAFETY: the system-preferred CSPRNG accepts a null algorithm handle
            // and bytes is a fully writable, correctly sized output buffer.
            let status = unsafe {
                BCryptGenRandom(
                    null_mut(),
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            };
            if status != 0 {
                return None;
            }
            let markers = Self::from_seed(usize::from_le_bytes(bytes));
            if markers.replay > 3
                && !reserved_test_marker(markers.replay)
                && !reserved_test_marker(markers.paste)
                && !reserved_test_marker(markers.dummy)
            {
                return Some(markers);
            }
        }
        None
    }

    const fn from_seed(seed: usize) -> Self {
        let base = seed & !3;
        Self {
            replay: base | 1,
            paste: base | 2,
            dummy: base | 3,
        }
    }
}

// Deliberately absent from ordinary/release helpers. The integration harness
// duplicates this build-contract value when it emits test-only SendInput.
#[cfg(all(
    feature = "windows-native-test-input",
    debug_assertions,
    target_pointer_width = "64"
))]
pub(super) const TEST_PHYSICAL_MARKER: usize = 0x5451_5445_5354_0008;
#[cfg(all(
    feature = "windows-native-test-input",
    debug_assertions,
    target_pointer_width = "32"
))]
pub(super) const TEST_PHYSICAL_MARKER: usize = 0x5445_5308;

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
const fn reserved_test_marker(marker: usize) -> bool {
    marker == TEST_PHYSICAL_MARKER
}

#[cfg(not(all(feature = "windows-native-test-input", debug_assertions)))]
const fn reserved_test_marker(_marker: usize) -> bool {
    false
}

const PASTE_INPUT_COUNT: usize = 4;

#[derive(Clone, Copy)]
pub(super) struct PasteInjectionOutcome {
    pub result: PasteResult,
    pub initial_accepted: usize,
    pub pending_cleanup: PasteCleanup,
}

#[derive(Clone, Copy, Default)]
pub(super) struct PasteCleanup {
    inputs: [INPUT; 2],
    len: u8,
}

impl PasteCleanup {
    pub(super) const fn is_empty(self) -> bool {
        self.len == 0
    }

    const fn len(self) -> usize {
        self.len as usize
    }

    fn as_slice(&self) -> &[INPUT] {
        &self.inputs[..self.len()]
    }

    fn suffix(self, accepted: usize) -> Self {
        let accepted = accepted.min(self.len());
        let mut remaining = Self::default();
        for input in self.as_slice()[accepted..].iter().copied() {
            remaining.inputs[remaining.len()] = input;
            remaining.len += 1;
        }
        remaining
    }

    pub(super) fn without_virtual_key(self, removed: u16) -> Self {
        let mut retained = Self::default();
        for input in self.as_slice().iter().copied() {
            // SAFETY: PasteCleanup contains only INPUT_KEYBOARD records built
            // by this module.
            let key = unsafe { input.Anonymous.ki.wVk };
            if key != removed {
                retained.inputs[retained.len()] = input;
                retained.len += 1;
            }
        }
        retained
    }

    fn partition_blocked(self, ctrl_down: bool, v_down: bool) -> (Self, Self) {
        let mut ready = Self::default();
        let mut blocked = Self::default();
        for input in self.as_slice().iter().copied() {
            // SAFETY: PasteCleanup contains only INPUT_KEYBOARD records built
            // by this module.
            let key = unsafe { input.Anonymous.ki.wVk };
            let destination = if (key == VK_CONTROL && ctrl_down) || (key == VK_V && v_down) {
                &mut blocked
            } else {
                &mut ready
            };
            destination.inputs[destination.len()] = input;
            destination.len += 1;
        }
        (ready, blocked)
    }

    fn followed_by(self, suffix: Self) -> Self {
        let mut combined = Self::default();
        for input in self
            .as_slice()
            .iter()
            .chain(suffix.as_slice().iter())
            .copied()
        {
            combined.inputs[combined.len()] = input;
            combined.len += 1;
        }
        combined
    }
}

#[must_use]
pub(super) const fn classify(markers: InjectionMarkers, flags: u32, marker: usize) -> InputSource {
    if flags & LLKHF_INJECTED == 0 {
        return InputSource::Physical;
    }
    if marker == markers.replay {
        InputSource::HelperReplay
    } else if marker == markers.paste {
        InputSource::HelperPaste
    } else if marker == markers.dummy {
        InputSource::HelperDummy
    } else {
        classify_test_or_external(marker)
    }
}

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
const fn classify_test_or_external(marker: usize) -> InputSource {
    if marker == TEST_PHYSICAL_MARKER {
        InputSource::test_physical()
    } else {
        InputSource::External
    }
}

#[cfg(not(all(feature = "windows-native-test-input", debug_assertions)))]
const fn classify_test_or_external(_marker: usize) -> InputSource {
    InputSource::External
}

/// Replays the captured journal in exactly one bounded SendInput call.
pub(super) fn inject_replay(markers: InjectionMarkers, batch: ReplayBatch) -> usize {
    let mut inputs = [INPUT::default(); JOURNAL_CAPACITY];
    if !build_replay_inputs(batch.entries(), markers.replay, &mut inputs) {
        return 0;
    }
    send_once(&inputs[..batch.len()])
}

/// Attempts every retained helper-owned release in one bounded cleanup call.
pub(super) fn inject_cleanup(markers: InjectionMarkers, batch: CleanupBatch) -> usize {
    let mut inputs = [INPUT::default(); JOURNAL_CAPACITY];
    if !build_replay_inputs(batch.entries(), markers.replay, &mut inputs) {
        return 0;
    }
    send_once(&inputs[..batch.len()])
}

pub(super) fn inject_modifier_releases(markers: InjectionMarkers, releases: &[NativeKey]) -> usize {
    let mut inputs = [INPUT::default(); 4];
    if releases.len() > inputs.len() {
        return 0;
    }
    for (slot, native) in inputs.iter_mut().zip(releases.iter().copied()) {
        let Ok(scan_code) = u16::try_from(native.scan_code) else {
            return 0;
        };
        *slot = if scan_code == 0 {
            if native.virtual_key == 0 {
                return 0;
            }
            virtual_key_replay_input(native, PhysicalPhase::Up, markers.replay)
        } else {
            scan_code_input(native, scan_code, PhysicalPhase::Up, markers.replay)
        };
    }
    send_once(&inputs[..releases.len()])
}

/// Marks an Alt/Win cycle as used with an unassigned VK 0xFF pair.
///
/// This behavioral strategy is adapted from Microsoft PowerToys Keyboard
/// Manager (MIT, Copyright Microsoft Corporation). The Rust implementation is
/// local; the preserved license is in `docs/attribution/powertoys-mit.txt` and
/// the generated `THIRD_PARTY_NOTICES.txt`.
pub(super) fn neutralize_menu(markers: InjectionMarkers, _modifiers: MenuModifiers) -> usize {
    const VK_DUMMY: u16 = 0x00FF;
    send_once(&[
        virtual_key_input(VK_DUMMY, false, markers.dummy),
        virtual_key_input(VK_DUMMY, true, markers.dummy),
    ])
}

pub(super) fn cleanup_menu_neutralization(
    markers: InjectionMarkers,
    _modifiers: MenuModifiers,
) -> usize {
    const VK_DUMMY: u16 = 0x00FF;
    send_once(&[virtual_key_input(VK_DUMMY, true, markers.dummy)])
}

/// Injects one fully helper-owned semantic Ctrl+V chord. A partial accepted
/// prefix is never retried. V-down is the irreversible paste boundary. The
/// exact accepted-prefix cleanup is returned without issuing a second
/// SendInput call so the caller can publish commitment first.
pub(super) fn inject_paste_initial_if(
    markers: InjectionMarkers,
    pre_send_clipboard_check: impl FnOnce() -> bool,
) -> Option<PasteInjectionOutcome> {
    let plan = paste_inputs(markers);
    if !pre_send_clipboard_check() {
        return None;
    }
    #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
    if let Ok(value) = std::env::var("TALKING_QUILL_WINDOWS_TEST_POST_CAS_STALL_MS")
        && let Ok(milliseconds) = value.parse::<u64>()
        && milliseconds <= 3_000
    {
        std::thread::sleep(std::time::Duration::from_millis(milliseconds));
    }
    // No production target, modifier, clipboard conversion, allocation, or
    // other Win32 operation may be inserted between the sequence check and
    // SendInput. The debug-only stall above proves bounded post-CAS handling.
    Some(paste_injection_outcome(send_once(&plan), markers))
}

#[cfg(test)]
fn inject_paste_initial_with(
    markers: InjectionMarkers,
    mut submit: impl FnMut(&[INPUT]) -> usize,
) -> PasteInjectionOutcome {
    let plan = paste_inputs(markers);
    paste_injection_outcome(submit(&plan), markers)
}

fn paste_injection_outcome(accepted: usize, markers: InjectionMarkers) -> PasteInjectionOutcome {
    PasteInjectionOutcome {
        result: paste_result(accepted),
        initial_accepted: accepted,
        pending_cleanup: paste_cleanup_inputs(accepted, markers),
    }
}

#[cfg(test)]
pub(super) fn test_paste_initial_outcome(
    markers: InjectionMarkers,
    accepted: usize,
) -> PasteInjectionOutcome {
    PasteInjectionOutcome {
        result: paste_result(accepted),
        initial_accepted: accepted,
        pending_cleanup: paste_cleanup_inputs(accepted, markers),
    }
}

const fn paste_result(accepted: usize) -> PasteResult {
    PasteResult {
        submitted: accepted >= 2,
        reason: if accepted < 2 {
            Some(PasteFailure::OsRejected)
        } else {
            None
        },
    }
}

pub(super) fn retry_paste_cleanup(
    cleanup: PasteCleanup,
    physical_ctrl_down: bool,
    physical_v_down: bool,
) -> (usize, PasteCleanup) {
    let (ready, blocked) = cleanup.partition_blocked(physical_ctrl_down, physical_v_down);
    let accepted = send_once(ready.as_slice());
    (accepted, blocked.followed_by(ready.suffix(accepted)))
}

fn build_replay_inputs(records: &[ReplayRecord], marker: usize, output: &mut [INPUT]) -> bool {
    if records.len() > output.len() {
        return false;
    }
    for (slot, record) in output.iter_mut().zip(records.iter().copied()) {
        let Ok(scan_code) = u16::try_from(record.native.scan_code) else {
            return false;
        };
        *slot = if scan_code == 0 {
            if record.native.virtual_key == 0 {
                return false;
            }
            virtual_key_replay_input(record.native, record.phase, marker)
        } else {
            scan_code_input(record.native, scan_code, record.phase, marker)
        };
    }
    true
}

fn scan_code_input(
    native: NativeKey,
    scan_code: u16,
    phase: PhysicalPhase,
    marker: usize,
) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if native.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if phase == PhysicalPhase::Up {
        flags |= KEYEVENTF_KEYUP;
    }
    keyboard_input(0, scan_code, flags, marker)
}

fn virtual_key_replay_input(native: NativeKey, phase: PhysicalPhase, marker: usize) -> INPUT {
    let mut flags = 0;
    if native.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if phase == PhysicalPhase::Up {
        flags |= KEYEVENTF_KEYUP;
    }
    keyboard_input(native.virtual_key, 0, flags, marker)
}

fn virtual_key_input(virtual_key: u16, key_up: bool, marker: usize) -> INPUT {
    keyboard_input(
        virtual_key,
        0,
        if key_up { KEYEVENTF_KEYUP } else { 0 },
        marker,
    )
}

fn keyboard_input(virtual_key: u16, scan_code: u16, flags: u32, marker: usize) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: marker,
            },
        },
    }
}

fn paste_inputs(markers: InjectionMarkers) -> [INPUT; PASTE_INPUT_COUNT] {
    [
        virtual_key_input(VK_CONTROL, false, markers.paste),
        virtual_key_input(VK_V, false, markers.paste),
        virtual_key_input(VK_V, true, markers.paste),
        virtual_key_input(VK_CONTROL, true, markers.paste),
    ]
}

fn paste_cleanup_inputs(accepted: usize, markers: InjectionMarkers) -> PasteCleanup {
    let mut ctrl_owned = false;
    let mut v_owned = false;
    for index in 0..accepted.min(PASTE_INPUT_COUNT) {
        match index {
            0 => ctrl_owned = true,
            1 => v_owned = true,
            2 => v_owned = false,
            3 => ctrl_owned = false,
            _ => unreachable!(),
        }
    }
    let mut cleanup = PasteCleanup::default();
    if v_owned {
        cleanup.inputs[cleanup.len()] = virtual_key_input(VK_V, true, markers.paste);
        cleanup.len += 1;
    }
    if ctrl_owned {
        cleanup.inputs[cleanup.len()] = virtual_key_input(VK_CONTROL, true, markers.paste);
        cleanup.len += 1;
    }
    cleanup
}

#[cfg(not(test))]
fn send_once(inputs: &[INPUT]) -> usize {
    if inputs.is_empty() {
        return 0;
    }
    // SAFETY: the slice contains initialized INPUT records and the structure
    // size exactly matches the User32 ABI selected by windows-sys.
    let accepted = unsafe {
        SendInput(
            u32::try_from(inputs.len()).expect("bounded input count fits u32"),
            inputs.as_ptr(),
            i32::try_from(size_of::<INPUT>()).expect("INPUT size fits i32"),
        )
    };
    usize::try_from(accepted).expect("SendInput count fits usize")
}

#[cfg(test)]
fn send_once(inputs: &[INPUT]) -> usize {
    // Unit tests validate reducer and adapter invariants without emitting
    // native input into the developer or CI desktop. The separately gated
    // native harness exercises the real SendInput implementation.
    inputs.len()
}

#[cfg(all(test, feature = "windows-native-test-input"))]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED};

    use super::*;
    use talking_quill_keyboard_core::{ActivationKey, transactional::KeyIdentity};

    const fn markers() -> InjectionMarkers {
        InjectionMarkers::from_seed(0x1234_5678)
    }

    const REPLAY_MARKER: usize = markers().replay;
    const PASTE_MARKER: usize = markers().paste;
    const DUMMY_MARKER: usize = markers().dummy;
    const VK_DUMMY: u16 = 0x00FF;

    fn key_input(input: INPUT) -> KEYBDINPUT {
        // SAFETY: every tested INPUT was constructed with INPUT_KEYBOARD.
        unsafe { input.Anonymous.ki }
    }

    #[test]
    fn every_injection_class_is_distinct_and_requires_the_injected_flag() {
        let set = markers();
        let values = [set.replay, set.paste, set.dummy];
        for (index, marker) in values.iter().copied().enumerate() {
            assert!(values[index + 1..].iter().all(|other| *other != marker));
            assert_eq!(classify(set, 0, marker), InputSource::Physical);
        }
        assert_eq!(
            classify(set, LLKHF_INJECTED, set.replay),
            InputSource::HelperReplay
        );
        assert_eq!(
            classify(set, LLKHF_INJECTED, set.paste),
            InputSource::HelperPaste
        );
        assert_eq!(
            classify(set, LLKHF_INJECTED, set.dummy),
            InputSource::HelperDummy
        );
        assert_eq!(classify(set, LLKHF_INJECTED, 0), InputSource::External);
        assert!(InputSource::External.is_physical());
        assert_eq!(
            classify(set, LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED, 0),
            InputSource::External
        );
    }

    #[test]
    fn marker_sets_are_install_scoped_and_never_cross_classify() {
        let first = InjectionMarkers::from_seed(0x1234_5678);
        let second = InjectionMarkers::from_seed(0x8765_4320);
        assert_ne!(first, second);
        for marker in [first.replay, first.paste, first.dummy] {
            assert_eq!(
                classify(second, LLKHF_INJECTED, marker),
                InputSource::External
            );
        }
    }

    #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
    #[test]
    fn debug_harness_marker_is_the_only_injected_physical_source() {
        assert_eq!(
            classify(markers(), LLKHF_INJECTED, TEST_PHYSICAL_MARKER),
            InputSource::test_physical()
        );
        assert_eq!(
            classify(markers(), 0, TEST_PHYSICAL_MARKER),
            InputSource::Physical
        );
    }

    #[test]
    fn replay_records_use_scan_code_extended_keyup_and_replay_marker() {
        let records = [
            ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::A),
                native: NativeKey {
                    virtual_key: 0x41,
                    scan_code: 0x1E,
                    extended: false,
                    platform_flags: 0xFFFF,
                },
                phase: PhysicalPhase::Repeat,
                observed_at_ms: 10,
            },
            ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::A),
                native: NativeKey {
                    virtual_key: 0x41,
                    scan_code: 0x1E,
                    extended: true,
                    platform_flags: 0,
                },
                phase: PhysicalPhase::Up,
                observed_at_ms: 11,
            },
        ];
        let mut output = [INPUT::default(); 2];
        assert!(build_replay_inputs(&records, REPLAY_MARKER, &mut output));
        let down = key_input(output[0]);
        assert_eq!(down.wVk, 0);
        assert_eq!(down.wScan, 0x1E);
        assert_eq!(down.dwFlags, KEYEVENTF_SCANCODE);
        assert_eq!(down.dwExtraInfo, REPLAY_MARKER);
        let up = key_input(output[1]);
        assert_eq!(
            up.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
    }

    #[test]
    fn zero_scan_code_replays_the_exact_virtual_key_shape() {
        let record = ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::A),
            native: NativeKey {
                virtual_key: 0x41,
                scan_code: 0,
                extended: true,
                platform_flags: 0,
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 0,
        };
        let mut output = [INPUT::default(); 1];
        assert!(build_replay_inputs(&[record], REPLAY_MARKER, &mut output));
        let replay = key_input(output[0]);
        assert_eq!(replay.wVk, 0x41);
        assert_eq!(replay.wScan, 0);
        assert_eq!(replay.dwFlags, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP);
        assert_eq!(replay.dwExtraInfo, REPLAY_MARKER);
    }

    #[test]
    fn unrepresentable_replay_identity_is_never_submitted() {
        for native in [
            NativeKey {
                virtual_key: 0,
                scan_code: 0,
                ..NativeKey::default()
            },
            NativeKey {
                virtual_key: 0x41,
                scan_code: u32::from(u16::MAX) + 1,
                ..NativeKey::default()
            },
        ] {
            let record = ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::A),
                native,
                phase: PhysicalPhase::Down,
                observed_at_ms: 0,
            };
            let mut output = [INPUT::default(); 1];
            assert!(!build_replay_inputs(&[record], REPLAY_MARKER, &mut output));
        }
    }

    #[test]
    fn paste_partial_prefix_cleanup_releases_only_accepted_owned_downs() {
        let expected_keys: [&[u16]; 5] =
            [&[], &[VK_CONTROL], &[VK_V, VK_CONTROL], &[VK_CONTROL], &[]];
        for (accepted, expected) in expected_keys.iter().enumerate() {
            let cleanup = paste_cleanup_inputs(accepted, markers());
            let keys = cleanup
                .as_slice()
                .iter()
                .copied()
                .map(key_input)
                .map(|input| {
                    assert_eq!(input.dwExtraInfo, PASTE_MARKER);
                    assert_eq!(input.dwFlags, KEYEVENTF_KEYUP);
                    assert_eq!(input.wScan, 0);
                    input.wVk
                })
                .collect::<Vec<_>>();
            assert_eq!(keys, *expected);
        }
    }

    #[test]
    fn foreground_physical_up_discharges_matching_cleanup_only() {
        let cleanup = paste_cleanup_inputs(2, markers());
        let without_v = cleanup.without_virtual_key(VK_V);
        assert_eq!(without_v.len(), 1);
        assert_eq!(key_input(without_v.as_slice()[0]).wVk, VK_CONTROL);
        let without_ctrl = cleanup.without_virtual_key(VK_CONTROL);
        assert_eq!(without_ctrl.len(), 1);
        assert_eq!(key_input(without_ctrl.as_slice()[0]).wVk, VK_V);
    }

    #[test]
    fn paste_cleanup_defers_releases_that_collide_with_physical_keys() {
        let cleanup = paste_cleanup_inputs(2, markers());
        let (ready, blocked) = cleanup.partition_blocked(true, false);
        assert_eq!(ready.len(), 1);
        assert_eq!(blocked.len(), 1);
        assert_eq!(key_input(ready.as_slice()[0]).wVk, VK_V);
        assert_eq!(key_input(blocked.as_slice()[0]).wVk, VK_CONTROL);

        let (ready, blocked) = cleanup.partition_blocked(false, true);
        assert_eq!(ready.len(), 1);
        assert_eq!(blocked.len(), 1);
        assert_eq!(key_input(ready.as_slice()[0]).wVk, VK_CONTROL);
        assert_eq!(key_input(blocked.as_slice()[0]).wVk, VK_V);

        let (ready, blocked) = cleanup.partition_blocked(true, true);
        assert!(ready.is_empty());
        assert_eq!(blocked.len(), 2);
    }

    #[test]
    fn paste_plan_is_one_fully_owned_layout_semantic_chord() {
        let inputs = paste_inputs(markers()).map(key_input);
        assert_eq!(
            inputs.map(|input| (input.wVk, input.wScan, input.dwFlags, input.dwExtraInfo)),
            [
                (VK_CONTROL, 0, 0, PASTE_MARKER),
                (VK_V, 0, 0, PASTE_MARKER),
                (VK_V, 0, KEYEVENTF_KEYUP, PASTE_MARKER),
                (VK_CONTROL, 0, KEYEVENTF_KEYUP, PASTE_MARKER),
            ]
        );
    }

    #[test]
    fn clipboard_sequence_failure_prevents_the_only_send_input_call() {
        let mut checked = 0;
        assert!(
            inject_paste_initial_if(markers(), || {
                checked += 1;
                false
            })
            .is_none()
        );
        assert_eq!(checked, 1);
    }

    #[test]
    fn initial_paste_boundary_returns_cleanup_without_a_second_send() {
        for initial in 0..=PASTE_INPUT_COUNT {
            let mut calls = 0;
            let outcome = inject_paste_initial_with(markers(), |inputs| {
                calls += 1;
                assert_eq!(inputs.len(), PASTE_INPUT_COUNT);
                initial
            });
            assert_eq!(calls, 1);
            assert_eq!(outcome.initial_accepted, initial);
            assert_eq!(outcome.result.submitted, initial >= 2);
            assert_eq!(
                outcome.pending_cleanup.len(),
                paste_cleanup_inputs(initial, markers()).len()
            );
        }
    }

    #[test]
    fn accepted_v_down_is_conservatively_committed_and_cleanup_suffix_is_retained() {
        for accepted in 0..=PASTE_INPUT_COUNT {
            assert_eq!(paste_result(accepted).submitted, accepted >= 2);
            let cleanup = paste_cleanup_inputs(accepted, markers());
            for cleanup_accepted in 0..=cleanup.len() {
                let remaining = cleanup.suffix(cleanup_accepted);
                assert_eq!(
                    remaining.len(),
                    cleanup.len() - cleanup_accepted,
                    "initial {accepted}, cleanup {cleanup_accepted}"
                );
            }
        }
    }

    #[test]
    fn dummy_pair_uses_unassigned_vk_and_a_dedicated_marker() {
        for (input, key_up) in [
            (virtual_key_input(VK_DUMMY, false, DUMMY_MARKER), false),
            (virtual_key_input(VK_DUMMY, true, DUMMY_MARKER), true),
        ] {
            let input = key_input(input);
            assert_eq!(input.wVk, VK_DUMMY);
            assert_eq!(input.wScan, 0);
            assert_eq!(input.dwExtraInfo, DUMMY_MARKER);
            assert_eq!(input.dwFlags & KEYEVENTF_KEYUP != 0, key_up);
        }
        let menu = MenuModifiers {
            alt: true,
            meta: false,
        };
        assert_eq!(neutralize_menu(markers(), menu), 2);
        assert_eq!(cleanup_menu_neutralization(markers(), menu), 1);
    }
}
