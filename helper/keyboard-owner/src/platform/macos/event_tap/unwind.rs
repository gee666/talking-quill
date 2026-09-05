//! Contain callback/owner unwinds without discarding submitted obligations.

use super::*;

pub(super) fn enter_owner_lifecycle_recovery(context: &CallbackContext) {
    if callback_recovery_requires_deferred_mode(context) {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.terminal.trigger(TerminalReason::CallbackPanicked);
}
/// Attempts recovery once and rechecks the release-published gate. `false`
/// permits only rearming wake resources or classifying drain edges, not ordinary
/// owner controls, effects, or admission.
pub(super) fn attempt_pending_recovery(context: &CallbackContext) -> bool {
    let _ = recover_unwind(context, false);
    let recovered = !context.state.recovery_pending.load(Ordering::Acquire);
    if !recovered {
        arm_maintenance_timer(context);
    }
    recovered
}
pub(super) fn callback_recovery_requires_deferred_mode(context: &CallbackContext) -> bool {
    let journal_ordering = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.ordering_pending());
    let keyboard_ordering = context.keyboard.try_lock().map_or(true, |keyboard| {
        keyboard.transactional.journal_len() != 0
            || keyboard.transactional.owned_letters() != 0
            || keyboard.inflight_effect.is_some()
            || keyboard.replay_observation.is_some()
            || keyboard.transactional.pending_injected_cleanup().is_some()
            || keyboard.transactional.pending_menu_cleanup().is_some()
            || has_session_ownership(&keyboard)
            || keyboard.gap_barrier_pending
            || keyboard.gap_barrier_token.is_some()
    });
    let paste_ordering = context
        .pending_paste
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    journal_ordering || keyboard_ordering || paste_ordering
}

pub(super) fn recover_callback_unwind(context: &CallbackContext) -> CurrentEdgeDisposition {
    recover_unwind(context, callback_recovery_requires_deferred_mode(context))
}

pub(super) fn recover_owner_unwind(context: &CallbackContext) -> CurrentEdgeDisposition {
    recover_unwind(context, callback_recovery_requires_deferred_mode(context))
}

pub(super) fn recover_unwind(
    context: &CallbackContext,
    enable_deferred_mode: bool,
) -> CurrentEdgeDisposition {
    if enable_deferred_mode {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.terminal.trigger(TerminalReason::CallbackPanicked);

    // Recovery is itself an FFI-boundary operation. A second unwind must never
    // escape the callback; leave poison set and retain drain ownership so the
    // owner timer can retry instead of pretending teardown is safe.
    let recovered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Preflight every callback-critical lock before mutating a continuation
        // or control state. Owner/event callbacks are serialized on this run
        // loop, so a successful preflight makes a later WouldBlock impossible
        // without re-entrancy; failed preflight leaves all submitted outcomes
        // and continuations untouched for the permanent-wake retry.
        drop(context.keyboard.try_lock().ok()?);
        drop(context.pending_activation.try_lock().ok()?);
        drop(context.pending_paste.try_lock().ok()?);
        drop(context.recovery_edges.try_lock().ok()?);
        drop(context.native_events.try_lock().ok()?);

        let mut recovered_inflight = false;
        let mut preserve_current_cleanup_pass = false;
        let mut keyboard = context.keyboard.try_lock().ok()?;
        {
            let mut recovery_edges = context.recovery_edges.try_lock().ok()?;
            recovery_edges.resolve_uncertain_ownership(&mut keyboard);
        }
        if let Some(inflight) = keyboard.inflight_effect.take() {
            #[cfg(test)]
            if PANIC_ONCE_DURING_RECOVERY.with(|flag| flag.replace(false)) {
                keyboard.inflight_effect = Some(inflight);
                panic!("induced second panic during recovery");
            }
            if let Some(target_recovery) = target_changed_replay_recovery(&keyboard, &inflight) {
                keyboard.inflight_effect = Some(inflight);
                let completion = match target_recovery {
                    TargetChangedReplayRecovery::AwaitObservation => None,
                    TargetChangedReplayRecovery::ReconcileVisibleDowns => {
                        Some(reconcile_target_changed_replay(context, &mut keyboard))
                    }
                    TargetChangedReplayRecovery::FinishAfterCleanup => {
                        preserve_current_cleanup_pass = atomic_current_edge_disposition(context)
                            == CurrentEdgeDisposition::Pass;
                        Some(finish_target_changed_replay(context, &mut keyboard))
                    }
                };
                if let Some(completion) = completion {
                    match completion {
                        DriveCompletion::Complete(_) => recovered_inflight = true,
                        DriveCompletion::NativeObservationPending => {}
                        DriveCompletion::ActivationDeferred | DriveCompletion::Failed => {
                            return None;
                        }
                    }
                }
            } else if inflight.outcome.is_none() && keyboard.replay_observation.is_some() {
                // HID submission is not completion. Preserve the continuation
                // untouched until the exact observation cursor reaches its end.
                keyboard.inflight_effect = Some(inflight);
            } else {
                let outcome = inflight
                    .outcome
                    .unwrap_or_else(|| failed_effect_outcome(inflight.effect));
                let turn = inflight.continuation.resume(outcome);
                let _ = drive_transaction_turn(context, &mut keyboard, turn, None);
                recovered_inflight = true;
            }
        }
        drop(keyboard);
        if recovered_inflight {
            let mut pending = context.pending_activation.try_lock().ok()?;
            if pending
                .as_ref()
                .is_some_and(|pending| pending.resolved_delivery.is_some())
            {
                let _ = pending.take();
            }
        }
        let awaiting_exact_native_observation = context
            .keyboard
            .try_lock()
            .ok()?
            .has_submitted_replay_authority();
        if !awaiting_exact_native_observation
            && !resolve_pending_activation(context, PendingActivationResolution::FailDelivery)
        {
            return None;
        }
        let mut keyboard = context.keyboard.try_lock().ok()?;
        if !awaiting_exact_native_observation {
            let _ = begin_transaction_control(
                context,
                &mut keyboard,
                Control::CloseAdmission(CancelReason::EffectProtocolViolation),
            );
            if !reconcile_recovery_locked(context, &mut keyboard) {
                return None;
            }
        }
        if preserve_current_cleanup_pass {
            // The exact cleanup up was authenticated and made visible before
            // the unwind. Terminal replay completion must not retroactively
            // suppress that already-authoritative current edge.
            set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Pass);
        }
        drop(keyboard);

        // Revalidate every lock after recovery effects commit. No evidence or
        // pooled event is discarded here, and poison remains set if a nested
        // callback somehow made a lock unavailable.
        drop(context.pending_activation.try_lock().ok()?);
        drop(context.pending_paste.try_lock().ok()?);
        drop(context.recovery_edges.try_lock().ok()?);
        drop(context.native_events.try_lock().ok()?);
        keep_strict_drain_tap_enabled(context);
        arm_maintenance_timer(context);
        Some(())
    }))
    .ok()
    .flatten()
    .is_some();

    if recovered {
        context.keyboard.clear_poison();
        context.pending_activation.clear_poison();
        context.pending_paste.clear_poison();
        context.recovery_edges.clear_poison();
        context.native_events.clear_poison();
        context
            .state
            .recovery_pending
            .store(false, Ordering::Release);
        let _ = try_clear_recovery_deferred_mode(context);
    } else {
        // Permanent source/timer resources make this bounded wake safe even if
        // a nested callback currently holds one lock.
        arm_maintenance_timer(context);
    }
    atomic_current_edge_disposition(context)
}
