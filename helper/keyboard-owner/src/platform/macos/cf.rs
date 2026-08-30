use std::{ffi::CStr, ptr::null, time::Instant};

use sha2::{Digest, Sha256};

use super::ffi;
use crate::platform::{
    ClipboardTextHash, MAX_FRONT_APP_FIELD_ESCAPED_BYTES, MAX_INSERTION_UTF8_BYTES, PlatformError,
};

/// Owns one Core Foundation object returned by a Create/Copy function.
pub(super) struct OwnedCf(ffi::CFTypeRef);

impl OwnedCf {
    pub(super) fn from_created(value: ffi::CFTypeRef) -> Result<Self, PlatformError> {
        if value.is_null() {
            Err(PlatformError::NativeFailure)
        } else {
            Ok(Self(value))
        }
    }

    pub(super) const fn as_type_ref(&self) -> ffi::CFTypeRef {
        self.0
    }

    pub(super) fn retained_clone(&self) -> Result<Self, PlatformError> {
        // SAFETY: the source reference is valid and retained for this call.
        // CFRetain returns another owned reference to the same CF identity.
        Self::from_created(unsafe { ffi::CFRetain(self.0) })
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: this wrapper is created only from Create/Copy-rule values and
        // releases its non-null reference exactly once.
        unsafe { ffi::CFRelease(self.0) };
    }
}

pub(super) fn create_cf_string(value: &CStr) -> Result<OwnedCf, PlatformError> {
    // SAFETY: value is NUL-terminated for the full call and UTF-8 is the
    // declared encoding. The returned Create-rule object is owned.
    let string = unsafe {
        ffi::CFStringCreateWithCString(null(), value.as_ptr(), ffi::K_CF_STRING_ENCODING_UTF8)
    };
    OwnedCf::from_created(string)
}

pub(super) fn cf_string_sha256_bounded(
    value: ffi::CFTypeRef,
    conversion_deadline: Instant,
) -> Result<ClipboardTextHash, PlatformError> {
    // SAFETY: both functions only inspect a non-null Core Foundation object.
    if unsafe { ffi::CFGetTypeID(value) } != unsafe { ffi::CFStringGetTypeID() } {
        return Err(PlatformError::NativeFailure);
    }
    let string = value.cast();
    let length = unsafe { ffi::CFStringGetLength(string) };
    let length_usize = usize::try_from(length).map_err(|_| PlatformError::NativeFailure)?;
    // Every valid Unicode scalar consumes at least as many UTF-8 bytes as
    // UTF-16 code units. This rejects a definitely oversized value before even
    // the fixed stack buffer is initialized.
    if length_usize > MAX_INSERTION_UTF8_BYTES {
        return Err(PlatformError::NativeFailure);
    }
    let maximum =
        unsafe { ffi::CFStringGetMaximumSizeForEncoding(length, ffi::K_CF_STRING_ENCODING_UTF8) };
    let maximum = usize::try_from(maximum).map_err(|_| PlatformError::NativeFailure)?;
    if maximum < length_usize {
        return Err(PlatformError::NativeFailure);
    }

    let mut digest = Sha256::new();
    let mut total_bytes = 0_usize;
    let mut converted_total = 0_usize;
    let mut chunk = [0_u8; 4 * 1024];
    while converted_total < length_usize {
        if Instant::now() >= conversion_deadline {
            return Err(PlatformError::NativeFailure);
        }
        let mut used = 0;
        let remaining = length_usize - converted_total;
        // SAFETY: string has the validated CFString type. The range remains
        // within its UTF-16 length, chunk is fixed writable storage, and used
        // remains live for this call.
        let converted = unsafe {
            ffi::CFStringGetBytes(
                string,
                ffi::CFRange {
                    location: ffi::CFIndex::try_from(converted_total)
                        .map_err(|_| PlatformError::NativeFailure)?,
                    length: ffi::CFIndex::try_from(remaining)
                        .map_err(|_| PlatformError::NativeFailure)?,
                },
                ffi::K_CF_STRING_ENCODING_UTF8,
                0,
                0,
                chunk.as_mut_ptr(),
                ffi::CFIndex::try_from(chunk.len()).map_err(|_| PlatformError::NativeFailure)?,
                &raw mut used,
            )
        };
        let converted = usize::try_from(converted).map_err(|_| PlatformError::NativeFailure)?;
        let used = usize::try_from(used).map_err(|_| PlatformError::NativeFailure)?;
        if converted == 0 || converted > remaining || used == 0 || used > chunk.len() {
            return Err(PlatformError::NativeFailure);
        }
        total_bytes = total_bytes
            .checked_add(used)
            .ok_or(PlatformError::NativeFailure)?;
        if total_bytes > MAX_INSERTION_UTF8_BYTES {
            return Err(PlatformError::NativeFailure);
        }
        digest.update(&chunk[..used]);
        converted_total += converted;
    }
    Ok(ClipboardTextHash::from_bytes(digest.finalize().into()))
}

pub(super) fn cf_string_to_string(value: ffi::CFTypeRef) -> Result<String, PlatformError> {
    // SAFETY: both functions only inspect a non-null Core Foundation object.
    if unsafe { ffi::CFGetTypeID(value) } != unsafe { ffi::CFStringGetTypeID() } {
        return Err(PlatformError::NativeFailure);
    }
    let string = value.cast();
    // SAFETY: the type-ID check above proves this object is a CFString.
    let length = unsafe { ffi::CFStringGetLength(string) };
    if length < 0 {
        return Err(PlatformError::NativeFailure);
    }
    let mut bytes = vec![0_u8; MAX_FRONT_APP_FIELD_ESCAPED_BYTES];
    let mut used = 0;
    // SAFETY: bytes is writable for the supplied bound, used remains valid for
    // the call, and Core Foundation truncates only at a converted character.
    let converted = unsafe {
        ffi::CFStringGetBytes(
            string,
            ffi::CFRange {
                location: 0,
                length,
            },
            ffi::K_CF_STRING_ENCODING_UTF8,
            0,
            0,
            bytes.as_mut_ptr(),
            ffi::CFIndex::try_from(bytes.len()).map_err(|_| PlatformError::NativeFailure)?,
            &raw mut used,
        )
    };
    let used = usize::try_from(used).map_err(|_| PlatformError::NativeFailure)?;
    if (length > 0 && converted == 0) || used > bytes.len() {
        return Err(PlatformError::NativeFailure);
    }
    bytes.truncate(used);
    String::from_utf8(bytes).map_err(|_| PlatformError::NativeFailure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_clipboard_hash_rejects_oversize_before_conversion() {
        let source = "a".repeat(MAX_INSERTION_UTF8_BYTES + 1);
        let source_c = std::ffi::CString::new(source).unwrap();
        let value = create_cf_string(&source_c).unwrap();
        assert!(
            cf_string_sha256_bounded(
                value.as_type_ref(),
                Instant::now() + std::time::Duration::from_secs(1),
            )
            .is_err()
        );
    }

    #[test]
    fn bounded_clipboard_hash_matches_exact_utf8_without_a_large_buffer() {
        let source = "bounded 🦀 clipboard";
        let source_c = std::ffi::CString::new(source).unwrap();
        let value = create_cf_string(&source_c).unwrap();
        assert_eq!(
            cf_string_sha256_bounded(
                value.as_type_ref(),
                Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap(),
            ClipboardTextHash::from_bytes(Sha256::digest(source.as_bytes()).into())
        );
    }

    #[test]
    fn long_accessibility_strings_return_a_bounded_utf8_prefix() {
        let source = "🦀".repeat(MAX_FRONT_APP_FIELD_ESCAPED_BYTES);
        let source_c = std::ffi::CString::new(source.as_str()).unwrap();
        let value = create_cf_string(&source_c).unwrap();
        let converted = cf_string_to_string(value.as_type_ref()).unwrap();

        assert!(!converted.is_empty());
        assert!(converted.len() <= MAX_FRONT_APP_FIELD_ESCAPED_BYTES);
        assert!(source.starts_with(&converted));
        assert!(converted.is_char_boundary(converted.len()));
    }
}
