//! Development-only native tap disable and split-barrier controls.

use super::*;

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn service_test_tap_disable_request(context: &CallbackContext) {
    let Some(path) = context.test_tap_disable_request.as_ref() else {
        return;
    };
    if !path.try_exists().unwrap_or(false) {
        return;
    }
    let _ = std::fs::remove_file(path);
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if tap.is_null() {
        context
            .terminal
            .trigger(TerminalReason::EventTapTimeoutRecoveryFailed);
        return;
    }
    // SAFETY: owner test control runs on the tap's run loop. It genuinely
    // disables the live tap, verifies disabled state, then routes the exact
    // production disabled-event callback using the retained refcon.
    let disabled = unsafe {
        ffi::CGEventTapEnable(tap, false);
        !ffi::CGEventTapIsEnabled(tap)
    };
    let _ = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT,
            null_mut(),
            (context as *const CallbackContext).cast_mut().cast(),
        )
    };
    let reenabled = unsafe { ffi::CGEventTapIsEnabled(tap) };
    crate::platform::macos::record_macos_test_tap_disable(disabled, reenabled);
    if !disabled || !reenabled {
        context
            .terminal
            .trigger(TerminalReason::EventTapTimeoutRecoveryFailed);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn service_test_paste_barrier_pause(context: &CallbackContext) {
    if !context
        .test_paste_barrier_split_active
        .load(Ordering::Acquire)
        || !context
            .test_paste_barrier_down_observed
            .load(Ordering::Acquire)
    {
        return;
    }
    let (Some(paused), Some(release)) = (
        context.test_paste_barrier_paused.as_ref(),
        context.test_paste_barrier_release.as_ref(),
    ) else {
        return;
    };
    if !context
        .test_paste_barrier_pause_announced
        .swap(true, Ordering::AcqRel)
        && std::fs::write(paused, b"authenticated barrier down observed\n").is_err()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return;
    }
    if !release.try_exists().unwrap_or(false) {
        arm_maintenance_timer(context);
        return;
    }
    let _ = std::fs::remove_file(release);
    let posted = context.native_events.try_lock().is_ok_and(|mut events| {
        let Some(pool) = events.as_mut() else {
            return false;
        };
        injection::post_prepared_paste_barrier_up(pool);
        true
    });
    if posted {
        context
            .test_paste_barrier_split_active
            .store(false, Ordering::Release);
    } else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}
#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn test_modifier_barrier_contract() -> bool {
    paste_modifier_barrier_valid(Some(9), 9, true, true, true)
        && !paste_modifier_barrier_valid(Some(9), 10, true, true, true)
        && !paste_modifier_barrier_valid(Some(9), 9, true, false, true)
        && !paste_modifier_barrier_valid(Some(9), 9, true, true, false)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn test_permission_disable_recovery_contract() -> bool {
    owned_native_transition_is_terminal(true, true, true)
        && owned_native_transition_is_terminal(true, false, false)
        && !owned_native_transition_is_terminal(false, true, false)
        && shutdown_drain_action(
            true,
            Some(Instant::now() - Duration::from_nanos(1)),
            false,
            Instant::now(),
        ) == ShutdownDrainAction::ReportUnresponsive
}
