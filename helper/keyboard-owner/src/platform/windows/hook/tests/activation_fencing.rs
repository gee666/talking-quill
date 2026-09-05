use super::*;

#[test]
fn closed_gate_tracks_native_state_without_retaining_future_prefixes() {
    let (context, outbound, _terminal) = test_context(4);
    context.gate.close();
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
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
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    );
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );

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
}

#[test]
fn every_binding_revision_fences_physically_held_letters_until_release() {
    let (context, outbound, _terminal) = test_context(4);
    context.gate.close();
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
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
        ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[one_key]).unwrap(),
        },
    );
    context.gate.open();
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Up,
    );
    record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Up,
    );
    assert!(record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
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

#[test]
fn modifier_changes_fence_a_passive_native_prefix_until_all_letters_release() {
    let (context, outbound, _terminal) = test_context(4);
    assert!(!record(
        &context,
        0x58,
        PhysicalKey::Letter(ActivationKey::X),
        KeyPhase::Down,
    ));
    modifier(&context, VK_LMENU, KeyPhase::Down);
    assert!(!record(
        &context,
        0x50,
        PhysicalKey::Letter(ActivationKey::P),
        KeyPhase::Down,
    ));
    assert!(outbound.try_recv().is_err());
    for (virtual_key, physical) in [
        (0x50, PhysicalKey::Letter(ActivationKey::P)),
        (0x58, PhysicalKey::Letter(ActivationKey::X)),
    ] {
        record(&context, virtual_key, physical, KeyPhase::Up);
    }
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
}
