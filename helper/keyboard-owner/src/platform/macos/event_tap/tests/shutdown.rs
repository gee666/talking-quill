//! Shutdown contracts.

use super::*;

#[test]
fn shutdown_deadline_selects_terminal_incomplete_instead_of_success() {
    let now = Instant::now();
    let expired = Some(now - Duration::from_millis(1));
    assert_eq!(
        shutdown_drain_action(true, expired, false, now),
        ShutdownDrainAction::ReportUnresponsive
    );
    assert_eq!(
        shutdown_drain_action(false, expired, false, now),
        ShutdownDrainAction::Stop
    );
}

#[test]
fn shutdown_candidate_retains_semantic_ownership_without_posting_replay() {
    let Turn::Complete {
        engine,
        completion: Completion::Control(outcome),
    } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Shutdown))
    else {
        panic!("candidate shutdown must complete without an injection effect");
    };
    assert!(matches!(
        outcome.shutdown,
        ShutdownState::Draining { owned_letters } if owned_letters != 0
    ));
    assert_ne!(engine.owned_letters(), 0);
    assert_eq!(engine.journal_len(), 0);

    let (context, _outbound, _commands) = test_context();
    context.state.stopping.store(true, Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.shutdown_requested = true;
        keyboard.transactional = engine;
    }
    assert!(pending_native_work(&context));
    assert_eq!(
        shutdown_drain_action(true, None, false, Instant::now()),
        ShutdownDrainAction::Continue
    );
}
