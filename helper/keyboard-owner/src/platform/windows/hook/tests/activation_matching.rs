use super::*;

#[test]
fn every_nonempty_exact_modifier_mask_can_activate_in_the_native_path() {
    for bits in 1_u8..16 {
        let modifiers = ShortcutModifiers {
            ctrl: bits & 0b0001 != 0,
            alt: bits & 0b0010 != 0,
            shift: bits & 0b0100 != 0,
            meta: bits & 0b1000 != 0,
        };
        let expected = shortcut(modifiers, &[ActivationKey::P]);
        let expected_binding = ActivationBinding::new(ProfileId::GENERAL, expected);
        let (context, outbound, _terminal) = test_context(2);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[ActivationBinding::new(
                ProfileId::GENERAL,
                expected,
            )])
            .unwrap(),
        };
        for (enabled, virtual_key) in [
            (modifiers.ctrl, VK_LCONTROL),
            (modifiers.alt, VK_LMENU),
            (modifiers.shift, VK_LSHIFT),
            (modifiers.meta, VK_LWIN),
        ] {
            if enabled {
                modifier(&context, virtual_key, KeyPhase::Down);
            }
        }

        assert!(
            record(
                &context,
                0x50,
                PhysicalKey::Letter(ActivationKey::P),
                KeyPhase::Down,
            ),
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
fn alt_x_p_passes_prefix_and_modifiers_but_swallows_trigger_sequence() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: full_bindings().iter().next().unwrap(),
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
    // Activation delivery alone must not globally capture Enter/Escape.
    // Electron explicitly enables that capture only after accepting and
    // visibly starting the session.
    assert_eq!(
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
        SessionCaptureMode::Off,
    );
    assert!(!record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Down
    ));
    assert!(!record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Up
    ));
    assert!(outbound.try_recv().is_err());

    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());

    // Config, prefix, and modifier changes cannot alter the accepted up.
    apply_config(&context, ActivationConfig::default());
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Up));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    ));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: full_bindings().iter().next().unwrap(),
            context: activation_context(),
            phase: EventPhase::Up,
        }
    );
}

#[test]
fn ctrl_shift_p_matches_exactly_and_extra_or_missing_state_does_not() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!modifier(&context, VK_LCONTROL, KeyPhase::Down));
    assert!(!modifier(&context, VK_RSHIFT, KeyPhase::Down));
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    let expected = full_bindings().iter().nth(1).unwrap();
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: expected,
            context: activation_context(),
            phase: EventPhase::Down,
        }
    );
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding: expected,
            context: activation_context(),
            phase: EventPhase::Up,
        }
    );

    // Missing Shift prevents a separate fresh gesture.
    let (missing, missing_outbound, _terminal) = test_context(2);
    modifier(&missing, VK_LCONTROL, KeyPhase::Down);
    assert!(!record(
        &missing,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(missing_outbound.try_recv().is_err());

    // An extra modifier prevents the next fresh gesture.
    assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    ));
    assert!(outbound.try_recv().is_err());

    // An extra held letter also prevents the otherwise exact chord.
    let (extra, extra_outbound, _terminal) = test_context(2);
    modifier(&extra, VK_LCONTROL, KeyPhase::Down);
    modifier(&extra, VK_LSHIFT, KeyPhase::Down);
    record(
        &extra,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    );
    assert!(!record(
        &extra,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(extra_outbound.try_recv().is_err());
}

#[test]
fn wrong_order_extra_letters_and_injected_records_never_activate_or_mutate() {
    let (context, outbound, _terminal) = test_context(4);
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());

    // Externally injected modifiers and letters cannot mutate physical
    // state or complete a sequence.
    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        true,
    ));
    assert!(!process_hook_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false)
    );
    assert!(context.keyboard.lock().unwrap().physical.observe(
        PhysicalKey::Letter(ActivationKey::P),
        None,
        KeyPhase::Down
    ));

    // Talking Quill's own marked SendInput records remain entirely inert.
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!process_hook_record_at(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        InjectionKind::Helper,
        HookObservation::default(),
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false)
    );
}
