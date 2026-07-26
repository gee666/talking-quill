use std::{
    ffi::{CStr, c_int},
    ptr::null,
};

use super::{
    cf::{OwnedCf, cf_string_to_string, create_cf_string},
    ffi,
};
use crate::platform::{FrontApp, PermissionState, Permissions, PlatformError, WindowBounds};

const AX_FOCUSED_APPLICATION: &CStr = c"AXFocusedApplication";
const AX_FOCUSED_WINDOW: &CStr = c"AXFocusedWindow";
const AX_TITLE: &CStr = c"AXTitle";
const AX_POSITION: &CStr = c"AXPosition";
const AX_SIZE: &CStr = c"AXSize";

struct AxAttributes {
    focused_application: OwnedCf,
    focused_window: OwnedCf,
    title: OwnedCf,
    position: OwnedCf,
    size: OwnedCf,
}

impl AxAttributes {
    fn new() -> Result<Self, PlatformError> {
        Ok(Self {
            focused_application: create_cf_string(AX_FOCUSED_APPLICATION)?,
            focused_window: create_cf_string(AX_FOCUSED_WINDOW)?,
            title: create_cf_string(AX_TITLE)?,
            position: create_cf_string(AX_POSITION)?,
            size: create_cf_string(AX_SIZE)?,
        })
    }
}

pub(super) fn front_app() -> Result<FrontApp, PlatformError> {
    if permission_snapshot().accessibility != PermissionState::Granted {
        return Err(PlatformError::PermissionDenied);
    }
    let attributes = AxAttributes::new()?;
    // SAFETY: AXUIElementCreateSystemWide follows the Core Foundation Create
    // rule and returns an owned reference when non-null.
    let system = OwnedCf::from_created(unsafe { ffi::AXUIElementCreateSystemWide() }.cast_const())?;
    let application = ax_copy_attribute(
        system.as_type_ref().cast_mut(),
        attributes.focused_application.as_type_ref(),
    )?;
    let app_element = application.as_type_ref().cast_mut();

    let application_name =
        ax_string_attribute(app_element, attributes.title.as_type_ref()).unwrap_or_default();
    let mut pid = 0;
    // SAFETY: pid is writable and app_element is a valid retained AX object.
    if unsafe { ffi::AXUIElementGetPid(app_element, &raw mut pid) } != 0 || pid <= 0 {
        return Err(PlatformError::NativeFailure);
    }

    let process_name = process_name(pid).unwrap_or(application_name);
    if process_name.is_empty() {
        return Err(PlatformError::NativeFailure);
    }

    let focused_window =
        ax_copy_attribute(app_element, attributes.focused_window.as_type_ref()).ok();
    let window_title = focused_window
        .as_ref()
        .and_then(|window| {
            ax_string_attribute(
                window.as_type_ref().cast_mut(),
                attributes.title.as_type_ref(),
            )
            .ok()
        })
        .unwrap_or_default();
    let window_bounds = focused_window.as_ref().and_then(|window| {
        ax_window_bounds(
            window.as_type_ref().cast_mut(),
            attributes.position.as_type_ref(),
            attributes.size.as_type_ref(),
        )
    });

    Ok(FrontApp {
        process_name,
        window_title,
        window_bounds,
    })
}

fn ax_window_bounds(
    window: ffi::AXUIElementRef,
    position_attribute: ffi::CFStringRef,
    size_attribute: ffi::CFStringRef,
) -> Option<WindowBounds> {
    let position_value = ax_copy_attribute(window, position_attribute).ok()?;
    let size_value = ax_copy_attribute(window, size_attribute).ok()?;
    // SAFETY: both retained values came from the documented AXPosition/AXSize attributes.
    if unsafe { ffi::AXValueGetType(position_value.as_type_ref()) } != ffi::K_AX_VALUE_CGPOINT_TYPE
        || unsafe { ffi::AXValueGetType(size_value.as_type_ref()) } != ffi::K_AX_VALUE_CGSIZE_TYPE
    {
        return None;
    }
    let mut position = ffi::CGPoint::default();
    let mut size = ffi::CGSize::default();
    // SAFETY: output pointers match the requested AXValue types and remain writable for the calls.
    if unsafe {
        ffi::AXValueGetValue(
            position_value.as_type_ref(),
            ffi::K_AX_VALUE_CGPOINT_TYPE,
            (&raw mut position).cast(),
        )
    } == 0
        || unsafe {
            ffi::AXValueGetValue(
                size_value.as_type_ref(),
                ffi::K_AX_VALUE_CGSIZE_TYPE,
                (&raw mut size).cast(),
            )
        } == 0
        || !position.x.is_finite()
        || !position.y.is_finite()
        || !size.width.is_finite()
        || !size.height.is_finite()
        || size.width < 1.0
        || size.height < 1.0
    {
        return None;
    }
    Some(WindowBounds {
        x: position
            .x
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        y: position
            .y
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        width: size.width.round().clamp(1.0, f64::from(u32::MAX)) as u32,
        height: size.height.round().clamp(1.0, f64::from(u32::MAX)) as u32,
    })
}

fn process_name(pid: c_int) -> Option<String> {
    let mut bytes = vec![0_u8; 1024];
    // SAFETY: the buffer is writable for the exact supplied size and pid was
    // returned by Accessibility for the focused application.
    let copied = unsafe {
        ffi::proc_name(
            pid,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).expect("fixed process-name buffer fits u32"),
        )
    };
    let copied = usize::try_from(copied).ok()?;
    if copied == 0 || copied > bytes.len() {
        return None;
    }
    String::from_utf8(bytes[..copied].to_vec()).ok()
}

fn ax_copy_attribute(
    element: ffi::AXUIElementRef,
    attribute: ffi::CFStringRef,
) -> Result<OwnedCf, PlatformError> {
    let mut value: ffi::CFTypeRef = null();
    // SAFETY: element and attribute are valid retained references and value is
    // writable. CopyAttributeValue returns a retained value on success.
    let error = unsafe { ffi::AXUIElementCopyAttributeValue(element, attribute, &raw mut value) };
    if error != 0 {
        return Err(PlatformError::NativeFailure);
    }
    OwnedCf::from_created(value)
}

fn ax_string_attribute(
    element: ffi::AXUIElementRef,
    attribute: ffi::CFStringRef,
) -> Result<String, PlatformError> {
    let value = ax_copy_attribute(element, attribute)?;
    cf_string_to_string(value.as_type_ref())
}

pub(super) fn permission_snapshot() -> Permissions {
    // SAFETY: all three functions are side-effect-free permission preflight
    // queries and explicitly do not trigger system prompts.
    unsafe {
        Permissions {
            accessibility: permission_state(ffi::AXIsProcessTrusted() != 0),
            input_monitoring: permission_state(ffi::CGPreflightListenEventAccess()),
            event_post: permission_state(ffi::CGPreflightPostEventAccess()),
        }
    }
}

const fn permission_state(granted: bool) -> PermissionState {
    if granted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_attribute_names_match_the_header_macros() {
        assert_eq!(AX_FOCUSED_APPLICATION.to_bytes(), b"AXFocusedApplication");
        assert_eq!(AX_FOCUSED_WINDOW.to_bytes(), b"AXFocusedWindow");
        assert_eq!(AX_TITLE.to_bytes(), b"AXTitle");
    }

    #[test]
    fn permission_booleans_have_explicit_wire_states() {
        assert_eq!(permission_state(true), PermissionState::Granted);
        assert_eq!(permission_state(false), PermissionState::Denied);
    }
}
