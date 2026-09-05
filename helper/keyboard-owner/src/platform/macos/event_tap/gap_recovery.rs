//! Reconcile hidden releases and retire tombstones through an exact barrier.

use super::*;

pub(super) fn reconcile_strict_drain_after_gap(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if keyboard.has_submitted_replay_authority() {
        // The disabled tap is re-enabled by the caller so the original HID
        // suffix can continue to reach this exact observation cursor. A gap
        // snapshot/control turn here would overwrite immutable authority.
        arm_maintenance_timer(context);
        return;
    }
    let owned_before = keyboard.transactional.owned_letters();
    let escape_owned = keyboard.session_escape_native_owned;
    let enter_owned = keyboard.session_enter_native_owned.is_some();
    let cleanup_pending = keyboard.transactional.pending_injected_cleanup().is_some()
        || keyboard.transactional.pending_menu_cleanup().is_some();
    let physical_fences_pending = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.physical_fences_pending());
    if owned_before == 0
        && !escape_owned
        && !enter_owned
        && !cleanup_pending
        && !physical_fences_pending
    {
        return;
    }

    // A disabled-tap notification is the proof of a callback gap. While this
    // owner callback is linearized, rebuild native state and let the shared
    // engine intersect committed ownership with authoritative held keys.
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(&keyboard);
    let _ = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    let cleared_letters = owned_before & !snapshot.held_letters;

    let escape_is_down = native_key_is_down(ESCAPE_KEY_CODE);
    let captured_enter = keyboard.session_enter_native_owned;
    let enter_is_down = captured_enter.is_some_and(native_key_is_down);
    let (escape_cleared, enter_cleared, _) =
        reconcile_hidden_session_native_ownership(&mut keyboard, escape_is_down, enter_is_down);

    if cleared_letters != 0 || escape_cleared || enter_cleared || physical_fences_pending {
        keyboard.gap_reconciled_letters |= cleared_letters;
        keyboard.gap_reconciled_escape |= escape_cleared;
        if enter_cleared {
            keyboard.gap_reconciled_enter_key_code = captured_enter;
        }
        keyboard.gap_barrier_pending = true;
        keyboard.gap_barrier_observed_down = false;
    }
    // Reconciliation may leave a retained helper-injected release obligation.
    // Retry it on the owner before deciding that shutdown is drain-complete.
    let _ = begin_transaction_control(context, &mut keyboard, Control::RetryCleanup);
}

pub(super) fn keep_strict_drain_tap_enabled(context: &CallbackContext) {
    let ownership_pending = pending_native_work(context);
    if !ownership_pending {
        return;
    }
    reconcile_strict_drain_after_gap(context);
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if tap.is_null() {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
        return;
    }
    // SAFETY: reconciliation runs while the tap is disabled. Re-enable only
    // after the owner snapshot and tombstones are installed.
    let enabled = unsafe {
        ffi::CGEventTapEnable(tap, true);
        ffi::CGEventTapIsEnabled(tap)
    };
    if !enabled {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
        return;
    }
    let barrier_needed = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
        if keyboard.gap_barrier_pending && keyboard.gap_barrier_token.is_none() {
            keyboard.gap_barrier_observed_down = false;
            true
        } else {
            false
        }
    });
    if barrier_needed {
        let Some((mut native_events, barrier)) =
            context
                .native_events
                .try_lock()
                .ok()
                .and_then(|mut events| {
                    let barrier = injection::prepare_gap_barrier(events.as_mut()?)?;
                    Some((events, barrier))
                })
        else {
            // No timer fallback may discard tombstones; retain drain and retry.
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return;
        };
        let installed = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
            if keyboard.gap_barrier_pending && keyboard.gap_barrier_token.is_none() {
                keyboard.gap_barrier_token = Some(barrier.token());
                true
            } else {
                false
            }
        });
        if installed {
            barrier.post(
                native_events
                    .as_mut()
                    .expect("native event pool prepared the gap barrier"),
            );
        }
    }
}

pub(super) fn complete_gap_barrier(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => return,
    };
    keyboard.clear_gap_tombstones();
    if let Ok(mut journal) = context.recovery_edges.try_lock() {
        journal.reconcile_physical_fences(native_key_is_down, native_mouse_button_is_down);
    }
    let shutdown_may_drain =
        context.state.stopping.load(Ordering::Acquire) && keyboard.shutdown_requested;
    drop(keyboard);
    let _ = try_clear_recovery_deferred_mode(context);
    if shutdown_may_drain {
        let _ = stop_owner_run_loop_if_drained(context);
    }
}
