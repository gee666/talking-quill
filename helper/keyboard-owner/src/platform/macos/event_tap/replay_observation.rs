//! Normal-callback replay observation; recovery classification stays separate.

use super::*;

pub(super) fn replay_observation(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    replay_operation: Option<(injection::OperationToken, bool)>,
) -> bool {
    let key_code =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let repeat = unsafe {
        ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
    };
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    if !injection::replay_shape_is_valid(event_type, key_code, repeat) {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return true;
    }
    let (observation, replay_disposition) = u16::try_from(key_code)
        .ok()
        .and_then(|key_code| {
            context.keyboard.try_lock().ok().map(|mut keyboard| {
                let classified =
                    keyboard.classify_replay_event(event_type, key_code, repeat, flags);
                if classified == GapBarrierObservation::Forged {
                    return (classified, CurrentEdgeDisposition::Owned);
                }
                let cleanup = replay_operation.is_some_and(|(_, cleanup)| cleanup);
                let target_is_current = !keyboard.candidate_target_captured
                    || keyboard.candidate_target.is_some_and(|reservation| {
                        context
                            .target_cache
                            .as_ref()
                            .is_some_and(|cache| cache.reservation_is_current(&reservation))
                    });
                let disposition =
                    keyboard.replay_disposition_after_target_check(cleanup || target_is_current);
                set_current_edge_disposition(context, &mut keyboard, disposition);
                let observation =
                    keyboard.observe_replay_event(event_type, key_code, repeat, flags);
                #[cfg(test)]
                if PANIC_AFTER_REPLAY_RECOGNITION.with(|flag| flag.replace(false)) {
                    panic!("induced panic after replay recognition");
                }
                if observation == GapBarrierObservation::Complete {
                    let completion = if keyboard.replay_target_cleanup_pending && cleanup {
                        finish_target_changed_replay(context, &mut keyboard)
                    } else if keyboard.replay_target_changed && !cleanup {
                        reconcile_target_changed_replay(context, &mut keyboard)
                    } else {
                        resume_observed_native_effect(context, &mut keyboard)
                    };
                    if !matches!(
                        completion,
                        DriveCompletion::Complete(_) | DriveCompletion::NativeObservationPending
                    ) {
                        context.terminal.trigger(TerminalReason::ReducerPoisoned);
                    }
                    set_current_edge_disposition(context, &mut keyboard, disposition);
                }
                (observation, disposition)
            })
        })
        .unwrap_or((GapBarrierObservation::Forged, CurrentEdgeDisposition::Owned));
    if observation == GapBarrierObservation::Forged {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return true;
    }
    invalidate_target_cache(context);
    #[cfg(feature = "transactional-shortcuts-dev")]
    if observation != GapBarrierObservation::Forged
        && context.test_physical_seam_enabled
        && let Some((token, cleanup)) = replay_operation
    {
        crate::platform::macos::record_test_marker_acknowledgement(
            if cleanup {
                crate::platform::macos::MacosTestOperationClass::Cleanup
            } else {
                crate::platform::macos::MacosTestOperationClass::Replay
            },
            token,
        );
    }
    if observation == GapBarrierObservation::Complete {
        let _ = try_clear_recovery_deferred_mode(context);
        if context.state.stopping.load(Ordering::Acquire) {
            let _ = stop_owner_run_loop_if_drained(context);
        }
    }
    return replay_disposition != CurrentEdgeDisposition::Pass;
}
