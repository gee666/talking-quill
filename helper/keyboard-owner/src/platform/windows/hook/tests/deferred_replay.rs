use super::*;

#[test]
fn callback_racing_deferred_replay_passes_without_replacing_authority() {
    let (context, _outbound, _terminal) = test_context(2);
    let mut keyboard = context.keyboard.lock().unwrap();
    let before_journal_len = keyboard.transactional.journal_len();
    keyboard.transaction_authority = Some(TransactionAuthority::AwaitingDeferredReplay);
    drop(keyboard);

    let disposition = Cell::new(CallbackDisposition::Capture);
    let captured = process_transactional_hook_record_at(
        &context,
        TransactionalHookRecord {
            virtual_key: 0x41,
            scan_code: 0x1e,
            extended: false,
            platform_flags: 0,
            phase: KeyPhase::Down,
            source: InputSource::Physical,
        },
        HookObservation {
            observed_at_ms: 1,
            native_modifiers: None,
        },
        &disposition,
    );

    let keyboard = context.keyboard.lock().unwrap();
    assert!(!captured);
    assert_eq!(disposition.get(), CallbackDisposition::Pass);
    assert!(matches!(
        keyboard.transaction_authority,
        Some(TransactionAuthority::AwaitingDeferredReplay)
    ));
    assert_eq!(keyboard.transactional.journal_len(), before_journal_len);
}

#[test]
fn deferred_menu_release_is_skipped_after_same_side_is_repressed() {
    let (context, _outbound, _terminal) = test_context(1);
    let mut keyboard = context.keyboard.lock().unwrap();
    assert!(menu_modifier_release_still_needed(&keyboard, 0));
    keyboard
        .modifiers
        .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    assert!(!menu_modifier_release_still_needed(&keyboard, 0));
    keyboard
        .modifiers
        .observe(VK_LMENU, 0x38, false, KeyPhase::Up);
    assert!(menu_modifier_release_still_needed(&keyboard, 0));
}

#[test]
fn async_poll_cannot_release_suppressed_physical_prefix_before_suffix() {
    let (mut context, outbound, terminal) = test_context(4);
    let (sender, receiver) = bounded(1);
    context.replay_sender = Some(sender);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    let edge = |key, scan, phase, at| {
        transactional_record(&context, key, scan, false, phase, InputSource::Physical, at)
    };
    assert!(!edge(VK_LMENU, 0x38, KeyPhase::Down, 1));
    assert!(edge(0x58, 0x2d, KeyPhase::Down, 2));
    let mut modifiers = ModifierTracker::default();
    modifiers.observe(VK_LMENU, 0x38, false, KeyPhase::Down);
    // Windows sees Alt, but X-down was suppressed. Multiple timer samples
    // while the user holds the prefix must leave it available for Alt+X+P.
    for _ in 0..20 {
        reconcile_sampled_state(
            &context,
            current_input_desktop(),
            WindowsPhysicalTracker::default(),
            modifiers,
            false,
        );
        assert_eq!(
            context.keyboard.lock().unwrap().transactional.journal_len(),
            1
        );
        assert!(
            receiver.try_recv().is_err(),
            "poll must not replay the prefix"
        );
        assert!(outbound.try_recv().is_err());
        assert!(terminal.try_recv().is_err());
    }
    assert!(edge(0x50, 0x19, KeyPhase::Down, 1000));
    assert!(matches!(
        receiver.try_recv(),
        Ok(ReplayWork::NeutralizeMenu { .. })
    ));
    context.replay_accepted.store(4, Ordering::Release);
    process_deferred_callback_replay(&context);
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    ));
    assert!(edge(0x50, 0x19, KeyPhase::Up, 1001));
    assert!(edge(0x58, 0x2d, KeyPhase::Up, 1002));
    assert!(!edge(VK_LMENU, 0x38, KeyPhase::Up, 1003));
    assert!(matches!(
        receive_event(&outbound),
        KeyboardEvent::Activation {
            phase: EventPhase::Up,
            ..
        }
    ));
    assert_eq!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .owned_letters(),
        0
    );
}

#[test]
fn quick_suffix_release_during_menu_work_delivers_up_and_allows_the_next_shortcut() {
    for source in [InputSource::Physical, InputSource::External] {
        let (mut context, outbound, terminal) = test_context(8);
        let (sender, receiver) = bounded(1);
        context.replay_sender = Some(sender);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        for round in 0..2 {
            for (index, (key, scan, phase)) in [
                (VK_LMENU, 0x38, KeyPhase::Down),
                (0x58, 0x2d, KeyPhase::Down),
                (0x50, 0x19, KeyPhase::Down),
                (0x50, 0x19, KeyPhase::Up),
                (0x58, 0x2d, KeyPhase::Up),
                (VK_LMENU, 0x38, KeyPhase::Up),
            ]
            .into_iter()
            .enumerate()
            {
                transactional_record(
                    &context,
                    key,
                    scan,
                    false,
                    phase,
                    source,
                    round * 100 + index as u64,
                );
            }
            assert!(matches!(
                receiver.try_recv(),
                Ok(ReplayWork::NeutralizeMenu { .. })
            ));
            context.replay_accepted.store(4, Ordering::Release);
            process_deferred_callback_replay(&context);
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
            assert!(terminal.try_recv().is_err());
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.owned_letters(), 0);
            assert!(keyboard.transactional.admission_open());
            assert!(keyboard.dispatcher.active.is_none());
        }
    }
}

#[test]
fn unavailable_replay_worker_publishes_a_consumable_suppression_result() {
    let (context, _outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: true,
            bindings: full_bindings(),
        },
    );
    assert!(!transactional_record(
        &context,
        VK_MENU,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        1,
    ));
    assert!(transactional_record(
        &context,
        0x58,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        2,
    ));
    assert!(transactional_record(
        &context,
        0x5a,
        0,
        false,
        KeyPhase::Down,
        InputSource::External,
        3,
    ));
    assert_eq!(context.replay_accepted.load(Ordering::Acquire), 1);
    process_deferred_callback_replay(&context);
    let keyboard = context.keyboard.lock().unwrap();
    assert!(keyboard.deferred_callback_replay.is_none());
    assert!(keyboard.transaction_authority.is_none());
    assert!(terminal.try_recv().is_ok());
}
