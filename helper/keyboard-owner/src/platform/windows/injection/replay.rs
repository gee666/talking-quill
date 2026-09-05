//! Bounded journal replay, modifier releases, and menu neutralization.
use super::*;

/// Replays the captured journal in exactly one bounded SendInput call.
pub(in crate::platform::windows) fn inject_replay(
    markers: InjectionMarkers,
    batch: ReplayBatch,
) -> usize {
    let mut inputs = [INPUT::default(); JOURNAL_CAPACITY];
    if !build_replay_inputs(batch.entries(), markers.replay, &mut inputs) {
        return 0;
    }
    send_once(&inputs[..batch.len()])
}

/// Attempts every retained helper-owned release in one bounded cleanup call.
pub(in crate::platform::windows) fn inject_cleanup(
    markers: InjectionMarkers,
    batch: CleanupBatch,
) -> usize {
    let mut inputs = [INPUT::default(); JOURNAL_CAPACITY];
    if !build_replay_inputs(batch.entries(), markers.replay, &mut inputs) {
        return 0;
    }
    send_once(&inputs[..batch.len()])
}

pub(in crate::platform::windows) fn inject_modifier_releases(
    markers: InjectionMarkers,
    releases: &[NativeKey],
) -> usize {
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
pub(in crate::platform::windows) fn neutralize_menu(
    markers: InjectionMarkers,
    _modifiers: MenuModifiers,
) -> usize {
    const VK_DUMMY: u16 = 0x00FF;
    send_once(&[
        virtual_key_input(VK_DUMMY, false, markers.dummy),
        virtual_key_input(VK_DUMMY, true, markers.dummy),
    ])
}

pub(in crate::platform::windows) fn cleanup_menu_neutralization(
    markers: InjectionMarkers,
    _modifiers: MenuModifiers,
) -> usize {
    const VK_DUMMY: u16 = 0x00FF;
    send_once(&[virtual_key_input(VK_DUMMY, true, markers.dummy)])
}

pub(super) fn build_replay_inputs(
    records: &[ReplayRecord],
    marker: usize,
    output: &mut [INPUT],
) -> bool {
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

pub(super) fn scan_code_input(
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

pub(super) fn virtual_key_replay_input(
    native: NativeKey,
    phase: PhysicalPhase,
    marker: usize,
) -> INPUT {
    let mut flags = 0;
    if native.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if phase == PhysicalPhase::Up {
        flags |= KEYEVENTF_KEYUP;
    }
    keyboard_input(native.virtual_key, 0, flags, marker)
}
