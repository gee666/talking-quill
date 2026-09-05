//! Track ordinary mouse edges and discard overflow-fenced generations.

use super::*;

pub(super) fn observe_normal_mouse_transition(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> bool {
    if !is_mouse_event_type(event_type) || event.is_null() {
        return true;
    }
    let Some(identity) = context.injection_identity else {
        return false;
    };
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let test_physical = recovery_test_physical_source(context, marker);
    if source_pid == identity.source_pid && !test_physical {
        return true;
    }
    let source = if test_physical {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
    else {
        return false;
    };
    let Ok(mut journal) = context.recovery_edges.try_lock() else {
        return false;
    };
    journal.normal_mouse_transition(source, source_pid, button, mouse_event_is_down(event_type));
    true
}

pub(super) fn discard_overflow_fenced_nonhelper(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    source: InputSource,
    source_pid: i64,
) -> bool {
    if event.is_null() {
        return false;
    }
    let Ok(mut journal) = context.recovery_edges.try_lock() else {
        return false;
    };
    if is_mouse_event_type(event_type) {
        let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
        else {
            return false;
        };
        if !journal.mouse_is_discard_fenced(source, source_pid, button) {
            return false;
        }
        journal.consume_mouse_discard_fence(
            source,
            source_pid,
            button,
            mouse_event_is_down(event_type),
        );
        drop(journal);
        let _ = try_clear_recovery_deferred_mode(context);
        return true;
    }
    if !matches!(
        event_type,
        ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
    ) {
        return false;
    }
    let key_code =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let Ok(key_code) = u16::try_from(key_code)
        .ok()
        .filter(|key| *key < 128)
        .ok_or(())
    else {
        return false;
    };
    if !journal.key_is_discard_fenced(source, source_pid, key_code) {
        return false;
    }
    let is_down = if event_type == ffi::K_CG_EVENT_KEY_UP {
        false
    } else if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        let physical_hid = (source == InputSource::Physical).then(|| native_key_is_down(key_code));
        journal
            .modifier_side_transition(source, source_pid, key_code, physical_hid)
            .is_some_and(|(is_down, _)| is_down)
    } else {
        true
    };
    journal.consume_key_discard_fence(source, source_pid, key_code, is_down);
    drop(journal);
    if source.is_physical()
        && let Ok(mut keyboard) = context.keyboard.try_lock()
    {
        if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
            let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
        } else {
            let _ = keyboard.physical.observe(
                key_code,
                if is_down {
                    KeyPhase::Down
                } else {
                    KeyPhase::Up
                },
            );
        }
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled
        && recovery_test_physical_source(context, unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA)
        })
    {
        crate::platform::macos::observe_macos_test_physical_key(key_code, is_down);
    }
    let _ = try_clear_recovery_deferred_mode(context);
    true
}

pub(super) fn defer_nonhelper_if_ordered(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> Option<CurrentEdgeDisposition> {
    if event.is_null()
        || (!is_mouse_event_type(event_type)
            && !matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
            ))
    {
        return None;
    }
    let identity = context.injection_identity?;
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let test_physical = recovery_test_physical_source(context, marker);
    if source_pid == identity.source_pid && !test_physical {
        return None;
    }
    let source = if test_physical {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    if discard_overflow_fenced_nonhelper(context, event_type, event, source, source_pid) {
        return Some(CurrentEdgeDisposition::Owned);
    }
    if !recovery_ordering_exists(context) {
        return None;
    }
    Some(defer_callback_edge(
        context, event_type, event, source, source_pid, marker, false,
    ))
}
