use std::{ffi::CStr, ptr::null};

use super::ffi;
use crate::platform::{MAX_FRONT_APP_FIELD_ESCAPED_BYTES, PlatformError};

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
