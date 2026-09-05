//! Session capture contracts.

use super::*;

#[test]
fn simultaneous_main_and_keypad_enter_keep_the_captured_source_balanced() {
    let (context, outbound, _commands) = test_context();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    assert!(process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(!process_key_event(
        &context,
        KEYPAD_ENTER_KEY_CODE,
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(!process_key_event(
        &context,
        KEYPAD_ENTER_KEY_CODE,
        KeyPhase::Up,
        false,
        false,
    ));
    assert!(process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Up,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: talking_quill_keyboard_core::SessionKey::Enter,
            phase: EventPhase::Down,
        }
    );
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: talking_quill_keyboard_core::SessionKey::Enter,
            phase: EventPhase::Up,
        }
    );
}

#[test]
fn capture_mode_changes_preserve_balancing_and_cancel_only_passes_fresh_enter() {
    let (context, outbound, _commands) = test_context();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    assert!(process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Down,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Down,
        },
    );

    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);
    assert!(process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Up,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Up,
        },
    );
    assert!(!process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(!process_key_event(
        &context,
        RETURN_KEY_CODE,
        KeyPhase::Up,
        false,
        false,
    ));

    assert!(process_key_event(
        &context,
        ESCAPE_KEY_CODE,
        KeyPhase::Down,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        },
    );
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    assert!(process_key_event(
        &context,
        ESCAPE_KEY_CODE,
        KeyPhase::Up,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Up,
        },
    );
}
