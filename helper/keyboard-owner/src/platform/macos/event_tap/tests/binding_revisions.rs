//! Binding revisions contracts.

use super::*;

#[test]
fn every_nonempty_modifier_mask_can_activate_in_the_macos_model() {
    for bits in 1_u8..16 {
        let modifiers = ShortcutModifiers {
            ctrl: bits & 0b0001 != 0,
            alt: bits & 0b0010 != 0,
            shift: bits & 0b0100 != 0,
            meta: bits & 0b1000 != 0,
        };
        let expected = shortcut(modifiers, &[ActivationKey::P]);
        let expected_binding = ActivationBinding::new(ProfileId::GENERAL, expected);
        let (context, outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.activation = ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[ActivationBinding::new(
                    ProfileId::GENERAL,
                    expected,
                )])
                .unwrap(),
            };
            for (enabled, key_code) in [
                (modifiers.ctrl, LEFT_CONTROL_KEY_CODE),
                (modifiers.alt, LEFT_OPTION_KEY_CODE),
                (modifiers.shift, LEFT_SHIFT_KEY_CODE),
                (modifiers.meta, LEFT_COMMAND_KEY_CODE),
            ] {
                if enabled {
                    keyboard.modifiers.observe_flags_changed(key_code, true);
                }
            }
        }
        assert!(
            process_key_event(&context, LETTER_KEY_CODES[15], KeyPhase::Down, false, false,),
            "modifier bits {bits:04b}",
        );
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                binding: expected_binding,
                context: activation_context(),
                phase: EventPhase::Down,
            }
        );
    }
}

#[test]
fn every_binding_revision_fences_held_letters_until_release() {
    let (context, outbound, commands) = test_context();
    context.gate.close();
    assert!(!process_key_event(
        &context,
        LETTER_KEY_CODES[23],
        KeyPhase::Down,
        false,
        false,
    ));

    let one_key = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    );
    apply_config(
        &context,
        &commands,
        ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[one_key]).unwrap(),
        },
    );
    context.gate.open();
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
    process_key_event(&context, LETTER_KEY_CODES[15], KeyPhase::Up, false, false);
    process_key_event(&context, LETTER_KEY_CODES[23], KeyPhase::Up, false, false);
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
            binding: one_key,
            context: activation_context(),
            phase: EventPhase::Down,
        },
    );
}
