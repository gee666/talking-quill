//! Policy fences contracts.

use super::*;

#[test]
fn preheld_snapshot_fences_letters_and_distinguishes_both_enter_keys() {
    let held = [LETTER_KEY_CODES[23], RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE];
    let mut queried = Vec::new();
    let mut keyboard = CallbackKeyboard::default();
    keyboard.seed_from_state(|key_code| {
        queried.push(key_code);
        held.contains(&key_code)
    });
    assert_eq!(queried.len(), 37);
    assert!(!keyboard.preheld_letters.is_empty());
    assert!(keyboard.physical.observe(RETURN_KEY_CODE, KeyPhase::Down));
    assert!(
        keyboard
            .physical
            .observe(KEYPAD_ENTER_KEY_CODE, KeyPhase::Down)
    );
    keyboard.physical.observe(RETURN_KEY_CODE, KeyPhase::Up);
    assert!(
        keyboard
            .physical
            .observe(KEYPAD_ENTER_KEY_CODE, KeyPhase::Down)
    );
}

#[test]
fn events_queued_before_policy_enable_are_tracked_but_never_captured() {
    let (context, outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.activation_revision_at = 100;
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    }
    assert!(!process_key_event_with_modifiers(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        None,
        99,
        false,
    ));
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .reducer
            .held_letters()
            .is_empty()
    );
    assert!(outbound.try_recv().is_err());

    let (capture_context, capture_outbound, _commands) = test_context();
    capture_context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    {
        let mut keyboard = capture_context.keyboard.lock().unwrap();
        keyboard.escape_capture_enabled_at = 100;
        keyboard.enter_capture_enabled_at = 100;
    }
    assert!(!process_key_event_with_modifiers(
        &capture_context,
        ESCAPE_KEY_CODE,
        KeyPhase::Down,
        false,
        None,
        99,
        false,
    ));
    assert!(capture_outbound.try_recv().is_err());
}

#[test]
fn pre_revision_passive_up_releases_the_reducer_fence() {
    let (context, outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    }
    assert!(!process_key_event_with_modifiers(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        None,
        50,
        false,
    ));
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.reducer.fence_activation_revision();
        keyboard.fence_current_letters();
        keyboard.activation_revision_at = 100;
    }
    assert!(!process_key_event_with_modifiers(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Up,
        false,
        None,
        99,
        false,
    ));
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .reducer
            .held_letters()
            .is_empty()
    );

    assert!(!process_key_event_with_modifiers(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        None,
        101,
        false,
    ));
    assert!(process_key_event_with_modifiers(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        None,
        102,
        false,
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
}
