//! Delivery failure contracts.

use super::*;

#[test]
fn outbound_failure_passes_trigger_and_closes_the_callback_gate() {
    let (context, _outbound, _commands) = test_context_with_capacity(0);
    context
        .keyboard
        .lock()
        .unwrap()
        .modifiers
        .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(!context.gate.is_open());
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Up,
        false,
        false,
    ));
}

#[test]
fn injected_marker_and_unknown_repeat_are_conservatively_ignored_or_fenced() {
    let identity = injection::InjectionIdentity::for_test(42);
    assert!(is_synthetic_event(identity, 101, 42));
    assert!(is_synthetic_event(identity, 0, 42));
    assert!(!is_synthetic_event(identity, 0, 0));

    let (context, outbound, _commands) = test_context();
    context
        .keyboard
        .lock()
        .unwrap()
        .modifiers
        .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        true,
        false,
    ));
    assert!(!context.keyboard.lock().unwrap().preheld_letters.is_empty());
    assert!(outbound.try_recv().is_err());
}
