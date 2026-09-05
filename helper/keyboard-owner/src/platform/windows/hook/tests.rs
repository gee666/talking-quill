//! Shared fixtures for the Windows hook tests. Behavioral tests live in topic modules.

use super::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, EventPhase, ProfileId, SessionKey,
    Shortcut, ShortcutModifiers,
};

mod activation_fencing;
mod activation_matching;
mod altgr;
mod deferred_replay;
mod input_sources;
mod key_tracking;
mod lifecycle;
mod owner_commands;
mod panic_recovery;
mod paste;
mod registered_observation;
mod serialized_input;
mod session_capture;
mod shutdown;
mod transactional_activation;

fn shared_state_for_test() -> SharedState {
    SharedState::new()
}

fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
    Shortcut::new(modifiers, keys).unwrap()
}

fn activation_context() -> ActivationContext {
    ActivationContext::target_unavailable(ActivationGeneration::FIRST)
}

fn full_bindings() -> ActivationBindings {
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

fn test_context(
    outbound_capacity: usize,
) -> (
    CallbackContext,
    Receiver<NativeEvent>,
    Receiver<TerminalReason>,
) {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound, outbound_rx) = bounded(outbound_capacity);
    let context = CallbackContext {
        state: Arc::new(shared_state_for_test()),
        keyboard: Mutex::new(CallbackKeyboard {
            activation: ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
            input_desktop: current_input_desktop(),
            ..CallbackKeyboard::default()
        }),
        owner_epoch: Instant::now(),
        suppression_enabled: true,
        outbound,
        gate,
        terminal,
        observability: Arc::new(TransactionObservability::new()),
        injection_markers: injection::InjectionMarkers::generate().unwrap(),
        replay_sender: None,
        replay_accepted: Arc::new(AtomicU64::new(0)),
    };
    (context, outbound_rx, terminal_rx)
}

fn record(context: &CallbackContext, virtual_key: u16, key: PhysicalKey, phase: KeyPhase) -> bool {
    let (scan_code, extended) = match key {
        PhysicalKey::Letter(letter) => (LETTER_SCAN_CODES[usize::from(letter.index())], false),
        PhysicalKey::Escape => (0x01, false),
        PhysicalKey::Enter => (0x1C, false),
        PhysicalKey::Other => (0, false),
    };
    process_hook_record(context, virtual_key, scan_code, extended, phase, false)
}

fn transactional_record(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    source: InputSource,
    observed_at_ms: u64,
) -> bool {
    let disposition = Cell::new(CallbackDisposition::Pass);
    transactional_record_with_disposition(
        context,
        virtual_key,
        scan_code,
        extended,
        phase,
        source,
        observed_at_ms,
        &disposition,
    )
}

#[allow(clippy::too_many_arguments)]
fn transactional_record_with_disposition(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    source: InputSource,
    observed_at_ms: u64,
    disposition: &Cell<CallbackDisposition>,
) -> bool {
    process_transactional_hook_record_at(
        context,
        TransactionalHookRecord {
            virtual_key,
            scan_code,
            extended,
            platform_flags: 0,
            phase,
            source,
        },
        HookObservation {
            observed_at_ms,
            native_modifiers: None,
        },
        disposition,
    )
}

fn shadow_letter(context: &CallbackContext, key: ActivationKey, phase: KeyPhase) -> bool {
    transactional_record(
        context,
        u16::from(b'A') + u16::from(key.index()),
        LETTER_SCAN_CODES[usize::from(key.index())],
        false,
        phase,
        InputSource::Physical,
        0,
    )
}

fn shadow_modifier(context: &CallbackContext, virtual_key: u16, phase: KeyPhase) -> bool {
    let (scan_code, extended) = match virtual_key {
        VK_LSHIFT => (0x2A, false),
        VK_LMENU => (0x38, false),
        _ => (0, false),
    };
    transactional_record(
        context,
        virtual_key,
        scan_code,
        extended,
        phase,
        InputSource::Physical,
        0,
    )
}

fn enter(context: &CallbackContext, source: EnterSource, phase: KeyPhase) -> bool {
    process_hook_record(
        context,
        VK_RETURN,
        0x1C,
        source == EnterSource::Numpad,
        phase,
        false,
    )
}

fn modifier(context: &CallbackContext, virtual_key: u16, phase: KeyPhase) -> bool {
    process_hook_record(context, virtual_key, 0, false, phase, false)
}

fn receive_event(receiver: &Receiver<NativeEvent>) -> KeyboardEvent {
    match receiver.recv_timeout(Duration::from_millis(50)).unwrap() {
        NativeEvent::Keyboard(event) => event,
        other => panic!("unexpected outbound: {other:?}"),
    }
}

fn apply_mutation(context: &CallbackContext, mutation: OwnerMutation) {
    let (command_tx, command_rx) = bounded(1);
    let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
    let (acknowledgement, response) = bounded(1);
    command_tx
        .send(OwnerCommand {
            mutation,
            state: Arc::clone(&state),
            acknowledgement,
        })
        .unwrap();
    process_owner_commands(context, &command_rx);
    assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
    assert!(response.recv().unwrap().is_ok());
}

fn apply_config(context: &CallbackContext, activation: ActivationConfig) {
    apply_mutation(context, OwnerMutation::configure(activation));
}
