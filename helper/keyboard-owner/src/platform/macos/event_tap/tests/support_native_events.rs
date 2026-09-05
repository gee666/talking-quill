//! Shared native events fixtures.

use super::*;

pub(super) fn tagged_keyboard_event(
    event_type: u32,
    key_code: u16,
    flags: u64,
    marker: i64,
) -> ffi::CGEventRef {
    let event = unsafe {
        ffi::CGEventCreateKeyboardEvent(null(), key_code, event_type != ffi::K_CG_EVENT_KEY_UP)
    };
    assert!(!event.is_null());
    unsafe {
        ffi::CGEventSetType(event, event_type);
        ffi::CGEventSetFlags(event, flags);
        ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
    }
    event
}

pub(super) fn tagged_mouse_event(
    event_type: u32,
    location: ffi::CGPoint,
    button: u32,
    click_state: i64,
    marker: i64,
) -> ffi::CGEventRef {
    let event = unsafe { ffi::CGEventCreateMouseEvent(null(), event_type, location, button) };
    assert!(!event.is_null());
    unsafe {
        ffi::CGEventSetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE, click_state);
        ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
    }
    event
}

pub(super) fn install_event_source_identity(context: &mut CallbackContext, event: ffi::CGEventRef) {
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    context.injection_identity = Some(injection::InjectionIdentity::for_test(source_pid));
}
