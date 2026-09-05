#[cfg(not(windows))]
fn main() {
    eprintln!("Windows acceptance broker requires Windows");
    std::process::exit(79);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("Windows acceptance broker failed: {error}");
        std::process::exit(79);
    }
}

#[cfg(windows)]
#[path = "broker/mod.rs"]
mod windows;
