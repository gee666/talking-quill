//! FFI event callback dispatch and panic containment.

use super::*;

/// # Safety
/// `user_info` is null or the live owner-thread context installed on this tap.
/// Ordinary events and their proxy remain valid for this invocation. Disabled
/// tap notifications may have a null event. Sources are invalidated before the
/// boxed context or native event pool is released.
pub(super) unsafe extern "C" fn event_tap_callback(
    proxy: ffi::CGEventTapProxy,
    event_type: u32,
    event: ffi::CGEventRef,
    user_info: *mut c_void,
) -> ffi::CGEventRef {
    let handled = std::panic::catch_unwind(|| {
        if user_info.is_null() {
            return false;
        }
        // SAFETY: user_info points to the boxed context retained for the tap lifetime.
        let context = unsafe { &*user_info.cast::<CallbackContext>() };
        // A closed process-lifetime gate is a passive, listen-only observer.
        // Bypass all reducer, ownership, recovery, and notification paths even
        // if a future configuration bug attempts to arm native capture.
        if !context.suppression_enabled {
            return false;
        }
        context.callback_proxy.store(proxy, Ordering::Release);
        let _proxy_guard = CallbackProxyGuard(&context.callback_proxy);
        if let Some(disposition) = observe_deferred_repost(context, event_type, event) {
            return disposition != CurrentEdgeDisposition::Pass;
        }
        if context.state.recovery_pending.load(Ordering::Acquire)
            && !attempt_pending_recovery(context)
        {
            return classify_recovery_drain_event(context, event_type, event)
                != CurrentEdgeDisposition::Pass;
        }
        if let Some(disposition) = defer_nonhelper_if_ordered(context, event_type, event) {
            set_atomic_current_edge_disposition(context, disposition);
            return disposition != CurrentEdgeDisposition::Pass;
        }
        let mouse_down = matches!(
            event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
                | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
                | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
        );
        if !mouse_down && !observe_normal_mouse_transition(context, event_type, event) {
            recover_callback_unwind(context);
            return true;
        }
        set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Pass);
        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            keyboard.current_edge_disposition = CurrentEdgeDisposition::Pass;
        }
        if mouse_down {
            return process_mouse_down(context, proxy, event_type, event);
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            handle_user_input_tap_disable(context);
            return false;
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
            let submitted_replay = context
                .keyboard
                .try_lock()
                .is_ok_and(|keyboard| keyboard.has_submitted_replay_authority());
            if !submitted_replay {
                let _ = resolve_pending_activation(
                    context,
                    if context.state.stopping.load(Ordering::Acquire)
                        || context.terminal.is_triggered()
                    {
                        PendingActivationResolution::FailDelivery
                    } else {
                        PendingActivationResolution::ForceTargetless
                    },
                );
            }
            invalidate_target_cache(context);
            let ownership_pending = pending_native_work(context);
            if ownership_pending {
                close_owned_native_admission(
                    context,
                    TerminalReason::EventTapTimeoutRecoveryFailed,
                    CancelReason::SecureDesktop,
                );
                keep_strict_drain_tap_enabled(context);
                return false;
            }
            if context.state.quiescing.load(Ordering::Acquire) {
                keep_strict_drain_tap_enabled(context);
                return false;
            }
            let tap = context.state.event_tap.load(Ordering::Acquire);
            let recovered = if tap.is_null() {
                false
            } else {
                // SAFETY: tap is owned by the active hook thread.
                unsafe {
                    ffi::CGEventTapEnable(tap, true);
                    ffi::CGEventTapIsEnabled(tap)
                }
            };
            let decision = apply_tap_recovery(
                context,
                if recovered {
                    TapRecoveryEvent::TimeoutRecovered
                } else {
                    TapRecoveryEvent::TimeoutRecoveryFailed
                },
            );
            if recovered && decision == TapRecoveryDecision::Continue {
                resynchronize_after_gap(context);
            } else {
                keep_strict_drain_tap_enabled(context);
            }
            return false;
        }
        if event.is_null() {
            return false;
        }
        // SAFETY: Core Graphics guarantees a valid event for ordinary callbacks.
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        let Some(injection_identity) = context.injection_identity else {
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        };
        let test_physical_source = recovery_test_physical_source(context, marker);
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled && marker == injection::TEST_PERMISSION_LOSS_MARKER {
            crate::platform::macos::set_macos_test_permission_loss(true);
            monitor_owned_native_state(context);
            return true;
        }
        if source_pid == injection_identity.source_pid && !test_physical_source {
            context
                .current_edge_disposition
                .store(CurrentEdgeDisposition::Owned as u8, Ordering::Release);
            if let Ok(mut keyboard) = context.keyboard.try_lock() {
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
            }
        }
        let gap_token = context
            .keyboard
            .try_lock()
            .ok()
            .and_then(|keyboard| keyboard.gap_barrier_token);
        if injection::token_matches(injection_identity, gap_token, marker, source_pid) {
            return gap_observation(context, event_type, event, gap_token);
        }
        let paste_barrier_token = context.pending_paste.try_lock().ok().and_then(|pending| {
            pending
                .as_ref()
                .and_then(|command| command.neutral_barrier_token)
        });
        if injection::token_matches(injection_identity, paste_barrier_token, marker, source_pid) {
            return paste_observation(context, event_type, event, paste_barrier_token);
        }
        let replay_operation = context.keyboard.try_lock().ok().and_then(|keyboard| {
            keyboard.replay_observation.as_ref().map(|work| {
                (
                    work.token,
                    matches!(work.batch, ExpectedReplayBatch::Cleanup(_)),
                )
            })
        });
        let replay_token = replay_operation.map(|(token, _)| token);
        if injection::token_matches(injection_identity, replay_token, marker, source_pid) {
            return replay_observation(context, event_type, event, replay_operation);
        }
        if source_pid == injection_identity.source_pid && !test_physical_source {
            // A delayed token from a completed operation, or any own-process
            // event that does not exactly match the currently installed
            // generation and shape, is never accepted as an acknowledgement.
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        }
        let source = if test_physical_source {
            InputSource::Physical
        } else {
            injection::unmarked_source(injection_identity, source_pid)
        };
        if !resolve_pending_activation(context, PendingActivationResolution::ForceTargetless) {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return true;
        }
        // SAFETY: keycode, timestamp, flags and autorepeat are defined for the
        // keyboard event types included in this tap.
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let Ok(key_code) = u16::try_from(key_code) else {
            return false;
        };
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        let event_timestamp = unsafe { ffi::CGEventGetTimestamp(event) };
        let native_repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        if source == InputSource::External
            && matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
            )
            && let Ok(mut journal) = context.recovery_edges.try_lock()
        {
            journal.normal_external_key_transition(
                source_pid,
                key_code,
                event_type == ffi::K_CG_EVENT_KEY_DOWN,
                native_repeat,
            );
        }
        let candidate_start = source.is_physical()
            && matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
            )
            && context.keyboard.try_lock().is_ok_and(|keyboard| {
                keyboard.transactional.event_starts_candidate(
                    transactional_key_identity(key_code),
                    if event_type == ffi::K_CG_EVENT_KEY_UP {
                        PhysicalPhase::Up
                    } else if native_repeat {
                        PhysicalPhase::Repeat
                    } else {
                        PhysicalPhase::Down
                    },
                )
            });
        let activation_reservation = candidate_start
            .then(|| {
                context
                    .target_cache
                    .as_ref()
                    .and_then(TargetCache::reserve_activation)
            })
            .flatten();
        #[cfg(test)]
        let activation_reservation =
            activation_reservation.or(context.forced_activation_reservation);
        // Reserve only at a possible activation boundary, then establish the
        // exact event epoch before any validation request can be queued.
        invalidate_target_cache(context);
        apply_tap_recovery(context, TapRecoveryEvent::Activity);
        process_transactional_event(
            context,
            CallbackEvent {
                event_ref: event,
                marker,
                event_type,
                key_code,
                native_repeat,
                flags,
                event_timestamp,
                source,
                source_pid,
                test_physical_source,
            },
            activation_reservation,
        )
    });

    match handled {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            if !user_info.is_null() {
                // SAFETY: context remains alive until owner-thread tap cleanup.
                let context = unsafe { &*user_info.cast::<CallbackContext>() };
                return match recover_callback_unwind(context) {
                    CurrentEdgeDisposition::Pass => event,
                    CurrentEdgeDisposition::Owned | CurrentEdgeDisposition::Replaced => null_mut(),
                };
            }
            event
        }
    }
}
