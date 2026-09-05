//! Unwind authority contracts.

use super::*;

#[test]
fn transaction_snapshot_survives_an_induced_effect_executor_unwind() {
    let keyboard = CallbackKeyboard::default();
    let authoritative = keyboard.transactional.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let turn = begin_transaction_snapshot(
            &keyboard.transactional,
            EngineInput::Control(Control::RetryCleanup),
        );
        assert!(matches!(turn, Turn::Complete { .. }));
        panic!("injected effect executor panic");
    }));
    assert!(result.is_err());
    assert_eq!(keyboard.transactional, authoritative);
}

#[test]
fn unwind_passes_only_an_unowned_current_edge() {
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Pass);
    }
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Pass
    );
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Replaced);
    }
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Replaced
    );
}

#[test]
fn callback_recovery_restores_and_clears_every_critical_poisoned_lock() {
    let (context, _outbound, _commands) = test_context();
    for poison in [
        &context.pending_activation as &dyn FnLockPoison,
        &context.pending_paste as &dyn FnLockPoison,
        &context.recovery_edges as &dyn FnLockPoison,
        &context.native_events as &dyn FnLockPoison,
    ] {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poison.poison()));
    }
    assert!(context.pending_activation.is_poisoned());
    assert!(context.pending_paste.is_poisoned());
    assert!(context.recovery_edges.is_poisoned());
    assert!(context.native_events.is_poisoned());
    let _ = recover_callback_unwind(&context);
    assert!(!context.pending_activation.is_poisoned());
    assert!(!context.pending_paste.is_poisoned());
    assert!(!context.recovery_edges.is_poisoned());
    assert!(!context.native_events.is_poisoned());
    assert!(!pending_native_work(&context));
}
