//! Finalize replay/control effects and fail closed on malformed outcomes.
use super::*;

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

pub(super) const fn reconciliation_snapshot(
    completion: ReplayCompletion,
) -> Option<PhysicalSnapshot> {
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
