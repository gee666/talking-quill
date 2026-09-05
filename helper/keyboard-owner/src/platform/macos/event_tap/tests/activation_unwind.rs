//! Activation unwind contracts.

use super::*;

#[cfg(feature = "transactional-shortcuts-dev")]
#[test]
fn actual_event_tap_callback_recovers_poisoned_deferred_trigger_lock() {
    let (mut context, _outbound, _commands) = test_context();
    context.test_physical_seam_enabled = true;
    context.injection_identity = Some(injection::InjectionIdentity::for_test(i64::from(unsafe {
        ffi::getpid()
    })));
    context.target_cache = Some(TargetCache::with_open_validation_queue_for_test());
    context.forced_activation_reservation =
        Some(crate::platform::macos::target::activation_reservation_for_test(7, 11));
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine_with_ctrl_shift_x_candidate();
        let _ = keyboard
            .physical
            .observe(LEFT_CONTROL_KEY_CODE, KeyPhase::Down);
        let _ = keyboard
            .physical
            .observe(LEFT_SHIFT_KEY_CODE, KeyPhase::Down);
        let _ = keyboard.physical.observe(
            LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            KeyPhase::Down,
        );
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
    }
    let event = unsafe {
        ffi::CGEventCreateKeyboardEvent(
            null(),
            LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
            true,
        )
    };
    assert!(!event.is_null());
    unsafe {
        ffi::CGEventSetFlags(
            event,
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
        );
        ffi::CGEventSetIntegerValueField(
            event,
            ffi::K_CG_EVENT_SOURCE_USER_DATA,
            injection::TEST_PHYSICAL_MARKER,
        );
    }
    PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.set(true));
    let returned = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_KEY_DOWN,
            event,
            (&raw mut context).cast(),
        )
    };
    assert!(returned.is_null(), "owned trigger remains suppressed");
    assert!(!context.keyboard.is_poisoned());
    assert!(context.pending_activation.lock().unwrap().is_none());
    assert!(context.keyboard.lock().unwrap().inflight_effect.is_none());
    unsafe { ffi::CFRelease(event.cast_const()) };
}

#[test]
fn deferred_trigger_continuation_remains_installed_when_install_turn_panics() {
    let engine = engine_with_ctrl_shift_x_candidate();
    let mut sides = ModifierSides::default();
    sides.insert(ModifierSide::LeftCtrl);
    sides.insert(ModifierSide::LeftShift);
    let trigger = NormalizedEvent {
        key: KeyIdentity::Letter(ActivationKey::P),
        phase: PhysicalPhase::Down,
        source: InputSource::test_physical(),
        native: NativeKey {
            virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
            ..NativeKey::default()
        },
        observed_at_ms: 3,
        config_revision: ConfigRevision::new(1),
        gate: GateState::Open,
        snapshot: PhysicalSnapshot::new(
            (1_u32 << u32::from(ActivationKey::X.index()))
                | (1_u32 << u32::from(ActivationKey::P.index())),
            sides,
            false,
        ),
    };
    let turn = engine.begin(EngineInput::Event(trigger));
    let (mut context, _outbound, _commands) = test_context();
    context.target_cache = Some(TargetCache::with_open_validation_queue_for_test());
    PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.set(true));
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut keyboard = context.keyboard.lock().unwrap();
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
        let _ = drive_transaction_turn(
            &context,
            &mut keyboard,
            turn,
            Some(crate::platform::macos::target::activation_reservation_for_test(7, 11)),
        );
    }));
    assert!(unwind.is_err());
    assert!(context.keyboard.is_poisoned());
    assert!(context.pending_activation.lock().unwrap().is_some());
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Owned
    );
    assert!(!context.keyboard.is_poisoned());
    assert!(context.pending_activation.lock().unwrap().is_none());
}

#[test]
fn deferred_activation_and_submitted_replay_survive_induced_unwind_without_duplicate_effect() {
    let engine = engine_with_ctrl_shift_x_candidate();
    let mut sides = ModifierSides::default();
    sides.insert(ModifierSide::LeftCtrl);
    sides.insert(ModifierSide::LeftShift);
    let held = (1_u32 << u32::from(ActivationKey::X.index()))
        | (1_u32 << u32::from(ActivationKey::P.index()));
    let trigger = NormalizedEvent {
        key: KeyIdentity::Letter(ActivationKey::P),
        phase: PhysicalPhase::Down,
        source: InputSource::test_physical(),
        native: NativeKey {
            virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
            ..NativeKey::default()
        },
        observed_at_ms: 3,
        config_revision: ConfigRevision::new(1),
        gate: GateState::Open,
        snapshot: PhysicalSnapshot::new(held, sides, false),
    };
    let Turn::NeedEffect {
        effect: EffectRequest::DeliverActivation(notice),
        continuation,
    } = engine.begin(EngineInput::Event(trigger))
    else {
        panic!("exact ordered trigger must defer activation delivery");
    };
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
    }
    *context.pending_activation.lock().unwrap() = Some(PendingActivation {
        continuation,
        notice,
        reservation: crate::platform::macos::target::activation_reservation_for_test(7, 11),
        validation_request: crate::platform::macos::target::validation_request_for_test(3, 11),
        resolved_delivery: None,
        deadline: Instant::now() + Duration::from_secs(1),
    });
    PANIC_AFTER_EFFECT_OUTCOME.with(|flag| flag.set(true));
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = resolve_pending_activation(&context, PendingActivationResolution::FailDelivery);
    }));
    assert!(unwind.is_err());
    assert!(context.keyboard.is_poisoned());
    assert!(context.pending_activation.lock().unwrap().is_some());
    assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Owned
    );
    assert!(!context.keyboard.is_poisoned());
    assert!(context.pending_activation.lock().unwrap().is_none());
    assert!(context.keyboard.lock().unwrap().inflight_effect.is_none());
}
