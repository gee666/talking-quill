#[cfg(windows)]
fn main() {
    use std::mem::size_of;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    };

    #[used]
    static SOURCE_COMMIT: &str = concat!(
        "TALKING_QUILL_SOURCE_COMMIT=",
        env!("TALKING_QUILL_SOURCE_COMMIT")
    );
    #[used]
    static SOURCE_TREE: &str = concat!(
        "TALKING_QUILL_SOURCE_TREE=",
        env!("TALKING_QUILL_SOURCE_TREE")
    );
    std::hint::black_box(SOURCE_COMMIT);
    std::hint::black_box(SOURCE_TREE);

    const MARKER: usize = 0x5451_5445_5354_0008;
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0x87,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MARKER,
            },
        },
    };
    let mut input = [key(0), key(KEYEVENTF_KEYUP)];
    // SAFETY: the initialized fixed array remains valid during this synchronous call.
    let sent = unsafe {
        SendInput(
            input.len() as u32,
            input.as_mut_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != input.len() as u32 {
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The acceptance synthetic sender is Windows-only.");
    std::process::exit(1);
}
