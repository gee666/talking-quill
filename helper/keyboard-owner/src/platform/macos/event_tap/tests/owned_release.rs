//! Owned release contracts.

use super::*;

#[test]
fn failed_recovery_classifier_suppresses_owned_up_then_retry_reconciles_once() {
    let key = ActivationKey::X;
    let key_code = LETTER_KEY_CODES[usize::from(key.index())];
    let event = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_UP,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let session_up = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_UP,
        ESCAPE_KEY_CODE,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let (context, _outbound, _commands) = test_context();
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine_with_ctrl_shift_x_candidate();
        keyboard.session_escape_native_owned = true;
        let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
        let _ = keyboard.physical.observe(ESCAPE_KEY_CODE, KeyPhase::Down);
    }

    let context_ptr = (&raw const context).cast_mut().cast();
    let native_events = context.native_events.try_lock().unwrap();
    let returned =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, event, context_ptr) };
    assert!(returned.is_null(), "the exact owned up cannot leak");
    let returned_session =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, session_up, context_ptr) };
    assert!(
        returned_session.is_null(),
        "the exact session-owned up cannot leak"
    );
    assert!(context.state.recovery_pending.load(Ordering::Acquire));
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_ne!(keyboard.transactional.owned_letters(), 0);
        assert!(!keyboard.session_escape_native_owned);
    }
    assert!(
        context
            .recovery_edges
            .lock()
            .unwrap()
            .owned_release_recorded(key_code)
    );
    drop(native_events);

    // SAFETY: the local context remains live on this thread for the callback.
    unsafe {
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
    }
    assert!(!context.state.recovery_pending.load(Ordering::Acquire));
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.owned_letters(), 0);
    }
    assert!(
        !context
            .recovery_edges
            .lock()
            .unwrap()
            .owned_release_recorded(key_code)
    );
    assert!(!pending_native_work(&context));
    // A later permanent wake sees no retained release and cannot drain it
    // or execute recovery a second time.
    // SAFETY: the same live local context is still owned by this thread.
    unsafe {
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
    }
    assert!(!pending_native_work(&context));
    unsafe {
        ffi::CFRelease(session_up.cast_const());
        ffi::CFRelease(event.cast_const());
    }
}
