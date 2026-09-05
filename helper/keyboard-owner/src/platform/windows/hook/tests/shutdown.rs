use super::*;

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
