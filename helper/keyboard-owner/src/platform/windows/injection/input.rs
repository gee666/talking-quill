//! Keyboard INPUT construction and the single native submission boundary.
use super::*;

pub(super) fn virtual_key_input(virtual_key: u16, key_up: bool, marker: usize) -> INPUT {
    keyboard_input(
        virtual_key,
        0,
        if key_up { KEYEVENTF_KEYUP } else { 0 },
        marker,
    )
}

pub(super) fn keyboard_input(virtual_key: u16, scan_code: u16, flags: u32, marker: usize) -> INPUT {
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

#[cfg(not(test))]
pub(super) fn send_once(inputs: &[INPUT]) -> usize {
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
pub(super) fn send_once(inputs: &[INPUT]) -> usize {
    // Unit tests validate reducer and adapter invariants without emitting
    // native input into the developer or CI desktop. The separately gated
    // native harness exercises the real SendInput implementation.
    inputs.len()
}
