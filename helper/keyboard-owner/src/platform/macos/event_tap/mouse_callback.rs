//! Mouse cancellation preserves replay-before-focus-change ordering.

use super::*;

pub(super) fn repost_current_mouse_after_replay(
    proxy: ffi::CGEventTapProxy,
    event: ffi::CGEventRef,
) -> bool {
    #[cfg(test)]
    if proxy.is_null() {
        return true;
    }
    // SAFETY: production callers provide the live callback proxy and the
    // current event remains valid for the complete callback invocation.
    unsafe { ffi::CGEventTapPostEvent(proxy, event) };
    true
}

pub(super) fn process_mouse_down(
    context: &CallbackContext,
    proxy: ffi::CGEventTapProxy,
    event_type: u32,
    event: ffi::CGEventRef,
) -> bool {
    let _ = resolve_pending_activation(context, PendingActivationResolution::ForceTargetless);
    invalidate_target_cache(context);
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            recover_callback_unwind(context);
            return true;
        }
    };
    if keyboard.transactional.journal_len() == 0 {
        drop(keyboard);
        if !observe_normal_mouse_transition(context, event_type, event) {
            recover_callback_unwind(context);
            return true;
        }
        return false;
    }
    let outcome = begin_transaction_control(
        context,
        &mut keyboard,
        Control::Cancel(CancelReason::InvalidContinuation),
    );
    if !outcome.applied || keyboard.last_native_effect_failed {
        let native_failed = keyboard.last_native_effect_failed
            || outcome.cancellation == Some(CancelReason::EffectProtocolViolation);
        drop(keyboard);
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        let source = if recovery_test_physical_source(context, marker) {
            InputSource::test_physical()
        } else {
            context
                .injection_identity
                .map_or(InputSource::Physical, |identity| {
                    injection::unmarked_source(identity, source_pid)
                })
        };
        if native_failed {
            fail_recovery_edge_journal(context);
        } else {
            context
                .state
                .recovery_deferred_mode
                .store(true, Ordering::Release);
        }
        set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Owned);
        let _ = defer_callback_edge(
            context, event_type, event, source, source_pid, marker, false,
        );
        return true;
    }
    set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Replaced);
    drop(keyboard);
    // Replay was inserted through this same proxy first. Repost the
    // suppressed mouse event only afterwards, preserving replay ->
    // focus-change ordering without copying or allocating in callback.
    if repost_current_mouse_after_replay(proxy, event) {
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled {
            crate::platform::macos::record_test_mouse_repost();
        }
        if !observe_normal_mouse_transition(context, event_type, event) {
            recover_callback_unwind(context);
        }
    }
    return true;
}
