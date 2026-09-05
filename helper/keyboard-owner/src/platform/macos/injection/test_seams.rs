//! Feature-gated physical-input and permission-loss posting seams.

use super::*;

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) const fn is_test_physical_marker(marker: i64) -> bool {
    matches!(marker, TEST_PHYSICAL_MARKER | TEST_PERMISSION_LOSS_MARKER)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn post_test_physical_key(
    key_code: u16,
    key_down: bool,
    flags: u64,
) -> bool {
    // This source exists only in explicitly feature-built trusted harnesses. It
    // traverses CoreGraphics and the production tap callback, but callback-side
    // classification treats its dedicated marker as physical input.
    let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), key_code, key_down) };
    if event.is_null() {
        return false;
    }
    unsafe {
        if matches!(key_code, 54 | 55 | 56 | 58 | 59 | 60 | 61 | 62) {
            ffi::CGEventSetType(event, ffi::K_CG_EVENT_FLAGS_CHANGED);
        }
        ffi::CGEventSetFlags(event, flags);
        ffi::CGEventSetIntegerValueField(
            event,
            ffi::K_CG_EVENT_SOURCE_USER_DATA,
            TEST_PHYSICAL_MARKER,
        );
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, event);
        ffi::CFRelease(event.cast_const());
    }
    true
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn post_test_control_marker(marker: i64) -> bool {
    let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 127, true) };
    if event.is_null() {
        return false;
    }
    unsafe {
        ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, event);
        ffi::CFRelease(event.cast_const());
    }
    true
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn post_test_permission_loss() -> bool {
    post_test_control_marker(TEST_PERMISSION_LOSS_MARKER)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn post_test_physical_mouse_down() -> bool {
    // Snapshot the real cursor rather than injecting at (0,0), which can hit a
    // menu/display boundary instead of the controlled focused fixture.
    let cursor_event = unsafe { ffi::CGEventCreate(null()) };
    if cursor_event.is_null() {
        return false;
    }
    let cursor = unsafe { ffi::CGEventGetLocation(cursor_event) };
    let location = if cursor.x == 0.0 && cursor.y == 0.0 {
        // Trusted fixture pins its TextEdit window over this fallback point.
        ffi::CGPoint { x: 300.0, y: 250.0 }
    } else {
        cursor
    };
    unsafe { ffi::CFRelease(cursor_event.cast_const()) };
    let down = unsafe {
        ffi::CGEventCreateMouseEvent(null(), ffi::K_CG_EVENT_LEFT_MOUSE_DOWN, location, 0)
    };
    let up =
        unsafe { ffi::CGEventCreateMouseEvent(null(), ffi::K_CG_EVENT_LEFT_MOUSE_UP, location, 0) };
    if down.is_null() || up.is_null() {
        unsafe {
            if !down.is_null() {
                ffi::CFRelease(down.cast_const());
            }
            if !up.is_null() {
                ffi::CFRelease(up.cast_const());
            }
        }
        return false;
    }
    unsafe {
        for event in [down, up] {
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_EVENT_SOURCE_USER_DATA,
                TEST_PHYSICAL_MARKER,
            );
        }
        // Both events are prebuilt before either post, so every successful
        // seam invocation is balanced even when the tap cancels/reposts down.
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, down);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, up);
        ffi::CFRelease(up.cast_const());
        ffi::CFRelease(down.cast_const());
    }
    true
}
