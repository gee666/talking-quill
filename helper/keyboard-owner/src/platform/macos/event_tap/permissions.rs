//! Close admission on native permission, secure-input, or observation failure.

use super::*;

pub(super) const fn owned_native_transition_is_terminal(
    ownership_pending: bool,
    secure_input: bool,
    permissions_available: bool,
) -> bool {
    ownership_pending && (secure_input || !permissions_available)
}

pub(super) fn insertion_has_irreversible_authority(status: InsertionStatus) -> bool {
    matches!(
        status,
        InsertionStatus::Claimed | InsertionStatus::Ambiguous
    )
}

pub(super) fn irreversible_native_ownership(context: &CallbackContext) -> bool {
    let keyboard_owned = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| keyboard_has_pending_native_work(&keyboard));
    let deferred_owned = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.has_pending());
    let paste_owned = context.pending_paste.try_lock().map_or(true, |pending| {
        pending.as_ref().is_some_and(|command| {
            (command.neutral_barrier_token.is_some() && command.neutral_barrier_state < 2)
                || command
                    .insertion_request
                    .as_ref()
                    .is_some_and(|request| insertion_has_irreversible_authority(request.status()))
        })
    });
    context.state.recovery_pending.load(Ordering::Acquire)
        || context.state.recovery_deferred_mode.load(Ordering::Acquire)
        || keyboard_owned
        || deferred_owned
        || paste_owned
}

pub(super) fn native_permissions_available() -> bool {
    let granted = permissions_allow_native_input(permission_snapshot());
    #[cfg(feature = "transactional-shortcuts-dev")]
    {
        granted && !crate::platform::macos::macos_test_permission_loss_active()
    }
    #[cfg(not(feature = "transactional-shortcuts-dev"))]
    {
        granted
    }
}

pub(super) fn close_owned_native_admission(
    context: &CallbackContext,
    reason: TerminalReason,
    cancellation: CancelReason,
) {
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.terminal.trigger(reason);
    if let Ok(mut keyboard) = context.keyboard.try_lock()
        && !keyboard.has_submitted_replay_authority()
    {
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::CloseAdmission(cancellation),
        );
    }
}

pub(super) fn monitor_owned_native_state(context: &CallbackContext) {
    let replay_observation_timed_out = context.keyboard.try_lock().map_or(true, |keyboard| {
        keyboard
            .replay_observation
            .as_ref()
            .is_some_and(|observation| Instant::now() >= observation.deadline)
    });
    let deferred_observation_timed_out =
        context.recovery_edges.try_lock().map_or(true, |journal| {
            journal
                .submission_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        });
    if deferred_observation_timed_out && let Ok(mut journal) = context.recovery_edges.try_lock() {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    // A fully observed barrier followed by Pending AX work is still safely
    // cancellable clipboard-only. Only actual native/replay ownership or an
    // explicit AX claim makes a Secure Input/permission transition terminal.
    let ownership_pending = irreversible_native_ownership(context);
    let secure = secure_input_active();
    let permissions_available = native_permissions_available();
    if !replay_observation_timed_out
        && !deferred_observation_timed_out
        && !owned_native_transition_is_terminal(ownership_pending, secure, permissions_available)
    {
        return;
    }
    close_owned_native_admission(
        context,
        if replay_observation_timed_out || deferred_observation_timed_out {
            TerminalReason::InputInjectionUnavailable
        } else if secure {
            TerminalReason::EventTapDisabledByUserInput
        } else {
            TerminalReason::InputInjectionUnavailable
        },
        CancelReason::SecureDesktop,
    );
    keep_strict_drain_tap_enabled(context);
}
