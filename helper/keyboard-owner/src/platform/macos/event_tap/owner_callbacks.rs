//! FFI command/timer entry points and maintenance scheduling.

use super::*;

/// # Safety
/// `info` is null or the live owner-thread refcon installed by `hook_thread`.
pub(super) unsafe extern "C" fn owner_command_perform(info: *mut c_void) {
    // SAFETY: Core Foundation invokes this with the retained source refcon.
    unsafe {
        run_owner_callback(info, |context| {
            if context.state.stopping.load(Ordering::Acquire) || context.terminal.is_triggered() {
                let _ =
                    resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
                context.state.quiescing.store(true, Ordering::Release);
                context.state.stopping.store(true, Ordering::Release);
                context
                    .state
                    .session_capture_mode
                    .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
                cancel_pending_pastes(context);
                begin_owner_shutdown(context);
            } else {
                let _ = resolve_pending_activation(
                    context,
                    PendingActivationResolution::ForceTargetless,
                );
                process_owner_commands(context);
                process_paste_commands(context);
            }
        })
    };
}

/// # Safety
/// `info` is null or the live owner-thread refcon installed by `hook_thread`.
pub(super) unsafe extern "C" fn maintenance_timer_callback(
    _timer: ffi::CFRunLoopTimerRef,
    info: *mut c_void,
) {
    // SAFETY: Core Foundation invokes this with the retained timer refcon.
    unsafe {
        run_owner_callback(info, |context| {
            #[cfg(feature = "transactional-shortcuts-dev")]
            {
                service_test_tap_disable_request(context);
                service_test_paste_barrier_pause(context);
            }
            submit_deferred_edges_if_ready(context);
            monitor_owned_native_state(context);
            if context.terminal.is_triggered() {
                context.state.quiescing.store(true, Ordering::Release);
                context.state.stopping.store(true, Ordering::Release);
                context
                    .state
                    .session_capture_mode
                    .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
            }
            if context.state.stopping.load(Ordering::Acquire) {
                let _ =
                    resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
                cancel_pending_pastes(context);
            } else {
                let _ = resolve_pending_activation(context, PendingActivationResolution::Poll);
                poll_pending_paste(context);
            }
            poll_shutdown_drain(context);
            park_maintenance_timer_if_idle(context);
        })
    };
}

/// # Safety
/// A non-null `info` must point to a live, aligned `CallbackContext` on its
/// owner thread, with no exclusive borrow lasting through this invocation.
/// The source/timer must be invalidated before that context is dropped.
pub(super) unsafe fn run_owner_callback(
    info: *mut c_void,
    callback: impl FnOnce(&CallbackContext),
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() {
            return;
        }
        // SAFETY: info is the boxed callback context retained through source cleanup.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        if context.state.recovery_pending.load(Ordering::Acquire)
            && !attempt_pending_recovery(context)
        {
            return;
        }
        callback(context);
    }));
    if result.is_err() && !info.is_null() {
        // SAFETY: context remains alive through source invalidation. Never stop
        // the run loop directly: the retained authoritative engine must first
        // replay a candidate or enter strict owned-up drain.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        recover_owner_unwind(context);
    }
}

pub(super) fn arm_maintenance_timer(context: &CallbackContext) {
    let _ = pending_native_work(context);
    let timer = context.state.maintenance_timer.load(Ordering::Acquire);
    if timer.is_null() {
        return;
    }
    // SAFETY: the owner timer is retained until run-loop cleanup.
    unsafe {
        ffi::CFRunLoopTimerSetNextFireDate(
            timer,
            ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
        );
    }
}

pub(super) fn park_maintenance_timer_if_idle(context: &CallbackContext) {
    if pending_native_work(context) {
        return;
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_tap_disable_request.is_some() {
        let timer = context.state.maintenance_timer.load(Ordering::Acquire);
        if !timer.is_null() {
            unsafe {
                ffi::CFRunLoopTimerSetNextFireDate(
                    timer,
                    ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
                );
            }
        }
        return;
    }
    let timer = context.state.maintenance_timer.load(Ordering::Acquire);
    if !timer.is_null() {
        // SAFETY: the timer is retained until owner cleanup.
        unsafe {
            ffi::CFRunLoopTimerSetNextFireDate(
                timer,
                ffi::CFAbsoluteTimeGetCurrent() + PARKED_TIMER_SECONDS,
            );
        }
    }
}
