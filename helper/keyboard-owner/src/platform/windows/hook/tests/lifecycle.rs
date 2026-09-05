use super::*;
use std::collections::VecDeque;

#[test]
fn auxiliary_audio_fault_cannot_close_keyboard_capture() {
    let keyboard_gate = Arc::new(CallbackGate::new());
    keyboard_gate.open();
    let (keyboard_sender, _keyboard_events) = bounded(1);
    let keyboard_terminal = TerminalSignal::new(Arc::clone(&keyboard_gate), keyboard_sender);
    let (audio_gate, audio_terminal) = isolated_audio_terminal();

    audio_terminal.trigger(TerminalReason::AudioDeviceMonitorUnavailable);

    assert!(audio_terminal.is_triggered());
    assert!(!audio_gate.is_open());
    assert!(!keyboard_terminal.is_triggered());
    assert!(keyboard_gate.is_open());
}

#[test]
fn closed_process_gate_bypasses_every_physical_callback_source() {
    assert!(!callback_may_process_source(false, InputSource::Physical));
    assert!(!callback_may_process_source(
        false,
        InputSource::test_physical()
    ));
    assert!(!callback_may_process_source(false, InputSource::External));
    assert!(callback_may_process_source(true, InputSource::Physical));
}

#[test]
fn low_level_hook_uses_no_injectable_module() {
    assert!(low_level_hook_module().is_null());
}

#[test]
fn owner_wake_retries_post_failures_and_stops_after_exhaustion() {
    let mut outcomes = VecDeque::from([false, false, true]);
    let mut delays = Vec::new();
    assert!(retry_owner_wake(
        || outcomes.pop_front().unwrap_or(false),
        |delay| delays.push(delay),
    ));
    assert_eq!(delays, OWNER_WAKE_RETRY_DELAYS[..2]);

    let mut attempts = 0;
    let mut delays = Vec::new();
    assert!(!retry_owner_wake(
        || {
            attempts += 1;
            false
        },
        |delay| delays.push(delay),
    ));
    assert_eq!(attempts, OWNER_WAKE_RETRY_DELAYS.len() + 1);
    assert_eq!(delays, OWNER_WAKE_RETRY_DELAYS);
}

#[test]
fn startup_handoff_has_exclusive_running_or_cancelled_outcomes() {
    let cancelled = AtomicU8::new(StartupState::Pending as u8);
    assert_eq!(cancel_startup(&cancelled), StartupState::Cancelled);
    assert!(!claim_startup(&cancelled));

    let running = AtomicU8::new(StartupState::Pending as u8);
    assert!(claim_startup(&running));
    assert_eq!(cancel_startup(&running), StartupState::Running);
}

#[test]
fn owner_completion_wait_is_bounded_and_accepts_normal_completion() {
    let (completed_tx, completed_rx) = bounded(1);
    completed_tx.send(()).unwrap();
    assert!(owner_completed(&completed_rx, Duration::from_millis(1)));

    let (_pending_tx, pending_rx) = bounded(1);
    assert!(!owner_completed(&pending_rx, Duration::from_millis(1)));
}

#[test]
fn outbound_failure_passes_current_trigger_and_every_later_record() {
    let (context, _outbound, terminal) = test_context(0);
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert_eq!(
        terminal.recv_timeout(Duration::from_millis(50)).unwrap(),
        TerminalReason::OutboundQueueUnavailable
    );
    assert!(!context.gate.is_open());
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
}
