use super::*;

#[test]
fn altgr_layout_keeps_right_alt_suppressed_without_a_visible_ctrl_edge() {
    let mut modifiers = ModifierTracker::default();
    modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Down);
    assert!(conservative_altgr_for_layout(&modifiers, false, true));
    assert!(!conservative_altgr_for_layout(&modifiers, false, false));
    modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Up);
    assert!(!conservative_altgr_for_layout(&modifiers, false, true));
}

#[test]
fn altgr_never_supplies_or_matches_activation_modifiers() {
    for modifiers in [
        ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        },
        ShortcutModifiers {
            ctrl: true,
            alt: true,
            shift: false,
            meta: false,
        },
    ] {
        let (context, outbound, _terminal) = test_context(4);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[ActivationBinding::new(
                ProfileId::GENERAL,
                shortcut(modifiers, &[ActivationKey::X]),
            )])
            .unwrap(),
        };

        // Windows synthesizes an injected left-Ctrl immediately before the
        // physical right-Alt record for AltGr. The synthetic record may
        // suppress activation, but it must never supply a modifier.
        assert!(!process_hook_record(
            &context,
            if modifiers.ctrl {
                VK_LCONTROL
            } else {
                VK_CONTROL
            },
            0x1D,
            false,
            KeyPhase::Down,
            true,
        ));
        assert!(!process_hook_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Down,
            false,
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::new(false, true, false, false),
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
        assert!(!process_hook_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Up,
            false,
        ));
        assert!(!context.keyboard.lock().unwrap().altgr_active);
    }
}

#[test]
fn physical_looking_altgr_and_missed_release_never_activate_plain_typing() {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        shortcut(
            ShortcutModifiers {
                ctrl: true,
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

    // Some layouts expose AltGr's synthetic Ctrl as a physical-looking
    // record. Right Alt suppression must still prevent Ctrl+Alt activation.
    assert!(!process_hook_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        false,
    ));
    assert!(!process_hook_record(
        &context,
        VK_RMENU,
        0x38,
        true,
        KeyPhase::Down,
        false,
    ));
    let mut held_altgr = ModifierTracker::default();
    held_altgr.observe(VK_LCONTROL, 0x1D, false, KeyPhase::Down);
    held_altgr.observe(VK_RMENU, 0x38, true, KeyPhase::Down);
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(held_altgr),
        },
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

    // If the desktop transition loses every AltGr release event, the next
    // native snapshot repairs both modifiers and suppression without using
    // the ordinary X as a shortcut.
    let no_modifiers = ModifierTracker::default();
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(no_modifiers),
        },
    ));
    assert!(!context.keyboard.lock().unwrap().altgr_active);
    assert!(outbound.try_recv().is_err());
}
