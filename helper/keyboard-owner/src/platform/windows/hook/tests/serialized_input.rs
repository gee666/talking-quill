use super::*;

#[test]
fn hardware_shaped_physical_syskey_alt_x_uses_ordered_hook_edges() {
    let (mut context, outbound, _terminal) = test_context(4);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let alt = ShortcutModifiers {
        ctrl: false,
        alt: true,
        shift: false,
        meta: false,
    };
    let bindings = ActivationBindings::new(&[
        ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        ),
    ])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings,
        },
    );
    let process = |virtual_key, scan_code, phase, flags| {
        let disposition = Cell::new(CallbackDisposition::Pass);
        process_transactional_hook_record_at(
            &context,
            TransactionalHookRecord {
                virtual_key,
                scan_code,
                extended: false,
                platform_flags: flags,
                phase,
                source: InputSource::Physical,
            },
            HookObservation {
                observed_at_ms: 1,
                // This is the production physical callback contract: the
                // asynchronous state that can lag Alt is diagnostic only.
                native_modifiers: None,
            },
            &disposition,
        )
    };
    const ALT_CONTEXT_FLAG: u32 = 0x20;
    // Exact physical shape captured on the affected machine. Alt-up won
    // the release race by 16 ms, before X-up. The shorter Alt+X binding
    // must still commit, suppress X-up, and leave the owner ready for the
    // following Alt+X+P attempt.
    assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(VK_LMENU, 0x38, KeyPhase::Up, 0x80));
    assert!(matches!(
        replay_receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    assert!(process(0x58, 0x2d, KeyPhase::Up, 0x80));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(outbound.try_iter().any(|event| matches!(
        event,
        NativeEvent::Keyboard(KeyboardEvent::ActivationComplete { .. })
    )));
    {
        let keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.transactional.admission_open());
        assert_eq!(keyboard.transactional.owned_letters(), 0);
    }

    assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
    assert!(process(0x50, 0x19, KeyPhase::Down, ALT_CONTEXT_FLAG));
}

#[test]
fn exact_virtual_key_sendinput_sequence_matches_serialized_alt_x_profiles() {
    let (mut context, outbound, _terminal) = test_context(16);
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let alt = ShortcutModifiers {
        ctrl: false,
        alt: true,
        shift: false,
        meta: false,
    };
    let bindings = ActivationBindings::new(&[
        ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        ),
    ])
    .unwrap();
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings,
        },
    );

    let mut at = 1;
    let mut input = |vk, phase| {
        let captured =
            transactional_record(&context, vk, 0, false, phase, InputSource::External, at);
        at += 1;
        captured
    };
    // Exact serialized virtual-key edges emitted by the installed
    // PowerShell validator: unmatched Alt+X+Z, Alt+X, Alt+X+P, then U.
    for (index, (vk, phase, expected)) in [
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x5A, KeyPhase::Down, true),
        (0x5A, KeyPhase::Up, false),
        (0x58, KeyPhase::Up, false),
        (VK_MENU, KeyPhase::Up, false),
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x58, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Down, false),
        (0x58, KeyPhase::Down, true),
        (0x50, KeyPhase::Down, true),
        (0x50, KeyPhase::Up, true),
        (0x58, KeyPhase::Up, true),
        (VK_MENU, KeyPhase::Up, false),
        (0x55, KeyPhase::Down, false),
        (0x55, KeyPhase::Up, false),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            input(vk, phase),
            expected,
            "event={index} vk={vk:#x} phase={phase:?}"
        );
        if index == 5 {
            // Unit tests have no replay worker or owner message pump.
            // Publish the exact accepted count at a deterministic boundary
            // after proving racing physical edges preserve its authority.
            let work = replay_receiver.try_recv().unwrap();
            process_deferred_callback_replay(&context);
            {
                let keyboard = context.keyboard.lock().unwrap();
                assert!(keyboard.deferred_callback_replay.is_some());
                assert!(matches!(
                    keyboard.transaction_authority,
                    Some(TransactionAuthority::AwaitingDeferredReplay)
                ));
            }
            let ReplayWork::Replay { batch, .. } = work else {
                panic!("unmatched sequence must defer replay");
            };
            context.replay_accepted.store(
                u64::try_from(batch.len()).unwrap().saturating_add(2),
                Ordering::Release,
            );
            process_deferred_callback_replay(&context);
            let mut keyboard = context.keyboard.lock().unwrap();
            let outcome = begin_transaction_control(
                &context,
                &mut keyboard,
                Control::Reconcile(PhysicalSnapshot::default()),
            );
            assert!(outcome.is_some_and(|outcome| outcome.applied));
            assert!(keyboard.transactional.config().enabled());
            assert!(keyboard.transactional.admission_open());
            assert_eq!(keyboard.transactional.physical_letters(), 0);
            assert_eq!(keyboard.transactional.fenced_letters(), 0);
            assert_eq!(keyboard.transactional.physical_modifiers().bits(), 0);
            assert_eq!(keyboard.transactional.fenced_modifiers().bits(), 0);
        }
        if index == 9 || index == 12 {
            assert!(matches!(
                replay_receiver.try_recv(),
                Ok(ReplayWork::NeutralizeMenu { .. })
            ));
            context.replay_accepted.store(4, Ordering::Release);
            process_deferred_callback_replay(&context);
        }
    }
    let events: Vec<_> = outbound.try_iter().collect();
    let downs = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                NativeEvent::Keyboard(KeyboardEvent::Activation {
                    phase: EventPhase::Down,
                    ..
                })
            )
        })
        .count();
    let ups = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                NativeEvent::Keyboard(KeyboardEvent::Activation {
                    phase: EventPhase::Up,
                    ..
                })
            )
        })
        .count();
    assert_eq!(downs, ups, "activation notifications remain balanced");
    assert!(downs >= 1);
    let observed = context.observability.snapshot();
    assert_eq!(observed.registered_input.registered_candidate_callbacks, 3);
    assert_eq!(observed.transactions.committed, 2);
    assert_eq!(observed.transactions.replayed, 1);
    assert!(!context.terminal.is_triggered());
    assert!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .admission_open()
    );
}
