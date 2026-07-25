use std::sync::Arc;

use crossbeam_channel::{TryRecvError, bounded};
use talking_quill_helper::platform::{CallbackGate, TerminalReason, TerminalSignal};

#[test]
fn terminal_signal_closes_gate_and_reports_only_first_failure() {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (sender, receiver) = bounded(1);
    let terminal = TerminalSignal::new(Arc::clone(&gate), sender);

    terminal.trigger(TerminalReason::CallbackPanicked);
    assert!(!gate.is_open());
    assert!(terminal.is_triggered());
    assert_eq!(receiver.try_recv(), Ok(TerminalReason::CallbackPanicked));

    terminal.trigger(TerminalReason::ReducerPoisoned);
    assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(terminal.reason(), Some(TerminalReason::CallbackPanicked));
}

#[test]
fn terminal_signal_is_nonblocking_when_receiver_is_gone() {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (sender, receiver) = bounded(1);
    drop(receiver);
    let terminal = TerminalSignal::new(Arc::clone(&gate), sender);

    terminal.trigger(TerminalReason::OutboundQueueUnavailable);
    assert!(!gate.is_open());
    assert!(terminal.is_triggered());
}
