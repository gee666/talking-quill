use std::fmt;

use super::{
    CleanupBatch, CompiledActivationConfig, ConfigRevision, EngineInput, EventJournal, GateState,
    InputSource, JournalError, KeyIdentity, MatchClass, MatchCursor, ModifierSides,
    NormalizedEvent, PhysicalPhase, PhysicalSnapshot, ReplayBatch, ReplayRecord,
};
use crate::{ActivationBinding, ActivationKey, ModifierMask};

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
use completion::*;
mod completion;

mod contracts;
mod metrics;
mod turn;
pub use contracts::*;
pub use metrics::TransactionMetrics;
use turn::{CleanupCompletion, ControlAfterReplay, PendingEffect, ReplayCompletion};
pub use turn::{Continuation, Turn};

mod event;
use event::*;

mod cancellation;
