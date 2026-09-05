//! Ordered shortcuts contracts.

use super::*;

#[test]
fn modifier_changes_fence_a_passive_macos_prefix_until_release() {
    let (context, outbound, _commands) = test_context();
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        false,
    ));
    context
        .keyboard
        .lock()
        .unwrap()
        .modifiers
        .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(outbound.try_recv().is_err());
    for key_code in [LETTER_KEY_CODES[15], LETTER_KEY_CODES[23]] {
        process_key_event(&context, key_code, KeyPhase::Up, false, false);
    }
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
}

#[test]
fn alt_x_p_and_ctrl_shift_p_use_ordered_native_tracking_and_snapshots() {
    let (context, outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    }
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
    let accepted = bindings().iter().next().unwrap();
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: accepted,
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
    apply_config(&context, &_commands, ActivationConfig::default());
    assert!(process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Up,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: accepted,
            context: activation_context(),
            phase: EventPhase::Up,
        }
    );

    let (context, outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
    }
    assert!(process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: bindings().iter().nth(1).unwrap(),
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
}

#[test]
fn closed_gate_inputs_cannot_invent_order() {
    let (context, outbound, _commands) = test_context();
    context.gate.close();
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
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
    context.gate.open();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
    }
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[15],
        KeyPhase::Down,
        false,
        false,
    ));
    assert!(outbound.try_recv().is_err());
}
