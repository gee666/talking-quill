//! Owned continuations retain rollback state without allocation or aliasing.
use super::*;

// Effect turns retain a full rollback state and payload by value so callback
// execution never allocates or aliases mutable reducer state.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Eq, PartialEq)]
pub enum Turn {
    Complete {
        engine: TransactionEngine,
        completion: Completion,
    },
    NeedEffect {
        effect: EffectRequest,
        continuation: Continuation,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Continuation {
    pub(super) pending: PendingEffect,
}

impl fmt::Debug for Turn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Complete { .. } => "Turn::Complete(<redacted>)",
            Self::NeedEffect { .. } => "Turn::NeedEffect(<redacted>)",
        })
    }
}

impl Turn {
    /// Disposition already owned by this event turn. Native adapters use this
    /// only for unwind recovery so a panic passes an untouched edge but never
    /// releases an edge retained or replaced by transactional replay.
    #[must_use]
    pub const fn event_disposition_hint(&self) -> Option<EventDisposition> {
        match self {
            Self::Complete {
                completion: Completion::Event(outcome),
                ..
            } => Some(outcome.disposition),
            Self::Complete {
                completion: Completion::Control(_),
                ..
            } => None,
            Self::NeedEffect { continuation, .. } => continuation.event_disposition_hint(),
        }
    }

    /// Cleanup releases are the only planned effects that remain admissible
    /// after callback admission closes. They discharge native downs that were
    /// already submitted; every other effect would create new authority.
    #[must_use]
    pub const fn effect_is_cleanup(&self) -> bool {
        matches!(
            self,
            Self::NeedEffect {
                effect: EffectRequest::CleanupMenuNeutralization(_)
                    | EffectRequest::CleanupInjected(_),
                ..
            }
        )
    }

    /// Cancels a planned but unsubmitted effect without synthesizing its
    /// failure outcome. Fresh admission closes, but replayable candidate state
    /// is retained for the required later [`Control::CancelCandidate`] step.
    #[must_use]
    pub fn close_before_unsubmitted_effect(self, reason: CancelReason) -> Self {
        match self {
            Self::Complete { .. } => self,
            Self::NeedEffect { continuation, .. } => continuation.close_before_submission(reason),
        }
    }
}

impl Continuation {
    #[must_use]
    const fn event_disposition_hint(&self) -> Option<EventDisposition> {
        match &self.pending {
            PendingEffect::Neutralize { .. }
            | PendingEffect::ActivationDownOrComplete { .. }
            | PendingEffect::ActivationUp { .. } => Some(EventDisposition::CaptureCurrent),
            PendingEffect::ObservedActivationUp { .. } => None,
            PendingEffect::MenuCleanup { event, .. } => {
                if event.is_some() {
                    Some(EventDisposition::CaptureCurrent)
                } else {
                    None
                }
            }
            PendingEffect::Replay { completion, .. } => (*completion).event_disposition_hint(),
            PendingEffect::Cleanup { completion, .. } => (*completion).event_disposition_hint(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PendingEffect {
    Neutralize {
        engine: TransactionEngine,
        event: NormalizedEvent,
        notice: ActivationNotice,
        menu: MenuModifiers,
    },
    MenuCleanup {
        engine: TransactionEngine,
        event: Option<NormalizedEvent>,
    },
    ActivationDownOrComplete {
        engine: TransactionEngine,
        event: NormalizedEvent,
        notice: ActivationNotice,
    },
    ActivationUp {
        engine: TransactionEngine,
        event: NormalizedEvent,
        notice: ActivationNotice,
    },
    ObservedActivationUp {
        engine: TransactionEngine,
    },
    Replay {
        engine: TransactionEngine,
        batch: ReplayBatch,
        completion: ReplayCompletion,
    },
    Cleanup {
        engine: TransactionEngine,
        requested: CleanupBatch,
        deferred: CleanupBatch,
        completion: CleanupCompletion,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CleanupCompletion {
    Replay(ReplayCompletion),
    ReconciledReplay(ReplayCompletion),
    RetryControl,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReplayCompletion {
    Event {
        event: NormalizedEvent,
        disposition: EventDisposition,
        partial_disposition: EventDisposition,
        reason: CancelReason,
        close_admission: bool,
        revision_fence: bool,
    },
    Control(ControlAfterReplay),
}

impl CleanupCompletion {
    const fn event_disposition_hint(self) -> Option<EventDisposition> {
        match self {
            Self::Replay(completion) | Self::ReconciledReplay(completion) => {
                completion.event_disposition_hint()
            }
            Self::RetryControl => None,
        }
    }
}

impl ReplayCompletion {
    const fn event_disposition_hint(self) -> Option<EventDisposition> {
        match self {
            Self::Event { disposition, .. } => Some(disposition),
            Self::Control(_) => None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlAfterReplay {
    Install(CompiledActivationConfig),
    Cancel(CancelReason),
    Reconcile(PhysicalSnapshot),
}
