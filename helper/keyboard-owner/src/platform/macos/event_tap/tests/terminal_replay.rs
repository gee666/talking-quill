//! Terminal replay contracts.

use super::*;

#[test]
fn full_capacity_submitted_replay_is_immutable_across_every_terminal_interruption() {
    let positions = [0, JOURNAL_CAPACITY / 2, JOURNAL_CAPACITY - 1];
    let disruptions = [
        TerminalReason::InputInjectionUnavailable, // observation timeout
        TerminalReason::EventTapDisabledByUserInput, // Secure Input
        TerminalReason::InputInjectionUnavailable, // permission loss
        TerminalReason::EventTapTimeoutRecoveryFailed, // disabled/timeout tap
    ];
    for current_phase in [PhysicalPhase::Repeat, PhysicalPhase::Down] {
        let batch = full_replay_batch(current_phase);
        for next in positions {
            for (disruption_index, reason) in disruptions.into_iter().enumerate() {
                let (context, _outbound, _commands) = test_context();
                let token = injection::OperationToken::for_test(
                    1_000 + disruption_index as u64 * 100 + next as u64,
                );
                {
                    let mut keyboard = context.keyboard.lock().unwrap();
                    assert!(
                        keyboard
                            .begin_replay_observation(ExpectedReplayBatch::Replay(batch), token,)
                    );
                    keyboard.inflight_effect = Some(InflightEffect {
                        effect: EffectRequest::Replay(batch),
                        continuation: replay_continuation_for_test(),
                        outcome: None,
                    });
                    for index in 0..next {
                        let record = batch.entries()[index];
                        let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
                            ffi::K_CG_EVENT_FLAGS_CHANGED
                        } else if record.phase == PhysicalPhase::Up {
                            ffi::K_CG_EVENT_KEY_UP
                        } else {
                            ffi::K_CG_EVENT_KEY_DOWN
                        };
                        assert_ne!(
                            keyboard.observe_replay_event(
                                event_type,
                                record.native.virtual_key,
                                record.phase == PhysicalPhase::Repeat,
                                record.native.platform_flags,
                            ),
                            GapBarrierObservation::Forged,
                        );
                    }
                }

                close_owned_native_admission(&context, reason, CancelReason::SecureDesktop);
                let mut keyboard = context.keyboard.lock().unwrap();
                TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|attempts| attempts.set(0));
                let snapshot = physical_snapshot(&keyboard);
                for control in [
                    Control::CloseAdmission(CancelReason::SecureDesktop),
                    Control::Reconcile(snapshot),
                    Control::RetryCleanup,
                ] {
                    let outcome = begin_transaction_control(&context, &mut keyboard, control);
                    assert!(!outcome.applied);
                    assert_eq!(outcome.cancellation, None);
                }
                assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|value| value.get()), 0);
                let observation = keyboard
                    .submitted_replay_authority()
                    .expect("submitted replay remains authoritative");
                assert_eq!(observation.token, token);
                assert_eq!(observation.next, next);
                let terminal_suffix = SubmittedReplayTerminalSuffix::capture(observation);
                assert_eq!(terminal_suffix.suffix_len, JOURNAL_CAPACITY - next);
                assert_eq!(
                    &terminal_suffix.suffix[..terminal_suffix.suffix_len],
                    &batch.entries()[next..],
                    "terminal teardown preserves the exact unobserved suffix once",
                );
                assert_eq!(
                    terminal_suffix
                        .suffix
                        .iter()
                        .take(terminal_suffix.suffix_len)
                        .filter(|record| record.observed_at_ms == JOURNAL_CAPACITY as u64)
                        .count(),
                    1,
                    "reserved current repeat/terminator remains exactly once",
                );
            }
        }
    }
}

#[test]
fn owner_lifecycle_failed_recovery_never_duplicates_submitted_effect_or_shutdown_control() {
    let engine = engine_with_ctrl_shift_x_candidate();
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(batch),
        continuation,
    } = engine.begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation requires replay");
    };
    let (context, _outbound, _commands) = test_context();
    let token = injection::OperationToken::for_test(81);
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token));
        keyboard.inflight_effect = Some(InflightEffect {
            effect: EffectRequest::Replay(batch),
            continuation,
            outcome: None,
        });
        // The one-shot authority is installed before the original
        // Shutdown turn submits this replay.
        keyboard.shutdown_requested = true;
        keyboard.shutdown_deadline = Some(Instant::now() + SHUTDOWN_DRAIN_TIMEOUT);
    }
    TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
    TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.set(1));
    enter_owner_lifecycle_recovery(&context);

    // Model an unresolved callback-critical resource. Preflight must fail
    // before consuming the submitted outcome, and shutdown is gated even
    // if a future caller invokes it defensively.
    let native_events = context.native_events.try_lock().unwrap();
    assert!(!attempt_pending_recovery(&context));
    begin_owner_shutdown(&context);
    assert!(context.state.recovery_pending.load(Ordering::Acquire));
    assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
    assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 0);
    assert_eq!(TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.get()), 1);
    drop(native_events);

    assert!(attempt_pending_recovery(&context));
    begin_owner_shutdown(&context);
    begin_owner_shutdown(&context);
    assert!(!context.state.recovery_pending.load(Ordering::Acquire));
    assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
    assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 0);
    assert_eq!(TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.get()), 1);
    let keyboard = context.keyboard.lock().unwrap();
    let observation = keyboard
        .replay_observation
        .as_ref()
        .expect("the original submitted replay remains the only observation");
    assert_eq!(observation.token, token);
    assert_eq!(observation.next, 0);
}
