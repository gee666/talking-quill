//! Authenticate complete scalar reposts before advancing journal authority.

use super::*;

pub(super) const fn is_mouse_event_type(event_type: u32) -> bool {
    matches!(
        event_type,
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
            | ffi::K_CG_EVENT_LEFT_MOUSE_UP
            | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
            | ffi::K_CG_EVENT_RIGHT_MOUSE_UP
            | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
            | ffi::K_CG_EVENT_OTHER_MOUSE_UP
    )
}

pub(super) const fn mouse_event_is_down(event_type: u32) -> bool {
    matches!(
        event_type,
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
            | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
            | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
    )
}

pub(super) fn recovery_ordering_exists(context: &CallbackContext) -> bool {
    // Release-published before recovery-only suppression and retained through
    // every exact tail rollover. A normal transactional candidate alone is
    // never authority to defer foreground input.
    context.state.recovery_deferred_mode.load(Ordering::Acquire)
}

pub(super) fn fail_recovery_edge_journal(context: &CallbackContext) {
    context
        .state
        .recovery_deferred_mode
        .store(true, Ordering::Release);
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::InputInjectionUnavailable);
    arm_maintenance_timer(context);
}

pub(super) fn deferred_mouse_button(event_type: u32, event: ffi::CGEventRef) -> Option<u32> {
    let reported =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER) };
    u32::try_from(reported).ok().or(match event_type {
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN | ffi::K_CG_EVENT_LEFT_MOUSE_UP => Some(0),
        ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN | ffi::K_CG_EVENT_RIGHT_MOUSE_UP => Some(1),
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN | ffi::K_CG_EVENT_OTHER_MOUSE_UP => Some(2),
        _ => None,
    })
}

pub(super) fn deferred_edge_matches_observed(
    expected: injection::DeferredEvent,
    event_type: u32,
    event: ffi::CGEventRef,
) -> bool {
    if event_type != expected.event_type || unsafe { ffi::CGEventGetFlags(event) } != expected.flags
    {
        return false;
    }
    if expected.is_mouse() {
        let location = unsafe { ffi::CGEventGetLocation(event) };
        let button =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER) };
        let click =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE) };
        let number =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_NUMBER) };
        let pressure =
            unsafe { ffi::CGEventGetDoubleValueField(event, ffi::K_CG_MOUSE_EVENT_PRESSURE) };
        let delta_x =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_X) };
        let delta_y =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_Y) };
        let instant_mouser = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
        };
        let subtype =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_SUBTYPE) };
        location == expected.location
            && button == expected.mouse_button
            && click == expected.mouse_click_state
            && number == expected.mouse_number
            && pressure == expected.mouse_pressure
            && delta_x == expected.mouse_delta_x
            && delta_y == expected.mouse_delta_y
            && instant_mouser == expected.mouse_instant_mouser
            && subtype == expected.mouse_subtype
    } else {
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        let keyboard_type = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        };
        key_code == i64::from(expected.key_code)
            && repeat == expected.repeat
            && keyboard_type == expected.keyboard_type
    }
}

pub(super) fn observe_deferred_repost(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> Option<CurrentEdgeDisposition> {
    if event.is_null() {
        return None;
    }
    let identity = context.injection_identity?;
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let mut journal = context.recovery_edges.try_lock().ok()?;
    if !injection::token_matches(identity, journal.token, marker, source_pid) {
        return None;
    }
    let Some(expected) = journal.expected() else {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        drop(journal);
        fail_recovery_edge_journal(context);
        return Some(CurrentEdgeDisposition::Owned);
    };
    if !deferred_edge_matches_observed(expected, event_type, event) {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        drop(journal);
        fail_recovery_edge_journal(context);
        return Some(CurrentEdgeDisposition::Owned);
    }
    // The original was suppressed. Publish Pass before advancing the exact
    // tagged replacement so unwind cannot lose a foreground-visible edge.
    set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Pass);
    journal.observe_foreground_exposure(expected);
    let observation = journal.advance_observation();
    drop(journal);
    if observation == GapBarrierObservation::Complete {
        let _ = try_clear_recovery_deferred_mode(context);
    }
    arm_maintenance_timer(context);
    Some(CurrentEdgeDisposition::Pass)
}
