//! Recovery-only classification without ordinary matcher admission.
//!
//! This single classifier intentionally exceeds 350 lines. Keeping authenticated
//! drain observations and physical-generation retirement in one decision tree
//! makes its fail-closed returns and lock-release order auditable.

use super::*;

/// Allocation-free, nonblocking classifier used only while callback recovery
/// remains unresolved. It advances already-installed exact drain observations
/// and retires exact native ownership, but never invokes matcher/admission,
/// submits an effect, or begins a control turn.
pub(super) fn classify_recovery_drain_event(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> CurrentEdgeDisposition {
    if callback_recovery_requires_deferred_mode(context) {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    // Suppress by default. Every Pass below is based on exact operation state or
    // a source/shape that cannot belong to retained helper/native ownership.
    set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Owned);

    if matches!(
        event_type,
        ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT | ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
    ) {
        let tap = context.state.event_tap.load(Ordering::Acquire);
        if !tap.is_null() {
            // Keep the drain tap alive without reconciliation/control while
            // poison recovery is unresolved.
            unsafe { ffi::CGEventTapEnable(tap, true) };
        }
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }
    if event.is_null() {
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    if is_mouse_event_type(event_type) {
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        if context.injection_identity.is_some_and(|identity| {
            source_pid == identity.source_pid && !recovery_test_physical_source(context, marker)
        }) {
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        }
        if let Some(disposition) = defer_nonhelper_if_ordered(context, event_type, event) {
            return recovery_drain_disposition(context, disposition);
        }
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }

    if !matches!(
        event_type,
        ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
    ) {
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }

    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let key_code_raw =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
        && unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
        };
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let Some(identity) = context.injection_identity else {
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    };
    let test_physical_source = recovery_test_physical_source(context, marker);

    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled && marker == injection::TEST_PERMISSION_LOSS_MARKER {
        // Test controls are ordinary work and cannot run during recovery.
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    if source_pid == identity.source_pid && !test_physical_source {
        let Ok(key_code) = u16::try_from(key_code_raw) else {
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        };

        // The target-specific AX path retains only the neutral barrier token.
        if let Ok(mut pending) = context.pending_paste.try_lock()
            && let Some(command) = pending.as_mut()
            && injection::token_matches(identity, command.neutral_barrier_token, marker, source_pid)
        {
            let observation = observe_exact_pair(
                &mut command.neutral_barrier_state,
                event_type,
                key_code,
                127,
                repeat,
                flags,
                0,
            );
            #[cfg(feature = "transactional-shortcuts-dev")]
            if observation != GapBarrierObservation::Forged
                && context.test_physical_seam_enabled
                && let Some(token) = command.neutral_barrier_token
            {
                crate::platform::macos::record_test_marker_acknowledgement(
                    crate::platform::macos::MacosTestOperationClass::PasteBarrier,
                    token,
                );
            }
            let _ = observation;
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        }

        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            let gap_token = keyboard.gap_barrier_token;
            if injection::token_matches(identity, gap_token, marker, source_pid) {
                let observation =
                    keyboard.observe_gap_barrier_event(event_type, key_code, repeat, flags);
                #[cfg(feature = "transactional-shortcuts-dev")]
                if observation != GapBarrierObservation::Forged
                    && context.test_physical_seam_enabled
                    && let Some(token) = gap_token
                {
                    crate::platform::macos::record_test_marker_acknowledgement(
                        crate::platform::macos::MacosTestOperationClass::GapBarrier,
                        token,
                    );
                }
                if observation == GapBarrierObservation::Complete
                    && let Ok(mut journal) = context.recovery_edges.try_lock()
                {
                    journal
                        .reconcile_physical_fences(native_key_is_down, native_mouse_button_is_down);
                }
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
                arm_maintenance_timer(context);
                return CurrentEdgeDisposition::Owned;
            }

            let replay = keyboard.replay_observation.as_ref().map(|observation| {
                (
                    observation.token,
                    matches!(observation.batch, ExpectedReplayBatch::Cleanup(_)),
                )
            });
            if injection::token_matches(
                identity,
                replay.map(|(token, _)| token),
                marker,
                source_pid,
            ) {
                let observation =
                    keyboard.classify_replay_event(event_type, key_code, repeat, flags);
                if observation != GapBarrierObservation::Forged {
                    let cleanup = replay.is_some_and(|(_, cleanup)| cleanup);
                    let target_is_current = !keyboard.candidate_target_captured
                        || keyboard.candidate_target.is_some_and(|reservation| {
                            context
                                .target_cache
                                .as_ref()
                                .is_some_and(|cache| cache.reservation_is_current(&reservation))
                        });
                    let disposition = keyboard
                        .replay_disposition_after_target_check(cleanup || target_is_current);
                    // Publish the exact foreground disposition before advancing
                    // the authenticated cursor. A target change suppresses the
                    // entire remaining suffix instead of exposing it elsewhere.
                    set_current_edge_disposition(context, &mut keyboard, disposition);
                    let advanced =
                        keyboard.observe_replay_event(event_type, key_code, repeat, flags);
                    debug_assert_eq!(advanced, observation);
                    if advanced == GapBarrierObservation::Complete {
                        let completion = if keyboard.replay_target_cleanup_pending && cleanup {
                            Some(finish_target_changed_replay(context, &mut keyboard))
                        } else if keyboard.replay_target_changed && !cleanup {
                            Some(reconcile_target_changed_replay(context, &mut keyboard))
                        } else {
                            (!mark_observed_native_effect(&mut keyboard))
                                .then_some(DriveCompletion::Failed)
                        };
                        if completion.is_some_and(|completion| {
                            !matches!(
                                completion,
                                DriveCompletion::Complete(_)
                                    | DriveCompletion::NativeObservationPending
                            )
                        }) {
                            context.terminal.trigger(TerminalReason::ReducerPoisoned);
                        }
                    }
                    #[cfg(feature = "transactional-shortcuts-dev")]
                    if context.test_physical_seam_enabled
                        && let Some((token, cleanup)) = replay
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
                    arm_maintenance_timer(context);
                    return disposition;
                }
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
            }
        }

        // Own-process events are never unrelated. Busy/poison-unresolved state,
        // stale generations, and malformed exact shapes all remain suppressed.
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    let source = if test_physical_source {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    if !source.is_physical() {
        if recovery_ordering_exists(context) {
            return defer_callback_edge(
                context, event_type, event, source, source_pid, marker, false,
            );
        }
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }
    let Ok(key_code) = u16::try_from(key_code_raw) else {
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    };
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            // Preserve the complete scalar edge in the independent journal.
            // Recovery resolves whether it belonged to old native ownership
            // before any deferred batch can be submitted.
            return defer_callback_edge(
                context, event_type, event, source, source_pid, marker, true,
            );
        }
    };
    keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;

    if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        // `defer_callback_edge` owns side-specific HID/tracker/source-model
        // advancement. Never infer one side from the aggregate family flag.
        drop(keyboard);
        return defer_callback_edge(
            context, event_type, event, source, source_pid, marker, false,
        );
    }

    let phase = if event_type == ffi::K_CG_EVENT_KEY_UP {
        KeyPhase::Up
    } else {
        KeyPhase::Down
    };
    if keyboard.handle_gap_tombstone(key_code, phase, repeat) {
        drop(keyboard);
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    let key = map_key_code(key_code);
    let transactional_owned = match key {
        PhysicalKey::Letter(letter) => {
            keyboard.transactional.owned_letters() & (1_u32 << u32::from(letter.index())) != 0
        }
        PhysicalKey::Escape | PhysicalKey::Enter | PhysicalKey::Other => false,
    };
    let session_owned = match key {
        PhysicalKey::Escape => keyboard.session_escape_native_owned,
        PhysicalKey::Enter => keyboard.session_enter_native_owned == Some(key_code),
        PhysicalKey::Letter(_) | PhysicalKey::Other => false,
    };
    let ordering_sensitive =
        keyboard.transactional.journal_len() != 0 || keyboard.inflight_effect.is_some();

    let mut fresh_owned_generation = false;
    if transactional_owned {
        let mut journal = match context.recovery_edges.try_lock() {
            Ok(journal) => journal,
            Err(_) => {
                drop(keyboard);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        if phase == KeyPhase::Up && !journal.physical_newer_generation_held(key_code) {
            journal.record_owned_release(key_code);
        } else if journal.owned_release_recorded(key_code) {
            fresh_owned_generation = (phase == KeyPhase::Down && !repeat)
                || journal.physical_newer_generation_held(key_code);
        }
    }
    if phase == KeyPhase::Up && session_owned {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = None;
                keyboard.captured_enter_key_code = None;
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
        let escape_still_down = keyboard.session_escape_native_owned;
        let enter_still_down = keyboard.session_enter_native_owned.is_some();
        let _ = keyboard
            .reducer
            .reconcile_hidden_session_releases(escape_still_down, enter_still_down);
    }

    // A nonrepeat down retires a gap tombstone and starts a new generation.
    // Once an old owned up was observed, that fresh generation and all of its
    // repeats/up are deferred rather than confused with stale ownership.
    let deferred_ordering = ordering_sensitive
        || context
            .recovery_edges
            .try_lock()
            .map_or(true, |journal| journal.ordering_pending());
    let owned_old_edge = (transactional_owned && !fresh_owned_generation) || session_owned;
    if !owned_old_edge && (deferred_ordering || fresh_owned_generation) {
        drop(keyboard);
        return defer_callback_edge(
            context, event_type, event, source, source_pid, marker, false,
        );
    }
    let _ = keyboard.physical.observe(key_code, phase);
    let disposition = if owned_old_edge {
        CurrentEdgeDisposition::Owned
    } else {
        CurrentEdgeDisposition::Pass
    };
    keyboard.current_edge_disposition = disposition;
    drop(keyboard);
    arm_maintenance_timer(context);
    recovery_drain_disposition(context, disposition)
}
