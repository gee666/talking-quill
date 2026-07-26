use std::ptr::null;

use super::{INJECTED_MARKER, accessibility::permission_snapshot, ffi};
use crate::platform::{PasteFailure, PasteResult, PermissionState, secure_input_paste_result};

pub(super) fn inject_paste() -> PasteResult {
    let permissions = permission_snapshot();
    if permissions.event_post != PermissionState::Granted {
        return PasteResult {
            submitted: false,
            reason: Some(PasteFailure::PermissionDenied),
        };
    }

    // SAFETY: this HIToolbox probe has no pointer arguments or ownership.
    if let Some(result) =
        secure_input_paste_result(unsafe { ffi::IsSecureEventInputEnabled() != 0 })
    {
        return result;
    }

    // Physical V key on the macOS virtual-key map.
    // SAFETY: null event source requests the system source and key code 9 is V.
    let down = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 9, true) };
    let up = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 9, false) };
    if down.is_null() || up.is_null() {
        if !down.is_null() {
            // SAFETY: down is owned by this function.
            unsafe { ffi::CFRelease(down.cast_const()) };
        }
        if !up.is_null() {
            // SAFETY: up is owned by this function.
            unsafe { ffi::CFRelease(up.cast_const()) };
        }
        return PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        };
    }

    // SAFETY: both event references are valid and owned until released below.
    unsafe {
        ffi::CGEventSetFlags(down, ffi::K_CG_EVENT_FLAG_MASK_COMMAND);
        ffi::CGEventSetFlags(up, ffi::K_CG_EVENT_FLAG_MASK_COMMAND);
        ffi::CGEventSetIntegerValueField(down, ffi::K_CG_EVENT_SOURCE_USER_DATA, INJECTED_MARKER);
        ffi::CGEventSetIntegerValueField(up, ffi::K_CG_EVENT_SOURCE_USER_DATA, INJECTED_MARKER);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, down);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, up);
        ffi::CFRelease(down.cast_const());
        ffi::CFRelease(up.cast_const());
    }
    PasteResult {
        submitted: true,
        reason: None,
    }
}
