//! Resume transactions after native effects complete or fail.
use super::*;

impl Continuation {
    pub(super) fn close_before_submission(self, reason: CancelReason) -> Turn {
        match self.pending {
            PendingEffect::Neutralize { engine, event, .. }
            | PendingEffect::ActivationDownOrComplete { engine, event, .. }
            | PendingEffect::MenuCleanup {
                engine,
                event: Some(event),
            } => engine.close_candidate_event_for_later_cancellation(
                event,
                EventDisposition::CaptureCurrent,
                reason,
            ),
            PendingEffect::ActivationUp { engine, event, .. } => {
                engine.terminal_event(event, EventDisposition::CaptureCurrent, reason)
            }
            PendingEffect::ObservedActivationUp { engine } => {
                engine.discard_candidate_control_into_terminal_drain(reason)
            }
            PendingEffect::Replay {
                engine, completion, ..
            } => match completion {
                ReplayCompletion::Event {
                    event,
                    partial_disposition,
                    ..
                } => engine.close_candidate_event_for_later_cancellation(
                    event,
                    partial_disposition,
                    reason,
                ),
                ReplayCompletion::Control(_) => {
                    engine.close_candidate_control_for_later_cancellation(reason)
                }
            },
            PendingEffect::MenuCleanup {
                engine,
                event: None,
            }
            | PendingEffect::Cleanup { engine, .. } => {
                engine.discard_candidate_control_into_terminal_drain(reason)
            }
        }
    }

    #[must_use]
    pub fn resume(self, outcome: EffectOutcome) -> Turn {
        let outcome = match outcome {
            EffectOutcome::ReplayAccepted { accepted } => EffectOutcome::ReplaySubmitted {
                submitted: accepted,
            },
            EffectOutcome::CleanupAccepted { accepted } => EffectOutcome::CleanupSubmitted {
                submitted: accepted,
            },
            outcome => outcome,
        };
        match (self.pending, outcome) {
            (
                PendingEffect::Neutralize {
                    mut engine,
                    event,
                    notice,
                    menu,
                },
                EffectOutcome::Neutralized { accepted: 2 },
            ) => {
                engine.metrics.record_dummy(2);
                if menu.alt {
                    engine.alt_cycle_neutralized = true;
                }
                if menu.meta {
                    engine.meta_cycle_neutralized = true;
                }
                engine.request_activation_delivery(event, notice)
            }
            (
                PendingEffect::Neutralize {
                    mut engine,
                    event,
                    menu,
                    ..
                },
                EffectOutcome::Neutralized { accepted: 1 },
            ) => {
                engine.metrics.record_dummy(1);
                engine.pending_menu_cleanup = Some(menu);
                Turn::NeedEffect {
                    effect: EffectRequest::CleanupMenuNeutralization(menu),
                    continuation: Continuation {
                        pending: PendingEffect::MenuCleanup {
                            engine,
                            event: Some(event),
                        },
                    },
                }
            }
            (
                PendingEffect::Neutralize {
                    mut engine, event, ..
                },
                EffectOutcome::Neutralized { accepted: 0 },
            ) => {
                engine.metrics.record_dummy(0);
                engine.admission_open = false;
                engine.terminal = true;
                engine.request_candidate_cancellation(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::NeutralizationFailed,
                    true,
                    false,
                )
            }
            (
                PendingEffect::MenuCleanup { mut engine, event },
                EffectOutcome::MenuCleanupAccepted { accepted },
            ) if accepted <= 1 => {
                if accepted == 1 {
                    engine.pending_menu_cleanup = None;
                }
                if let Some(event) = event {
                    engine.admission_open = false;
                    engine.terminal = true;
                    engine.request_candidate_cancellation(
                        event,
                        EventDisposition::CaptureCurrent,
                        CancelReason::NeutralizationFailed,
                        true,
                        false,
                    )
                } else if accepted == 1 && engine.pending_injected_cleanup.is_some() {
                    engine.request_retry_cleanup()
                } else {
                    let injected_pending = engine.pending_injected_cleanup.is_some();
                    engine.complete_control(
                        accepted == 1 && !injected_pending,
                        (accepted == 0 || injected_pending)
                            .then_some(CancelReason::NeutralizationFailed),
                    )
                }
            }
            (
                PendingEffect::ActivationDownOrComplete {
                    engine,
                    event,
                    notice,
                },
                EffectOutcome::ActivationDelivered(true),
            ) => engine.finish_successful_activation(event, notice),
            (
                PendingEffect::ActivationDownOrComplete {
                    mut engine, event, ..
                },
                EffectOutcome::ActivationDelivered(false),
            ) => {
                engine.admission_open = false;
                engine.terminal = true;
                engine.request_candidate_cancellation(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::ActivationDeliveryFailed,
                    true,
                    false,
                )
            }
            (
                PendingEffect::ActivationUp { engine, event, .. },
                EffectOutcome::ActivationDelivered(delivered),
            ) => engine.finish_activation_up(event, delivered),
            (
                PendingEffect::ObservedActivationUp { engine },
                EffectOutcome::ActivationDelivered(delivered),
            ) => {
                if delivered {
                    engine.complete_control(true, None)
                } else {
                    engine.discard_candidate_control_into_terminal_drain(
                        CancelReason::ActivationDeliveryFailed,
                    )
                }
            }
            (
                PendingEffect::Replay {
                    mut engine,
                    batch,
                    completion,
                },
                EffectOutcome::ReplaySubmitted { submitted },
            ) => {
                engine.metrics.record_replay(submitted, batch.len());
                engine.metrics.observe_journal(batch.len());
                if submitted == batch.len() {
                    TransactionMetrics::increment(&mut engine.metrics.replayed);
                    let ActivationState::Candidate(mut candidate) = engine.state else {
                        return finish_terminal_completion(
                            engine,
                            completion,
                            CancelReason::EffectProtocolViolation,
                        );
                    };
                    candidate
                        .journal
                        .mark_replayed()
                        .expect("candidate journal is open until full replay");
                    engine.apply_replay(batch);
                    engine.fence_current_physical(false);
                    engine.state = ActivationState::Idle;
                    if let Some(snapshot) = reconciliation_snapshot(completion) {
                        let cleanup = batch
                            .cleanup_for_accepted(batch.len())
                            .expect("full replay count is valid")
                            .for_missing_letters(snapshot.held_letters);
                        if !cleanup.is_empty() {
                            engine.apply_reconciliation(snapshot, false);
                            engine.mark_cleanup_pending_visible(cleanup);
                            engine.pending_injected_cleanup = Some(cleanup);
                            return Turn::NeedEffect {
                                effect: EffectRequest::CleanupInjected(cleanup),
                                continuation: Continuation {
                                    pending: PendingEffect::Cleanup {
                                        engine,
                                        requested: cleanup,
                                        deferred: CleanupBatch::default(),
                                        completion: CleanupCompletion::ReconciledReplay(completion),
                                    },
                                },
                            };
                        }
                    }
                    finish_replay_completion(engine, completion)
                } else {
                    if submitted < batch.len() {
                        engine.apply_replay_prefix(batch, submitted);
                    }
                    let cleanup = match batch.cleanup_for_accepted(submitted) {
                        Ok(cleanup) => cleanup,
                        Err(JournalError::InvalidAcceptedCount) => {
                            engine.admission_open = false;
                            engine.terminal = true;
                            engine.move_owned_to_drain();
                            return finish_terminal_completion(
                                engine,
                                completion,
                                CancelReason::ReplayFailed,
                            );
                        }
                        Err(JournalError::Full | JournalError::Finalized) => unreachable!(),
                    };
                    engine.admission_open = false;
                    engine.terminal = true;
                    engine.move_owned_to_drain();
                    if cleanup.is_empty() {
                        finish_terminal_completion(engine, completion, CancelReason::ReplayFailed)
                    } else {
                        engine.pending_injected_cleanup = Some(cleanup);
                        Turn::NeedEffect {
                            effect: EffectRequest::CleanupInjected(cleanup),
                            continuation: Continuation {
                                pending: PendingEffect::Cleanup {
                                    engine,
                                    requested: cleanup,
                                    deferred: CleanupBatch::default(),
                                    completion: CleanupCompletion::Replay(completion),
                                },
                            },
                        }
                    }
                }
            }
            (
                PendingEffect::Replay {
                    mut engine,
                    completion,
                    ..
                },
                EffectOutcome::ReplaySuppressedTargetChanged,
            ) => {
                let ActivationState::Candidate(mut candidate) = engine.state else {
                    return finish_terminal_completion(
                        engine,
                        completion,
                        CancelReason::EffectProtocolViolation,
                    );
                };
                candidate
                    .journal
                    .commit()
                    .expect("candidate journal is open until replay disposition");
                engine.state = ActivationState::DrainOnly(DrainOnly {
                    owned_letters: candidate.owned_letters & engine.physical_letters,
                });
                engine.admission_open = false;
                engine.terminal = true;
                finish_terminal_completion(engine, completion, CancelReason::TargetChanged)
            }
            (
                PendingEffect::Cleanup {
                    mut engine,
                    requested,
                    deferred,
                    completion,
                },
                EffectOutcome::CleanupSubmitted { submitted },
            ) => {
                let valid = submitted <= requested.len();
                if valid {
                    engine.apply_cleanup_prefix(requested, submitted);
                    let unaccepted = requested
                        .suffix(submitted)
                        .expect("accepted count was checked");
                    let remaining = deferred.followed_by(unaccepted);
                    engine.pending_injected_cleanup = (!remaining.is_empty()).then_some(remaining);
                }
                let pending = engine.pending_injected_cleanup.is_some();
                match completion {
                    CleanupCompletion::Replay(replay) => {
                        finish_terminal_completion(engine, replay, CancelReason::ReplayFailed)
                    }
                    CleanupCompletion::ReconciledReplay(replay) if valid && !pending => {
                        finish_replay_completion(engine, replay)
                    }
                    CleanupCompletion::ReconciledReplay(replay) => {
                        finish_terminal_completion(engine, replay, CancelReason::ReplayFailed)
                    }
                    CleanupCompletion::RetryControl => engine.complete_control(
                        valid && !pending,
                        pending.then_some(CancelReason::ReplayFailed),
                    ),
                }
            }
            (pending, _) => pending.protocol_violation(),
        }
    }
}

impl PendingEffect {
    pub(super) fn protocol_violation(self) -> Turn {
        match self {
            Self::Neutralize {
                mut engine, event, ..
            } => {
                engine.metrics.record_dummy(usize::MAX);
                engine.admission_open = false;
                engine.terminal = true;
                engine.request_candidate_cancellation(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::EffectProtocolViolation,
                    true,
                    false,
                )
            }
            Self::ActivationDownOrComplete {
                mut engine, event, ..
            }
            | Self::MenuCleanup {
                mut engine,
                event: Some(event),
            } => {
                engine.admission_open = false;
                engine.terminal = true;
                engine.request_candidate_cancellation(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::EffectProtocolViolation,
                    true,
                    false,
                )
            }
            Self::ActivationUp { engine, event, .. } => engine.terminal_event(
                event,
                EventDisposition::CaptureCurrent,
                CancelReason::EffectProtocolViolation,
            ),
            Self::ObservedActivationUp { engine } => engine
                .discard_candidate_control_into_terminal_drain(
                    CancelReason::EffectProtocolViolation,
                ),
            Self::Replay {
                mut engine,
                completion,
                batch,
            } => {
                engine.metrics.record_replay(usize::MAX, batch.len());
                engine.admission_open = false;
                engine.terminal = true;
                engine.move_owned_to_drain();
                finish_terminal_completion(
                    engine,
                    completion,
                    CancelReason::EffectProtocolViolation,
                )
            }
            Self::Cleanup {
                mut engine,
                completion,
                ..
            } => match completion {
                CleanupCompletion::Replay(replay) | CleanupCompletion::ReconciledReplay(replay) => {
                    finish_terminal_completion(
                        engine,
                        replay,
                        CancelReason::EffectProtocolViolation,
                    )
                }
                CleanupCompletion::RetryControl => {
                    engine.admission_open = false;
                    engine.terminal = true;
                    engine.complete_control(false, Some(CancelReason::EffectProtocolViolation))
                }
            },
            Self::MenuCleanup {
                mut engine,
                event: None,
            } => {
                engine.admission_open = false;
                engine.terminal = true;
                engine.complete_control(false, Some(CancelReason::EffectProtocolViolation))
            }
        }
    }
}

const fn reconciliation_snapshot(completion: ReplayCompletion) -> Option<PhysicalSnapshot> {
    match completion {
        ReplayCompletion::Event {
            event,
            reason: CancelReason::PhysicalStateMismatch,
            ..
        } => Some(event.snapshot),
        ReplayCompletion::Control(ControlAfterReplay::Reconcile(snapshot)) => Some(snapshot),
        _ => None,
    }
}

pub(super) fn finish_replay_completion(
    mut engine: TransactionEngine,
    completion: ReplayCompletion,
) -> Turn {
    let cancellation = replay_completion_reason(completion);
    engine.metrics.record_cancellation(cancellation);
    match completion {
        ReplayCompletion::Event {
            event,
            disposition,
            reason,
            close_admission,
            revision_fence,
            ..
        } => {
            if close_admission {
                engine.admission_open = false;
                engine.terminal = true;
            }
            if reason == CancelReason::PhysicalStateMismatch {
                engine.apply_reconciliation(event.snapshot, true);
            } else if revision_fence {
                engine.fence_current_physical(true);
            }
            engine.complete_event(event, disposition, Some(reason))
        }
        ReplayCompletion::Control(action) => match action {
            ControlAfterReplay::Install(config) => {
                engine.install_config(config);
                engine.complete_control(true, Some(CancelReason::ConfigurationReplaced))
            }
            ControlAfterReplay::Cancel(reason) => engine.complete_control(true, Some(reason)),
            ControlAfterReplay::Reconcile(snapshot) => {
                engine.apply_reconciliation(snapshot, false);
                engine.complete_control(true, Some(CancelReason::PhysicalStateMismatch))
            }
        },
    }
}

const fn replay_completion_reason(completion: ReplayCompletion) -> CancelReason {
    match completion {
        ReplayCompletion::Event { reason, .. } => reason,
        ReplayCompletion::Control(ControlAfterReplay::Install(_)) => {
            CancelReason::ConfigurationReplaced
        }
        ReplayCompletion::Control(ControlAfterReplay::Cancel(reason)) => reason,
        ReplayCompletion::Control(ControlAfterReplay::Reconcile(_)) => {
            CancelReason::PhysicalStateMismatch
        }
    }
}

pub(super) fn finish_terminal_completion(
    mut engine: TransactionEngine,
    completion: ReplayCompletion,
    reason: CancelReason,
) -> Turn {
    engine.metrics.record_cancellation(reason);
    engine.admission_open = false;
    engine.terminal = true;
    match completion {
        ReplayCompletion::Event {
            event,
            partial_disposition,
            ..
        } => engine.complete_event(event, partial_disposition, Some(reason)),
        ReplayCompletion::Control(_) => engine.complete_control(false, Some(reason)),
    }
}

pub(super) fn modifier_release_completes_pending_exact(
    event: NormalizedEvent,
    pending: Option<PendingExact>,
    expected: ModifierMask,
    current: ModifierMask,
) -> bool {
    pending.is_some()
        && event.phase == PhysicalPhase::Up
        && matches!(event.key, KeyIdentity::Modifier(_))
        && current != expected
        // A release may remove required modifiers, but an added modifier is an
        // unambiguous cancellation rather than a shortcut completion.
        && (!current.ctrl() || expected.ctrl())
        && (!current.alt() || expected.alt())
        && (!current.shift() || expected.shift())
        && (!current.meta() || expected.meta())
}

pub(super) fn pending_exact(
    result: MatchClass,
    trigger: ActivationKey,
    started_at_ms: u64,
) -> Option<PendingExact> {
    let MatchClass::ExactWithLonger { binding, .. } = result else {
        return None;
    };
    Some(PendingExact {
        binding,
        trigger,
        started_at_ms,
    })
}

pub(super) const fn letter_bit(key: ActivationKey) -> u32 {
    1_u32 << key.index()
}

pub(super) const fn replay_record(event: NormalizedEvent) -> ReplayRecord {
    ReplayRecord {
        key: event.key,
        native: event.native,
        phase: event.phase,
        observed_at_ms: event.observed_at_ms,
    }
}
