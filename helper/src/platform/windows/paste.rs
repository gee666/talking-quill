use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
};

use super::hook::{INJECTED_MARKER, key_is_down};
use crate::platform::{PasteFailure, PasteResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PasteModifiers {
    ctrl: bool,
    shift: bool,
    alt: bool,
    left_win: bool,
    right_win: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PasteKeyInput {
    key: u16,
    key_up: bool,
}

impl PasteKeyInput {
    const fn down(key: u16) -> Self {
        Self { key, key_up: false }
    }

    const fn up(key: u16) -> Self {
        Self { key, key_up: true }
    }
}

const PASTE_WITH_CTRL: [PasteKeyInput; 4] = [
    PasteKeyInput::down(VK_CONTROL),
    PasteKeyInput::down(VK_V),
    PasteKeyInput::up(VK_V),
    PasteKeyInput::up(VK_CONTROL),
];
const PASTE_WITH_PREHELD_CTRL: [PasteKeyInput; 2] =
    [PasteKeyInput::down(VK_V), PasteKeyInput::up(VK_V)];

fn paste_plan(modifiers: PasteModifiers) -> Result<&'static [PasteKeyInput], PasteFailure> {
    if modifiers.shift || modifiers.alt || modifiers.left_win || modifiers.right_win {
        Err(PasteFailure::ConflictingModifiers)
    } else if modifiers.ctrl {
        Ok(&PASTE_WITH_PREHELD_CTRL)
    } else {
        Ok(&PASTE_WITH_CTRL)
    }
}

fn paste_cleanup_plan(inputs: &[PasteKeyInput], accepted: usize) -> Vec<PasteKeyInput> {
    let mut ctrl_owned = false;
    let mut v_owned = false;
    for input in inputs.iter().take(accepted) {
        match (input.key, input.key_up) {
            (VK_CONTROL, false) => ctrl_owned = true,
            (VK_CONTROL, true) => ctrl_owned = false,
            (VK_V, false) => v_owned = true,
            (VK_V, true) => v_owned = false,
            _ => {}
        }
    }

    let mut cleanup = Vec::with_capacity(2);
    if v_owned {
        cleanup.push(PasteKeyInput::up(VK_V));
    }
    if ctrl_owned {
        cleanup.push(PasteKeyInput::up(VK_CONTROL));
    }
    cleanup
}

fn paste_outcome(inputs: &[PasteKeyInput], accepted: usize) -> (PasteResult, Vec<PasteKeyInput>) {
    if accepted == inputs.len() {
        (
            PasteResult {
                submitted: true,
                reason: None,
            },
            Vec::new(),
        )
    } else {
        (
            PasteResult {
                submitted: false,
                reason: Some(PasteFailure::OsRejected),
            },
            paste_cleanup_plan(inputs, accepted),
        )
    }
}

pub(super) fn inject_paste() -> PasteResult {
    let modifiers = PasteModifiers {
        ctrl: key_is_down(VK_CONTROL),
        shift: key_is_down(VK_SHIFT),
        alt: key_is_down(VK_MENU),
        left_win: key_is_down(VK_LWIN),
        right_win: key_is_down(VK_RWIN),
    };
    let plan = match paste_plan(modifiers) {
        Ok(plan) => plan,
        Err(reason) => {
            return PasteResult {
                submitted: false,
                reason: Some(reason),
            };
        }
    };
    let inputs = plan
        .iter()
        .map(|input| keyboard_input(input.key, input.flags()))
        .collect::<Vec<_>>();
    // SAFETY: the slice contains initialized INPUT values and its byte size is
    // exactly the structure size expected by SendInput.
    let sent = unsafe {
        SendInput(
            u32::try_from(inputs.len()).expect("fixed input count fits u32"),
            inputs.as_ptr(),
            i32::try_from(size_of::<INPUT>()).expect("INPUT size fits i32"),
        )
    };
    let accepted = usize::try_from(sent).expect("SendInput count fits usize");
    let (result, cleanup) = paste_outcome(plan, accepted);
    for release in cleanup {
        let release = keyboard_input(release.key, release.flags());
        // Attempt each helper-owned release separately so one rejected cleanup
        // does not prevent the remaining release from being attempted.
        // SAFETY: `release` is one initialized INPUT value.
        unsafe {
            SendInput(
                1,
                &raw const release,
                i32::try_from(size_of::<INPUT>()).expect("INPUT size fits i32"),
            );
        }
    }

    // Cleanup cannot turn the original partial submission into success.
    result
}

impl PasteKeyInput {
    const fn flags(self) -> u32 {
        if self.key_up { KEYEVENTF_KEYUP } else { 0 }
    }
}

fn keyboard_input(key: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_MARKER,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(mask: u8) -> PasteModifiers {
        PasteModifiers {
            ctrl: mask & 0b00001 != 0,
            shift: mask & 0b00010 != 0,
            alt: mask & 0b00100 != 0,
            left_win: mask & 0b01000 != 0,
            right_win: mask & 0b10000 != 0,
        }
    }

    #[test]
    fn paste_plan_rejects_every_conflicting_modifier_combination() {
        for mask in 0_u8..0b100000 {
            let expected = if mask & 0b11110 != 0 {
                Err(PasteFailure::ConflictingModifiers)
            } else if mask & 0b00001 != 0 {
                Ok(PASTE_WITH_PREHELD_CTRL.as_slice())
            } else {
                Ok(PASTE_WITH_CTRL.as_slice())
            };
            assert_eq!(paste_plan(modifiers(mask)), expected, "mask {mask:05b}");
        }
    }

    #[test]
    fn paste_plan_only_owns_ctrl_when_it_was_not_preheld() {
        assert_eq!(
            paste_plan(modifiers(0)),
            Ok([
                PasteKeyInput::down(VK_CONTROL),
                PasteKeyInput::down(VK_V),
                PasteKeyInput::up(VK_V),
                PasteKeyInput::up(VK_CONTROL),
            ]
            .as_slice())
        );
        assert_eq!(
            paste_plan(modifiers(1)),
            Ok([PasteKeyInput::down(VK_V), PasteKeyInput::up(VK_V)].as_slice())
        );
    }

    #[test]
    fn cleanup_covers_every_partial_count_when_ctrl_is_helper_owned() {
        let expected = [
            vec![],
            vec![PasteKeyInput::up(VK_CONTROL)],
            vec![PasteKeyInput::up(VK_V), PasteKeyInput::up(VK_CONTROL)],
            vec![PasteKeyInput::up(VK_CONTROL)],
        ];

        for (accepted, expected) in expected.iter().enumerate() {
            let (result, cleanup) = paste_outcome(&PASTE_WITH_CTRL, accepted);
            assert_eq!(
                result,
                PasteResult {
                    submitted: false,
                    reason: Some(PasteFailure::OsRejected),
                },
                "accepted {accepted}"
            );
            assert_eq!(
                cleanup.as_slice(),
                expected.as_slice(),
                "accepted {accepted}"
            );
        }
    }

    #[test]
    fn cleanup_covers_every_partial_count_when_ctrl_was_preheld() {
        let expected = [vec![], vec![PasteKeyInput::up(VK_V)]];

        for (accepted, expected) in expected.iter().enumerate() {
            let (result, cleanup) = paste_outcome(&PASTE_WITH_PREHELD_CTRL, accepted);
            assert_eq!(
                result,
                PasteResult {
                    submitted: false,
                    reason: Some(PasteFailure::OsRejected),
                },
                "accepted {accepted}"
            );
            assert_eq!(
                cleanup.as_slice(),
                expected.as_slice(),
                "accepted {accepted}"
            );
        }
    }

    #[test]
    fn paste_and_cleanup_plans_never_emit_keys_other_than_ctrl_or_v() {
        for plan in [&PASTE_WITH_CTRL[..], &PASTE_WITH_PREHELD_CTRL[..]] {
            for accepted in 0..=plan.len() {
                for input in plan.iter().chain(&paste_cleanup_plan(plan, accepted)) {
                    assert!(matches!(input.key, VK_CONTROL | VK_V));
                }
            }
        }
    }
}
