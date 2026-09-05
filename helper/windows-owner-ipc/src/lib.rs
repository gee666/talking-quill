//! Shared unprivileged Windows gateway and keyboard-owner IPC types.
//!
//! The active Windows product uses two same-interactive-user processes and an
//! adjacent owner selected by the gateway. This library defines shared identities,
//! creates session-scoped pipes, and verifies kernel peer facts and installed
//! release metadata. It has no executable, elevation, or keyboard implementation.

pub mod channel;
#[cfg(windows)]
pub mod endpoint;
pub mod image_policy;
#[cfg(windows)]
pub mod installed;
#[cfg(windows)]
pub mod peer;
#[cfg(windows)]
mod token;
