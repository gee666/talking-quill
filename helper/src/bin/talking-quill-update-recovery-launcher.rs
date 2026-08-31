#![cfg_attr(windows, windows_subsystem = "windows")]

#[used]
static RECOVERY_LAUNCHER_MARKER: &str =
    "TALKING_QUILL_WINDOWS_UPDATE_RECOVERY_LAUNCHER=MEDIUM_SELF_ELEVATING_V1";

#[cfg(windows)]
fn main() {
    talking_quill_helper::retain_source_identity();
    std::hint::black_box(RECOVERY_LAUNCHER_MARKER);
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if arguments.len() != 1 {
        std::process::exit(64);
    }
    std::process::exit(
        talking_quill_helper::windows_update::run_recovery_launcher_argument(&arguments[0]),
    );
}

#[cfg(not(windows))]
fn main() {
    std::process::exit(64);
}
