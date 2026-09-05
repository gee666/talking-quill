//! Native adapter requests, outcomes, and cancellation policy.
use super::{CleanupBatch, CompiledActivationConfig, PhysicalSnapshot, ReplayBatch};
use crate::ActivationBinding;
use std::fmt;

/// Hard upper bound for neutralize -> delivery -> replay -> cleanup.
pub const MAX_EFFECTS_PER_TURN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventDisposition {
    PassCurrent,
    CaptureCurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelReason {
    InvalidContinuation,
    ModifierChanged,
    AltGr,
    JournalOverflow,
    ConfigurationReplaced,
    RevisionMismatch,
    GateClosed,
    Shutdown,
    HelperDisconnected,
    SecureDesktop,
    Timeout,
    ActivationDeliveryFailed,
    NeutralizationFailed,
    ReplayFailed,
    EffectProtocolViolation,
    PhysicalStateMismatch,
    TargetChanged,
}

impl CancelReason {
    #[doc(hidden)]
    pub const COUNT: usize = 17;

    #[doc(hidden)]
    pub const fn index(self) -> usize {
        self as usize
    }

    pub(super) const fn closes_admission(self) -> bool {
        matches!(
            self,
            Self::GateClosed
                | Self::HelperDisconnected
                | Self::SecureDesktop
                | Self::ActivationDeliveryFailed
                | Self::NeutralizationFailed
                | Self::ReplayFailed
                | Self::EffectProtocolViolation
                | Self::PhysicalStateMismatch
        )
    }

    pub(super) const fn is_nonterminal_control_cancellation(self) -> bool {
        matches!(
            self,
            Self::InvalidContinuation
                | Self::ModifierChanged
                | Self::AltGr
                | Self::Timeout
                | Self::TargetChanged
        )
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ActivationNotice {
    Down {
        binding: ActivationBinding,
    },
    Up {
        binding: ActivationBinding,
        held_ms: u64,
    },
    Complete {
        binding: ActivationBinding,
        held_ms: u64,
    },
}

impl fmt::Debug for ActivationNotice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Down { .. } => "ActivationNotice::Down(<redacted>)",
            Self::Up { .. } => "ActivationNotice::Up(<redacted>)",
            Self::Complete { .. } => "ActivationNotice::Complete(<redacted>)",
        })
    }
}

impl ActivationNotice {
    #[must_use]
    pub const fn binding(self) -> ActivationBinding {
        match self {
            Self::Down { binding } | Self::Up { binding, .. } | Self::Complete { binding, .. } => {
                binding
            }
        }
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct MenuModifiers {
    pub alt: bool,
    pub meta: bool,
}

/// Native policy for menu-modifier neutralization. Windows requires the
/// PowerToys-style dummy pair; macOS has no such native effect and must never
/// fabricate an accepted count for work it did not perform.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuNeutralizationPolicy {
    #[default]
    Required,
    NotRequired,
}

impl fmt::Debug for MenuModifiers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MenuModifiers(<redacted>)")
    }
}

impl MenuModifiers {
    pub(super) const fn any(self) -> bool {
        self.alt || self.meta
    }
}

// Replay and cleanup remain fixed-size `Copy` callback payloads; boxing would
// allocate in the native path this core is designed to keep allocation-free.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum EffectRequest {
    /// One tagged dummy down/up pair (two native records).
    NeutralizeMenu(MenuModifiers),
    /// Release a dummy down accepted from a partial neutralization pair.
    CleanupMenuNeutralization(MenuModifiers),
    DeliverActivation(ActivationNotice),
    Replay(ReplayBatch),
    CleanupInjected(CleanupBatch),
}

impl fmt::Debug for EffectRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NeutralizeMenu(_) => "EffectRequest::NeutralizeMenu(<redacted>)",
            Self::CleanupMenuNeutralization(_) => {
                "EffectRequest::CleanupMenuNeutralization(<redacted>)"
            }
            Self::DeliverActivation(_) => "EffectRequest::DeliverActivation(<redacted>)",
            Self::Replay(_) => "EffectRequest::Replay(<redacted>)",
            Self::CleanupInjected(_) => "EffectRequest::CleanupInjected(<redacted>)",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectOutcome {
    Neutralized {
        accepted: usize,
    },
    MenuCleanupAccepted {
        accepted: usize,
    },
    ActivationDelivered(bool),
    /// Number synchronously submitted to the native pipeline. Adapters that
    /// can observe tagged callbacks must separately verify those observations.
    ReplaySubmitted {
        submitted: usize,
    },
    CleanupSubmitted {
        submitted: usize,
    },
    /// Synchronous accepted-count compatibility used by Windows SendInput.
    ReplayAccepted {
        accepted: usize,
    },
    CleanupAccepted {
        accepted: usize,
    },
    /// The native adapter proved the candidate-start insertion target changed.
    /// No journal record was synthesized; retained physical downs move to the
    /// ordinary owned-up drain.
    ReplaySuppressedTargetChanged,
}

// Deliberately fixed-size: boxing a compiled config would allocate on the
// owner/callback handoff this core is designed to keep bounded.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Control {
    ReplaceConfig(CompiledActivationConfig),
    /// Close fresh matcher admission without disposing an unresolved candidate.
    /// A later `CancelCandidate` performs the ordered replay/cancellation step.
    CloseAdmission(CancelReason),
    CancelCandidate(CancelReason),
    Cancel(CancelReason),
    /// Rebase after startup, secure-desktop, or callback-gap recovery.
    Reconcile(PhysicalSnapshot),
    /// Apply exact callback edges observed while a native effect ran outside
    /// the low-level hook callback. Unlike gap recovery, these edges preserve
    /// admission because the adapter accounted for their foreground disposition.
    ReconcileObserved {
        snapshot: PhysicalSnapshot,
        observed_at_ms: u64,
    },
    /// Retry retained helper-injected releases after a degraded partial effect.
    RetryCleanup,
    Shutdown,
}

impl fmt::Debug for Control {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReplaceConfig(_) => "Control::ReplaceConfig(<redacted>)",
            Self::CloseAdmission(_) => "Control::CloseAdmission",
            Self::CancelCandidate(_) => "Control::CancelCandidate",
            Self::Cancel(_) => "Control::Cancel",
            Self::Reconcile(_) => "Control::Reconcile(<redacted>)",
            Self::ReconcileObserved { .. } => "Control::ReconcileObserved(<redacted>)",
            Self::RetryCleanup => "Control::RetryCleanup",
            Self::Shutdown => "Control::Shutdown",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventOutcome {
    pub disposition: EventDisposition,
    pub cancellation: Option<CancelReason>,
    pub terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlOutcome {
    pub applied: bool,
    pub cancellation: Option<CancelReason>,
    pub shutdown: ShutdownState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Event(EventOutcome),
    Control(ControlOutcome),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ShutdownState {
    Running,
    Quiescent,
    Draining { owned_letters: u32 },
    Terminal,
}

impl fmt::Debug for ShutdownState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Running => "ShutdownState::Running",
            Self::Quiescent => "ShutdownState::Quiescent",
            Self::Draining { .. } => "ShutdownState::Draining(<redacted>)",
            Self::Terminal => "ShutdownState::Terminal",
        })
    }
}
