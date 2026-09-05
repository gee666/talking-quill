use std::fmt;

use super::{
    CleanupBatch, CompiledActivationConfig, ConfigRevision, EngineInput, EventJournal, GateState,
    InputSource, JournalError, KeyIdentity, MatchClass, MatchCursor, ModifierSides,
    NormalizedEvent, PhysicalPhase, PhysicalSnapshot, ReplayBatch, ReplayRecord,
};
use crate::{ActivationBinding, ActivationKey, ModifierMask};

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

    const fn closes_admission(self) -> bool {
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

    const fn is_nonterminal_control_cancellation(self) -> bool {
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
    const fn any(self) -> bool {
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
    pending: PendingEffect,
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
enum PendingEffect {
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
enum CleanupCompletion {
    Replay(ReplayCompletion),
    ReconciledReplay(ReplayCompletion),
    RetryControl,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplayCompletion {
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
enum ControlAfterReplay {
    Install(CompiledActivationConfig),
    Cancel(CancelReason),
    Reconcile(PhysicalSnapshot),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[doc(hidden)]
pub struct TransactionMetrics {
    pub started: u64,
    pub committed: u64,
    pub replayed: u64,
    pub cancelled: u64,
    pub cancellation_reasons: [u64; CancelReason::COUNT],
    pub journal_high_water: u64,
    pub replay_attempted: u64,
    pub replay_succeeded: u64,
    pub replay_partial: u64,
    pub replay_failed: u64,
    pub dummy_attempted: u64,
    pub dummy_succeeded: u64,
    pub dummy_partial: u64,
    pub dummy_failed: u64,
}

impl TransactionMetrics {
    fn increment(value: &mut u64) {
        *value = value.saturating_add(1);
    }

    fn observe_journal(&mut self, len: usize) {
        self.journal_high_water = self.journal_high_water.max(len as u64);
    }

    fn record_cancellation(&mut self, reason: CancelReason) {
        Self::increment(&mut self.cancelled);
        Self::increment(&mut self.cancellation_reasons[reason.index()]);
    }

    fn record_replay(&mut self, submitted: usize, requested: usize) {
        Self::increment(&mut self.replay_attempted);
        if submitted == requested {
            Self::increment(&mut self.replay_succeeded);
        } else if submitted == 0 || submitted > requested {
            Self::increment(&mut self.replay_failed);
        } else {
            Self::increment(&mut self.replay_partial);
        }
    }

    fn record_dummy(&mut self, accepted: usize) {
        Self::increment(&mut self.dummy_attempted);
        match accepted {
            2 => Self::increment(&mut self.dummy_succeeded),
            1 => Self::increment(&mut self.dummy_partial),
            _ => Self::increment(&mut self.dummy_failed),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingExact {
    binding: ActivationBinding,
    trigger: ActivationKey,
    started_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Candidate {
    revision: ConfigRevision,
    expected_modifiers: ModifierMask,
    cursor: MatchCursor,
    pending_exact: Option<PendingExact>,
    journal: EventJournal,
    owned_letters: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Committed {
    binding: ActivationBinding,
    trigger: ActivationKey,
    trigger_down_at_ms: u64,
    owned_letters: u32,
    activation_up_pending: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DrainOnly {
    owned_letters: u32,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ActivationState {
    #[default]
    Idle,
    Candidate(Candidate),
    Committed(Committed),
    DrainOnly(DrainOnly),
}

/// Pure transactional reducer. It owns no locks, queues, protocol encoders, or
/// native injector. Every state and effect payload has a compile-time bound.
#[derive(Clone, Eq, PartialEq)]
pub struct TransactionEngine {
    config: CompiledActivationConfig,
    state: ActivationState,
    admission_open: bool,
    shutting_down: bool,
    terminal: bool,
    physical_letters: u32,
    foreground_letters: u32,
    physical_modifiers: ModifierSides,
    foreground_modifiers: ModifierSides,
    fenced_letters: u32,
    fenced_modifiers: ModifierSides,
    alt_gr_active: bool,
    alt_cycle_neutralized: bool,
    meta_cycle_neutralized: bool,
    pending_injected_cleanup: Option<CleanupBatch>,
    pending_menu_cleanup: Option<MenuModifiers>,
    menu_neutralization_policy: MenuNeutralizationPolicy,
    metrics: TransactionMetrics,
}

impl fmt::Debug for TransactionEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransactionEngine(<redacted>)")
    }
}

impl TransactionEngine {
    #[must_use]
    pub fn new(config: CompiledActivationConfig) -> Self {
        Self {
            config,
            state: ActivationState::Idle,
            admission_open: true,
            shutting_down: false,
            terminal: false,
            physical_letters: 0,
            foreground_letters: 0,
            physical_modifiers: ModifierSides::default(),
            foreground_modifiers: ModifierSides::default(),
            fenced_letters: 0,
            fenced_modifiers: ModifierSides::default(),
            alt_gr_active: false,
            alt_cycle_neutralized: false,
            meta_cycle_neutralized: false,
            pending_injected_cleanup: None,
            pending_menu_cleanup: None,
            menu_neutralization_policy: MenuNeutralizationPolicy::Required,
            metrics: TransactionMetrics::default(),
        }
    }

    /// Selects a truthful native neutralization policy before the engine is
    /// exposed to callback input. This is immutable for the engine lifetime.
    #[must_use]
    pub fn with_menu_neutralization_policy(mut self, policy: MenuNeutralizationPolicy) -> Self {
        self.menu_neutralization_policy = policy;
        self
    }

    /// Seeds a newly installed owner from exact native state. Preheld input is
    /// foreground-visible but fenced until every seeded key and modifier side
    /// is released.
    #[must_use]
    pub fn with_physical_snapshot(
        config: CompiledActivationConfig,
        snapshot: PhysicalSnapshot,
    ) -> Self {
        let mut engine = Self::new(config);
        engine.apply_reconciliation(snapshot, false);
        engine
    }

    #[must_use]
    pub fn begin(self, input: EngineInput) -> Turn {
        match input {
            EngineInput::Event(event) => self.begin_event(event),
            EngineInput::Control(control) => self.begin_control(control),
        }
    }

    #[must_use]
    pub const fn config(&self) -> CompiledActivationConfig {
        self.config
    }

    #[must_use]
    pub const fn admission_open(&self) -> bool {
        self.admission_open
    }

    #[must_use]
    pub const fn physical_letters(&self) -> u32 {
        self.physical_letters
    }

    #[must_use]
    pub const fn foreground_letters(&self) -> u32 {
        self.foreground_letters
    }

    #[must_use]
    pub const fn physical_modifiers(&self) -> ModifierSides {
        self.physical_modifiers
    }

    #[must_use]
    pub const fn foreground_modifiers(&self) -> ModifierSides {
        self.foreground_modifiers
    }

    #[must_use]
    pub const fn fenced_letters(&self) -> u32 {
        self.fenced_letters
    }

    #[must_use]
    pub const fn fenced_modifiers(&self) -> ModifierSides {
        self.fenced_modifiers
    }

    #[must_use]
    pub const fn owned_letters(&self) -> u32 {
        match self.state {
            ActivationState::Idle => 0,
            ActivationState::Candidate(candidate) => candidate.owned_letters,
            ActivationState::Committed(committed) => committed.owned_letters,
            ActivationState::DrainOnly(drain) => drain.owned_letters,
        }
    }

    /// Nonmutating candidate-start hint for native target capture. Evidence is
    /// frozen on the first captured key, never at a later completion edge.
    #[must_use]
    pub fn event_starts_candidate(&self, key: KeyIdentity, phase: PhysicalPhase) -> bool {
        let KeyIdentity::Letter(letter) = key else {
            return false;
        };
        matches!(self.state, ActivationState::Idle)
            && phase == PhysicalPhase::Down
            && self.physical_letters == 0
            && self.fenced_letters == 0
            && self.fenced_modifiers.bits() == 0
            && !self.alt_gr_active
            && self.admission_open
            && !self.shutting_down
            && !self.terminal
            && self.config.enabled()
            && self
                .config
                .matcher()
                .start(self.physical_modifiers.combined(), letter)
                .cursor()
                .is_some()
    }

    /// Observes a physical edge that native target preflight refused to own.
    /// It remains foreground-visible and fences that physical generation until
    /// release so it cannot later be reinterpreted as a shortcut start.
    #[must_use]
    pub fn pass_uncapturable_event(mut self, event: NormalizedEvent) -> Turn {
        let snapshot_consistent = self.observe_physical(event);
        if !snapshot_consistent {
            return self.handle_physical_state_mismatch(event);
        }
        self.satisfy_pending_cleanup_with_unowned_physical_up(event);
        self.fence_current_physical(false);
        self.complete_event(event, EventDisposition::PassCurrent, None)
    }

    /// Nonmutating activation-boundary hint used by the macOS target cache.
    #[must_use]
    pub fn event_may_request_activation(&self, key: KeyIdentity, phase: PhysicalPhase) -> bool {
        let KeyIdentity::Letter(letter) = key else {
            return false;
        };
        match self.state {
            ActivationState::Idle if phase == PhysicalPhase::Down => matches!(
                self.config
                    .matcher()
                    .start(self.physical_modifiers.combined(), letter),
                MatchClass::Exact { .. }
            ),
            ActivationState::Candidate(candidate) if phase == PhysicalPhase::Down => matches!(
                self.config.matcher().advance(candidate.cursor, letter),
                MatchClass::Exact { .. }
            ),
            ActivationState::Candidate(candidate) if phase == PhysicalPhase::Up => candidate
                .pending_exact
                .is_some_and(|pending| pending.trigger == letter),
            _ => false,
        }
    }

    #[must_use]
    #[doc(hidden)]
    pub const fn metrics(&self) -> TransactionMetrics {
        self.metrics
    }

    #[must_use]
    pub const fn journal_len(&self) -> usize {
        match self.state {
            ActivationState::Candidate(candidate) => candidate.journal.len(),
            _ => 0,
        }
    }

    #[must_use]
    pub const fn pending_injected_cleanup(&self) -> Option<CleanupBatch> {
        self.pending_injected_cleanup
    }

    #[must_use]
    pub const fn pending_menu_cleanup(&self) -> Option<MenuModifiers> {
        self.pending_menu_cleanup
    }

    /// Retires exact native ownership generations proved released or exposed
    /// by a platform handoff without changing the current physical snapshot.
    /// A newer physical generation of the same key may remain held and is
    /// reconciled separately as foreground/fenced state.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[doc(hidden)]
    pub fn retire_released_owned_letters(&mut self, released: u32) {
        match self.state {
            ActivationState::Committed(mut committed) => {
                committed.owned_letters &= !released;
                self.state = if committed.owned_letters == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::Committed(committed)
                };
            }
            ActivationState::DrainOnly(mut drain) => {
                drain.owned_letters &= !released;
                self.state = if drain.owned_letters == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(drain)
                };
            }
            ActivationState::Candidate(mut candidate) => {
                candidate.owned_letters &= !released;
                self.state = ActivationState::Candidate(candidate);
            }
            ActivationState::Idle => {}
        }
    }

    #[must_use]
    pub const fn shutdown_state(&self) -> ShutdownState {
        if self.terminal {
            return ShutdownState::Terminal;
        }
        if !self.shutting_down {
            return ShutdownState::Running;
        }
        let owned_letters = self.owned_letters();
        if owned_letters == 0 {
            ShutdownState::Quiescent
        } else {
            ShutdownState::Draining { owned_letters }
        }
    }
}

impl Default for TransactionEngine {
    fn default() -> Self {
        Self::new(CompiledActivationConfig::default())
    }
}

mod continuation;
mod dispatch;
mod effects;
mod matching;
mod reconciliation;
use continuation::*;
