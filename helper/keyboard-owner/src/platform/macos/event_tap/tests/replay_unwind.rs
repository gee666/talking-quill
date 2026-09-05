//! Replay unwind contracts.

use super::*;

#[test]
fn panic_after_exact_replay_recognition_keeps_foreground_event_passed_once() {
    let engine = engine_with_ctrl_shift_x_candidate();
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(batch),
        ..
    } = engine.begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation requires replay");
    };
    let first = batch.entries()[0];
    let event_type = if matches!(first.key, KeyIdentity::Modifier(_)) {
        ffi::K_CG_EVENT_FLAGS_CHANGED
    } else if first.phase == PhysicalPhase::Up {
        ffi::K_CG_EVENT_KEY_UP
    } else {
        ffi::K_CG_EVENT_KEY_DOWN
    };
    let token = injection::OperationToken::for_test(72);
    let event = tagged_keyboard_event(
        event_type,
        first.native.virtual_key,
        first.native.platform_flags,
        token.marker(),
    );
    let (mut context, _outbound, _commands) = test_context();
    install_event_source_identity(&mut context, event);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token,));
    }
    PANIC_AFTER_REPLAY_RECOGNITION.with(|flag| flag.set(true));
    let returned =
        unsafe { event_tap_callback(null_mut(), event_type, event, (&raw mut context).cast()) };
    assert_eq!(
        returned, event,
        "recognized replay must remain foreground-visible"
    );
    assert_eq!(
        atomic_current_edge_disposition(&context),
        CurrentEdgeDisposition::Pass
    );
    let keyboard = context.keyboard.lock().unwrap();
    let observation = keyboard
        .replay_observation
        .as_ref()
        .expect("remaining replay suffix stays installed");
    assert_eq!(observation.next, 1, "recognized edge advances exactly once");
    drop(keyboard);
    unsafe { ffi::CFRelease(event.cast_const()) };
}

#[test]
fn panic_after_hid_replay_submission_waits_for_exact_observation_without_reposting() {
    let engine = engine_with_ctrl_shift_x_candidate();
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(batch),
        continuation,
    } = engine.begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation requires replay");
    };
    let (context, _outbound, _commands) = test_context();
    let token = injection::OperationToken::for_test(27);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
        assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token,));
        keyboard.inflight_effect = Some(InflightEffect {
            effect: EffectRequest::Replay(batch),
            continuation,
            outcome: None,
        });
    }
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _keyboard = context.keyboard.lock().unwrap();
        panic!("after replay submission while callback lock is held");
    }));
    assert!(unwind.is_err());
    assert!(context.keyboard.is_poisoned());
    PANIC_ONCE_DURING_RECOVERY.with(|flag| flag.set(true));
    assert_eq!(
        recover_callback_unwind(&context),
        CurrentEdgeDisposition::Owned
    );
    assert!(context.state.recovery_pending.load(Ordering::Acquire));
    assert!(context.keyboard.is_poisoned());
    // Permanent owner source/timer entry retries recovery before its normal
    // callback body. The injected second panic is one-shot.
    // SAFETY: the local context remains live on this thread for the callback.
    unsafe {
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
    }
    assert!(!context.state.recovery_pending.load(Ordering::Acquire));
    assert!(!context.keyboard.is_poisoned());
    let keyboard = context.keyboard.lock().unwrap();
    assert!(
        keyboard.inflight_effect.is_some(),
        "submission alone cannot consume the continuation"
    );
    let observation = keyboard
        .replay_observation
        .as_ref()
        .expect("original submitted operation remains pending observation");
    assert_eq!(observation.token, token);
    assert_eq!(observation.next, 0);
}
