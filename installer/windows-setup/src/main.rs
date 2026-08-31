#![cfg_attr(windows, windows_subsystem = "windows")]

mod package;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
fn main() {
    std::process::exit(windows::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Talking Quill installer bootstrap is Windows-only.");
    std::process::exit(64);
}
