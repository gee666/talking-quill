use std::collections::VecDeque;

use super::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, EventPhase, ProfileId, SessionKey,
    Shortcut, ShortcutModifiers,
};

fn shared_state_for_test() -> SharedState {
    SharedState::new()
}

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
fn paste_target_evidence_starts_closed_until_all_native_hooks_install() {
    let state = SharedState::new();
    assert!(!state.target_change_evidence_ready.load(Ordering::Acquire));
}

#[test]
fn paste_result_slot_is_the_only_irreversible_commit_authority() {
    let success = PasteResultSlot::new();
    assert!(success.result().is_none());
    assert!(
        success
            .publish(PasteResult {
                submitted: true,
                reason: None,
            })
            .submitted,
    );
    assert!(
        success
            .publish(failed_paste(PasteFailure::Unavailable))
            .submitted,
        "a later disconnect/failure cannot revoke accepted SendInput",
    );

    let failure = PasteResultSlot::new();
    assert!(
        !failure
            .publish(failed_paste(PasteFailure::OsRejected))
            .submitted
    );
    assert!(
        !failure
            .publish(PasteResult {
                submitted: true,
                reason: None,
            })
            .submitted,
        "state alone cannot fabricate submitted:true after rejection",
    );
}

#[test]
fn caller_timeout_wins_the_final_waiting_to_injecting_race() {
    let state = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert_eq!(cancel_paste_command(&state), PasteCommandState::Cancelled);
    assert!(!claim_paste_injection(&state));
    assert_eq!(paste_command_state(&state), PasteCommandState::Cancelled);
}

#[test]
fn post_cas_completion_wait_is_bounded_and_reports_indeterminate() {
    let (_sender, response) = bounded::<()>(1);
    let result = PasteResultSlot::new();
    let started = Instant::now();
    assert_eq!(
        wait_for_claimed_paste_completion(&response, &result, Duration::from_millis(5)),
        failed_paste(PasteFailure::Indeterminate)
    );
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn final_injection_claim_wins_the_cancellation_race_authoritatively() {
    let state = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert!(claim_paste_injection(&state));
    assert_eq!(cancel_paste_command(&state), PasteCommandState::Injecting);
    assert_eq!(paste_command_state(&state), PasteCommandState::Injecting);
}

#[test]
fn exact_send_input_rejection_leaves_injecting_before_cleanup() {
    let state = AtomicU8::new(PasteCommandState::Injecting as u8);
    let result = PasteResultSlot::new();
    let mut keyboard = CallbackKeyboard::default();
    let published = publish_initial_paste_acceptance(
        &state,
        &result,
        &mut keyboard,
        injection::test_paste_initial_outcome(injection::InjectionMarkers::generate().unwrap(), 0),
        || {},
    );
    assert!(!published.submitted);
    assert_eq!(paste_command_state(&state), PasteCommandState::ResultReady);
    assert!(keyboard.pending_paste_cleanup.is_empty());
}

#[test]
fn paste_commit_is_published_before_cleanup_pause() {
    let state = AtomicU8::new(PasteCommandState::Injecting as u8);
    let result = PasteResultSlot::new();
    let mut keyboard = CallbackKeyboard::default();
    let mut paused = false;

    let published = publish_initial_paste_acceptance(
        &state,
        &result,
        &mut keyboard,
        injection::test_paste_initial_outcome(injection::InjectionMarkers::generate().unwrap(), 2),
        || {
            paused = true;
            assert!(result.result().is_some_and(|result| result.submitted));
            assert_eq!(paste_command_state(&state), PasteCommandState::Committed);
        },
    );

    assert!(paused);
    assert!(published.submitted);
    assert!(!keyboard.pending_paste_cleanup.is_empty());
    assert!(
        result
            .publish(failed_paste(PasteFailure::Unavailable))
            .submitted,
        "cleanup failure after the pause cannot revoke commitment",
    );
}

#[test]
fn session_native_ownership_participates_in_shutdown_drain() {
    let mut keyboard = CallbackKeyboard {
        session_escape_native_owned: true,
        ..CallbackKeyboard::default()
    };
    assert!(!transaction_obligations_drained(&keyboard));
    keyboard.session_escape_native_owned = false;
    keyboard.captured_enter_source = Some(EnterSource::Numpad);
    assert!(!transaction_obligations_drained(&keyboard));
    keyboard.captured_enter_source = None;
    assert!(transaction_obligations_drained(&keyboard));
}

#[test]
fn shutdown_never_retires_session_ownership_without_the_exact_up() {
    let (context, _outbound, _terminal) = test_context(2);
    let mut keyboard = context.keyboard.lock().unwrap();
    keyboard.session_escape_native_owned = true;
    assert!(!transaction_obligations_drained(&keyboard));

    let _ = process_session_event(
        &context,
        &mut keyboard,
        PhysicalKey::Escape,
        KeyPhase::Up,
        false,
        None,
    );
    assert!(transaction_obligations_drained(&keyboard));
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

#[test]
fn callback_racing_deferred_replay_passes_without_replacing_authority() {
    let (context, _outbound, _terminal) = test_context(2);
    let mut keyboard = context.keyboard.lock().unwrap();
    let before_journal_len = keyboard.transactional.journal_len();
    keyboard.transaction_authority = Some(TransactionAuthority::AwaitingDeferredReplay);
    drop(keyboard);

    let disposition = Cell::new(CallbackDisposition::Capture);
    let captured = process_transactional_hook_record_at(
        &context,
        TransactionalHookRecord {
            virtual_key: 0x41,
            scan_code: 0x1e,
            extended: false,
            platform_flags: 0,
            phase: KeyPhase::Down,
            source: InputSource::Physical,
        },
        HookObservation {
            observed_at_ms: 1,
            native_modifiers: None,
        },
        &disposition,
    );

    let keyboard = context.keyboard.lock().unwrap();
    assert!(!captured);
    assert_eq!(disposition.get(), CallbackDisposition::Pass);
    assert!(matches!(
        keyboard.transaction_authority,
        Some(TransactionAuthority::AwaitingDeferredReplay)
    ));
    assert_eq!(keyboard.transactional.journal_len(), before_journal_len);
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
fn paste_deadline_distinguishes_modifier_wait_from_other_native_work() {
    assert_eq!(
        paste_deadline_reason(false),
        PasteFailure::ConflictingModifiers
    );
    assert_eq!(paste_deadline_reason(true), PasteFailure::Unavailable);
}

#[test]
fn stale_paste_context_counts_target_fallback_without_exposing_evidence() {
    let (context, _outbound, _terminal) = test_context(8);
    let (sender, receiver) = bounded(1);
    let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
    let result = Arc::new(PasteResultSlot::new());
    let (acknowledgement, _acknowledged) = bounded(1);
    sender
        .send(PasteCommand {
            context: activation_context(),
            expected_clipboard_sha256: ClipboardTextHash::from_bytes([0; 32]),
            injection_deadline: Instant::now() + Duration::from_secs(1),
            state,
            result: Arc::clone(&result),
            acknowledgement,
        })
        .unwrap();
    let mut pending = None;

    process_paste_commands(&context, &receiver, &mut pending);

    assert_eq!(
        result.result(),
        Some(failed_paste(PasteFailure::Unavailable))
    );
    assert_eq!(
        context
            .observability
            .snapshot()
            .native_paste
            .target_validation_fallbacks,
        1
    );
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

#[test]
fn gateway_eof_retains_external_candidate_until_exact_up_drain() {
    let (context, _outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        u16::from(b'X'),
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    publish_pending_native_work(&context);
    assert!(context.state.pending_native_work.load(Ordering::Acquire));

    apply_mutation(&context, OwnerMutation::close_admission());
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.journal_len(), 1);
        assert_eq!(
            keyboard.transactional.owned_letters(),
            1 << ActivationKey::X.index()
        );
    }

    apply_mutation(&context, OwnerMutation::cancel_candidate());
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.journal_len(), 0);
        assert_eq!(
            keyboard.transactional.owned_letters(),
            1 << ActivationKey::X.index()
        );
        assert_eq!(keyboard.transactional.metrics().replayed, 0);
    }
    assert!(context.state.pending_native_work.load(Ordering::Acquire));
    assert_ne!(
        context.keyboard.lock().unwrap().external_held_letters & (1 << ActivationKey::X.index()),
        0,
    );
    let release_disposition = Cell::new(CallbackDisposition::Pass);
    assert!(process_transactional_hook_record_at(
        &context,
        TransactionalHookRecord {
            virtual_key: u16::from(b'X'),
            scan_code: LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            extended: false,
            platform_flags: 0,
            phase: KeyPhase::Up,
            source: InputSource::External,
        },
        HookObservation {
            observed_at_ms: 3,
            // Simulate GetAsyncKeyState lagging the serialized external Alt
            // edge. The callback release must remain authoritative.
            native_modifiers: Some(ModifierTracker::default()),
        },
        &release_disposition,
    ));
    assert_eq!(release_disposition.get(), CallbackDisposition::Capture);
    assert_eq!(context.keyboard.lock().unwrap().external_held_letters, 0);
    publish_pending_native_work(&context);
    assert!(!context.state.pending_native_work.load(Ordering::Acquire));
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Up,
        InputSource::External,
        4,
    ));
    assert!(terminal.try_recv().is_err());
}

#[test]
fn gateway_eof_target_change_retains_external_candidate_until_exact_up() {
    let (context, _outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        u16::from(b'X'),
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    context.keyboard.lock().unwrap().candidate_target_changed = true;
    apply_mutation(&context, OwnerMutation::close_admission());
    apply_mutation(&context, OwnerMutation::cancel_candidate());
    publish_pending_native_work(&context);
    assert!(context.state.pending_native_work.load(Ordering::Acquire));
    assert!(transactional_record(
        &context,
        u16::from(b'X'),
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Up,
        InputSource::External,
        3,
    ));
    publish_pending_native_work(&context);
    assert!(!context.state.pending_native_work.load(Ordering::Acquire));
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Up,
        InputSource::External,
        4,
    ));
    assert!(terminal.try_recv().is_err());
}

#[test]
fn registered_observation_callback_is_exact_release_only_and_always_passes_through() {
    let (context, outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );

    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(
        outbound.try_recv().is_err(),
        "candidate or match emitted proof"
    );
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    let observed = context.observability.snapshot().registered_input;
    assert_eq!(
        (
            observed.registered_candidate_callbacks,
            observed.registered_match_callbacks,
            observed.registered_release_callbacks,
            observed.callback_channel_accepted,
        ),
        (1, 1, 1, 1),
    );
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 1 }
    ));
    assert!(outbound.try_recv().is_err());
    assert!(terminal.try_recv().is_err());

    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.journal_len(), 0);
    assert!(keyboard.dispatcher.active.is_none());
    assert!(keyboard.candidate_target.is_none());
    assert!(keyboard.pending_paste_cleanup.is_empty());
    drop(keyboard);
    let counters = context.observability.snapshot().registered_input;
    assert_eq!(counters.registered_candidate_callbacks, 1);
    assert_eq!(counters.registered_match_callbacks, 1);
    assert_eq!(counters.registered_release_callbacks, 1);
    assert_eq!(counters.callback_channel_accepted, 1);
    assert_eq!(counters.callback_channel_rejected, 0);
}

#[test]
fn registered_observation_callback_rejection_never_succeeds_or_activates() {
    let (context, outbound, _terminal) = test_context(0);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());
    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.journal_len(), 0);
    assert!(keyboard.dispatcher.active.is_none());
    assert!(keyboard.candidate_target.is_none());
    drop(keyboard);
    let counters = context.observability.snapshot().registered_input;
    assert_eq!(counters.callback_channel_accepted, 0);
    assert_eq!(counters.callback_channel_rejected, 1);
}

#[test]
fn registered_observation_handles_repeats_and_cancels_modifier_changes_or_wrong_releases() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));

    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Down));
    assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Up));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());

    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 3 }
    ));
}

#[test]
fn registered_observation_preserves_shared_prefix_candidate_sets() {
    let shared = ActivationBindings::new(&[
        ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X],
            ),
        ),
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
    ])
    .unwrap();
    let (context, outbound, _terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: shared,
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 1 }
    ));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 2 }
    ));
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
fn owner_commands_apply_full_config_and_capture_in_fifo_order() {
    let (context, _outbound, _terminal) = test_context(4);
    let (command_tx, command_rx) = bounded(4);
    let updated = ActivationConfig {
        enabled: true,
        bindings: ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: false,
                    shift: false,
                    meta: true,
                },
                &[ActivationKey::Q, ActivationKey::P],
            ),
        )])
        .unwrap(),
    };
    let mut states = Vec::new();
    let mut responses = Vec::new();
    for mutation in [
        OwnerMutation::configure(updated),
        OwnerMutation::set_session_capture(SessionCaptureMode::Recording),
    ] {
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (ack, response) = bounded(1);
        command_tx
            .send(OwnerCommand {
                mutation,
                state: Arc::clone(&state),
                acknowledgement: ack,
            })
            .unwrap();
        states.push(state);
        responses.push(response);
    }

    process_owner_commands(&context, &command_rx);

    assert_eq!(context.keyboard.lock().unwrap().activation, updated);
    assert_eq!(
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
        SessionCaptureMode::Recording,
    );
    for state in states {
        assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
    }
    for response in responses {
        assert!(response.recv().unwrap().is_ok());
    }
}

#[test]
fn cancelled_owner_command_never_applies_late() {
    let (context, _outbound, _terminal) = test_context(1);
    let previous = context.keyboard.lock().unwrap().activation;
    let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
    let (command_tx, command_rx) = bounded(1);
    let (ack, response) = bounded(1);
    command_tx
        .send(OwnerCommand {
            mutation: OwnerMutation::configure(ActivationConfig::default()),
            state: Arc::clone(&state),
            acknowledgement: ack,
        })
        .unwrap();
    assert_eq!(cancel_owner_command(&state), OwnerCommandState::Cancelled);

    process_owner_commands(&context, &command_rx);

    assert_eq!(context.keyboard.lock().unwrap().activation, previous);
    assert!(response.recv().unwrap().is_err());
}

#[test]
fn scan_codes_map_every_dom_letter_position_independent_of_virtual_key() {
    for (index, scan_code) in LETTER_SCAN_CODES.iter().copied().enumerate() {
        assert_eq!(
            map_scan_code(scan_code, false),
            PhysicalKey::Letter(ActivationKey::from_index(index as u8).unwrap())
        );
        assert_eq!(map_scan_code(scan_code, true), PhysicalKey::Other);
    }
    assert_eq!(map_scan_code(0x01, false), PhysicalKey::Escape);
    assert_eq!(map_scan_code(0x1C, false), PhysicalKey::Enter);
    assert_eq!(map_scan_code(0x1C, true), PhysicalKey::Enter);
    assert_eq!(enter_source(0x1C, false), Some(EnterSource::Main));
    assert_eq!(enter_source(0x1C, true), Some(EnterSource::Numpad));
    assert_eq!(enter_source(0x01, false), None);
    assert_eq!(map_scan_code(0, false), PhysicalKey::Other);
    assert_eq!(
        map_key_identity(0x58, 0, false),
        KeyIdentity::Letter(ActivationKey::X)
    );
    assert_eq!(map_key_identity(0x30, 0, false), KeyIdentity::Other(0x30));
}

#[test]
fn post_install_snapshot_seeds_every_tracked_key_without_seeding_reducer() {
    let held = [
        PhysicalKey::Letter(ActivationKey::X),
        PhysicalKey::Escape,
        PhysicalKey::Enter,
    ];
    let mut queried = Vec::new();
    let mut tracker = physical_tracker_from_state(|key| {
        queried.push(key);
        held.contains(&key)
    });
    assert_eq!(queried.len(), 28);
    for index in 0_u8..26 {
        let key = PhysicalKey::Letter(ActivationKey::from_index(index).unwrap());
        assert_eq!(
            tracker.observe(key, None, KeyPhase::Down),
            held.contains(&key),
            "physical letter index {index}"
        );
    }
    assert!(tracker.observe(PhysicalKey::Escape, None, KeyPhase::Down));
    assert!(tracker.observe(PhysicalKey::Enter, Some(EnterSource::Main), KeyPhase::Down,));
    assert!(tracker.observe(
        PhysicalKey::Enter,
        Some(EnterSource::Numpad),
        KeyPhase::Down,
    ));
}

#[test]
fn deferred_menu_release_is_skipped_after_same_side_is_repressed() {
    let (context, _outbound, _terminal) = test_context(1);
    let mut keyboard = context.keyboard.lock().unwrap();
    assert!(menu_modifier_release_still_needed(&keyboard, 0));
    keyboard
        .modifiers
        .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    assert!(!menu_modifier_release_still_needed(&keyboard, 0));
    keyboard
        .modifiers
        .observe(VK_LMENU, 0x38, false, KeyPhase::Up);
    assert!(menu_modifier_release_still_needed(&keyboard, 0));
}

#[test]
fn modifier_tracker_is_exact_side_aware_and_generic_safe() {
    let mut tracker = ModifierTracker::from_state(|key| key == VK_LSHIFT);
    assert_eq!(tracker.mask(), ModifierMask::new(false, false, true, false));
    tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Down);
    tracker.observe(VK_LSHIFT, 0x2A, false, KeyPhase::Up);
    assert!(tracker.mask().shift());
    tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Up);
    assert_eq!(tracker.mask(), ModifierMask::default());

    for (key, expected) in [
        (VK_CONTROL, ModifierMask::new(true, false, false, false)),
        (VK_MENU, ModifierMask::new(false, true, false, false)),
        (VK_SHIFT, ModifierMask::new(false, false, true, false)),
        (VK_LWIN, ModifierMask::new(false, false, false, true)),
    ] {
        let mut tracker = ModifierTracker::default();
        assert!(tracker.observe(key, 0, false, KeyPhase::Down));
        assert_eq!(tracker.mask(), expected);
        assert!(tracker.observe(key, 0, false, KeyPhase::Up));
        assert_eq!(tracker.mask(), ModifierMask::default());
    }

    // Generic virtual keys are resolved by scan code/extended state, so
    // releasing one side cannot clear the other. AltGr remains exact
    // Ctrl+Alt rather than an injected or implicit modifier.
    let mut tracker = ModifierTracker::default();
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
    tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Down);
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Up);
    assert!(tracker.mask().ctrl());
    tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Up);
    assert!(!tracker.mask().ctrl());
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
    tracker.observe(VK_MENU, 0x38, true, KeyPhase::Down);
    assert_eq!(tracker.mask(), ModifierMask::new(true, true, false, false));
}

#[test]
fn altgr_layout_keeps_right_alt_suppressed_without_a_visible_ctrl_edge() {
    let mut modifiers = ModifierTracker::default();
    modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Down);
    assert!(conservative_altgr_for_layout(&modifiers, false, true));
    assert!(!conservative_altgr_for_layout(&modifiers, false, false));
    modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Up);
    assert!(!conservative_altgr_for_layout(&modifiers, false, true));
}

#[test]
fn altgr_never_supplies_or_matches_activation_modifiers() {
    for modifiers in [
        ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        },
        ShortcutModifiers {
            ctrl: true,
            alt: true,
            shift: false,
            meta: false,
        },
    ] {
        let (context, outbound, _terminal) = test_context(4);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[ActivationBinding::new(
                ProfileId::GENERAL,
                shortcut(modifiers, &[ActivationKey::X]),
            )])
            .unwrap(),
        };

        // Windows synthesizes an injected left-Ctrl immediately before the
        // physical right-Alt record for AltGr. The synthetic record may
        // suppress activation, but it must never supply a modifier.
        assert!(!process_hook_record(
            &context,
            if modifiers.ctrl {
                VK_LCONTROL
            } else {
                VK_CONTROL
            },
            0x1D,
            false,
            KeyPhase::Down,
            true,
        ));
        assert!(!process_hook_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Down,
            false,
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::new(false, true, false, false),
        );
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());

        record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        );
        assert!(!process_hook_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Up,
            false,
        ));
        assert!(!context.keyboard.lock().unwrap().altgr_active);
    }
}

#[test]
fn physical_looking_altgr_and_missed_release_never_activate_plain_typing() {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::X],
        ),
    );
    let (context, outbound, _terminal) = test_context(4);
    context.keyboard.lock().unwrap().activation = ActivationConfig {
        enabled: true,
        bindings: ActivationBindings::new(&[binding]).unwrap(),
    };

    // Some layouts expose AltGr's synthetic Ctrl as a physical-looking
    // record. Right Alt suppression must still prevent Ctrl+Alt activation.
    assert!(!process_hook_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        false,
    ));
    assert!(!process_hook_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        false,
    ));
    let mut held_altgr = ModifierTracker::default();
    held_altgr.observe(VK_LCONTROL, 0x1D, false, KeyPhase::Down);
    held_altgr.observe(VK_RMENU, 0x38, true, KeyPhase::Down);
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(held_altgr),
        },
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

    // If the desktop transition loses every AltGr release event, the next
    // native snapshot repairs both modifiers and suppression without using
    // the ordinary X as a shortcut.
    let no_modifiers = ModifierTracker::default();
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(no_modifiers),
        },
    ));
    assert!(!context.keyboard.lock().unwrap().altgr_active);
    assert!(outbound.try_recv().is_err());
}

#[test]
fn external_injected_modifiers_never_activate_or_clear_physical_modifiers() {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::X],
        ),
    );
    let (context, outbound, _terminal) = test_context(4);
    context.keyboard.lock().unwrap().activation = ActivationConfig {
        enabled: true,
        bindings: ActivationBindings::new(&[binding]).unwrap(),
    };

    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Down,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::default(),
    );
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false),
    );
    assert!(record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding,
            context: activation_context(),
            phase: EventPhase::Down,
        },
    );
}

#[test]
fn stale_tracked_alt_is_resynchronized_without_activating_plain_typing() {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::X],
        ),
    );
    let (context, outbound, _terminal) = test_context(4);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[binding]).unwrap(),
        };
        keyboard
            .modifiers
            .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    }

    let no_modifiers = ModifierTracker::default();
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(no_modifiers),
        },
    ));
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Up,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(no_modifiers),
        },
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::default(),
    );
    assert!(outbound.try_recv().is_err());

    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding,
            context: activation_context(),
            phase: EventPhase::Down,
        },
    );
}

#[test]
fn every_nonempty_exact_modifier_mask_can_activate_in_the_native_path() {
    for bits in 1_u8..16 {
        let modifiers = ShortcutModifiers {
            ctrl: bits & 0b0001 != 0,
            alt: bits & 0b0010 != 0,
            shift: bits & 0b0100 != 0,
            meta: bits & 0b1000 != 0,
        };
        let expected = shortcut(modifiers, &[ActivationKey::P]);
        let expected_binding = ActivationBinding::new(ProfileId::GENERAL, expected);
        let (context, outbound, _terminal) = test_context(2);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[ActivationBinding::new(
                ProfileId::GENERAL,
                expected,
            )])
            .unwrap(),
        };
        for (enabled, virtual_key) in [
            (modifiers.ctrl, VK_LCONTROL),
            (modifiers.alt, VK_LMENU),
            (modifiers.shift, VK_LSHIFT),
            (modifiers.meta, VK_LWIN),
        ] {
            if enabled {
                modifier(&context, virtual_key, KeyPhase::Down);
            }
        }

        assert!(
            record(
                &context,
                0x50,
                PhysicalKey::Letter(ActivationKey::P),
                KeyPhase::Down,
            ),
            "modifier bits {bits:04b}",
        );
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                binding: expected_binding,
                context: activation_context(),
                phase: EventPhase::Down,
            }
        );
    }
}

#[test]
fn closed_gate_tracks_native_state_without_retaining_future_prefixes() {
    let (context, outbound, _terminal) = test_context(4);
    context.gate.close();
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .reducer
            .held_letters()
            .is_empty()
    );

    context.gate.open();
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    );
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
}

#[test]
fn every_binding_revision_fences_physically_held_letters_until_release() {
    let (context, outbound, _terminal) = test_context(4);
    context.gate.close();
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .reducer
            .held_letters()
            .is_empty()
    );

    let one_key = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    );
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[one_key]).unwrap(),
        },
    );
    context.gate.open();
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    );
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: one_key,
            context: activation_context(),
            phase: EventPhase::Down,
        },
    );
}

#[test]
fn modifier_changes_fence_a_passive_native_prefix_until_all_letters_release() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    for (virtual_key, physical) in [
        (0x50, PhysicalKey::Letter(ActivationKey::P)),
        (0x58, PhysicalKey::Letter(ActivationKey::X)),
    ] {
        record(&context, virtual_key, physical, KeyPhase::Up);
    }
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
}

#[test]
fn alt_x_p_passes_prefix_and_modifiers_but_swallows_trigger_sequence() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: full_bindings().iter().next().unwrap(),
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
    // Activation delivery alone must not globally capture Enter/Escape.
    // Electron explicitly enables that capture only after accepting and
    // visibly starting the session.
    assert_eq!(
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
        SessionCaptureMode::Off,
    );
    assert!(!record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Down
    ));
    assert!(!record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Up
    ));
    assert!(outbound.try_recv().is_err());

    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());

    // Config, prefix, and modifier changes cannot alter the accepted up.
    apply_config(&context, ActivationConfig::default());
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Up));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: full_bindings().iter().next().unwrap(),
            context: activation_context(),
            phase: EventPhase::Up,
        }
    );
}

#[test]
fn ctrl_shift_p_matches_exactly_and_extra_or_missing_state_does_not() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!modifier(&context, VK_LCONTROL, KeyPhase::Down));
    assert!(!modifier(&context, VK_RSHIFT, KeyPhase::Down));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    let expected = full_bindings().iter().nth(1).unwrap();
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: expected,
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: expected,
            context: activation_context(),
            phase: EventPhase::Up,
        }
    );

    // Missing Shift prevents a separate fresh gesture.
    let (missing, missing_outbound, _terminal) = test_context(2);
    modifier(&missing, VK_LCONTROL, KeyPhase::Down);
    assert!(!record(
        &missing,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(missing_outbound.try_recv().is_err());

    // An extra modifier prevents the next fresh gesture.
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
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
    assert!(outbound.try_recv().is_err());

    // An extra held letter also prevents the otherwise exact chord.
    let (extra, extra_outbound, _terminal) = test_context(2);
    modifier(&extra, VK_LCONTROL, KeyPhase::Down);
    modifier(&extra, VK_LSHIFT, KeyPhase::Down);
    record(
        &extra,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    );
    assert!(!record(
        &extra,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(extra_outbound.try_recv().is_err());
}

#[test]
fn wrong_order_extra_letters_and_injected_records_never_activate_or_mutate() {
    let (context, outbound, _terminal) = test_context(4);
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());

    // Externally injected modifiers and letters cannot mutate physical
    // state or complete a sequence.
    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        true,
    ));
    assert!(!process_hook_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false)
    );
    assert!(context.keyboard.lock().unwrap().physical.observe(
        PhysicalKey::Letter(ActivationKey::P),
        None,
        KeyPhase::Down
    ));

    // Talking Quill's own marked SendInput records remain entirely inert.
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!process_hook_record_at(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        InjectionKind::Helper,
        HookObservation::default(),
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false)
    );
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

#[test]
fn simultaneous_enter_sources_latch_one_balanced_sequence_in_every_order() {
    for (first, second) in [
        (EnterSource::Main, EnterSource::Numpad),
        (EnterSource::Numpad, EnterSource::Main),
    ] {
        for release_accepted_first in [false, true] {
            let (context, outbound, _terminal) = test_context(4);
            context
                .state
                .session_capture_mode
                .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);

            assert!(enter(&context, first, KeyPhase::Down));
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: SessionKey::Enter,
                    phase: EventPhase::Down,
                },
            );
            assert!(enter(&context, first, KeyPhase::Down));
            assert!(!enter(&context, second, KeyPhase::Down));
            assert!(!enter(&context, second, KeyPhase::Down));
            assert!(outbound.try_recv().is_err());

            let releases = if release_accepted_first {
                [first, second]
            } else {
                [second, first]
            };
            for source in releases {
                assert_eq!(
                    enter(&context, source, KeyPhase::Up),
                    source == first,
                    "first={first:?}, release={source:?}",
                );
            }
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: SessionKey::Enter,
                    phase: EventPhase::Up,
                },
            );
            assert!(outbound.try_recv().is_err());
            assert_eq!(context.keyboard.lock().unwrap().captured_enter_source, None,);
        }
    }
}

#[test]
fn enter_source_tracking_survives_capture_and_config_transitions() {
    let (context, outbound, _terminal) = test_context(4);

    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Down,
        },
    );
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));

    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);
    apply_config(&context, ActivationConfig::default());
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Up));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Up,
        },
    );
    assert!(outbound.try_recv().is_err());
}

#[test]
fn cancel_only_captures_escape_but_passes_enter_and_balances_after_off() {
    let (context, outbound, _terminal) = test_context(4);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);

    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
    assert!(record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        },
    );

    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    assert!(record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Up,
        },
    );
}

#[test]
fn real_transactional_hook_path_captures_ctrl_shift_activation_and_balances_up() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );

    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Down,
        InputSource::Physical,
        3,
    ));
    let KeyboardEvent::Activation {
        binding,
        context: activation_context,
        phase: EventPhase::Down,
    } = receive_event(&outbound)
    else {
        panic!("expected transactional activation down")
    };
    assert_eq!(binding.profile_id(), ProfileId::GENERAL);
    assert_eq!(
        activation_context.activation_generation(),
        ActivationGeneration::FIRST
    );
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        InputSource::Physical,
        9,
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            context,
            phase: EventPhase::Up,
            ..
        } if context == activation_context
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Up,
        InputSource::Physical,
        10,
    ));
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Up,
        InputSource::Physical,
        11,
    ));
    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.owned_letters(), 0);
    assert_eq!(
        keyboard.transactional.physical_modifiers(),
        TransactionalModifierSides::default()
    );
}

#[test]
fn reentrant_helper_injection_cannot_overwrite_outer_panic_disposition() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = keyboard
            .transactional
            .clone()
            .with_menu_neutralization_policy(
                talking_quill_keyboard_core::transactional::MenuNeutralizationPolicy::NotRequired,
            );
    }
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));

    let outer = Cell::new(CallbackDisposition::Pass);
    arm_transaction_panic(TestTransactionPanicPoint::AfterEffect);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transactional_record_with_disposition(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Down,
            InputSource::Physical,
            3,
            &outer,
        )
    }));
    assert!(panicked.is_err());
    assert_eq!(outer.get(), CallbackDisposition::Capture);
    handle_callback_panic(&context);

    let mut keyboard = lock_keyboard_recovering(&context).expect("recover poison");
    assert!(matches!(
        keyboard.transaction_authority,
        Some(TransactionAuthority::Resume { .. })
    ));
    assert!(recover_transaction_authority(&context, &mut keyboard));
    assert!(keyboard.transaction_authority.is_none());
    drop(keyboard);
    assert!(matches!(
        outbound.try_recv(),
        Ok(NativeEvent::Keyboard(KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }))
    ));
    assert!(
        outbound.try_recv().is_err(),
        "panic recovery cannot redeliver the accepted activation"
    );
}

#[test]
fn panic_after_engine_turn_retains_snapshot_and_closes_admission() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    let before = context.keyboard.lock().unwrap().transactional.clone();
    arm_transaction_panic(TestTransactionPanicPoint::AfterTurn);
    let disposition = Cell::new(CallbackDisposition::Pass);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transactional_record_with_disposition(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Down,
            InputSource::Physical,
            3,
            &disposition,
        )
    }));
    assert!(panicked.is_err());
    handle_callback_panic(&context);
    assert!(!context.gate.is_open());
    assert_eq!(disposition.get(), CallbackDisposition::Capture);
    let keyboard = match context.keyboard.lock() {
        Err(poisoned) => poisoned.into_inner(),
        Ok(_) => panic!("panic seam must poison the authoritative keyboard lock"),
    };
    assert_eq!(
        keyboard.transactional, before,
        "the live engine was never taken"
    );
    assert!(matches!(
        keyboard.transaction_authority,
        Some(TransactionAuthority::Turn(_))
    ));
    drop(keyboard);
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        InputSource::Physical,
        4,
    ));
    let keyboard = lock_keyboard_recovering(&context).expect("recover terminal drain");
    assert!(keyboard.transaction_authority.is_none());
    assert_eq!(keyboard.transactional.owned_letters(), 0);
    assert!(
        outbound.try_recv().is_err(),
        "closed admission cannot execute the unsubmitted activation"
    );
}

#[test]
fn panic_recovery_processes_current_owned_escape_and_enter_ups() {
    for (virtual_key, scan_code, session_key) in [
        (VK_ESCAPE, 0x01, SessionKey::Escape),
        (VK_RETURN, 0x1C, SessionKey::Enter),
    ] {
        let (context, outbound, _terminal) = test_context(8);
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        assert!(transactional_record(
            &context,
            virtual_key,
            scan_code,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: session_key,
                phase: EventPhase::Down,
            }
        );

        arm_transaction_panic(TestTransactionPanicPoint::AfterTurn);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transactional_record(
                &context,
                0x41,
                LETTER_SCAN_CODES[usize::from(ActivationKey::A.index())],
                false,
                KeyPhase::Down,
                InputSource::Physical,
                2,
            )
        }));
        assert!(panicked.is_err());
        handle_callback_panic(&context);

        assert!(transactional_record(
            &context,
            virtual_key,
            scan_code,
            false,
            KeyPhase::Up,
            InputSource::Physical,
            3,
        ));
        assert!(
            outbound.try_recv().is_err(),
            "terminal drain clears ownership without publishing a new session event"
        );
        let keyboard = lock_keyboard_recovering(&context).expect("recovered terminal owner");
        assert!(!keyboard.session_escape_native_owned);
        assert!(keyboard.captured_enter_source.is_none());
    }
}

#[test]
fn panic_after_effect_recovers_exact_outcome_without_redelivery() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    arm_transaction_panic(TestTransactionPanicPoint::AfterEffect);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Down,
            InputSource::Physical,
            3,
        )
    }));
    assert!(panicked.is_err());
    handle_callback_panic(&context);
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    {
        let mut keyboard = lock_keyboard_recovering(&context).expect("recover poison");
        assert!(matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::Resume { .. })
        ));
        assert!(recover_transaction_authority(&context, &mut keyboard));
        assert!(keyboard.transaction_authority.is_none());
        assert_ne!(keyboard.transactional, TransactionEngine::default());
    }
    assert!(
        outbound.try_recv().is_err(),
        "effect was not delivered twice"
    );
}

#[test]
fn external_alt_x_p_is_input_equivalent_and_activates_once() {
    let (mut context, outbound, _terminal) = test_context(4);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        3,
    ));
    assert!(matches!(
        replay_receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        InputSource::External,
        4,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Up,
        InputSource::External,
        5,
    ));
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Up,
        InputSource::External,
        6,
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Up,
            ..
        }
    ));
    assert!(outbound.try_recv().is_err());
}

#[test]
fn hardware_shaped_physical_syskey_alt_x_uses_ordered_hook_edges() {
    let (mut context, outbound, _terminal) = test_context(4);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let alt = ShortcutModifiers {
        ctrl: false,
        alt: true,
        shift: false,
        meta: false,
    };
    let bindings = ActivationBindings::new(&[
        ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        ),
    ])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings,
        },
    );
    let process = |virtual_key, scan_code, phase, flags| {
        let disposition = Cell::new(CallbackDisposition::Pass);
        process_transactional_hook_record_at(
            &context,
            TransactionalHookRecord {
                virtual_key,
                scan_code,
                extended: false,
                platform_flags: flags,
                phase,
                source: InputSource::Physical,
            },
            HookObservation {
                observed_at_ms: 1,
                // This is the production physical callback contract: the
                // asynchronous state that can lag Alt is diagnostic only.
                native_modifiers: None,
            },
            &disposition,
        )
    };
    const ALT_CONTEXT_FLAG: u32 = 0x20;
    // Exact physical shape captured on the affected machine. Alt-up won
    // the release race by 16 ms, before X-up. The shorter Alt+X binding
    // must still commit, suppress X-up, and leave the owner ready for the
    // following Alt+X+P attempt.
    assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(VK_LMENU, 0x38, KeyPhase::Up, 0x80));
    assert!(matches!(
        replay_receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    assert!(process(0x58, 0x2d, KeyPhase::Up, 0x80));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(outbound.try_iter().any(|event| matches!(
        event,
        NativeEvent::Keyboard(KeyboardEvent::ActivationComplete { .. })
    )));
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.transactional.admission_open());
        assert_eq!(keyboard.transactional.owned_letters(), 0);
    }

    assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x50, 0x19, KeyPhase::Down, ALT_CONTEXT_FLAG));
}

#[test]
fn async_poll_cannot_release_suppressed_physical_prefix_before_suffix() {
    let (mut context, outbound, terminal) = test_context(4);
    let (sender, receiver) = bounded(1);
    context.replay_sender = Some(sender);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    let edge = |key, scan, phase, at| {
        transactional_record(&context, key, scan, false, phase, InputSource::Physical, at)
    };
    assert!(!edge(VK_LMENU, 0x38, KeyPhase::Down, 1));
    assert!(edge(0x58, 0x2d, KeyPhase::Down, 2));
    let mut modifiers = ModifierTracker::default();
    modifiers.observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    // Windows sees Alt, but X-down was suppressed. Multiple timer samples
    // while the user holds the prefix must leave it available for Alt+X+P.
    for _ in 0..20 {
        reconcile_sampled_state(
            &context,
            current_input_desktop(),
            WindowsPhysicalTracker::default(),
            modifiers,
            false,
        );
        assert_eq!(
            context.keyboard.lock().unwrap().transactional.journal_len(),
            1
        );
        assert!(
            receiver.try_recv().is_err(),
            "poll must not replay the prefix"
        );
        assert!(outbound.try_recv().is_err());
        assert!(terminal.try_recv().is_err());
    }
    assert!(edge(0x50, 0x19, KeyPhase::Down, 1000));
    assert!(matches!(
        receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(edge(0x50, 0x19, KeyPhase::Up, 1001));
    assert!(edge(0x58, 0x2d, KeyPhase::Up, 1002));
    assert!(!edge(VK_LMENU, 0x38, KeyPhase::Up, 1003));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Up,
            ..
        }
    ));
    assert_eq!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .owned_letters(),
        0
    );
}

#[test]
fn quick_suffix_release_during_menu_work_delivers_up_and_allows_the_next_shortcut() {
    for source in [InputSource::Physical, InputSource::External] {
        let (mut context, outbound, terminal) = test_context(8);
        let (sender, receiver) = bounded(1);
        context.replay_sender = Some(sender);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        for round in 0..2 {
            for (index, (key, scan, phase)) in [
                (VK_LMENU, 0x38, KeyPhase::Down),
                (0x58, 0x2d, KeyPhase::Down),
                (0x50, 0x19, KeyPhase::Down),
                (0x50, 0x19, KeyPhase::Up),
                (0x58, 0x2d, KeyPhase::Up),
                (VK_LMENU, 0x38, KeyPhase::Up),
            ]
            .into_iter()
            .enumerate()
            {
                transactional_record(
                    &context,
                    key,
                    scan,
                    false,
                    phase,
                    source,
                    round * 100 + index as u64,
                );
            }
            assert!(matches!(
                receiver.try_recv(),
                Ok(ReplayWork::NeutralizeMenu { .. })
            ));
            context.replay_accepted.store(4, Ordering::Release);
            process_deferred_callback_replay(&context);
            assert!(matches!(
                receive_event(&outbound),
                KeyboardEvent::Activation {
                    phase: EventPhase::Down,
                    ..
                }
            ));
            assert!(matches!(
                receive_event(&outbound),
                KeyboardEvent::Activation {
                    phase: EventPhase::Up,
                    ..
                }
            ));
            assert!(outbound.try_recv().is_err());
            assert!(terminal.try_recv().is_err());
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.owned_letters(), 0);
            assert!(keyboard.transactional.admission_open());
            assert!(keyboard.dispatcher.active.is_none());
        }
    }
}

#[test]
fn physical_and_external_alt_x_share_matcher_suppression_and_delivery() {
    for source in [InputSource::Physical, InputSource::External] {
        let (mut context, outbound, _terminal) = test_context(4);
        let (replay_sender, replay_receiver) = bounded(1);
        context.replay_sender = Some(replay_sender);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        let records = [
            (VK_LMENU, 0x38, KeyPhase::Down, false),
            (
                0x58,
                LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                KeyPhase::Down,
                true,
            ),
            (
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                KeyPhase::Down,
                true,
            ),
            (
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                KeyPhase::Up,
                true,
            ),
            (
                0x58,
                LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                KeyPhase::Up,
                true,
            ),
            (VK_LMENU, 0x38, KeyPhase::Up, false),
        ];
        for (at, (virtual_key, scan_code, phase, expected_capture)) in
            records.into_iter().enumerate()
        {
            assert_eq!(
                transactional_record(
                    &context,
                    virtual_key,
                    scan_code,
                    false,
                    phase,
                    source,
                    u64::try_from(at + 1).unwrap(),
                ),
                expected_capture,
                "source={source:?} edge={at}",
            );
            if at == 2 {
                assert!(matches!(
                    replay_receiver.try_recv(),
                    Ok(ReplayWork::NeutralizeMenu { .. })
                ));
                context.replay_accepted.store(4, Ordering::Release);
                process_deferred_callback_replay(&context);
            }
        }
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Up,
                ..
            }
        ));
        assert!(outbound.try_recv().is_err());
    }
}

#[test]
fn unavailable_replay_worker_publishes_a_consumable_suppression_result() {
    let (context, _outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_MENU,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x5a,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        3,
    ));
    assert_eq!(context.replay_accepted.load(Ordering::Acquire), 1);
    process_deferred_callback_replay(&context);
    let keyboard = context.keyboard.lock().unwrap();
    assert!(keyboard.deferred_callback_replay.is_none());
    assert!(keyboard.transaction_authority.is_none());
    assert!(terminal.try_recv().is_ok());
}

#[test]
fn exact_virtual_key_sendinput_sequence_matches_serialized_alt_x_profiles() {
    let (mut context, outbound, _terminal) = test_context(16);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let alt = ShortcutModifiers {
        ctrl: false,
        alt: true,
        shift: false,
        meta: false,
    };
    let bindings = ActivationBindings::new(&[
        ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        ),
    ])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings,
        },
    );

    let mut at = 1;
    let mut input = |vk, phase| {
        let captured =
            transactional_record(&context, vk, 0, false, phase, InputSource::External, at);
        at += 1;
        captured
    };
    // Exact serialized virtual-key edges emitted by the installed
    // PowerShell validator: unmatched Alt+X+Z, Alt+X, Alt+X+P, then U.
    for (index, (vk, phase, expected)) in [
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x5A, KeyPhase::Down, true),
        (0x5A, KeyPhase::Up, false),
        (0x58, KeyPhase::Up, false),
        (VK_MENU, KeyPhase::Up, false),
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x58, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x50, KeyPhase::Down, true),
        (0x50, KeyPhase::Up, true),
        (0x58, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Up, false),
        (0x55, KeyPhase::Down, false),
        (0x55, KeyPhase::Up, false),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            input(vk, phase),
            expected,
            "event={index} vk={vk:#x} phase={phase:?}"
        );
        if index == 5 {
            // Unit tests have no replay worker or owner message pump.
            // Publish the exact accepted count at a deterministic boundary
            // after proving racing physical edges preserve its authority.
            let work = replay_receiver.try_recv().unwrap();
            process_deferred_callback_replay(&context);
            {
                let keyboard = context.keyboard.lock().unwrap();
                assert!(keyboard.deferred_callback_replay.is_some());
                assert!(matches!(
                    keyboard.transaction_authority,
                    Some(TransactionAuthority::AwaitingDeferredReplay)
                ));
            }
            let ReplayWork::Replay { batch, .. } = work else {
                panic!("unmatched sequence must defer replay");
            };
            context.replay_accepted.store(
                u64::try_from(batch.len()).unwrap().saturating_add(2),
                Ordering::Release,
            );
            process_deferred_callback_replay(&context);
            let mut keyboard = context.keyboard.lock().unwrap();
            let outcome = begin_transaction_control(
                &context,
                &mut keyboard,
                Control::Reconcile(PhysicalSnapshot::default()),
            );
            assert!(outcome.is_some_and(|outcome| outcome.applied));
            assert!(keyboard.transactional.config().enabled());
            assert!(keyboard.transactional.admission_open());
            assert_eq!(keyboard.transactional.physical_letters(), 0);
            assert_eq!(keyboard.transactional.fenced_letters(), 0);
            assert_eq!(keyboard.transactional.physical_modifiers().bits(), 0);
            assert_eq!(keyboard.transactional.fenced_modifiers().bits(), 0);
        }
        if index == 9 || index == 12 {
            assert!(matches!(
                replay_receiver.try_recv(),
                Ok(ReplayWork::NeutralizeMenu { .. })
            ));
            context.replay_accepted.store(4, Ordering::Release);
            process_deferred_callback_replay(&context);
        }
    }
    let events: Vec<_> = outbound.try_iter().collect();
    let downs = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                NativeEvent::Keyboard(KeyboardEvent::Activation {
                    phase: EventPhase::Down,
                    ..
                })
            )
        })
        .count();
    let ups = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                NativeEvent::Keyboard(KeyboardEvent::Activation {
                    phase: EventPhase::Up,
                    ..
                })
            )
        })
        .count();
    assert_eq!(downs, ups, "activation notifications remain balanced");
    assert!(downs >= 1);
    let observed = context.observability.snapshot();
    assert_eq!(observed.registered_input.registered_candidate_callbacks, 3);
    assert_eq!(observed.transactions.committed, 2);
    assert_eq!(observed.transactions.replayed, 1);
    assert!(!context.terminal.is_triggered());
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .admission_open()
    );
}

#[test]
fn generic_and_sided_alt_normalize_with_physical_or_virtual_key_letters() {
    for (alt_vk, alt_scan, alt_extended) in [
        (VK_MENU, 0, false),
        (VK_LMENU, 0x38, false),
        (VK_RMENU, 0x38, true),
    ] {
        let side = modifier_side(alt_vk, alt_scan, alt_extended).unwrap();
        assert_eq!(
            side,
            if alt_extended {
                ModifierSide::RightAlt
            } else {
                ModifierSide::LeftAlt
            }
        );
    }
    for (vk, scan, expected) in [
        (0x58, 0, ActivationKey::X),
        (0, 0x2D, ActivationKey::X),
        (0x50, 0, ActivationKey::P),
        (0, 0x19, ActivationKey::P),
    ] {
        assert_eq!(
            map_key_identity(vk, scan, false),
            KeyIdentity::Letter(expected)
        );
    }
}

#[test]
fn external_session_keys_are_input_equivalent_and_preserve_balancing() {
    let (context, outbound, _terminal) = test_context(8);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    for (virtual_key, scan_code) in [
        (VK_ESCAPE, 0x01),
        (VK_RETURN, 0x1C),
        (VK_ESCAPE, 0),
        (VK_RETURN, 0),
    ] {
        for phase in [KeyPhase::Down, KeyPhase::Up] {
            assert!(transactional_record(
                &context,
                virtual_key,
                scan_code,
                false,
                phase,
                InputSource::External,
                1,
            ));
        }
    }
    assert_eq!(outbound.try_iter().count(), 8);
}

#[test]
fn redundant_native_releases_do_not_retire_the_owner() {
    for source in [InputSource::Physical, InputSource::External] {
        let (context, outbound, terminal) = test_context(8);
        let bindings = ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::new("general").unwrap(),
            shortcut(
                ShortcutModifiers {
                    ctrl: true,
                    shift: true,
                    alt: false,
                    meta: false,
                },
                &[ActivationKey::J, ActivationKey::K, ActivationKey::L],
            ),
        )])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings,
            },
        );
        let edge =
            |key, scan, phase| transactional_record(&context, key, scan, false, phase, source, 1);
        for _ in 0..3 {
            // A release can arrive after polling has already observed neutral,
            // or from an external sender balancing its own modifier state.
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Down));
            assert!(!edge(VK_LSHIFT, 0x2a, KeyPhase::Down));
            for (key, scan) in [(0x4a, 0x24), (0x4b, 0x25), (0x4c, 0x26)] {
                assert!(edge(key, scan, KeyPhase::Down));
            }
            for (key, scan) in [(0x4c, 0x26), (0x4b, 0x25), (0x4a, 0x24)] {
                assert!(edge(key, scan, KeyPhase::Up));
                assert!(!edge(key, scan, KeyPhase::Up));
            }
            assert!(!edge(VK_LSHIFT, 0x2a, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert_eq!(outbound.try_iter().count(), 2);
            assert!(terminal.try_recv().is_err());
            assert!(!context.terminal.is_triggered());
        }
    }
}

#[test]
fn helper_classes_bypass_and_ctrl_right_alt_fails_closed_as_altgr() {
    let (context, outbound, _terminal) = test_context(8);
    let ctrl_alt = ActivationBindings::new(&[ActivationBinding::new(
        ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        shortcut(
            ShortcutModifiers {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    )])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: ctrl_alt,
        },
    );
    let before = context.keyboard.lock().unwrap().transactional.clone();
    for source in [
        InputSource::HelperReplay,
        InputSource::HelperPaste,
        InputSource::HelperDummy,
    ] {
        assert!(!transactional_record(
            &context,
            0x50,
            0x19,
            false,
            KeyPhase::Down,
            source,
            1,
        ));
    }
    assert_eq!(context.keyboard.lock().unwrap().transactional, before);

    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(!transactional_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        InputSource::Physical,
        10,
    ));
    assert!(context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
    assert!(!transactional_record(
        &context,
        0x50,
        0x19,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        11,
    ));
    assert!(outbound.try_recv().is_err());
}

#[test]
fn deferred_replay_race_tracks_ctrl_right_alt_as_altgr() {
    let mut keyboard = CallbackKeyboard::default();
    track_deferred_replay_race(&mut keyboard, VK_LCONTROL, 0x1D, false, KeyPhase::Down);
    track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Down);
    assert!(keyboard.altgr_synthetic_ctrl);
    assert!(keyboard.altgr_active);
    track_deferred_replay_race(&mut keyboard, 0x50, 0x19, false, KeyPhase::Down);
    assert!(keyboard.altgr_active);
    track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Up);
    assert!(!keyboard.altgr_synthetic_ctrl);
    assert!(!keyboard.altgr_active);
}

#[test]
fn external_ctrl_is_real_input_and_cannot_fake_an_alt_only_binding() {
    let (context, outbound, _terminal) = test_context(4);
    let alt = ActivationBindings::new(&[ActivationBinding::new(
        ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    )])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: alt,
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::External,
        0,
    ));
    assert!(!context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
    assert!(!transactional_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        0x50,
        0x19,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(outbound.try_recv().is_err());
    let keyboard = context.keyboard.lock().unwrap();
    assert!(
        keyboard
            .transactional
            .physical_modifiers()
            .combined()
            .ctrl()
    );
}

#[test]
fn escape_and_enter_capture_remains_paired_and_modifier_independent() {
    let (context, outbound, _terminal) = test_context(8);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    modifier(&context, VK_LWIN, KeyPhase::Down);
    for (key, session_key) in [
        (PhysicalKey::Escape, SessionKey::Escape),
        (PhysicalKey::Enter, SessionKey::Enter),
    ] {
        assert!(record(&context, 0, key, KeyPhase::Down));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: session_key,
                phase: EventPhase::Down,
            }
        );
        assert!(record(&context, 0, key, KeyPhase::Down));
        assert!(record(&context, 0, key, KeyPhase::Up));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: session_key,
                phase: EventPhase::Up,
            }
        );
    }
}
