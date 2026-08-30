//! Authenticated keyboard-owner client for the permanent no-suppression gateway.

pub mod client;
pub mod handshake;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod maintenance_cli;
pub mod platform_client;
#[cfg(windows)]
pub mod windows;
