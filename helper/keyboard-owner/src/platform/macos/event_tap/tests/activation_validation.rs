//! Activation validation contracts.

use super::*;

#[test]
fn pending_activation_resolves_before_a_later_physical_event() {
    let (context, outbound, _commands) = test_context();
    let compiled =
        CompiledActivationConfig::compile(ConfigRevision::new(1), true, bindings()).unwrap();
    let mut engine = TransactionEngine::new(compiled);
    let mut sides = ModifierSides::default();
    for (side, key_code) in [
        (ModifierSide::LeftCtrl, LEFT_CONTROL_KEY_CODE),
        (ModifierSide::LeftShift, LEFT_SHIFT_KEY_CODE),
    ] {
        sides.insert(side);
        let event = NormalizedEvent {
            key: KeyIdentity::Modifier(side),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: key_code,
                scan_code: u32::from(key_code),
                extended: false,
                platform_flags: 0,
            },
            observed_at_ms: 1,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(0, sides, false),
        };
        let Turn::Complete {
            engine: next,
            completion: Completion::Event(_),
        } = engine.begin(EngineInput::Event(event))
        else {
            panic!("modifier cannot request an effect");
        };
        engine = next;
    }
    let trigger = ActivationKey::P;
    let trigger_bit = 1_u32 << u32::from(trigger.index());
    let down = NormalizedEvent {
        key: KeyIdentity::Letter(trigger),
        phase: PhysicalPhase::Down,
        source: InputSource::test_physical(),
        native: NativeKey {
            virtual_key: LETTER_KEY_CODES[usize::from(trigger.index())],
            scan_code: u32::from(LETTER_KEY_CODES[usize::from(trigger.index())]),
            extended: false,
            platform_flags: 0,
        },
        observed_at_ms: 2,
        config_revision: ConfigRevision::new(1),
        gate: GateState::Open,
        snapshot: PhysicalSnapshot::new(trigger_bit, sides, false),
    };
    let Turn::NeedEffect {
        effect: EffectRequest::DeliverActivation(notice),
        continuation,
    } = engine.begin(EngineInput::Event(down))
    else {
        panic!("exact trigger requests activation delivery");
    };
    *context.pending_activation.lock().unwrap() = Some(PendingActivation {
        continuation,
        notice,
        reservation: crate::platform::macos::target::activation_reservation_for_test(7, 11),
        validation_request: crate::platform::macos::target::validation_request_for_test(3, 11),
        resolved_delivery: None,
        deadline: Instant::now() + Duration::from_secs(1),
    });

    // A later source edge never enters a placeholder reducer. If the AX
    // response is not ready, the pending activation first commits
    // targetless, then the later edge is processed by the restored engine.
    assert!(resolve_pending_activation(
        &context,
        PendingActivationResolution::ForceTargetless,
    ));
    assert!(context.pending_activation.lock().unwrap().is_none());
    assert_ne!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .owned_letters(),
        0
    );
    let KeyboardEvent::Activation {
        context: activation,
        phase: EventPhase::Down,
        ..
    } = receive_event(&outbound)
    else {
        panic!("expected activation down");
    };
    assert!(activation.target_token().is_none());

    let up = NormalizedEvent {
        phase: PhysicalPhase::Up,
        snapshot: PhysicalSnapshot::new(0, sides, false),
        observed_at_ms: 3,
        ..down
    };
    let mut keyboard = context.keyboard.lock().unwrap();
    let engine = std::mem::take(&mut keyboard.transactional);
    let completion = drive_transaction_turn(
        &context,
        &mut keyboard,
        engine.begin(EngineInput::Event(up)),
        None,
    );
    assert!(matches!(
        completion,
        DriveCompletion::Complete(Completion::Event(
            talking_quill_keyboard_core::transactional::EventOutcome {
                disposition: EventDisposition::CaptureCurrent,
                ..
            }
        ))
    ));
    drop(keyboard);
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Up,
            ..
        }
    ));
}

#[test]
fn activation_event_boundary_without_cached_evidence_is_immediately_targetless() {
    let (context, outbound, _commands) = test_context();
    let binding = bindings().iter().next().unwrap();
    let notice = ActivationNotice::Down { binding };
    let mut keyboard = context.keyboard.lock().unwrap();
    assert!(
        keyboard
            .dispatcher
            .deliver(&context.outbound, &context.terminal, notice, None,)
    );
    let KeyboardEvent::Activation {
        context: activation,
        ..
    } = receive_event(&outbound)
    else {
        panic!("expected activation");
    };
    assert_eq!(
        activation.activation_generation(),
        ActivationGeneration::FIRST
    );
    // No cache means no IPC fallback and no later target binding.
    assert!(activation.target_token().is_none());
}
