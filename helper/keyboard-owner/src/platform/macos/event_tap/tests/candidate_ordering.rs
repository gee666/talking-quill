//! Candidate ordering contracts.

use super::*;

#[test]
fn normal_candidate_callbacks_never_enter_recovery_deferred_mode() {
    fn event(
        context: &mut CallbackContext,
        event_type: u32,
        key_code: u16,
        flags: u64,
    ) -> (ffi::CGEventRef, ffi::CGEventRef) {
        let event =
            tagged_keyboard_event(event_type, key_code, flags, TEST_RECOVERY_PHYSICAL_MARKER);
        let returned = unsafe {
            event_tap_callback(
                null_mut(),
                event_type,
                event,
                (context as *mut CallbackContext).cast(),
            )
        };
        (event, returned)
    }

    let (mut context, outbound, _commands) = test_context();
    for (event_type, key_code, flags) in [
        (
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_CONTROL_KEY_CODE,
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
        ),
        (
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_SHIFT_KEY_CODE,
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
        ),
        (
            ffi::K_CG_EVENT_KEY_DOWN,
            LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
        ),
        (
            ffi::K_CG_EVENT_KEY_DOWN,
            LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
        ),
    ] {
        let (event, returned) = event(&mut context, event_type, key_code, flags);
        if event_type == ffi::K_CG_EVENT_KEY_DOWN {
            assert!(returned.is_null(), "candidate/trigger is reducer-owned");
        }
        unsafe { ffi::CFRelease(event.cast_const()) };
    }
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));
    assert!(!context.recovery_edges.lock().unwrap().ordering_pending());
}

#[test]
fn replay_failure_preserves_keyboard_current_and_defers_balanced_mouse_pair() {
    fn candidate_context() -> CallbackContext {
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            for key_code in [
                LEFT_CONTROL_KEY_CODE,
                LEFT_SHIFT_KEY_CODE,
                LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            ] {
                let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
            }
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
        }
        context
    }

    let key_flags = ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT;
    for (event_type, key_code, flags) in [
        (ffi::K_CG_EVENT_KEY_DOWN, 100, key_flags),
        (
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_SHIFT_KEY_CODE,
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
        ),
    ] {
        let mut context = candidate_context();
        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
        let event =
            tagged_keyboard_event(event_type, key_code, flags, TEST_RECOVERY_PHYSICAL_MARKER);
        let returned =
            unsafe { event_tap_callback(null_mut(), event_type, event, (&raw mut context).cast()) };
        assert_eq!(
            returned, event,
            "unsubmitted terminating original survives exactly once"
        );
        assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
        assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));
        assert!(!context.recovery_edges.lock().unwrap().ordering_pending());
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    let mut context = candidate_context();
    TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
    let mouse = tagged_mouse_event(
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
        ffi::CGPoint { x: 90.0, y: 70.0 },
        0,
        1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let returned = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
            mouse,
            (&raw mut context).cast(),
        )
    };
    assert!(returned.is_null(), "original mouse down is replaced");
    assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
    assert_eq!(
        atomic_current_edge_disposition(&context),
        CurrentEdgeDisposition::Owned
    );
    assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
    {
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 1);
        assert_eq!(
            journal.pending[0].event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
        );
        assert!(!journal.pending[0].foreground_balance);
        assert_eq!(journal.ready_len(), 0, "physical down waits for its up");
    }
    let mouse_up = tagged_mouse_event(
        ffi::K_CG_EVENT_LEFT_MOUSE_UP,
        ffi::CGPoint { x: 90.0, y: 70.0 },
        0,
        1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let returned_up = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_LEFT_MOUSE_UP,
            mouse_up,
            (&raw mut context).cast(),
        )
    };
    assert!(returned_up.is_null());
    let journal = context.recovery_edges.lock().unwrap();
    assert_eq!(journal.pending_len, 2);
    assert_eq!(journal.pending[1].event_type, ffi::K_CG_EVENT_LEFT_MOUSE_UP);
    assert_eq!(journal.ready_len(), 2);
    drop(journal);
    unsafe {
        ffi::CFRelease(mouse_up.cast_const());
        ffi::CFRelease(mouse.cast_const());
    }
}
