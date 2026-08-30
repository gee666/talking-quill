use std::{char::decode_utf16, ptr::null_mut, slice, time::Instant};

use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    Foundation::HGLOBAL,
    System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, GetClipboardSequenceNumber,
            IsClipboardFormatAvailable, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    },
};

use crate::platform::{ClipboardTextHash, MAX_INSERTION_UTF8_BYTES};

const CF_UNICODETEXT: u32 = 13;

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: this guard is created only after OpenClipboard succeeds on
        // the current owner thread and closes that one matching acquisition.
        unsafe { CloseClipboard() };
    }
}

struct GlobalLockGuard(HGLOBAL);

impl Drop for GlobalLockGuard {
    fn drop(&mut self) {
        // SAFETY: this handle was successfully locked by GlobalLock. A zero
        // return can also mean the lock count reached zero, so cleanup ignores
        // it as required by the Win32 contract.
        unsafe { GlobalUnlock(self.0) };
    }
}

/// Samples one stable CF_UNICODETEXT revision and verifies the SHA-256 of its
/// canonical UTF-8 encoding without mutating any clipboard format.
pub(super) fn matching_text_sequence(
    expected: ClipboardTextHash,
    deadline: Instant,
) -> Option<u32> {
    // SAFETY: clipboard APIs have no caller-owned pointer preconditions here.
    unsafe {
        if Instant::now() >= deadline {
            return None;
        }
        let before = GetClipboardSequenceNumber();
        if before == 0 || OpenClipboard(null_mut()) == 0 {
            return None;
        }
        let clipboard_guard = ClipboardGuard;
        if GetClipboardSequenceNumber() != before || IsClipboardFormatAvailable(CF_UNICODETEXT) == 0
        {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT) as HGLOBAL;
        if handle.is_null() {
            return None;
        }
        let allocation_bytes = GlobalSize(handle);
        if allocation_bytes < size_of::<u16>() || !allocation_bytes.is_multiple_of(size_of::<u16>())
        {
            return None;
        }
        let text = GlobalLock(handle).cast::<u16>();
        if text.is_null() {
            return None;
        }
        let lock_guard = GlobalLockGuard(handle);
        let allocation_units = allocation_bytes / size_of::<u16>();
        let scan_units = allocation_units.min(MAX_INSERTION_UTF8_BYTES.saturating_add(1));
        let terminated = slice::from_raw_parts(text, scan_units);
        let text_units = terminated.iter().position(|unit| *unit == 0)?;
        let hash = hash_canonical_utf16(&terminated[..text_units], deadline)?;
        if Instant::now() >= deadline || hash != expected || GetClipboardSequenceNumber() != before
        {
            return None;
        }
        drop(lock_guard);
        drop(clipboard_guard);
        (Instant::now() < deadline && GetClipboardSequenceNumber() == before).then_some(before)
    }
}

/// This is intentionally the final operation before SendInput. Any new
/// clipboard revision invalidates the already-sampled text/hash authority.
pub(super) fn sequence_is_current(expected: u32) -> bool {
    expected != 0 && unsafe { GetClipboardSequenceNumber() == expected }
}

fn hash_canonical_utf16(units: &[u16], deadline: Instant) -> Option<ClipboardTextHash> {
    if units.len() > MAX_INSERTION_UTF8_BYTES {
        return None;
    }
    let mut digest = Sha256::new();
    let mut utf8_bytes = 0_usize;
    let mut encoded = [0_u8; 4];
    for (index, decoded) in decode_utf16(units.iter().copied()).enumerate() {
        if index.is_multiple_of(1_024) && Instant::now() >= deadline {
            return None;
        }
        let character = decoded.ok()?;
        let bytes = character.encode_utf8(&mut encoded).as_bytes();
        utf8_bytes = utf8_bytes.checked_add(bytes.len())?;
        if utf8_bytes > MAX_INSERTION_UTF8_BYTES {
            return None;
        }
        digest.update(bytes);
    }
    (Instant::now() < deadline).then(|| ClipboardTextHash::from_bytes(digest.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_utf8(value: &str) -> ClipboardTextHash {
        ClipboardTextHash::from_bytes(Sha256::digest(value.as_bytes()).into())
    }

    #[test]
    fn canonical_utf16_hash_matches_utf8_for_ascii_bmp_and_surrogate_pairs() {
        for value in ["transactional paste", "café", "plain 🦀 text"] {
            let utf16 = value.encode_utf16().collect::<Vec<_>>();
            assert_eq!(
                hash_canonical_utf16(&utf16, Instant::now() + std::time::Duration::from_secs(1)),
                Some(hash_utf8(value))
            );
        }
    }

    #[test]
    fn expired_absolute_deadline_stops_hashing_without_authority() {
        assert_eq!(
            hash_canonical_utf16(&[u16::from(b'a'); 2_048], Instant::now()),
            None
        );
    }

    #[test]
    fn malformed_or_oversized_utf16_is_rejected_before_injection_authority() {
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        assert_eq!(hash_canonical_utf16(&[0xD800], deadline), None);
        assert_eq!(
            hash_canonical_utf16(
                &vec![u16::from(b'a'); MAX_INSERTION_UTF8_BYTES + 1],
                deadline,
            ),
            None
        );
        assert_eq!(
            hash_canonical_utf16(&vec![0x0800; MAX_INSERTION_UTF8_BYTES / 2], deadline,),
            None,
            "UTF-8 byte size, not only UTF-16 unit count, is bounded"
        );
    }
}
