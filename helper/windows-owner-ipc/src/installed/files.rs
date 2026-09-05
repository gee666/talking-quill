//! Locked release file reads and recovery launcher inspection.

use super::InstalledReleaseError;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

pub(super) fn read_locked_manifest(path: &Path) -> Result<Vec<u8>, InstalledReleaseError> {
    read_locked_file(path, MAX_MANIFEST_BYTES)
}

pub(super) fn read_locked_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<u8>, InstalledReleaseError> {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(InstalledReleaseError::Manifest);
    }
    // SAFETY: CreateFileW returned a valid, uniquely owned handle.
    let mut file: std::fs::File = unsafe { OwnedHandle::from_raw_handle(handle) }.into();
    let length = file
        .metadata()
        .map_err(|_| InstalledReleaseError::Manifest)?
        .len();
    if !(1..=max_bytes).contains(&length) {
        return Err(InstalledReleaseError::Manifest);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| InstalledReleaseError::Manifest)?;
    if bytes.len() as u64 != length {
        return Err(InstalledReleaseError::Manifest);
    }
    Ok(bytes)
}

pub(super) fn pe_architecture(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 64 || bytes.get(..2) != Some(b"MZ") {
        return None;
    }
    let pe = u32::from_le_bytes(bytes.get(60..64)?.try_into().ok()?) as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return None;
    }
    match u16::from_le_bytes(bytes.get(pe + 4..pe + 6)?.try_into().ok()?) {
        0x8664 => Some("x64"),
        0xaa64 => Some("arm64"),
        _ => None,
    }
}

pub(super) fn source_marker_prefix(kind: &[u8]) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(21 + kind.len());
    prefix.extend_from_slice(b"TALKING_QUILL_");
    prefix.extend_from_slice(b"SOURCE_");
    prefix.extend_from_slice(kind);
    prefix.push(b'=');
    prefix
}

pub(super) fn has_source_identity(bytes: &[u8], commit: &str, tree: &str) -> bool {
    [(b"COMMIT".as_slice(), commit), (b"TREE".as_slice(), tree)]
        .iter()
        .all(|(kind, expected)| {
            let prefix = source_marker_prefix(kind);
            let offsets: Vec<usize> = bytes
                .windows(prefix.len())
                .enumerate()
                .filter(|(_, window)| *window == prefix)
                .map(|(offset, _)| offset)
                .collect();
            offsets.len() == 1
                && bytes.get(offsets[0] + prefix.len()..offsets[0] + prefix.len() + 40)
                    == Some(expected.as_bytes())
        })
}
