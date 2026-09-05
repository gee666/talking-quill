//! Recovery observation contracts.

use super::*;

#[test]
fn failed_recovery_classifier_enforces_tombstone_repeat_up_and_fresh_down_rules() {
    let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::P.index())];
    let repeat = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    unsafe {
        ffi::CGEventSetIntegerValueField(repeat, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT, 1);
    }
    let delayed_up = tagged_keyboard_event(
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
    let (context, _outbound, _commands) = test_context();
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.gap_reconciled_letters = 1_u32 << u32::from(ActivationKey::P.index());
        keyboard.gap_barrier_pending = true;
    }
    let context_ptr = (&raw const context).cast_mut().cast();
    let native_events = context.native_events.try_lock().unwrap();
    let returned_repeat =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_DOWN, repeat, context_ptr) };
    assert!(returned_repeat.is_null());
    assert!(context.keyboard.lock().unwrap().gap_tombstones_pending());

    let returned_up =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, delayed_up, context_ptr) };
    assert!(returned_up.is_null());
    assert!(!context.keyboard.lock().unwrap().gap_tombstones_pending());

    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.gap_reconciled_letters = 1_u32 << u32::from(ActivationKey::P.index());
        keyboard.gap_barrier_pending = true;
    }
    let returned_fresh = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_KEY_DOWN,
            fresh_down,
            context_ptr,
        )
    };
    assert_eq!(returned_fresh, fresh_down);
    let keyboard = context.keyboard.lock().unwrap();
    assert!(!keyboard.gap_tombstones_pending());
    assert!(!keyboard.gap_barrier_pending);
    drop(keyboard);
    drop(native_events);
    assert!(attempt_pending_recovery(&context));
    assert!(!pending_native_work(&context));
    unsafe {
        ffi::CFRelease(fresh_down.cast_const());
        ffi::CFRelease(delayed_up.cast_const());
        ffi::CFRelease(repeat.cast_const());
    }
}

#[test]
fn failed_recovery_classifier_advances_barrier_and_commits_only_exact_paste_up() {
    let barrier_token = injection::OperationToken::for_test(82);
    let barrier_down =
        tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, 127, 0, barrier_token.marker());
    let barrier_up = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, 127, 0, barrier_token.marker());
    let (mut barrier_context, _outbound, _commands) = test_context();
    install_event_source_identity(&mut barrier_context, barrier_down);
    barrier_context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    {
        let mut keyboard = barrier_context.keyboard.lock().unwrap();
        keyboard.gap_reconciled_escape = true;
        keyboard.gap_barrier_pending = true;
        keyboard.gap_barrier_token = Some(barrier_token);
    }
    let barrier_context_ptr = (&raw const barrier_context).cast_mut().cast();
    let native_events = barrier_context.native_events.try_lock().unwrap();
    for (event_type, event) in [
        (ffi::K_CG_EVENT_KEY_DOWN, barrier_down),
        (ffi::K_CG_EVENT_KEY_UP, barrier_up),
    ] {
        let returned =
            unsafe { event_tap_callback(null_mut(), event_type, event, barrier_context_ptr) };
        assert!(returned.is_null(), "barrier pair stays helper-owned");
    }
    assert!(!barrier_context.keyboard.lock().unwrap().gap_barrier_pending);
    drop(native_events);
    assert!(attempt_pending_recovery(&barrier_context));
    assert!(!pending_native_work(&barrier_context));

    unsafe {
        ffi::CFRelease(barrier_up.cast_const());
        ffi::CFRelease(barrier_down.cast_const());
    }
}

#[test]
fn failed_recovery_classifier_passes_exact_replay_and_cleanup_once() {
    fn assert_exact_operation(batch: ExpectedReplayBatch, generation: u64) {
        let token = injection::OperationToken::for_test(generation);
        let first = batch.record(0).expect("test operation is nonempty");
        let first_type = if matches!(first.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if first.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let first_event = tagged_keyboard_event(
            first_type,
            first.native.virtual_key,
            first.native.platform_flags,
            token.marker(),
        );
        let (mut context, _outbound, _commands) = test_context();
        install_event_source_identity(&mut context, first_event);
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .begin_replay_observation(batch, token)
        );
        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();

        for index in 0..batch.len() {
            let record = batch.record(index).expect("bounded operation record");
            let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
                ffi::K_CG_EVENT_FLAGS_CHANGED
            } else if record.phase == PhysicalPhase::Up {
                ffi::K_CG_EVENT_KEY_UP
            } else {
                ffi::K_CG_EVENT_KEY_DOWN
            };
            let event = if index == 0 {
                first_event
            } else {
                tagged_keyboard_event(
                    event_type,
                    record.native.virtual_key,
                    record.native.platform_flags,
                    token.marker(),
                )
            };
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert_eq!(returned, event, "exact replay/cleanup must pass");
            assert_eq!(
                atomic_current_edge_disposition(&context),
                CurrentEdgeDisposition::Pass
            );
            unsafe { ffi::CFRelease(event.cast_const()) };
        }
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .replay_observation
                .is_none()
        );
        drop(native_events);
        assert!(attempt_pending_recovery(&context));
        assert!(!pending_native_work(&context));
    }

    let engine = engine_with_ctrl_shift_x_candidate();
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(replay),
        ..
    } = engine.begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation requires replay");
    };
    let cleanup = replay
        .cleanup_for_accepted(replay.len())
        .expect("full candidate prefix has bounded cleanup");
    assert!(!cleanup.is_empty());
    assert_exact_operation(ExpectedReplayBatch::Replay(replay), 84);
    assert_exact_operation(ExpectedReplayBatch::Cleanup(cleanup), 85);
}

#[test]
fn failed_recovery_classifier_passes_provably_unrelated_external_event() {
    let event = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, 100, 0, 0);
    let (mut context, _outbound, _commands) = test_context();
    let event_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    context.injection_identity = Some(injection::InjectionIdentity::for_test(event_pid + 1));
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    let context_ptr = (&raw const context).cast_mut().cast();
    let native_events = context.native_events.try_lock().unwrap();
    let returned =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_DOWN, event, context_ptr) };
    assert_eq!(returned, event);
    assert_eq!(
        atomic_current_edge_disposition(&context),
        CurrentEdgeDisposition::Pass
    );
    assert!(context.state.recovery_pending.load(Ordering::Acquire));
    drop(native_events);
    assert!(attempt_pending_recovery(&context));
    assert!(!pending_native_work(&context));
    unsafe { ffi::CFRelease(event.cast_const()) };
}
