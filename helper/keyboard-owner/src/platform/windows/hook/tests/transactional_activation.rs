use super::*;

#[test]
fn real_transactional_hook_path_captures_ctrl_shift_activation_and_balances_up() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );

    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        1,
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Down,
        InputSource::Physical,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Down,
        InputSource::Physical,
        3,
    ));
    let KeyboardEvent::Activation {
        binding,
        context: activation_context,
        phase: EventPhase::Down,
    } = receive_event(&outbound)
    else {
        panic!("expected transactional activation down")
    };
    assert_eq!(binding.profile_id(), ProfileId::GENERAL);
    assert_eq!(
        activation_context.activation_generation(),
        ActivationGeneration::FIRST
    );
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        InputSource::Physical,
        9,
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            context,
            phase: EventPhase::Up,
            ..
        } if context == activation_context
    ));
    assert!(!transactional_record(
        &context,
        VK_LSHIFT,
        0x2A,
        false,
        KeyPhase::Up,
        InputSource::Physical,
        10,
    ));
    assert!(!transactional_record(
        &context,
        VK_LCONTROL,
        0x1D,
        false,
        KeyPhase::Up,
        InputSource::Physical,
        11,
    ));
    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.owned_letters(), 0);
    assert_eq!(
        keyboard.transactional.physical_modifiers(),
        TransactionalModifierSides::default()
    );
}

#[test]
fn external_alt_x_p_is_input_equivalent_and_activates_once() {
    let (mut context, outbound, _terminal) = test_context(4);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Down,
        InputSource::External,
        3,
    ));
    assert!(matches!(
        replay_receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(transactional_record(
        &context,
        0x50,
        LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
        false,
        KeyPhase::Up,
        InputSource::External,
        4,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
        false,
        KeyPhase::Up,
        InputSource::External,
        5,
    ));
    assert!(!transactional_record(
        &context,
        VK_LMENU,
        0x38,
        false,
        KeyPhase::Up,
        InputSource::External,
        6,
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Up,
            ..
        }
    ));
    assert!(outbound.try_recv().is_err());
}

#[test]
fn physical_and_external_alt_x_share_matcher_suppression_and_delivery() {
    for source in [InputSource::Physical, InputSource::External] {
        let (mut context, outbound, _terminal) = test_context(4);
        let (replay_sender, replay_receiver) = bounded(1);
        context.replay_sender = Some(replay_sender);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        let records = [
            (VK_LMENU, 0x38, KeyPhase::Down, false),
            (
                0x58,
                LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                KeyPhase::Down,
                true,
            ),
            (
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                KeyPhase::Down,
                true,
            ),
            (
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                KeyPhase::Up,
                true,
            ),
            (
                0x58,
                LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                KeyPhase::Up,
                true,
            ),
            (VK_LMENU, 0x38, KeyPhase::Up, false),
        ];
        for (at, (virtual_key, scan_code, phase, expected_capture)) in
            records.into_iter().enumerate()
        {
            assert_eq!(
                transactional_record(
                    &context,
                    virtual_key,
                    scan_code,
                    false,
                    phase,
                    source,
                    u64::try_from(at + 1).unwrap(),
                ),
                expected_capture,
                "source={source:?} edge={at}",
            );
            if at == 2 {
                assert!(matches!(
                    replay_receiver.try_recv(),
                    Ok(ReplayWork::NeutralizeMenu { .. })
                ));
                context.replay_accepted.store(4, Ordering::Release);
                process_deferred_callback_replay(&context);
            }
        }
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Up,
                ..
            }
        ));
        assert!(outbound.try_recv().is_err());
    }
}

#[test]
fn redundant_native_releases_do_not_retire_the_owner() {
    for source in [InputSource::Physical, InputSource::External] {
        let (context, outbound, terminal) = test_context(8);
        let bindings = ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::new("general").unwrap(),
            shortcut(
                ShortcutModifiers {
                    ctrl: true,
                    shift: true,
                    alt: false,
                    meta: false,
                },
                &[ActivationKey::J, ActivationKey::K, ActivationKey::L],
            ),
        )])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings,
            },
        );
        let edge =
            |key, scan, phase| transactional_record(&context, key, scan, false, phase, source, 1);
        for _ in 0..3 {
            // A release can arrive after polling has already observed neutral,
            // or from an external sender balancing its own modifier state.
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Down));
            assert!(!edge(VK_LSHIFT, 0x2a, KeyPhase::Down));
            for (key, scan) in [(0x4a, 0x24), (0x4b, 0x25), (0x4c, 0x26)] {
                assert!(edge(key, scan, KeyPhase::Down));
            }
            for (key, scan) in [(0x4c, 0x26), (0x4b, 0x25), (0x4a, 0x24)] {
                assert!(edge(key, scan, KeyPhase::Up));
                assert!(!edge(key, scan, KeyPhase::Up));
            }
            assert!(!edge(VK_LSHIFT, 0x2a, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert!(!edge(VK_LCONTROL, 0x1d, KeyPhase::Up));
            assert_eq!(outbound.try_iter().count(), 2);
            assert!(terminal.try_recv().is_err());
            assert!(!context.terminal.is_triggered());
        }
    }
}
