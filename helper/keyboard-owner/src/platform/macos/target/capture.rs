//! Retained AX evidence, double capture, and typed range checks.

use super::*;

pub(in crate::platform::macos) struct TargetEvidence {
    pub(super) process_id: i32,
    pub(super) application: OwnedCf,
    pub(super) window: OwnedCf,
    pub(super) focused_control: OwnedCf,
    pub(super) selected_text_range_value: OwnedCf,
    pub(super) selected_text_range: ffi::CFRange,
}

// SAFETY: AXUIElement and Core Foundation references may be retained, released,
// and compared across threads. AX messaging itself is confined to the cache
// worker; event-tap and owner code only move evidence and call local CFEqual.
unsafe impl Send for TargetEvidence {}

impl TargetEvidence {
    pub(super) fn retained_clone(&self) -> Result<Self, PlatformError> {
        Ok(Self {
            process_id: self.process_id,
            application: self.application.retained_clone()?,
            window: self.window.retained_clone()?,
            focused_control: self.focused_control.retained_clone()?,
            selected_text_range_value: self.selected_text_range_value.retained_clone()?,
            selected_text_range: self.selected_text_range,
        })
    }
}

pub(super) struct TargetCaptureResources {
    pub(super) system: OwnedCf,
    pub(super) focused_application: OwnedCf,
    pub(super) focused_window: OwnedCf,
    pub(super) focused_ui_element: OwnedCf,
    pub(super) window: OwnedCf,
    pub(super) selected_text: OwnedCf,
    pub(super) selected_text_range: OwnedCf,
    pub(super) focused_window_changed: OwnedCf,
    pub(super) focused_ui_element_changed: OwnedCf,
    pub(super) selected_text_changed: OwnedCf,
    pub(super) selected_text_range_changed: OwnedCf,
}

impl TargetCaptureResources {
    pub(super) fn new() -> Result<Self, PlatformError> {
        let focused_application = create_cf_string(AX_FOCUSED_APPLICATION)?;
        let focused_window = create_cf_string(AX_FOCUSED_WINDOW)?;
        let focused_ui_element = create_cf_string(AX_FOCUSED_UI_ELEMENT)?;
        let window = create_cf_string(AX_WINDOW)?;
        let selected_text = create_cf_string(AX_SELECTED_TEXT)?;
        let selected_text_range = create_cf_string(AX_SELECTED_TEXT_RANGE)?;
        let focused_window_changed = create_cf_string(AX_FOCUSED_WINDOW_CHANGED)?;
        let focused_ui_element_changed = create_cf_string(AX_FOCUSED_UI_ELEMENT_CHANGED)?;
        let selected_text_changed = create_cf_string(AX_SELECTED_TEXT_CHANGED)?;
        let selected_text_range_changed = create_cf_string(AX_SELECTED_TEXT_RANGE_CHANGED)?;
        // SAFETY: the system-wide element follows the Create rule. This runs on
        // the dedicated AX worker, never the event-tap owner.
        let system = unsafe {
            OwnedCf::from_created(ffi::AXUIElementCreateSystemWide().cast_const().cast())
        }?;
        set_ax_messaging_timeout(&system)?;
        Ok(Self {
            system,
            focused_application,
            focused_window,
            focused_ui_element,
            window,
            selected_text,
            selected_text_range,
            focused_window_changed,
            focused_ui_element_changed,
            selected_text_changed,
            selected_text_range_changed,
        })
    }
}

/// Captures the complete focused application/window/control tuple twice and
/// accepts it only when both complete samples are identical. Each sample also
/// requires the control's AXWindow identity to be exactly the sampled focused
/// window; unsupported or missing AXWindow evidence fails conservatively.
pub(super) fn capture_target(resources: &TargetCaptureResources) -> Option<TargetEvidence> {
    if permission_snapshot().accessibility != PermissionState::Granted {
        return None;
    }
    let first = capture_target_once(resources)?;
    let second = capture_target_once(resources)?;
    same_target(&first, &second).then_some(second)
}

pub(super) const fn selected_text_range_is_valid(
    value_type: u32,
    extracted: bool,
    range: ffi::CFRange,
) -> bool {
    value_type == ffi::K_AX_VALUE_CFRANGE_TYPE
        && extracted
        && range.location >= 0
        && range.length >= 0
        && range.location.checked_add(range.length).is_some()
}

pub(super) fn capture_target_once(resources: &TargetCaptureResources) -> Option<TargetEvidence> {
    let application = ax_copy_attribute(
        resources.system.as_type_ref().cast_mut(),
        resources.focused_application.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&application).ok()?;
    let window = ax_copy_attribute(
        application.as_type_ref().cast_mut(),
        resources.focused_window.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&window).ok()?;
    let focused_control = ax_copy_attribute(
        resources.system.as_type_ref().cast_mut(),
        resources.focused_ui_element.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&focused_control).ok()?;
    let control_window = ax_copy_attribute(
        focused_control.as_type_ref().cast_mut(),
        resources.window.as_type_ref(),
    )
    .ok()?;
    // A focused control without a stable insertion range is not strong enough
    // for deferred paste. Unsupported controls therefore remain targetless.
    let selected_text_range_value = ax_copy_attribute(
        focused_control.as_type_ref().cast_mut(),
        resources.selected_text_range.as_type_ref(),
    )
    .ok()?;
    // AXSelectedTextRange is authoritative only when it is the documented
    // kAXValueCFRangeType and can be copied into fixed scalar storage. CFEqual
    // on an arbitrary AX/CF object is not caret-position evidence.
    let value_type = unsafe { ffi::AXValueGetType(selected_text_range_value.as_type_ref()) };
    if value_type != ffi::K_AX_VALUE_CFRANGE_TYPE {
        return None;
    }
    let mut selected_text_range = ffi::CFRange::default();
    let extracted = unsafe {
        ffi::AXValueGetValue(
            selected_text_range_value.as_type_ref(),
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (&raw mut selected_text_range).cast(),
        )
    } != 0;
    if !selected_text_range_is_valid(value_type, extracted, selected_text_range) {
        return None;
    }
    set_ax_messaging_timeout(&control_window).ok()?;

    let process_id = ax_pid(&application)?;
    if ax_pid(&window)? != process_id
        || ax_pid(&focused_control)? != process_id
        || ax_pid(&control_window)? != process_id
        || !cf_equal(&window, &control_window)
    {
        return None;
    }
    Some(TargetEvidence {
        process_id,
        application,
        window,
        focused_control,
        selected_text_range_value,
        selected_text_range,
    })
}

pub(in crate::platform::macos) fn same_target(
    left: &TargetEvidence,
    right: &TargetEvidence,
) -> bool {
    left.process_id == right.process_id
        && cf_equal(&left.application, &right.application)
        && cf_equal(&left.window, &right.window)
        && cf_equal(&left.focused_control, &right.focused_control)
        && left.selected_text_range == right.selected_text_range
}

#[cfg(any(test, feature = "transactional-shortcuts-dev"))]
pub(super) fn create_range_value_for_test(range: &ffi::CFRange) -> Result<OwnedCf, PlatformError> {
    // SAFETY: range matches the requested AXValue type. The Create-rule result
    // transfers its owned reference to the wrapper.
    unsafe {
        OwnedCf::from_created(ffi::AXValueCreate(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (range as *const ffi::CFRange).cast(),
        ))
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn test_target_caret_identity_contract() -> bool {
    let result = (|| {
        let range = ffi::CFRange {
            location: 1,
            length: 0,
        };
        let baseline = TargetEvidence {
            process_id: 42,
            application: create_cf_string(c"app")?,
            window: create_cf_string(c"window")?,
            focused_control: create_cf_string(c"control")?,
            selected_text_range_value: create_range_value_for_test(&range)?,
            selected_text_range: range,
        };
        let same = baseline.retained_clone()?;
        let mut moved = baseline.retained_clone()?;
        moved.selected_text_range.location = 2;
        Ok::<_, PlatformError>(same_target(&baseline, &same) && !same_target(&baseline, &moved))
    })();
    result.unwrap_or(false)
}

pub(super) fn set_ax_messaging_timeout(element: &OwnedCf) -> Result<(), PlatformError> {
    set_ax_messaging_timeout_ref(element.as_type_ref().cast_mut())
}

pub(super) fn set_ax_messaging_timeout_ref(
    element: ffi::AXUIElementRef,
) -> Result<(), PlatformError> {
    // SAFETY: the retained object is an AXUIElement and the timeout is finite
    // and positive. Bounding AX messaging keeps a nonresponsive target from
    // indefinitely occupying the dedicated worker.
    if unsafe { ffi::AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECONDS) } == 0 {
        Ok(())
    } else {
        Err(PlatformError::NativeFailure)
    }
}

pub(super) fn ax_copy_attribute(
    element: ffi::AXUIElementRef,
    attribute: ffi::CFStringRef,
) -> Result<OwnedCf, PlatformError> {
    set_ax_messaging_timeout_ref(element)?;
    let mut value: ffi::CFTypeRef = null();
    // SAFETY: both inputs are retained AX/CF objects and `value` is writable.
    let error = unsafe { ffi::AXUIElementCopyAttributeValue(element, attribute, &raw mut value) };
    if error != 0 {
        return Err(PlatformError::NativeFailure);
    }
    // SAFETY: a successful CopyAttributeValue transfers an owned reference.
    unsafe { OwnedCf::from_created(value) }
}

pub(super) fn ax_pid(element: &OwnedCf) -> Option<i32> {
    set_ax_messaging_timeout(element).ok()?;
    let mut process_id = 0;
    // SAFETY: the retained object is an AXUIElement returned by an AX focused
    // object attribute and `process_id` is writable.
    if unsafe { ffi::AXUIElementGetPid(element.as_type_ref().cast_mut(), &raw mut process_id) } != 0
        || process_id <= 0
    {
        None
    } else {
        Some(process_id)
    }
}

pub(super) fn cf_equal(left: &OwnedCf, right: &OwnedCf) -> bool {
    // SAFETY: both references remain retained for this comparison.
    unsafe { ffi::CFEqual(left.as_type_ref(), right.as_type_ref()) != 0 }
}
