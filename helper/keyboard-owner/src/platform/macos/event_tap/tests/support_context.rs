//! Shared context fixtures.

use super::*;

pub(super) trait FnLockPoison {
    fn poison(&self);
}

impl<T> FnLockPoison for RecoveringMutex<T> {
    fn poison(&self) {
        let _guard = self.lock().unwrap();
        panic!("induced callback-critical mutex poison");
    }
}

pub(super) fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
    Shortcut::new(modifiers, keys).unwrap()
}

pub(super) fn activation_context() -> ActivationContext {
    ActivationContext::target_unavailable(ActivationGeneration::FIRST)
}

pub(super) fn bindings() -> ActivationBindings {
    ActivationBindings::new(&[
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X, ActivationKey::P],
            ),
        ),
        ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: true,
                    alt: false,
                    shift: true,
                    meta: false,
                },
                &[ActivationKey::P],
            ),
        ),
    ])
    .unwrap()
}

pub(super) fn test_context_with_capacity(
    outbound_capacity: usize,
) -> (CallbackContext, Receiver<NativeEvent>, Sender<OwnerCommand>) {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, _terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound, outbound_rx) = bounded(outbound_capacity);
    let (command_tx, owner_commands) = bounded(4);
    let (_paste_tx, paste_commands) = bounded(1);
    (
        CallbackContext {
            state: Arc::new(SharedState::new()),
            suppression_enabled: true,
            keyboard: RecoveringMutex::new(CallbackKeyboard {
                activation: ActivationConfig {
                    enabled: true,
                    bindings: bindings(),
                },
                ..CallbackKeyboard::default()
            }),
            pending_activation: RecoveringMutex::new(None),
            native_events: RecoveringMutex::new(None),
            injection_identity: Some(injection::InjectionIdentity::for_test(42)),
            test_physical_seam_enabled: false,
            callback_proxy: AtomicPtr::new(null_mut()),
            current_edge_disposition: AtomicU8::new(CurrentEdgeDisposition::Pass as u8),
            owner_commands,
            paste_commands,
            pending_paste: RecoveringMutex::new(None),
            recovery_edges: RecoveringMutex::new(RecoveryEdgeJournal::default()),
            target_cache: None,
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_tap_disable_request: None,
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_paste_barrier_paused: None,
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_paste_barrier_release: None,
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_paste_barrier_split_active: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_paste_barrier_down_observed: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "transactional-shortcuts-dev")]
            test_paste_barrier_pause_announced: std::sync::atomic::AtomicBool::new(false),
            forced_activation_reservation: None,
            outbound,
            gate,
            terminal,
            observability: Arc::new(TransactionObservability::new()),
        },
        outbound_rx,
        command_tx,
    )
}

pub(super) fn test_context() -> (CallbackContext, Receiver<NativeEvent>, Sender<OwnerCommand>) {
    test_context_with_capacity(8)
}

pub(super) fn receive_event(receiver: &Receiver<NativeEvent>) -> KeyboardEvent {
    match receiver.try_recv().unwrap() {
        NativeEvent::Keyboard(event) => event,
        other => panic!("unexpected outbound: {other:?}"),
    }
}

pub(super) fn deliver_test_activation(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    notice: ActivationNotice,
) -> bool {
    keyboard
        .dispatcher
        .deliver(&context.outbound, &context.terminal, notice, None)
}

pub(super) fn apply_config(
    context: &CallbackContext,
    commands: &Sender<OwnerCommand>,
    activation: ActivationConfig,
) {
    let state = Arc::new(std::sync::atomic::AtomicU8::new(
        OwnerCommandState::Pending as u8,
    ));
    let (acknowledgement, response) = bounded(1);
    commands
        .send(OwnerCommand {
            mutation: OwnerMutation {
                kind: OwnerMutationKind::Configure,
                activation,
                session_capture_mode: SessionCaptureMode::Off,
            },
            state: Arc::clone(&state),
            acknowledgement,
        })
        .unwrap();
    process_owner_commands(context);
    assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
    assert!(response.recv().unwrap().is_ok());
}
