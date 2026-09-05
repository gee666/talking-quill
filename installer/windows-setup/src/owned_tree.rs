use std::{io, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OwnedTreeError {
    #[error("invalid expected directory identity")]
    InvalidIdentity,
    #[cfg(not(any(windows, target_os = "macos")))]
    #[error("identity-bound deletion is unsupported on this platform")]
    Unsupported,
    #[error("owned directory identity did not match")]
    IdentityMismatch,
    #[error("owned directory crosses a filesystem or mount boundary")]
    MountBoundary,
    #[error("owned directory path is invalid")]
    InvalidPath,
    #[error("identity-bound deletion failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(windows)]
pub fn owned_tree_identity(path: &Path) -> Result<String, OwnedTreeError> {
    platform::identity(path)
}

#[cfg(all(windows, feature = "stale-schema2-cleanup"))]
pub fn retained_directory_names(
    handle: std::os::windows::io::RawHandle,
) -> Result<Vec<String>, OwnedTreeError> {
    platform::retained_names(handle)
}

pub fn remove_owned_tree(path: &Path, expected_identity: &str) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove(path, device, inode)
}

fn parse_identity(value: &str) -> Result<(u64, u64), OwnedTreeError> {
    let (device, inode) = value
        .split_once(':')
        .ok_or(OwnedTreeError::InvalidIdentity)?;
    if device.is_empty() || inode.is_empty() || value.matches(':').count() != 1 {
        return Err(OwnedTreeError::InvalidIdentity);
    }
    Ok((
        device
            .parse()
            .map_err(|_| OwnedTreeError::InvalidIdentity)?,
        inode.parse().map_err(|_| OwnedTreeError::InvalidIdentity)?,
    ))
}

#[cfg(windows)]
#[path = "owned_tree/windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "owned_tree/macos.rs"]
mod platform;

#[cfg(all(test, windows))]
mod tests;

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::OwnedTreeError;
    use std::path::Path;
    pub fn remove(_: &Path, _: u64, _: u64) -> Result<(), OwnedTreeError> {
        Err(OwnedTreeError::Unsupported)
    }
}
