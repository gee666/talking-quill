use super::*;

#[test]
fn external_injected_modifiers_never_activate_or_clear_physical_modifiers() {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::X],
        ),
    );
    let (context, outbound, _terminal) = test_context(4);
    context.keyboard.lock().unwrap().activation = ActivationConfig {
        enabled: true,
        bindings: ActivationBindings::new(&[binding]).unwrap(),
    };

    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Down,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::default(),
    );
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!process_hook_record(
        &context,
        VK_LMENU,
        0,
        false,
        KeyPhase::Up,
        true,
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::new(false, true, false, false),
    );
    assert!(record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            binding,
            context: activation_context(),
            phase: EventPhase::Down,
        },
    );
}

#[test]
fn helper_classes_bypass_and_ctrl_right_alt_fails_closed_as_altgr() {
    let (context, outbound, _terminal) = test_context(8);
    let ctrl_alt = ActivationBindings::new(&[ActivationBinding::new(
        ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        shortcut(
            ShortcutModifiers {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    )])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: ctrl_alt,
        },
    );
    let before = context.keyboard.lock().unwrap().transactional.clone();
    for source in [
        InputSource::HelperReplay,
        InputSource::HelperPaste,
        InputSource::HelperDummy,
    ] {
        assert!(!transactional_record(
            &context,
            0x50,
            0x19,
            false,
            KeyPhase::Down,
            source,
            1,
        ));
    }
    assert_eq!(context.keyboard.lock().unwrap().transactional, before);

    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(!transactional_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        InputSource::Physical,
        10,
    ));
    assert!(context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
    assert!(!transactional_record(
        &context,
        0x50,
        0x19,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        11,
    ));
    assert!(outbound.try_recv().is_err());
}

#[test]
fn deferred_replay_race_tracks_ctrl_right_alt_as_altgr() {
    let mut keyboard = CallbackKeyboard::default();
    track_deferred_replay_race(&mut keyboard, VK_LCONTROL, 0x1D, false, KeyPhase::Down);
    track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Down);
    assert!(keyboard.altgr_synthetic_ctrl);
    assert!(keyboard.altgr_active);
    track_deferred_replay_race(&mut keyboard, 0x50, 0x19, false, KeyPhase::Down);
    assert!(keyboard.altgr_active);
    track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Up);
    assert!(!keyboard.altgr_synthetic_ctrl);
    assert!(!keyboard.altgr_active);
}

#[test]
fn external_ctrl_is_real_input_and_cannot_fake_an_alt_only_binding() {
    let (context, outbound, _terminal) = test_context(4);
    let alt = ActivationBindings::new(&[ActivationBinding::new(
        ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        shortcut(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::P],
        ),
    )])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: alt,
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::External,
        0,
    ));
    assert!(!context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
    assert!(!transactional_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        0x50,
        0x19,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(outbound.try_recv().is_err());
    let keyboard = context.keyboard.lock().unwrap();
    assert!(
        keyboard
            .transactional
            .physical_modifiers()
            .combined()
            .ctrl()
    );
}
