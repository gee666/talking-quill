//! Recovery ordering contracts.

use super::*;

#[test]
fn owner_recovery_with_submitted_replay_defers_physical_edges_before_observation() {
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
    let token = injection::OperationToken::for_test(89);
    let modifier = tagged_keyboard_event(
        ffi::K_CG_EVENT_FLAGS_CHANGED,
        LEFT_CONTROL_KEY_CODE,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let key = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine_with_ctrl_shift_x_candidate();
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
        assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token));
    }
    enter_owner_lifecycle_recovery(&context);
    assert!(context.state.recovery_pending.load(Ordering::Acquire));
    assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
    let native_events = context.native_events.try_lock().unwrap();
    let context_ptr = (&raw const context).cast_mut().cast();
    for (event_type, event) in [
        (ffi::K_CG_EVENT_FLAGS_CHANGED, modifier),
        (ffi::K_CG_EVENT_KEY_DOWN, key),
    ] {
        let returned = unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
        assert!(returned.is_null());
    }
    let journal = context.recovery_edges.lock().unwrap();
    assert_eq!(journal.pending_len, 2);
    assert_eq!(journal.pending[0].key_code, LEFT_CONTROL_KEY_CODE);
    assert_eq!(
        journal.pending[1].key_code,
        LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())]
    );
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .replay_observation
            .is_some()
    );
    drop(journal);
    drop(native_events);
    unsafe {
        ffi::CFRelease(key.cast_const());
        ffi::CFRelease(modifier.cast_const());
    }
}

#[test]
fn failed_recovery_defers_external_modifier_keyboard_and_balanced_mouse_in_order() {
    let y = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
    let z = LETTER_KEY_CODES[usize::from(ActivationKey::Z.index())];
    let external_flags = 0x8000_0000_0010_0000;
    let external_down = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, z, external_flags, 0);
    let external_up = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, z, external_flags, 0);
    let ctrl_up = tagged_keyboard_event(
        ffi::K_CG_EVENT_FLAGS_CHANGED,
        LEFT_CONTROL_KEY_CODE,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let y_down = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        y,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let y_up = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, y, 0, TEST_RECOVERY_PHYSICAL_MARKER);
    let location = ffi::CGPoint { x: 321.0, y: 222.0 };
    let mouse_down = tagged_mouse_event(
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
        location,
        0,
        1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let mouse_up = tagged_mouse_event(
        ffi::K_CG_EVENT_LEFT_MOUSE_UP,
        location,
        0,
        1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let (mut context, _outbound, _commands) = test_context();
    let external_pid = unsafe {
        ffi::CGEventGetIntegerValueField(external_down, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
    };
    context.injection_identity = Some(injection::InjectionIdentity::for_test(external_pid + 1));
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine_with_ctrl_shift_x_candidate();
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
    }
    let context_ptr = (&raw const context).cast_mut().cast();
    let native_events = context.native_events.try_lock().unwrap();
    for (event_type, event) in [
        (ffi::K_CG_EVENT_KEY_DOWN, external_down),
        (ffi::K_CG_EVENT_KEY_UP, external_up),
        (ffi::K_CG_EVENT_FLAGS_CHANGED, ctrl_up),
        (ffi::K_CG_EVENT_KEY_DOWN, y_down),
        (ffi::K_CG_EVENT_KEY_UP, y_up),
        (ffi::K_CG_EVENT_LEFT_MOUSE_DOWN, mouse_down),
        (ffi::K_CG_EVENT_LEFT_MOUSE_UP, mouse_up),
    ] {
        let returned = unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
        assert!(returned.is_null(), "ordered original must be suppressed");
    }
    let journal = context.recovery_edges.lock().unwrap();
    assert_eq!(journal.pending_len, 7);
    assert!(journal.hidden_balanced());
    assert_eq!(journal.pending[0].source, InputSource::External);
    assert_eq!(journal.pending[0].key_code, z);
    assert_eq!(journal.pending[0].flags, unsafe {
        ffi::CGEventGetFlags(external_down)
    },);
    assert_eq!(journal.pending[0].keyboard_type, unsafe {
        ffi::CGEventGetIntegerValueField(external_down, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
    },);
    assert_eq!(journal.pending[0].original_timestamp, unsafe {
        ffi::CGEventGetTimestamp(external_down)
    },);
    assert_eq!(journal.pending[1].key_code, z);
    assert_eq!(journal.pending[2].key_code, LEFT_CONTROL_KEY_CODE);
    assert_eq!(journal.pending[3].key_code, y);
    assert_eq!(journal.pending[4].key_code, y);
    assert_eq!(
        journal.pending[5].event_type,
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
    );
    assert_eq!(journal.pending[6].event_type, ffi::K_CG_EVENT_LEFT_MOUSE_UP);
    assert_eq!(journal.pending[5].location, location);
    assert_eq!(journal.pending[6].location, location);
    drop(journal);
    drop(native_events);
    unsafe {
        for event in [
            mouse_up,
            mouse_down,
            y_up,
            y_down,
            ctrl_up,
            external_up,
            external_down,
        ] {
            ffi::CFRelease(event.cast_const());
        }
    }
}
