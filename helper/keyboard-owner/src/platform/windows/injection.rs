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

mod input;
mod markers;
mod paste;
mod replay;

use input::*;
#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
pub(super) use markers::TEST_PHYSICAL_MARKER;
pub(super) use markers::{InjectionMarkers, classify};
#[cfg(all(test, feature = "windows-native-test-input"))]
pub(super) use paste::test_paste_initial_outcome;
#[cfg(all(test, feature = "windows-native-test-input"))]
use paste::*;
pub(super) use paste::{
    PasteCleanup, PasteInjectionOutcome, inject_paste_initial_if, retry_paste_cleanup,
};
#[cfg(all(test, feature = "windows-native-test-input"))]
use replay::build_replay_inputs;
pub(super) use replay::{
    cleanup_menu_neutralization, inject_cleanup, inject_modifier_releases, inject_replay,
    neutralize_menu,
};

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
