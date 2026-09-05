//! Submit deferred originals only after replay and paste ordering drains.

use super::*;

pub(super) fn deferred_native_ordering_drained(keyboard: &CallbackKeyboard) -> bool {
    keyboard.transactional.journal_len() == 0
        && keyboard.transactional.owned_letters() == 0
        && keyboard.transactional.pending_injected_cleanup().is_none()
        && keyboard.transactional.pending_menu_cleanup().is_none()
        && !has_session_ownership(keyboard)
        && !keyboard.gap_barrier_pending
        && keyboard.gap_barrier_token.is_none()
        && keyboard.replay_observation.is_none()
        && keyboard.inflight_effect.is_none()
}

pub(super) fn try_clear_recovery_deferred_mode(context: &CallbackContext) -> bool {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        return false;
    }
    let journal_drained = context
        .recovery_edges
        .try_lock()
        .is_ok_and(|journal| !journal.ordering_pending());
    let native_drained = context
        .keyboard
        .try_lock()
        .is_ok_and(|keyboard| deferred_native_ordering_drained(&keyboard));
    let paste_drained = context
        .pending_paste
        .try_lock()
        .is_ok_and(|pending| pending.is_none());
    if journal_drained && native_drained && paste_drained {
        context
            .state
            .recovery_deferred_mode
            .store(false, Ordering::Release);
        true
    } else {
        false
    }
}

pub(super) fn submit_deferred_edges_if_ready(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    if let Ok(mut keyboard) = context.keyboard.try_lock()
        && let Ok(mut journal) = context.recovery_edges.try_lock()
        && journal.submitted_len == 0
    {
        journal.resolve_uncertain_ownership(&mut keyboard);
        let _ = journal.expire_external_collection(Instant::now());
    }
    let deferred_ready = context.recovery_edges.try_lock().is_ok_and(|mut journal| {
        journal.settle_overflow();
        journal.ready_slice().is_some()
    });
    if !deferred_ready {
        if !try_clear_recovery_deferred_mode(context) {
            arm_maintenance_timer(context);
        }
        return;
    }
    let native_ordering_drained = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
        if !deferred_native_ordering_drained(&keyboard) {
            return false;
        }
        // Deferred physical originals updated the fixed native tracker but
        // intentionally bypassed matcher/admission. Reconcile that final
        // physical generation only after retained replay/cleanup observations
        // drained and before any deferred foreground repost.
        let snapshot = physical_snapshot(&keyboard);
        begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot)).applied
    });
    let paste_drained = context
        .pending_paste
        .try_lock()
        .is_ok_and(|pending| pending.is_none());
    if !native_ordering_drained || !paste_drained {
        arm_maintenance_timer(context);
        return;
    }

    let mut journal = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return;
        }
    };
    journal.settle_overflow();
    let bank = journal.next_pool_bank;
    let Some(edges) = journal.ready_slice() else {
        return;
    };
    let count = edges.len();
    let mut native_events = match context.native_events.try_lock() {
        Ok(events) => events,
        Err(_) => {
            drop(journal);
            arm_maintenance_timer(context);
            return;
        }
    };
    let Some(pool) = native_events.as_mut() else {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    };
    let Some(prepared) = injection::prepare_deferred_events(pool, bank, edges) else {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    };
    if !journal.begin_submission(prepared.token(), bank, count) {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    }
    drop(journal);
    prepared.post(pool);
    drop(native_events);
    arm_maintenance_timer(context);
}
