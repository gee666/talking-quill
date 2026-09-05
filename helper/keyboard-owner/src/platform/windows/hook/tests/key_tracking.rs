use super::*;

#[test]
fn scan_codes_map_every_dom_letter_position_independent_of_virtual_key() {
    for (index, scan_code) in LETTER_SCAN_CODES.iter().copied().enumerate() {
        assert_eq!(
            map_scan_code(scan_code, false),
            PhysicalKey::Letter(ActivationKey::from_index(index as u8).unwrap())
        );
        assert_eq!(map_scan_code(scan_code, true), PhysicalKey::Other);
    }
    assert_eq!(map_scan_code(0x01, false), PhysicalKey::Escape);
    assert_eq!(map_scan_code(0x1C, false), PhysicalKey::Enter);
    assert_eq!(map_scan_code(0x1C, true), PhysicalKey::Enter);
    assert_eq!(enter_source(0x1C, false), Some(EnterSource::Main));
    assert_eq!(enter_source(0x1C, true), Some(EnterSource::Numpad));
    assert_eq!(enter_source(0x01, false), None);
    assert_eq!(map_scan_code(0, false), PhysicalKey::Other);
    assert_eq!(
        map_key_identity(0x58, 0, false),
        KeyIdentity::Letter(ActivationKey::X)
    );
    assert_eq!(map_key_identity(0x30, 0, false), KeyIdentity::Other(0x30));
}

#[test]
fn post_install_snapshot_seeds_every_tracked_key_without_seeding_reducer() {
    let held = [
        PhysicalKey::Letter(ActivationKey::X),
        PhysicalKey::Escape,
        PhysicalKey::Enter,
    ];
    let mut queried = Vec::new();
    let mut tracker = physical_tracker_from_state(|key| {
        queried.push(key);
        held.contains(&key)
    });
    assert_eq!(queried.len(), 28);
    for index in 0_u8..26 {
        let key = PhysicalKey::Letter(ActivationKey::from_index(index).unwrap());
        assert_eq!(
            tracker.observe(key, None, KeyPhase::Down),
            held.contains(&key),
            "physical letter index {index}"
        );
    }
    assert!(tracker.observe(PhysicalKey::Escape, None, KeyPhase::Down));
    assert!(tracker.observe(PhysicalKey::Enter, Some(EnterSource::Main), KeyPhase::Down,));
    assert!(tracker.observe(
        PhysicalKey::Enter,
        Some(EnterSource::Numpad),
        KeyPhase::Down,
    ));
}

#[test]
fn modifier_tracker_is_exact_side_aware_and_generic_safe() {
    let mut tracker = ModifierTracker::from_state(|key| key == VK_LSHIFT);
    assert_eq!(tracker.mask(), ModifierMask::new(false, false, true, false));
    tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Down);
    tracker.observe(VK_LSHIFT, 0x2A, false, KeyPhase::Up);
    assert!(tracker.mask().shift());
    tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Up);
    assert_eq!(tracker.mask(), ModifierMask::default());

    for (key, expected) in [
        (VK_CONTROL, ModifierMask::new(true, false, false, false)),
        (VK_MENU, ModifierMask::new(false, true, false, false)),
        (VK_SHIFT, ModifierMask::new(false, false, true, false)),
        (VK_LWIN, ModifierMask::new(false, false, false, true)),
    ] {
        let mut tracker = ModifierTracker::default();
        assert!(tracker.observe(key, 0, false, KeyPhase::Down));
        assert_eq!(tracker.mask(), expected);
        assert!(tracker.observe(key, 0, false, KeyPhase::Up));
        assert_eq!(tracker.mask(), ModifierMask::default());
    }

    // Generic virtual keys are resolved by scan code/extended state, so
    // releasing one side cannot clear the other. AltGr remains exact
    // Ctrl+Alt rather than an injected or implicit modifier.
    let mut tracker = ModifierTracker::default();
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
    tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Down);
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Up);
    assert!(tracker.mask().ctrl());
    tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Up);
    assert!(!tracker.mask().ctrl());
    tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
    tracker.observe(VK_MENU, 0x38, true, KeyPhase::Down);
    assert_eq!(tracker.mask(), ModifierMask::new(true, true, false, false));
}

#[test]
fn stale_tracked_alt_is_resynchronized_without_activating_plain_typing() {
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
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[binding]).unwrap(),
        };
        keyboard
            .modifiers
            .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    }

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
    assert!(!process_hook_record_at(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Up,
        InjectionKind::Physical,
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: Some(no_modifiers),
        },
    ));
    assert_eq!(
        context.keyboard.lock().unwrap().modifiers.mask(),
        ModifierMask::default(),
    );
    assert!(outbound.try_recv().is_err());

    modifier(&context, VK_LMENU, KeyPhase::Down);
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
fn generic_and_sided_alt_normalize_with_physical_or_virtual_key_letters() {
    for (alt_vk, alt_scan, alt_extended) in [
        (VK_MENU, 0, false),
        (VK_LMENU, 0x38, false),
        (VK_RMENU, 0x38, true),
    ] {
        let side = modifier_side(alt_vk, alt_scan, alt_extended).unwrap();
        assert_eq!(
            side,
            if alt_extended {
                ModifierSide::RightAlt
            } else {
                ModifierSide::LeftAlt
            }
        );
    }
    for (vk, scan, expected) in [
        (0x58, 0, ActivationKey::X),
        (0, 0x2D, ActivationKey::X),
        (0x50, 0, ActivationKey::P),
        (0, 0x19, ActivationKey::P),
    ] {
        assert_eq!(
            map_key_identity(vk, scan, false),
            KeyIdentity::Letter(expected)
        );
    }
}
