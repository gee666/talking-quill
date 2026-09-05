//! Execute transaction turns while retaining continuation authority.

use super::*;

pub(super) enum DriveCompletion {
    Complete(Completion),
    ActivationDeferred,
    NativeObservationPending,
    Failed,
}

pub(super) fn drive_transaction_turn(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    mut turn: Turn,
    mut activation_reservation: Option<ActivationReservation>,
) -> DriveCompletion {
    keyboard.last_native_effect_failed = false;
    let mut injection_failed = false;
    for _ in 0..=MAX_EFFECTS_PER_TURN {
        match turn {
            Turn::Complete { engine, completion } => {
                context.observability.publish(engine.metrics());
                keyboard.transactional = engine;
                keyboard.inflight_effect = None;
                if keyboard.transactional.owned_letters() == 0
                    && keyboard.transactional.journal_len() == 0
                {
                    keyboard.candidate_target_captured = false;
                    keyboard.candidate_target = None;
                    keyboard.owned_down_records = [None; 26];
                }
                if let Completion::Event(outcome) = completion {
                    set_current_edge_disposition(
                        context,
                        keyboard,
                        if outcome.disposition == EventDisposition::CaptureCurrent {
                            CurrentEdgeDisposition::Owned
                        } else {
                            CurrentEdgeDisposition::Pass
                        },
                    );
                }
                keyboard.last_native_effect_failed = injection_failed;
                if injection_failed && !context.terminal.is_triggered() {
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context
                        .terminal
                        .trigger(TerminalReason::InputInjectionUnavailable);
                }
                return DriveCompletion::Complete(completion);
            }
            Turn::NeedEffect {
                effect,
                continuation,
            } => {
                keyboard.inflight_effect = Some(InflightEffect {
                    effect,
                    continuation: continuation.clone(),
                    outcome: None,
                });
                let outcome = match effect {
                    EffectRequest::NeutralizeMenu(_) => {
                        // The macOS engine is constructed with NotRequired and
                        // must never report fictional native dummy acceptance.
                        injection_failed = true;
                        EffectOutcome::Neutralized { accepted: 0 }
                    }
                    EffectRequest::CleanupMenuNeutralization(_) => {
                        injection_failed = true;
                        EffectOutcome::MenuCleanupAccepted { accepted: 0 }
                    }
                    EffectRequest::DeliverActivation(notice) => {
                        let retained_reservation =
                            activation_reservation.take().or(keyboard.candidate_target);
                        if !matches!(notice, ActivationNotice::Up { .. })
                            && let Some(reservation) = retained_reservation
                            && let Some(cache) = context.target_cache.as_ref()
                            && let Some(validation_request) =
                                cache.request_activation_validation(&reservation)
                            && let Ok(mut pending) = context.pending_activation.try_lock()
                            && pending.is_none()
                        {
                            *pending = Some(PendingActivation {
                                continuation: continuation.clone(),
                                notice,
                                reservation,
                                validation_request,
                                resolved_delivery: None,
                                deadline: Instant::now() + ACTIVATION_TARGET_TIMEOUT,
                            });
                            drop(pending);
                            keyboard.inflight_effect = None;
                            arm_maintenance_timer(context);
                            #[cfg(test)]
                            if PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.replace(false)) {
                                panic!("induced panic after deferred activation install");
                            }
                            return DriveCompletion::ActivationDeferred;
                        }
                        EffectOutcome::ActivationDelivered(keyboard.dispatcher.deliver(
                            &context.outbound,
                            &context.terminal,
                            notice,
                            None,
                        ))
                    }
                    EffectRequest::Replay(batch) => {
                        #[cfg(test)]
                        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(count.get() + 1));
                        let target_is_current = !keyboard.candidate_target_captured
                            || keyboard.candidate_target.is_some_and(|reservation| {
                                context
                                    .target_cache
                                    .as_ref()
                                    .is_some_and(|cache| cache.reservation_is_current(&reservation))
                            });
                        if !target_is_current {
                            let outcome = EffectOutcome::ReplaySuppressedTargetChanged;
                            if let Some(inflight) = keyboard.inflight_effect.as_mut() {
                                inflight.outcome = Some(outcome);
                            }
                            turn = continuation.resume(outcome);
                            continue;
                        }
                        let submission = if keyboard.replay_observation.is_none() {
                            context.native_events.try_lock().ok().and_then(|mut pool| {
                                let pool = pool.as_mut()?;
                                let prepared = injection::prepare_replay(pool, batch)?;
                                let submission = prepared.submission();
                                let token = submission.token?;
                                if !keyboard.begin_replay_observation(
                                    ExpectedReplayBatch::Replay(batch),
                                    token,
                                ) {
                                    return None;
                                }
                                // Primary-callback replay always enters the HID
                                // stream. Reducer finalization waits for every
                                // exact authenticated tap observation; proxy
                                // submission alone is never treated as replay.
                                prepared.post(pool, None);
                                Some(submission)
                            })
                        } else {
                            None
                        }
                        .unwrap_or(injection::Submission {
                            count: 0,
                            token: None,
                        });
                        if submission.count == batch.len() {
                            #[cfg(feature = "transactional-shortcuts-dev")]
                            if context.test_physical_seam_enabled {
                                crate::platform::macos::record_test_replay_submission();
                            }
                            arm_maintenance_timer(context);
                            return DriveCompletion::NativeObservationPending;
                        }
                        injection_failed = true;
                        EffectOutcome::ReplaySubmitted { submitted: 0 }
                    }
                    EffectRequest::CleanupInjected(batch) => {
                        #[cfg(test)]
                        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(count.get() + 1));
                        // Cleanup contains balancing ups only. Once a replay
                        // down was visible, its release must not be stranded by
                        // a later focus change.
                        let submission = if keyboard.replay_observation.is_none() {
                            context.native_events.try_lock().ok().and_then(|mut pool| {
                                let pool = pool.as_mut()?;
                                let prepared = injection::prepare_cleanup(pool, batch)?;
                                let submission = prepared.submission();
                                let token = submission.token?;
                                if !keyboard.begin_replay_observation(
                                    ExpectedReplayBatch::Cleanup(batch),
                                    token,
                                ) {
                                    return None;
                                }
                                prepared.post(pool, None);
                                Some(submission)
                            })
                        } else {
                            None
                        }
                        .unwrap_or(injection::Submission {
                            count: 0,
                            token: None,
                        });
                        if submission.count == batch.len() {
                            arm_maintenance_timer(context);
                            return DriveCompletion::NativeObservationPending;
                        }
                        injection_failed = true;
                        EffectOutcome::CleanupSubmitted { submitted: 0 }
                    }
                };
                if let Some(inflight) = keyboard.inflight_effect.as_mut() {
                    inflight.outcome = Some(outcome);
                }
                #[cfg(test)]
                if PANIC_AFTER_EFFECT_OUTCOME.with(|flag| flag.replace(false)) {
                    panic!("induced panic after native effect outcome");
                }
                turn = continuation.resume(outcome);
            }
        }
    }
    // A malformed effect chain is terminal, but panicking here would discard
    // its continuation-owned engine. The caller's authoritative snapshot stays
    // installed and owner recovery replays/drains it.
    keyboard.last_native_effect_failed = true;
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.terminal.trigger(TerminalReason::ReducerPoisoned);
    DriveCompletion::Failed
}
pub(super) fn begin_transaction_snapshot(engine: &TransactionEngine, input: EngineInput) -> Turn {
    engine.clone().begin(input)
}

pub(super) fn begin_transaction_control(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    control: Control,
) -> talking_quill_keyboard_core::transactional::ControlOutcome {
    if keyboard.has_submitted_replay_authority() {
        // A successfully HID-submitted replay is immutable native authority.
        // No lifecycle/control request may replace its continuation, begin a
        // successor turn, or synthesize a failed outcome while any exact edge
        // remains unobserved. Admission/terminal atomics are owned by callers;
        // only exact observation may retire it. Terminal teardown stays incomplete.
        return talking_quill_keyboard_core::transactional::ControlOutcome {
            applied: false,
            cancellation: None,
            shutdown: keyboard.transactional.shutdown_state(),
        };
    }
    // Begin from an allocation-free snapshot. The authoritative engine stays
    // installed until a complete turn replaces it, so an unexpected unwind
    // can never leave Default in place or lose captured ownership.
    let turn = begin_transaction_snapshot(&keyboard.transactional, EngineInput::Control(control));
    let completion = drive_transaction_turn(context, keyboard, turn, None);
    match completion {
        DriveCompletion::Complete(Completion::Control(outcome)) => outcome,
        DriveCompletion::NativeObservationPending => {
            talking_quill_keyboard_core::transactional::ControlOutcome {
                applied: false,
                cancellation: None,
                shutdown: keyboard.transactional.shutdown_state(),
            }
        }
        DriveCompletion::Complete(Completion::Event(_)) | DriveCompletion::ActivationDeferred => {
            talking_quill_keyboard_core::transactional::ControlOutcome {
                applied: false,
                cancellation: Some(CancelReason::EffectProtocolViolation),
                shutdown: keyboard.transactional.shutdown_state(),
            }
        }
        DriveCompletion::Failed => talking_quill_keyboard_core::transactional::ControlOutcome {
            applied: false,
            cancellation: Some(CancelReason::EffectProtocolViolation),
            shutdown: keyboard.transactional.shutdown_state(),
        },
    }
}
