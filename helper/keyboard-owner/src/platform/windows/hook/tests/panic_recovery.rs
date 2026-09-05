use super::*;

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
