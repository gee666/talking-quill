//! Semantic paste submission and accepted-prefix cleanup ownership.
use super::*;

pub(super) const PASTE_INPUT_COUNT: usize = 4;

#[derive(Clone, Copy)]
pub(in crate::platform::windows) struct PasteInjectionOutcome {
    pub result: PasteResult,
    pub initial_accepted: usize,
    pub pending_cleanup: PasteCleanup,
}

#[derive(Clone, Copy, Default)]
pub(in crate::platform::windows) struct PasteCleanup {
    inputs: [INPUT; 2],
    len: u8,
}

impl PasteCleanup {
    pub(in crate::platform::windows) const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub(super) const fn len(self) -> usize {
        self.len as usize
    }

    pub(super) fn as_slice(&self) -> &[INPUT] {
        &self.inputs[..self.len()]
    }

    pub(super) fn suffix(self, accepted: usize) -> Self {
        let accepted = accepted.min(self.len());
        let mut remaining = Self::default();
        for input in self.as_slice()[accepted..].iter().copied() {
            remaining.inputs[remaining.len()] = input;
            remaining.len += 1;
        }
        remaining
    }

    pub(in crate::platform::windows) fn without_virtual_key(self, removed: u16) -> Self {
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

    pub(super) fn partition_blocked(self, ctrl_down: bool, v_down: bool) -> (Self, Self) {
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

/// Injects one fully helper-owned semantic Ctrl+V chord. A partial accepted
/// prefix is never retried. V-down is the irreversible paste boundary. The
/// exact accepted-prefix cleanup is returned without issuing a second
/// SendInput call so the caller can publish commitment first.
pub(in crate::platform::windows) fn inject_paste_initial_if(
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

#[cfg(all(test, feature = "windows-native-test-input"))]
pub(super) fn inject_paste_initial_with(
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

#[cfg(all(test, feature = "windows-native-test-input"))]
pub(in crate::platform::windows) fn test_paste_initial_outcome(
    markers: InjectionMarkers,
    accepted: usize,
) -> PasteInjectionOutcome {
    paste_injection_outcome(accepted, markers)
}

pub(super) const fn paste_result(accepted: usize) -> PasteResult {
    PasteResult {
        submitted: accepted >= 2,
        reason: if accepted < 2 {
            Some(PasteFailure::OsRejected)
        } else {
            None
        },
    }
}

pub(in crate::platform::windows) fn retry_paste_cleanup(
    cleanup: PasteCleanup,
    physical_ctrl_down: bool,
    physical_v_down: bool,
) -> (usize, PasteCleanup) {
    let (ready, blocked) = cleanup.partition_blocked(physical_ctrl_down, physical_v_down);
    let accepted = send_once(ready.as_slice());
    (accepted, blocked.followed_by(ready.suffix(accepted)))
}

pub(super) fn paste_inputs(markers: InjectionMarkers) -> [INPUT; PASTE_INPUT_COUNT] {
    [
        virtual_key_input(VK_CONTROL, false, markers.paste),
        virtual_key_input(VK_V, false, markers.paste),
        virtual_key_input(VK_V, true, markers.paste),
        virtual_key_input(VK_CONTROL, true, markers.paste),
    ]
}

pub(super) fn paste_cleanup_inputs(accepted: usize, markers: InjectionMarkers) -> PasteCleanup {
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
