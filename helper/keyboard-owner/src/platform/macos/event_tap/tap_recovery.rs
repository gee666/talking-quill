//! Tap disable policy, target boundaries, and native state resynchronization.

use super::*;

pub(super) fn invalidate_target_cache(context: &CallbackContext) {
    if let Some(cache) = &context.target_cache {
        cache.invalidate_boundary();
    }
}

pub(super) fn apply_tap_recovery(
    context: &CallbackContext,
    event: TapRecoveryEvent,
) -> TapRecoveryDecision {
    let policy = TapRecoveryPolicy::from_consecutive_timeouts(
        context.state.tap_recovery.load(Ordering::Acquire),
    );
    let (next, decision) = policy.observe(event);
    context
        .state
        .tap_recovery
        .store(next.consecutive_timeouts(), Ordering::Release);
    if let TapRecoveryDecision::Terminal(reason) = decision {
        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            let _ = begin_transaction_control(
                context,
                &mut keyboard,
                Control::CloseAdmission(CancelReason::SecureDesktop),
            );
            deliver_balancing_events(context, &mut keyboard.reducer);
        }
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context.terminal.trigger(reason);
    }
    decision
}

pub(super) fn handle_user_input_tap_disable(context: &CallbackContext) {
    let submitted_replay = context
        .keyboard
        .try_lock()
        .is_ok_and(|keyboard| keyboard.has_submitted_replay_authority());
    if !submitted_replay {
        let _ = resolve_pending_activation(
            context,
            if context.state.stopping.load(Ordering::Acquire) || context.terminal.is_triggered() {
                PendingActivationResolution::FailDelivery
            } else {
                PendingActivationResolution::ForceTargetless
            },
        );
    }
    invalidate_target_cache(context);
    if !context.state.quiescing.load(Ordering::Acquire) {
        let _ = apply_tap_recovery(context, TapRecoveryEvent::DisabledByUserInput);
    }
    // User-input disable is a callback gap. If ownership survives, re-enable
    // only the strict drain tap after HID reconciliation.
    keep_strict_drain_tap_enabled(context);
}

pub(super) fn resynchronize_after_gap(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return;
        }
    };
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(&keyboard);
    let outcome = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}
pub(super) fn reconcile_locked(context: &CallbackContext, keyboard: &mut CallbackKeyboard) {
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(keyboard);
    let outcome = begin_transaction_control(context, keyboard, Control::Reconcile(snapshot));
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

pub(super) fn reconcile_recovery_locked(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> bool {
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return true;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    let mut recovery_edges = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return false;
        }
    };
    keyboard
        .transactional
        .retire_released_owned_letters(recovery_edges.released_letter_bits());
    keyboard.seed_from_state(|key_code| {
        recovery_edges.physical_newer_generation_held(key_code)
            || (!recovery_edges.force_old_generation_released(key_code)
                && native_key_is_down(key_code))
    });
    let snapshot = physical_snapshot(keyboard);
    let outcome = begin_transaction_control(context, keyboard, Control::Reconcile(snapshot));
    if outcome.applied {
        // Clear only after the reconciled successor commits. A nested unwind
        // retains exact release generations for the next permanent-wake retry.
        recovery_edges.clear_owned_releases();
    }
    drop(recovery_edges);
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    outcome.applied
}
