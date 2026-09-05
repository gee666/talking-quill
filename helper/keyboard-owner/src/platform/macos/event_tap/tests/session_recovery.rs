//! Session recovery contracts.

use super::*;

#[test]
fn ui_balancing_never_discards_native_session_up_ownership() {
    let (context, outbound, _commands) = test_context();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(process_session_event(
            &context,
            &mut keyboard,
            ESCAPE_KEY_CODE,
            KeyPhase::Down,
            false,
            1,
        ));
        assert!(keyboard.session_escape_native_owned);
        deliver_balancing_events(&context, &mut keyboard.reducer);
        assert!(keyboard.session_escape_native_owned);
    }
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            phase: EventPhase::Up,
            ..
        }
    ));
    context.gate.close();
    let mut keyboard = context.keyboard.lock().unwrap();
    assert!(process_session_event(
        &context,
        &mut keyboard,
        ESCAPE_KEY_CODE,
        KeyPhase::Up,
        false,
        2,
    ));
    assert!(!keyboard.session_escape_native_owned);
}

#[test]
fn secure_and_permission_transition_balance_only_ui_state() {
    for _path in ["secure-input", "permission"] {
        let (context, _outbound, _commands) = test_context();
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.session_escape_native_owned = true;
        keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
        keyboard.captured_enter_key_code = Some(RETURN_KEY_CODE);
        finish_native_transition(&context, &mut keyboard);
        assert!(keyboard.session_escape_native_owned);
        assert_eq!(keyboard.session_enter_native_owned, Some(RETURN_KEY_CODE));
        assert!(has_session_ownership(&keyboard));
    }
}

#[test]
fn suspend_native_input_retains_native_session_ownership() {
    let (context, _outbound, commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.session_escape_native_owned = true;
        keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
    }
    let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
    let (acknowledgement, response) = bounded(1);
    commands
        .send(OwnerCommand {
            mutation: OwnerMutation {
                kind: OwnerMutationKind::SuspendNativeInput,
                activation: ActivationConfig::default(),
                session_capture_mode: SessionCaptureMode::Off,
            },
            state,
            acknowledgement,
        })
        .unwrap();
    process_owner_commands(&context);
    assert!(response.recv().unwrap().is_ok());
    assert!(has_session_ownership(&context.keyboard.lock().unwrap()));
}

#[test]
fn callback_panic_keeps_hidden_session_ups_owned_by_the_ordered_barrier() {
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.session_escape_native_owned = true;
        keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
        keyboard.captured_enter_key_code = Some(RETURN_KEY_CODE);
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
    }
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Owned
    );
    let keyboard = context.keyboard.lock().unwrap();
    assert!(!has_session_ownership(&keyboard));
    assert!(keyboard.gap_tombstones_pending());
    assert!(keyboard.gap_barrier_pending);
    assert!(keyboard.strict_drain_recovery_needed());
}

#[test]
fn terminal_tap_disable_recovery_preserves_session_suppression_until_barrier() {
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.session_escape_native_owned = true;
    }
    let decision = apply_tap_recovery(&context, TapRecoveryEvent::DisabledByUserInput);
    assert!(matches!(decision, TapRecoveryDecision::Terminal(_)));
    assert!(context.keyboard.lock().unwrap().session_escape_native_owned);
    keep_strict_drain_tap_enabled(&context);
    let keyboard = context.keyboard.lock().unwrap();
    assert!(keyboard.gap_reconciled_escape);
    assert!(keyboard.gap_barrier_pending);
}

#[test]
fn hid_hidden_session_release_still_requires_ordered_gap_barrier_retirement() {
    let mut keyboard = CallbackKeyboard {
        session_escape_native_owned: true,
        session_enter_native_owned: Some(RETURN_KEY_CODE),
        captured_enter_key_code: Some(RETURN_KEY_CODE),
        gap_barrier_pending: true,
        gap_barrier_token: Some(injection::OperationToken::for_test(40)),
        ..CallbackKeyboard::default()
    };
    let (escape, enter, captured_enter) =
        reconcile_hidden_session_native_ownership(&mut keyboard, false, false);
    assert!(escape && enter);
    assert_eq!(captured_enter, Some(RETURN_KEY_CODE));
    keyboard.gap_reconciled_escape = true;
    keyboard.gap_reconciled_enter_key_code = captured_enter;
    assert!(keyboard.strict_drain_recovery_needed());
    assert_eq!(
        keyboard.observe_gap_barrier_event(ffi::K_CG_EVENT_KEY_DOWN, 127, false, 0,),
        GapBarrierObservation::Down
    );
    assert!(keyboard.strict_drain_recovery_needed());
    assert_eq!(
        keyboard.observe_gap_barrier_event(ffi::K_CG_EVENT_KEY_UP, 127, false, 0),
        GapBarrierObservation::Complete
    );
    assert!(!keyboard.strict_drain_recovery_needed());
}
