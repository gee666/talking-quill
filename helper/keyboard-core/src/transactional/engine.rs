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
    ReconcileObserved(PhysicalSnapshot),
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
            Self::ReconcileObserved(_) => "Control::ReconcileObserved(<redacted>)",
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

    fn begin_event(mut self, event: NormalizedEvent) -> Turn {
        if matches!(
            event.source,
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
        ) {
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        }
        let prior_letters = self.physical_letters;
        let snapshot_consistent = self.observe_physical(event);
        if !snapshot_consistent {
            return self.handle_physical_state_mismatch(event);
        }
        self.satisfy_pending_cleanup_with_unowned_physical_up(event);

        if event.gate == GateState::Closed {
            return self.handle_closed_gate_event(event);
        }

        if event.config_revision != self.config.revision() {
            return self.handle_revision_mismatch(event);
        }

        match self.state {
            ActivationState::Idle => self.handle_idle(event, prior_letters),
            ActivationState::Candidate(candidate) => self.handle_candidate(event, candidate),
            ActivationState::Committed(committed) => self.handle_committed(event, committed),
            ActivationState::DrainOnly(drain) => self.handle_drain(event, drain),
        }
    }

    fn begin_control(mut self, control: Control) -> Turn {
        match control {
            Control::ReplaceConfig(config) => {
                if !config.revision().is_newer_than(self.config.revision()) {
                    return self.complete_control(false, Some(CancelReason::RevisionMismatch));
                }
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::Candidate(candidate);
                        self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Install(
                            config,
                        )))
                    }
                    ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                        self.install_config(config);
                        self.complete_control(true, Some(CancelReason::ConfigurationReplaced))
                    }
                    ActivationState::Idle => {
                        self.install_config(config);
                        self.complete_control(true, Some(CancelReason::ConfigurationReplaced))
                    }
                }
            }
            Control::CloseAdmission(reason) => {
                self.admission_open = false;
                // Closure is only the fresh-admission barrier. Candidate
                // cancellation/replay is deliberately a later ordered action.
                self.complete_control(true, Some(reason))
            }
            Control::CancelCandidate(reason) => match self.state {
                ActivationState::Candidate(candidate) => {
                    self.state = ActivationState::Candidate(candidate);
                    self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Cancel(
                        reason,
                    )))
                }
                ActivationState::Idle => self.complete_control(true, Some(reason)),
                ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                    self.complete_control(false, Some(reason))
                }
            },
            Control::Cancel(CancelReason::Shutdown) => self.begin_control(Control::Shutdown),
            Control::Cancel(reason) if reason.closes_admission() => {
                // Legacy atomic terminal cancellation remains available for
                // unrecoverable core faults. W1 owner loss ordering uses the
                // explicit CloseAdmission -> CancelCandidate pair instead.
                self.admission_open = false;
                self.terminal = true;
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: candidate.owned_letters,
                        });
                        self.metrics.record_cancellation(reason);
                    }
                    ActivationState::Committed(committed) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: committed.owned_letters,
                        });
                    }
                    ActivationState::Idle | ActivationState::DrainOnly(_) => {}
                }
                self.complete_control(true, Some(reason))
            }
            Control::Cancel(reason) if reason.is_nonterminal_control_cancellation() => {
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::Candidate(candidate);
                        self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Cancel(
                            reason,
                        )))
                    }
                    ActivationState::Idle => self.complete_control(true, Some(reason)),
                    ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                        self.complete_control(false, Some(reason))
                    }
                }
            }
            Control::Cancel(reason) => self.complete_control(false, Some(reason)),
            Control::Reconcile(snapshot) => match self.state {
                ActivationState::Candidate(candidate) => {
                    self.physical_letters = snapshot.held_letters;
                    self.physical_modifiers = snapshot.modifiers;
                    self.alt_gr_active = snapshot.alt_gr_active;
                    self.fenced_letters &= snapshot.held_letters;
                    self.fenced_modifiers = ModifierSides::from_bits(
                        self.fenced_modifiers.bits() & snapshot.modifiers.bits(),
                    );
                    self.state = ActivationState::Candidate(candidate);
                    self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Reconcile(
                        snapshot,
                    )))
                }
                ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                    self.apply_reconciliation(snapshot, true);
                    self.complete_control(true, Some(CancelReason::PhysicalStateMismatch))
                }
                ActivationState::Idle => {
                    self.apply_reconciliation(snapshot, false);
                    self.complete_control(true, Some(CancelReason::PhysicalStateMismatch))
                }
            },
            Control::ReconcileObserved(snapshot) => {
                self.apply_observed_reconciliation(snapshot);
                self.complete_control(true, None)
            }
            Control::RetryCleanup => self.request_retry_cleanup(),
            Control::Shutdown => {
                self.admission_open = false;
                self.shutting_down = true;
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        // The candidate journal contains physical edges hidden
                        // from the foreground. Shutdown must not replay a held
                        // down into a possibly changed target. Retain ownership
                        // and drain only on authoritative physical releases.
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: candidate.owned_letters,
                        });
                        self.metrics.record_cancellation(CancelReason::Shutdown);
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                    ActivationState::Committed(committed) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: committed.owned_letters,
                        });
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                    ActivationState::Idle | ActivationState::DrainOnly(_) => {
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                }
            }
        }
    }

    fn handle_closed_gate_event(mut self, event: NormalizedEvent) -> Turn {
        self.admission_open = false;
        self.terminal = true;
        match self.state {
            ActivationState::Candidate(_) => self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::GateClosed,
                true,
                false,
            ),
            ActivationState::Committed(committed) => {
                self.state = ActivationState::DrainOnly(DrainOnly {
                    owned_letters: committed.owned_letters,
                });
                let drain = match self.state {
                    ActivationState::DrainOnly(drain) => drain,
                    _ => unreachable!(),
                };
                self.handle_drain(event, drain)
            }
            ActivationState::Idle | ActivationState::DrainOnly(_) => {
                let state = self.state;
                match state {
                    ActivationState::DrainOnly(drain) => self.handle_drain(event, drain),
                    _ => self.complete_event(
                        event,
                        EventDisposition::PassCurrent,
                        Some(CancelReason::GateClosed),
                    ),
                }
            }
        }
    }

    fn handle_revision_mismatch(mut self, event: NormalizedEvent) -> Turn {
        self.fence_current_physical(true);
        if matches!(self.state, ActivationState::Candidate(_)) {
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::RevisionMismatch,
                false,
                true,
            );
        }
        match self.state {
            ActivationState::Committed(committed) => self.handle_committed(event, committed),
            ActivationState::DrainOnly(drain) => self.handle_drain(event, drain),
            ActivationState::Idle | ActivationState::Candidate(_) => self.complete_event(
                event,
                EventDisposition::PassCurrent,
                Some(CancelReason::RevisionMismatch),
            ),
        }
    }

    fn handle_idle(mut self, event: NormalizedEvent, prior_physical_letters: u32) -> Turn {
        let KeyIdentity::Letter(key) = event.key else {
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        };
        if event.phase != PhysicalPhase::Down
            || prior_physical_letters != 0
            || self.fenced_letters != 0
            || self.fenced_modifiers.bits() != 0
            || self.alt_gr_active
            || !self.admission_open
            || self.shutting_down
            || self.terminal
            || !self.config.enabled()
        {
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        }

        let result = self
            .config
            .matcher()
            .start(self.physical_modifiers.combined(), key);
        let Some(cursor) = result.cursor() else {
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        };
        let mut journal = EventJournal::new();
        journal
            .push(replay_record(event))
            .expect("one record fits the journal");
        TransactionMetrics::increment(&mut self.metrics.started);
        self.metrics.observe_journal(journal.len());
        let pending_exact = pending_exact(result, key, event.observed_at_ms);
        let candidate = Candidate {
            revision: self.config.revision(),
            expected_modifiers: self.physical_modifiers.combined(),
            cursor,
            pending_exact,
            journal,
            owned_letters: letter_bit(key),
        };
        self.state = ActivationState::Candidate(candidate);
        match result {
            MatchClass::Exact { binding, .. } => {
                self.request_activation(event, ActivationNotice::Down { binding })
            }
            MatchClass::Prefix(_) | MatchClass::ExactWithLonger { .. } => {
                self.complete_event(event, EventDisposition::CaptureCurrent, None)
            }
            MatchClass::NoCandidate => unreachable!(),
        }
    }

    fn handle_candidate(mut self, event: NormalizedEvent, mut candidate: Candidate) -> Turn {
        debug_assert_eq!(candidate.revision, self.config.revision());
        // Keep one fixed journal slot for the edge that actually terminates the
        // candidate. Repeats, modifier changes, key ups, unrelated keys and
        // adapter-generated mouse cancellation must never fill the journal and
        // then suppress an original that has no replay replacement.
        if event.source.is_physical() && candidate.journal.must_replay_before_next_capture() {
            self.state = ActivationState::Candidate(candidate);
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::JournalOverflow,
                false,
                false,
            );
        }
        if self.alt_gr_active {
            self.state = ActivationState::Candidate(candidate);
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::AltGr,
                false,
                false,
            );
        }
        if self.physical_modifiers.combined() != candidate.expected_modifiers {
            if modifier_release_completes_pending_exact(
                event,
                candidate.pending_exact,
                candidate.expected_modifiers,
                self.physical_modifiers.combined(),
            ) {
                let pending = candidate
                    .pending_exact
                    .expect("a completing modifier release has a pending exact binding");
                self.state = ActivationState::Candidate(candidate);
                return self.request_activation(
                    event,
                    ActivationNotice::Complete {
                        binding: pending.binding,
                        held_ms: event.observed_at_ms.saturating_sub(pending.started_at_ms),
                    },
                );
            }
            self.state = ActivationState::Candidate(candidate);
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::ModifierChanged,
                false,
                false,
            );
        }

        let KeyIdentity::Letter(key) = event.key else {
            self.state = ActivationState::Candidate(candidate);
            if matches!(event.key, KeyIdentity::Modifier(_)) {
                return self.complete_event(event, EventDisposition::PassCurrent, None);
            }
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::InvalidContinuation,
                false,
                false,
            );
        };
        let bit = letter_bit(key);

        match event.phase {
            PhysicalPhase::Repeat => {
                if candidate.owned_letters & bit == 0 {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::InvalidContinuation,
                        false,
                        false,
                    );
                }
                if candidate.journal.push(replay_record(event)).is_err() {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::JournalOverflow,
                        false,
                        false,
                    );
                }
                self.metrics.observe_journal(candidate.journal.len());
                self.state = ActivationState::Candidate(candidate);
                self.complete_event(event, EventDisposition::CaptureCurrent, None)
            }
            PhysicalPhase::Down => {
                if candidate.owned_letters & bit != 0 {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::InvalidContinuation,
                        false,
                        false,
                    );
                }
                if candidate.journal.push(replay_record(event)).is_err() {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::JournalOverflow,
                        false,
                        false,
                    );
                }
                self.metrics.observe_journal(candidate.journal.len());
                candidate.owned_letters |= bit;
                let result = self.config.matcher().advance(candidate.cursor, key);
                let Some(cursor) = result.cursor() else {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::CaptureCurrent,
                        CancelReason::InvalidContinuation,
                        false,
                        false,
                    );
                };
                candidate.cursor = cursor;
                candidate.pending_exact = pending_exact(result, key, event.observed_at_ms);
                self.state = ActivationState::Candidate(candidate);
                match result {
                    MatchClass::Exact { binding, .. } => {
                        self.request_activation(event, ActivationNotice::Down { binding })
                    }
                    MatchClass::Prefix(_) | MatchClass::ExactWithLonger { .. } => {
                        self.complete_event(event, EventDisposition::CaptureCurrent, None)
                    }
                    MatchClass::NoCandidate => unreachable!(),
                }
            }
            PhysicalPhase::Up => {
                if candidate.owned_letters & bit == 0 {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::InvalidContinuation,
                        false,
                        false,
                    );
                }
                if candidate.journal.push(replay_record(event)).is_err() {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_candidate_cancellation(
                        event,
                        EventDisposition::PassCurrent,
                        CancelReason::JournalOverflow,
                        false,
                        false,
                    );
                }
                self.metrics.observe_journal(candidate.journal.len());
                candidate.owned_letters &= !bit;
                if let Some(pending) = candidate.pending_exact
                    && pending.trigger == key
                {
                    self.state = ActivationState::Candidate(candidate);
                    return self.request_activation(
                        event,
                        ActivationNotice::Complete {
                            binding: pending.binding,
                            held_ms: event.observed_at_ms.saturating_sub(pending.started_at_ms),
                        },
                    );
                }
                self.state = ActivationState::Candidate(candidate);
                self.request_candidate_cancellation(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::InvalidContinuation,
                    false,
                    false,
                )
            }
        }
    }

    fn handle_committed(mut self, event: NormalizedEvent, mut committed: Committed) -> Turn {
        let KeyIdentity::Letter(key) = event.key else {
            self.state = ActivationState::Committed(committed);
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        };
        let bit = letter_bit(key);
        if committed.owned_letters & bit == 0 {
            self.state = ActivationState::Committed(committed);
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        }

        match event.phase {
            PhysicalPhase::Down | PhysicalPhase::Repeat => {
                self.state = ActivationState::Committed(committed);
                self.complete_event(event, EventDisposition::CaptureCurrent, None)
            }
            PhysicalPhase::Up => {
                committed.owned_letters &= !bit;
                if key == committed.trigger && committed.activation_up_pending {
                    committed.activation_up_pending = false;
                    let notice = ActivationNotice::Up {
                        binding: committed.binding,
                        held_ms: event
                            .observed_at_ms
                            .saturating_sub(committed.trigger_down_at_ms),
                    };
                    self.state = ActivationState::Committed(committed);
                    return self.request_activation_up(event, notice);
                }
                self.state = if committed.owned_letters == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::Committed(committed)
                };
                self.complete_event(event, EventDisposition::CaptureCurrent, None)
            }
        }
    }

    fn handle_drain(mut self, event: NormalizedEvent, mut drain: DrainOnly) -> Turn {
        if let KeyIdentity::Letter(key) = event.key {
            let bit = letter_bit(key);
            if drain.owned_letters & bit != 0 {
                if event.phase == PhysicalPhase::Up {
                    drain.owned_letters &= !bit;
                }
                self.state = if drain.owned_letters == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(drain)
                };
                return self.complete_event(event, EventDisposition::CaptureCurrent, None);
            }
        }
        self.state = ActivationState::DrainOnly(drain);
        self.complete_event(event, EventDisposition::PassCurrent, None)
    }

    fn request_activation(self, event: NormalizedEvent, notice: ActivationNotice) -> Turn {
        // A user commonly releases Alt just before the pending prefix key. The
        // low-level callback has already applied that release to physical state,
        // but the accepted shortcut still owns the Alt menu cycle recorded at
        // candidate admission.
        let activation_modifiers = match self.state {
            ActivationState::Candidate(candidate) => candidate.expected_modifiers,
            _ => self.physical_modifiers.combined(),
        };
        let menu = MenuModifiers {
            alt: activation_modifiers.alt() && !self.alt_cycle_neutralized,
            meta: activation_modifiers.meta() && !self.meta_cycle_neutralized,
        };
        if self.menu_neutralization_policy == MenuNeutralizationPolicy::Required && menu.any() {
            Turn::NeedEffect {
                effect: EffectRequest::NeutralizeMenu(menu),
                continuation: Continuation {
                    pending: PendingEffect::Neutralize {
                        engine: self,
                        event,
                        notice,
                        menu,
                    },
                },
            }
        } else {
            self.request_activation_delivery(event, notice)
        }
    }

    fn request_activation_delivery(self, event: NormalizedEvent, notice: ActivationNotice) -> Turn {
        Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation: Continuation {
                pending: PendingEffect::ActivationDownOrComplete {
                    engine: self,
                    event,
                    notice,
                },
            },
        }
    }

    fn request_activation_up(self, event: NormalizedEvent, notice: ActivationNotice) -> Turn {
        Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation: Continuation {
                pending: PendingEffect::ActivationUp {
                    engine: self,
                    event,
                    notice,
                },
            },
        }
    }

    fn request_candidate_cancellation(
        mut self,
        event: NormalizedEvent,
        mut disposition: EventDisposition,
        reason: CancelReason,
        close_admission: bool,
        revision_fence: bool,
    ) -> Turn {
        if close_admission {
            return self.discard_candidate_into_terminal_drain(event, disposition, reason);
        }
        let original_disposition = disposition;
        let mut current_appended_to_replay = false;
        // A platform post performed while the current native callback is still
        // in flight is not ordered before returning that original event. Put a
        // physical terminating edge at the end of the same tagged replay batch
        // and suppress its original instead. This gives every adapter one
        // explicit replay-before-current sequence rather than relying on native
        // queue timing. Events already captured by their phase handler are not
        // appended a second time.
        if disposition == EventDisposition::PassCurrent
            && event.source.is_physical()
            && let ActivationState::Candidate(mut candidate) = self.state
        {
            if candidate.journal.push(replay_record(event)).is_ok() {
                self.metrics.observe_journal(candidate.journal.len());
                self.state = ActivationState::Candidate(candidate);
                disposition = EventDisposition::CaptureCurrent;
                current_appended_to_replay = true;
            } else {
                // The reserved-slot invariant makes this unreachable for a
                // physical candidate. Preserve the original rather than ever
                // suppressing an edge that has no tagged replacement.
                debug_assert!(false, "candidate replay current slot was not reserved");
                self.state = ActivationState::Candidate(candidate);
                disposition = original_disposition;
            }
        }
        let owned_current = match (self.state, event.key) {
            (ActivationState::Candidate(candidate), KeyIdentity::Letter(key)) => {
                candidate.owned_letters & letter_bit(key) != 0
            }
            _ => false,
        };
        let disposition = if reason == CancelReason::PhysicalStateMismatch
            && owned_current
            && matches!(event.key, KeyIdentity::Letter(key)
                if event.snapshot.held_letters & letter_bit(key) == 0)
        {
            EventDisposition::CaptureCurrent
        } else {
            disposition
        };
        let partial_disposition = if current_appended_to_replay {
            // The appended replacement was not submitted, so the untouched
            // original must pass. Full submission alone authorizes capture.
            original_disposition
        } else if owned_current && event.source.is_physical() {
            EventDisposition::CaptureCurrent
        } else {
            disposition
        };
        self.request_replay(ReplayCompletion::Event {
            event,
            disposition,
            partial_disposition,
            reason,
            close_admission,
            revision_fence,
        })
    }

    fn discard_candidate_into_terminal_drain(
        mut self,
        event: NormalizedEvent,
        fallback_disposition: EventDisposition,
        reason: CancelReason,
    ) -> Turn {
        let ActivationState::Candidate(candidate) = self.state else {
            return self.terminal_event(event, fallback_disposition, reason);
        };
        let owned_current = matches!(event.key, KeyIdentity::Letter(key)
            if candidate.owned_letters & letter_bit(key) != 0);
        let mut owned_letters = candidate.owned_letters & self.physical_letters;
        if owned_current
            && event.phase == PhysicalPhase::Up
            && let KeyIdentity::Letter(key) = event.key
        {
            owned_letters &= !letter_bit(key);
        }
        self.state = if owned_letters == 0 {
            ActivationState::Idle
        } else {
            ActivationState::DrainOnly(DrainOnly { owned_letters })
        };
        self.admission_open = false;
        self.terminal = true;
        self.metrics.record_cancellation(reason);
        self.complete_event(
            event,
            if owned_current && event.source.is_physical() {
                EventDisposition::CaptureCurrent
            } else {
                fallback_disposition
            },
            Some(reason),
        )
    }

    fn close_candidate_event_for_later_cancellation(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        reason: CancelReason,
    ) -> Turn {
        self.admission_open = false;
        self.complete_event(event, disposition, Some(reason))
    }

    fn close_candidate_control_for_later_cancellation(mut self, reason: CancelReason) -> Turn {
        self.admission_open = false;
        self.complete_control(false, Some(reason))
    }

    fn discard_candidate_control_into_terminal_drain(mut self, reason: CancelReason) -> Turn {
        let owned_letters = match self.state {
            ActivationState::Candidate(candidate) => candidate.owned_letters,
            _ => self.owned_letters(),
        };
        self.state = if owned_letters == 0 {
            ActivationState::Idle
        } else {
            ActivationState::DrainOnly(DrainOnly { owned_letters })
        };
        self.admission_open = false;
        self.terminal = true;
        self.metrics.record_cancellation(reason);
        self.complete_control(false, Some(reason))
    }

    fn request_replay(self, completion: ReplayCompletion) -> Turn {
        let ActivationState::Candidate(candidate) = self.state else {
            return finish_terminal_completion(
                self,
                completion,
                CancelReason::EffectProtocolViolation,
            );
        };
        let batch = candidate
            .journal
            .replay_batch()
            .expect("an active candidate has an open journal");
        Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            continuation: Continuation {
                pending: PendingEffect::Replay {
                    engine: self,
                    batch,
                    completion,
                },
            },
        }
    }

    fn finish_successful_activation(
        mut self,
        event: NormalizedEvent,
        notice: ActivationNotice,
    ) -> Turn {
        let ActivationState::Candidate(mut candidate) = self.state else {
            return self.terminal_event(
                event,
                EventDisposition::CaptureCurrent,
                CancelReason::EffectProtocolViolation,
            );
        };
        candidate
            .journal
            .commit()
            .expect("activation candidate journal is open");
        self.metrics.observe_journal(candidate.journal.len());
        TransactionMetrics::increment(&mut self.metrics.committed);
        let (binding, trigger, trigger_down_at_ms, activation_up_pending) = match notice {
            ActivationNotice::Down { binding } => (
                binding,
                binding.shortcut().trigger(),
                event.observed_at_ms,
                true,
            ),
            ActivationNotice::Complete { binding, held_ms } => (
                binding,
                binding.shortcut().trigger(),
                event.observed_at_ms.saturating_sub(held_ms),
                false,
            ),
            ActivationNotice::Up { .. } => {
                return self.terminal_event(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::EffectProtocolViolation,
                );
            }
        };
        let committed = Committed {
            binding,
            trigger,
            trigger_down_at_ms,
            owned_letters: candidate.owned_letters,
            activation_up_pending,
        };
        self.state = if committed.owned_letters == 0 {
            ActivationState::Idle
        } else {
            ActivationState::Committed(committed)
        };
        let disposition = if matches!(notice, ActivationNotice::Complete { .. })
            && matches!(event.key, KeyIdentity::Modifier(_))
        {
            // Modifier down was never owned. Pass its physical up after the
            // dummy menu neutralization while retaining every candidate letter
            // until its matching physical release.
            EventDisposition::PassCurrent
        } else {
            EventDisposition::CaptureCurrent
        };
        self.complete_event(event, disposition, None)
    }

    fn finish_activation_up(mut self, event: NormalizedEvent, delivered: bool) -> Turn {
        let ActivationState::Committed(committed) = self.state else {
            return self.terminal_event(
                event,
                EventDisposition::CaptureCurrent,
                CancelReason::EffectProtocolViolation,
            );
        };
        if delivered {
            self.state = if committed.owned_letters == 0 {
                ActivationState::Idle
            } else {
                ActivationState::Committed(committed)
            };
            self.complete_event(event, EventDisposition::CaptureCurrent, None)
        } else {
            self.admission_open = false;
            self.terminal = true;
            self.state = ActivationState::DrainOnly(DrainOnly {
                owned_letters: committed.owned_letters,
            });
            self.complete_event(
                event,
                EventDisposition::CaptureCurrent,
                Some(CancelReason::ActivationDeliveryFailed),
            )
        }
    }

    fn complete_event(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        cancellation: Option<CancelReason>,
    ) -> Turn {
        if disposition == EventDisposition::PassCurrent && event.source.is_physical() {
            self.apply_foreground_event(event);
        }
        Turn::Complete {
            completion: Completion::Event(EventOutcome {
                disposition,
                cancellation,
                terminal: self.terminal,
            }),
            engine: self,
        }
    }

    fn complete_control(self, applied: bool, cancellation: Option<CancelReason>) -> Turn {
        let shutdown = self.shutdown_state();
        Turn::Complete {
            engine: self,
            completion: Completion::Control(ControlOutcome {
                applied,
                cancellation,
                shutdown,
            }),
        }
    }

    fn terminal_event(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        reason: CancelReason,
    ) -> Turn {
        self.admission_open = false;
        self.terminal = true;
        self.move_owned_to_drain();
        self.complete_event(event, disposition, Some(reason))
    }

    fn move_owned_to_drain(&mut self) {
        let owned_letters = self.owned_letters() & self.physical_letters;
        self.state = ActivationState::DrainOnly(DrainOnly { owned_letters });
    }

    /// Applies the required exact post-event snapshot and reports whether it
    /// agrees with the edge predicted from the prior reducer state.
    fn observe_physical(&mut self, event: NormalizedEvent) -> bool {
        debug_assert!(event.source.is_physical());
        if !event.snapshot.is_valid() {
            self.physical_letters = event.snapshot.held_letters;
            self.physical_modifiers = event.snapshot.modifiers;
            self.alt_gr_active = event.snapshot.alt_gr_active;
            return false;
        }
        let mut predicted = PhysicalSnapshot::new(
            self.physical_letters,
            self.physical_modifiers,
            event.snapshot.alt_gr_active,
        );
        let phase_valid = match event.key {
            KeyIdentity::Letter(key) => {
                let was_held = self.physical_letters & letter_bit(key) != 0;
                match event.phase {
                    PhysicalPhase::Down => predicted.held_letters |= letter_bit(key),
                    PhysicalPhase::Repeat => predicted.held_letters |= letter_bit(key),
                    PhysicalPhase::Up => predicted.held_letters &= !letter_bit(key),
                }
                matches!(event.phase, PhysicalPhase::Down) != was_held
            }
            KeyIdentity::Modifier(side) => {
                let was_held = self.physical_modifiers.contains(side);
                match event.phase {
                    PhysicalPhase::Down => predicted.modifiers.insert(side),
                    PhysicalPhase::Repeat => predicted.modifiers.insert(side),
                    PhysicalPhase::Up => predicted.modifiers.remove(side),
                }
                matches!(event.phase, PhysicalPhase::Down) != was_held
            }
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => true,
        };
        let consistent = phase_valid && predicted == event.snapshot;
        self.physical_letters = event.snapshot.held_letters;
        self.physical_modifiers = event.snapshot.modifiers;
        self.alt_gr_active = event.snapshot.alt_gr_active;
        self.fenced_letters &= self.physical_letters;
        self.fenced_modifiers =
            ModifierSides::from_bits(self.fenced_modifiers.bits() & self.physical_modifiers.bits());
        if !self.physical_modifiers.any_alt() {
            self.alt_cycle_neutralized = false;
        }
        if !self.physical_modifiers.any_meta() {
            self.meta_cycle_neutralized = false;
        }
        consistent
    }

    fn handle_physical_state_mismatch(mut self, event: NormalizedEvent) -> Turn {
        self.admission_open = false;
        self.terminal = true;
        self.fence_current_physical(true);
        match self.state {
            ActivationState::Candidate(_) => self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::PhysicalStateMismatch,
                true,
                true,
            ),
            ActivationState::Committed(committed) => {
                let current_was_owned = matches!(event.key, KeyIdentity::Letter(key)
                    if committed.owned_letters & letter_bit(key) != 0);
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    if current_was_owned {
                        EventDisposition::CaptureCurrent
                    } else {
                        EventDisposition::PassCurrent
                    },
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
            ActivationState::DrainOnly(drain) => {
                let current_was_owned = matches!(event.key, KeyIdentity::Letter(key)
                    if drain.owned_letters & letter_bit(key) != 0);
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    if current_was_owned {
                        EventDisposition::CaptureCurrent
                    } else {
                        EventDisposition::PassCurrent
                    },
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
            ActivationState::Idle => {
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    EventDisposition::PassCurrent,
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
        }
    }

    fn apply_observed_reconciliation(&mut self, snapshot: PhysicalSnapshot) {
        let owned = self.owned_letters() & snapshot.held_letters;
        self.physical_letters = snapshot.held_letters;
        self.physical_modifiers = snapshot.modifiers;
        self.alt_gr_active = snapshot.alt_gr_active;
        self.foreground_letters = snapshot.held_letters & !owned;
        self.foreground_modifiers = snapshot.modifiers;
        self.fenced_letters &= snapshot.held_letters;
        self.fenced_modifiers =
            ModifierSides::from_bits(self.fenced_modifiers.bits() & snapshot.modifiers.bits());
        if !snapshot.modifiers.any_alt() {
            self.alt_cycle_neutralized = false;
        }
        if !snapshot.modifiers.any_meta() {
            self.meta_cycle_neutralized = false;
        }
        self.state = match self.state {
            ActivationState::Committed(mut committed) => {
                committed.owned_letters = owned;
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::Committed(committed)
                }
            }
            ActivationState::DrainOnly(_) => {
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(DrainOnly {
                        owned_letters: owned,
                    })
                }
            }
            ActivationState::Idle => ActivationState::Idle,
            // A candidate reaches this path only when activation could not
            // commit after the deferred native effect. Its journal lacks any
            // callbacks that raced that effect, so replay is no longer a
            // balanced representation. Discard it and retain only physically
            // held ownership for exact-up draining.
            ActivationState::Candidate(_) => {
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(DrainOnly {
                        owned_letters: owned,
                    })
                }
            }
        };
    }

    fn apply_reconciliation(&mut self, snapshot: PhysicalSnapshot, close_admission: bool) {
        let owned = self.owned_letters() & snapshot.held_letters;
        self.physical_letters = snapshot.held_letters;
        self.physical_modifiers = snapshot.modifiers;
        self.alt_gr_active = snapshot.alt_gr_active;
        self.foreground_letters = snapshot.held_letters & !owned;
        self.foreground_modifiers = snapshot.modifiers;
        self.fenced_letters = snapshot.held_letters;
        self.fenced_modifiers = snapshot.modifiers;
        self.alt_cycle_neutralized = false;
        self.meta_cycle_neutralized = false;
        if close_admission {
            self.admission_open = false;
            self.terminal = true;
            self.state = ActivationState::DrainOnly(DrainOnly {
                owned_letters: owned,
            });
        } else {
            self.state = ActivationState::Idle;
        }
    }

    fn satisfy_pending_cleanup_with_unowned_physical_up(&mut self, event: NormalizedEvent) {
        let (KeyIdentity::Letter(key), PhysicalPhase::Up, Some(cleanup)) =
            (event.key, event.phase, self.pending_injected_cleanup)
        else {
            return;
        };
        if self.owned_letters() & letter_bit(key) != 0
            || cleanup.letter_bits() & letter_bit(key) == 0
        {
            return;
        }
        let remaining = cleanup.without_letter(key);
        self.pending_injected_cleanup = (!remaining.is_empty()).then_some(remaining);
    }

    fn request_retry_cleanup(self) -> Turn {
        if let Some(menu) = self.pending_menu_cleanup {
            return Turn::NeedEffect {
                effect: EffectRequest::CleanupMenuNeutralization(menu),
                continuation: Continuation {
                    pending: PendingEffect::MenuCleanup {
                        engine: self,
                        event: None,
                    },
                },
            };
        }
        if let Some(cleanup) = self.pending_injected_cleanup {
            let newly_held = self.physical_letters & !self.owned_letters();
            let (ready, blocked) = cleanup.partition_blocked(newly_held);
            if ready.is_empty() {
                return self.complete_control(false, Some(CancelReason::ReplayFailed));
            }
            return Turn::NeedEffect {
                effect: EffectRequest::CleanupInjected(ready),
                continuation: Continuation {
                    pending: PendingEffect::Cleanup {
                        engine: self,
                        requested: ready,
                        deferred: blocked,
                        completion: CleanupCompletion::RetryControl,
                    },
                },
            };
        }
        self.complete_control(true, None)
    }

    fn apply_foreground_event(&mut self, event: NormalizedEvent) {
        match event.key {
            KeyIdentity::Letter(key) => match event.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_letters |= letter_bit(key);
                }
                PhysicalPhase::Up => self.foreground_letters &= !letter_bit(key),
            },
            KeyIdentity::Modifier(side) => match event.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_modifiers.insert(side);
                }
                PhysicalPhase::Up => self.foreground_modifiers.remove(side),
            },
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
        }
    }

    fn apply_replay(&mut self, batch: ReplayBatch) {
        self.apply_replay_prefix(batch, batch.len());
    }

    fn apply_replay_prefix(&mut self, batch: ReplayBatch, accepted: usize) {
        for record in batch.entries()[..accepted].iter().copied() {
            self.apply_injected_record(record);
        }
    }

    fn apply_cleanup_prefix(&mut self, cleanup: CleanupBatch, accepted: usize) {
        for record in cleanup.entries()[..accepted].iter().copied() {
            self.apply_injected_record(record);
        }
    }

    fn mark_cleanup_pending_visible(&mut self, cleanup: CleanupBatch) {
        for record in cleanup.entries().iter().copied() {
            if let KeyIdentity::Letter(key) = record.key {
                self.foreground_letters |= letter_bit(key);
            }
        }
    }

    fn apply_injected_record(&mut self, record: ReplayRecord) {
        match record.key {
            KeyIdentity::Letter(key) => match record.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_letters |= letter_bit(key);
                }
                PhysicalPhase::Up => self.foreground_letters &= !letter_bit(key),
            },
            KeyIdentity::Modifier(side) => match record.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_modifiers.insert(side);
                }
                PhysicalPhase::Up => self.foreground_modifiers.remove(side),
            },
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
        }
    }

    fn install_config(&mut self, config: CompiledActivationConfig) {
        self.config = config;
        self.fence_current_physical(true);
    }

    fn fence_current_physical(&mut self, include_modifiers: bool) {
        self.fenced_letters |= self.physical_letters;
        if include_modifiers {
            self.fenced_modifiers = ModifierSides::from_bits(
                self.fenced_modifiers.bits() | self.physical_modifiers.bits(),
            );
        }
    }
}

impl Default for TransactionEngine {
    fn default() -> Self {
        Self::new(CompiledActivationConfig::default())
    }
}

impl Continuation {
    fn close_before_submission(self, reason: CancelReason) -> Turn {
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
    fn protocol_violation(self) -> Turn {
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

fn finish_replay_completion(mut engine: TransactionEngine, completion: ReplayCompletion) -> Turn {
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

fn finish_terminal_completion(
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

fn modifier_release_completes_pending_exact(
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

fn pending_exact(
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

const fn letter_bit(key: ActivationKey) -> u32 {
    1_u32 << key.index()
}

const fn replay_record(event: NormalizedEvent) -> ReplayRecord {
    ReplayRecord {
        key: event.key,
        native: event.native,
        phase: event.phase,
        observed_at_ms: event.observed_at_ms,
    }
}
