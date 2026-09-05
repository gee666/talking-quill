//! Target replay contracts.

use super::*;

#[test]
fn stale_candidate_target_suppresses_config_shutdown_callback_tap_and_permission_replays() {
    let controls = [
        Control::ReplaceConfig(
            CompiledActivationConfig::compile(ConfigRevision::new(2), true, bindings()).unwrap(),
        ),
        Control::Shutdown,
        Control::CloseAdmission(CancelReason::ActivationDeliveryFailed),
        Control::CloseAdmission(CancelReason::SecureDesktop),
        Control::CloseAdmission(CancelReason::GateClosed),
    ];
    for control in controls {
        let (mut context, _outbound, _commands) = test_context();
        let cache = TargetCache::with_open_validation_queue_for_test();
        cache.install_current_handle_for_test(
            crate::platform::macos::target::target_handle_for_test(8),
            11,
            1,
        );
        context.target_cache = Some(cache);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            keyboard.candidate_target_captured = true;
            keyboard.candidate_target =
                Some(crate::platform::macos::target::activation_reservation_for_test(7, 11));
            let outcome = begin_transaction_control(&context, &mut keyboard, control);
            assert!(!outcome.applied);
            assert_eq!(outcome.cancellation, Some(CancelReason::TargetChanged));
            assert_eq!(outcome.shutdown, ShutdownState::Terminal);
            assert!(keyboard.replay_observation.is_none());
            assert!(keyboard.transactional.owned_letters() != 0);
        }
        let snapshot = context.observability.snapshot();
        assert_eq!(snapshot.replay.attempted, 0);
        assert_eq!(snapshot.transactions.cancellation_reasons.target_changed, 1);
    }
}

#[test]
fn target_change_suppresses_new_replay_downs_but_allows_balancing_letter_and_modifier_ups() {
    let mut journal = EventJournal::new();
    let records = [
        ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::X),
            native: NativeKey {
                virtual_key: 7,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 1,
        },
        ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::X),
            native: NativeKey {
                virtual_key: 7,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 2,
        },
        ReplayRecord {
            key: KeyIdentity::Modifier(ModifierSide::LeftShift),
            native: NativeKey {
                virtual_key: LEFT_SHIFT_KEY_CODE,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 3,
        },
        ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::P),
            native: NativeKey {
                virtual_key: 35,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 4,
        },
    ];
    for record in records {
        journal.push(record).unwrap();
    }
    let batch = journal.replay_batch().unwrap();
    let mut keyboard = CallbackKeyboard::default();
    assert!(keyboard.begin_replay_observation(
        ExpectedReplayBatch::Replay(batch),
        injection::OperationToken::for_test(1),
    ));
    assert_eq!(
        keyboard.replay_disposition_after_target_check(true),
        CurrentEdgeDisposition::Pass
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 7, false, 0),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.replay_disposition_after_target_check(false),
        CurrentEdgeDisposition::Pass
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 7, false, 0),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.replay_disposition_after_target_check(false),
        CurrentEdgeDisposition::Pass
    );
    assert_eq!(
        keyboard
            .observe_replay_event(ffi::K_CG_EVENT_FLAGS_CHANGED, LEFT_SHIFT_KEY_CODE, false, 0,),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.replay_disposition_after_target_check(false),
        CurrentEdgeDisposition::Owned
    );
    assert!(
        keyboard.visible_replay_cleanup().is_empty(),
        "an already-visible down/up pair needs no terminal cleanup"
    );
}

#[test]
fn target_change_reconciles_the_exact_already_visible_replay_down() {
    let mut journal = EventJournal::new();
    let visible_down = ReplayRecord {
        key: KeyIdentity::Letter(ActivationKey::X),
        native: NativeKey {
            virtual_key: 7,
            scan_code: 53,
            extended: true,
            platform_flags: 0x1234,
        },
        phase: PhysicalPhase::Down,
        observed_at_ms: 9,
    };
    let second_visible_down = ReplayRecord {
        key: KeyIdentity::Letter(ActivationKey::P),
        native: NativeKey {
            virtual_key: 35,
            scan_code: 35,
            extended: false,
            platform_flags: 0x40,
        },
        phase: PhysicalPhase::Down,
        observed_at_ms: 10,
    };
    journal.push(visible_down).unwrap();
    journal.push(second_visible_down).unwrap();
    journal
        .push(ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::A),
            native: NativeKey {
                virtual_key: 0,
                scan_code: 0,
                extended: false,
                platform_flags: 0,
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 11,
        })
        .unwrap();
    let mut keyboard = CallbackKeyboard::default();
    assert!(keyboard.begin_replay_observation(
        ExpectedReplayBatch::Replay(journal.replay_batch().unwrap()),
        injection::OperationToken::for_test(2),
    ));
    assert_eq!(
        keyboard.replay_disposition_after_target_check(true),
        CurrentEdgeDisposition::Pass
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 7, false, 0x1234),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.replay_disposition_after_target_check(true),
        CurrentEdgeDisposition::Pass
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 35, false, 0x40),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.replay_disposition_after_target_check(false),
        CurrentEdgeDisposition::Owned
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 0, false, 0),
        GapBarrierObservation::Complete
    );

    let cleanup = keyboard.visible_replay_cleanup();
    assert_eq!(cleanup.len(), 2);
    assert_eq!(
        cleanup.entries(),
        &[
            ReplayRecord {
                phase: PhysicalPhase::Up,
                ..second_visible_down
            },
            ReplayRecord {
                phase: PhysicalPhase::Up,
                ..visible_down
            },
        ]
    );

    let Turn::NeedEffect {
        effect,
        continuation,
    } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation supplies replay authority")
    };
    let inflight = InflightEffect {
        effect,
        continuation,
        outcome: None,
    };
    assert_eq!(
        target_changed_replay_recovery(&keyboard, &inflight),
        Some(TargetChangedReplayRecovery::ReconcileVisibleDowns),
        "panic after the final original record must reconcile before drain"
    );
    keyboard.replay_target_cleanup_pending = true;
    assert_eq!(
        target_changed_replay_recovery(&keyboard, &inflight),
        Some(TargetChangedReplayRecovery::FinishAfterCleanup),
        "panic after the final cleanup up must finish without reposting it"
    );
    assert!(keyboard.begin_replay_observation(
        ExpectedReplayBatch::Cleanup(cleanup),
        injection::OperationToken::for_test(3),
    ));
    assert_eq!(
        target_changed_replay_recovery(&keyboard, &inflight),
        Some(TargetChangedReplayRecovery::AwaitObservation),
        "a submitted cleanup remains authoritative until exact observation"
    );
}
