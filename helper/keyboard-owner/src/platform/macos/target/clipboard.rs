//! Worker autorelease pools and revision-fenced clipboard samples.

use super::*;

pub(super) struct AutoreleasePool(*mut c_void);

impl AutoreleasePool {
    pub(super) fn push() -> Result<Self, PlatformError> {
        // SAFETY: Objective-C runtime returns an opaque pool token for this
        // thread; it is popped exactly once by Drop on the same worker.
        let token = unsafe { ffi::objc_autoreleasePoolPush() };
        (!token.is_null())
            .then_some(Self(token))
            .ok_or(PlatformError::NativeFailure)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        // SAFETY: token came from push on this worker and remains unmatched.
        unsafe { ffi::objc_autoreleasePoolPop(self.0) };
    }
}

pub(super) struct ClipboardTextSample {
    pub(super) text: OwnedCf,
    pub(super) hash: ClipboardTextHash,
    pub(super) change_count: isize,
}

pub(super) fn pasteboard_change_count(pasteboard: ffi::ObjcId) -> isize {
    // SAFETY: NSPasteboard changeCount returns NSInteger and takes no arguments.
    unsafe { ffi::objc_msgSend_isize(pasteboard, ffi::sel_registerName(c"changeCount".as_ptr())) }
}

pub(super) fn clipboard_plain_text(conversion_deadline: Instant) -> Option<ClipboardTextSample> {
    // SAFETY: all Objective-C objects are used synchronously on the AX worker
    // under its current autorelease pool. The sampled immutable NSString is
    // retained before claim and fenced by NSPasteboard changeCount.
    unsafe {
        let pasteboard_class = ffi::objc_getClass(c"NSPasteboard".as_ptr());
        let string_class = ffi::objc_getClass(c"NSString".as_ptr());
        if pasteboard_class.is_null() || string_class.is_null() {
            return None;
        }
        let pasteboard = ffi::objc_msgSend(
            pasteboard_class.cast(),
            ffi::sel_registerName(c"generalPasteboard".as_ptr()),
        );
        let plain_text_type = ffi::objc_msgSend(
            string_class.cast(),
            ffi::sel_registerName(c"stringWithUTF8String:".as_ptr()),
            c"public.utf8-plain-text".as_ptr(),
        );
        if pasteboard.is_null() || plain_text_type.is_null() {
            return None;
        }
        let before = pasteboard_change_count(pasteboard);
        let value = ffi::objc_msgSend(
            pasteboard,
            ffi::sel_registerName(c"stringForType:".as_ptr()),
            plain_text_type,
        );
        if value.is_null() {
            return None;
        }
        let text = OwnedCf::from_created(ffi::CFRetain(value.cast_const().cast())).ok()?;
        let hash = cf_string_sha256_bounded(&text, conversion_deadline).ok()?;
        let after = pasteboard_change_count(pasteboard);
        (before == after).then_some(ClipboardTextSample {
            text,
            hash,
            change_count: after,
        })
    }
}

pub(super) fn clipboard_sample_is_authorized(
    sample_hash: ClipboardTextHash,
    sample_change_count: isize,
    expected_hash: ClipboardTextHash,
    current_change_count: isize,
) -> bool {
    sample_hash == expected_hash && sample_change_count == current_change_count
}

pub(super) fn current_clipboard_change_count() -> Option<isize> {
    unsafe {
        let pasteboard_class = ffi::objc_getClass(c"NSPasteboard".as_ptr());
        if pasteboard_class.is_null() {
            return None;
        }
        let pasteboard = ffi::objc_msgSend(
            pasteboard_class.cast(),
            ffi::sel_registerName(c"generalPasteboard".as_ptr()),
        );
        (!pasteboard.is_null()).then(|| pasteboard_change_count(pasteboard))
    }
}

pub(super) fn clipboard_change_count_is_current(expected: isize) -> bool {
    current_clipboard_change_count() == Some(expected)
}

pub(super) const fn postclaim_clipboard_revision_is_authorized(
    expected: isize,
    current: isize,
) -> bool {
    expected == current
}
