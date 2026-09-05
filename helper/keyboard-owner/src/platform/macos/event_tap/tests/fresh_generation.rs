//! Fresh generation contracts.

use super::*;

#[test]
fn owned_up_fresh_generation_replays_after_retained_replay_and_balances_once() {
    let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::X.index())];
    let engine = engine_with_ctrl_shift_x_candidate();
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(batch),
        ..
    } = engine.clone().begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation requires replay");
    };
    let replay_token = injection::OperationToken::for_test(91);
    let first_replay = batch.entries()[0];
    let first_replay_type = if matches!(first_replay.key, KeyIdentity::Modifier(_)) {
        ffi::K_CG_EVENT_FLAGS_CHANGED
    } else if first_replay.phase == PhysicalPhase::Up {
        ffi::K_CG_EVENT_KEY_UP
    } else {
        ffi::K_CG_EVENT_KEY_DOWN
    };
    let first_replay_event = tagged_keyboard_event(
        first_replay_type,
        first_replay.native.virtual_key,
        first_replay.native.platform_flags,
        replay_token.marker(),
    );
    let old_up = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_UP,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let fresh_down = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let fresh_repeat = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    unsafe {
        ffi::CGEventSetIntegerValueField(fresh_repeat, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT, 1);
    }
    let fresh_up = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_UP,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let (mut context, _outbound, _commands) = test_context();
    install_event_source_identity(&mut context, first_replay_event);
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine;
        let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
        assert!(
            keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), replay_token,)
        );
    }
    let context_ptr = (&raw const context).cast_mut().cast();
    let native_events = context.native_events.try_lock().unwrap();
    for (event_type, event) in [
        (ffi::K_CG_EVENT_KEY_UP, old_up),
        (ffi::K_CG_EVENT_KEY_DOWN, fresh_down),
        (ffi::K_CG_EVENT_KEY_DOWN, fresh_repeat),
    ] {
        let returned = unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
        assert!(returned.is_null());
    }
    {
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 2);
        assert_eq!(journal.pending[0].generation, 2);
        assert_eq!(journal.pending[1].generation, 2);
        assert!(journal.physical_newer_generation_held(key_code));
        assert!(!journal.force_old_generation_released(key_code));
    }
    drop(native_events);
    assert!(attempt_pending_recovery(&context));
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.owned_letters(), 0);
        assert!(keyboard.physical.is_held(key_code));
        assert!(!deferred_native_ordering_drained(&keyboard));
    }

    let returned_up =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, fresh_up, context_ptr) };
    assert!(returned_up.is_null(), "fresh up is deferred with its down");
    {
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 3);
        assert_eq!(journal.pending[2].generation, 2);
        assert!(journal.hidden_balanced());
    }

    for index in 0..batch.len() {
        let record = batch.entries()[index];
        let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if record.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let event = if index == 0 {
            first_replay_event
        } else {
            tagged_keyboard_event(
                event_type,
                record.native.virtual_key,
                record.native.platform_flags,
                replay_token.marker(),
            )
        };
        let returned = unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
        assert_eq!(returned, event, "retained replay is foreground-first");
        unsafe { ffi::CFRelease(event.cast_const()) };
    }
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(deferred_native_ordering_drained(&keyboard));
        let snapshot = physical_snapshot(&keyboard);
        assert!(
            begin_transaction_control(&context, &mut keyboard, Control::Reconcile(snapshot),)
                .applied
        );
        assert_eq!(keyboard.transactional.physical_letters(), 0);
    }

    let deferred_token = injection::OperationToken::for_test(92);
    assert!(
        context
            .recovery_edges
            .lock()
            .unwrap()
            .begin_submission(deferred_token, 0, 3)
    );
    for (event_type, event, repeat) in [
        (ffi::K_CG_EVENT_KEY_DOWN, fresh_down, false),
        (ffi::K_CG_EVENT_KEY_DOWN, fresh_repeat, true),
        (ffi::K_CG_EVENT_KEY_UP, fresh_up, false),
    ] {
        unsafe {
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_EVENT_SOURCE_USER_DATA,
                deferred_token.marker(),
            );
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT,
                i64::from(repeat),
            );
        }
        let returned = unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
        assert_eq!(returned, event, "deferred generation passes once");
    }
    assert!(!context.recovery_edges.lock().unwrap().has_pending());
    let duplicate =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, fresh_up, context_ptr) };
    assert!(
        duplicate.is_null(),
        "stale deferred generation cannot pass twice"
    );
    unsafe {
        ffi::CFRelease(fresh_up.cast_const());
        ffi::CFRelease(fresh_repeat.cast_const());
        ffi::CFRelease(fresh_down.cast_const());
        ffi::CFRelease(old_up.cast_const());
    }
}
