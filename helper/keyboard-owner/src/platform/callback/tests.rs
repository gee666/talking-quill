use super::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, ActivationKey, ProfileId, Shortcut,
    ShortcutModifiers,
};

fn test_shortcut() -> Shortcut {
    Shortcut::new(
        ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        },
        &[ActivationKey::Z],
    )
    .unwrap()
}

#[test]
fn callback_delivery_queues_the_typed_event_without_closing_the_gate() {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, terminal_rx) = crossbeam_channel::bounded(1);
    let terminal = TerminalSignal::new(Arc::clone(&gate), terminal_tx);
    let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(1);
    let event = KeyboardEvent::Activation {
        binding: ActivationBinding::new(ProfileId::GENERAL, test_shortcut()),
        context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
        phase: talking_quill_keyboard_core::EventPhase::Down,
    };
    assert!(deliver_callback_event(&outbound_tx, &terminal, event));
    assert_eq!(outbound_rx.try_recv(), Ok(NativeEvent::Keyboard(event)));
    assert!(gate.is_open());
    assert!(!terminal.is_triggered());
    assert!(terminal_rx.try_recv().is_err());
}

#[test]
fn callback_delivery_lease_linearizes_against_gate_close() {
    let gate = CallbackGate::new();
    gate.open();
    let lease = gate.try_acquire_delivery().expect("open gate lease");
    gate.close();
    assert!(!gate.is_open());
    assert!(gate.try_acquire_delivery().is_none());
    drop(lease);
    assert_eq!(gate.state.load(Ordering::Acquire), CALLBACK_GATE_CLOSED);
}

#[test]
fn callback_queue_failure_closes_gate_and_is_terminal() {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, terminal_rx) = crossbeam_channel::bounded(1);
    let terminal = TerminalSignal::new(Arc::clone(&gate), terminal_tx);
    let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(1);
    drop(outbound_rx);

    assert!(!deliver_callback_event(
        &outbound_tx,
        &terminal,
        KeyboardEvent::Activation {
            binding: ActivationBinding::new(ProfileId::GENERAL, test_shortcut()),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
            phase: talking_quill_keyboard_core::EventPhase::Down,
        },
    ));
    assert!(!gate.is_open());
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
}

#[test]
fn callback_up_failure_is_terminal_but_current_captured_up_stays_swallowed() {
    use talking_quill_keyboard_core::{KeyInput, KeyPhase, KeyboardReducer, PhysicalKey};

    let key = PhysicalKey::Letter(ActivationKey::A);
    let mut reducer = KeyboardReducer::default();
    let down = reducer.plan(
        KeyInput {
            key,
            phase: KeyPhase::Down,
            modifiers: talking_quill_keyboard_core::ModifierMask::new(false, true, false, false),
            repeat: false,
            injected: false,
        },
        ActivationKey::A,
        true,
        SessionCaptureMode::Off,
    );
    assert!(reducer.apply(down, true));

    let up = reducer.plan(
        KeyInput {
            key,
            phase: KeyPhase::Up,
            modifiers: talking_quill_keyboard_core::ModifierMask::default(),
            repeat: false,
            injected: false,
        },
        ActivationKey::A,
        true,
        SessionCaptureMode::Off,
    );
    let event = up.event().unwrap();

    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, terminal_rx) = crossbeam_channel::bounded(1);
    let terminal = TerminalSignal::new(Arc::clone(&gate), terminal_tx);
    let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(1);
    drop(outbound_rx);
    let delivered = deliver_callback_event(&outbound_tx, &terminal, event);

    assert!(!delivered);
    assert!(!gate.is_open());
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
    assert!(reducer.apply(up, delivered));
}

#[test]
fn new_callback_terminal_reasons_round_trip_through_signal() {
    for reason in [
        TerminalReason::OutboundEncodingUnavailable,
        TerminalReason::EventTapTimeoutRecoveryFailed,
        TerminalReason::EventTapRepeatedTimeout,
        TerminalReason::EventTapDisabledByUserInput,
        TerminalReason::ActivationConfigurationUnavailable,
        TerminalReason::OwnerThreadUnresponsive,
        TerminalReason::AudioDeviceMonitorUnavailable,
        TerminalReason::InputInjectionUnavailable,
    ] {
        let gate = Arc::new(CallbackGate::new());
        let (sender, _receiver) = crossbeam_channel::bounded(1);
        let terminal = TerminalSignal::new(gate, sender);
        terminal.trigger(reason);
        assert_eq!(terminal.reason(), Some(reason));
    }
}
