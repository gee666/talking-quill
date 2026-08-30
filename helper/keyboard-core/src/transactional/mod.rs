//! Allocation-free transactional activation core.
//!
//! This module is intentionally platform neutral. Native callbacks normalize
//! input, execute requested effects, and feed the bounded outcome back through
//! [`Continuation::resume`]. Escape/Enter session capture is not part of this
//! state machine.

mod engine;
mod journal;
mod matcher;

use std::fmt;

pub use engine::{
    ActivationNotice, CancelReason, Completion, Continuation, Control, ControlOutcome,
    EffectOutcome, EffectRequest, EventDisposition, EventOutcome, MAX_EFFECTS_PER_TURN,
    MenuModifiers, MenuNeutralizationPolicy, ShutdownState, TransactionEngine, TransactionMetrics,
    Turn,
};
pub use journal::{
    CleanupBatch, EventJournal, JOURNAL_CAPACITY, JournalDisposition, JournalError, NativeKey,
    ReplayBatch, ReplayRecord,
};
pub use matcher::{
    CompileError, CompiledActivationConfig, CompiledMatcher, ConfigIdentity, ConfigRevision,
    MatchClass, MatchCursor,
};

use super::{ACTIVATION_KEY_CAPACITY, ActivationKey, ModifierMask};

/// Normalized physical phase. Repeats are not fresh matcher steps.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum PhysicalPhase {
    Down,
    Repeat,
    Up,
}

/// Side-specific modifier identity retained even though the v27 binding model
/// stores an aggregate four-modifier mask.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum ModifierSide {
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    LeftShift,
    RightShift,
    LeftMeta,
    RightMeta,
}

impl ModifierSide {
    #[must_use]
    pub const fn bit(self) -> u8 {
        1_u8 << self as u8
    }
}

/// Compact side-specific physical or foreground modifier state.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct ModifierSides(u8);

impl ModifierSides {
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, side: ModifierSide) -> bool {
        self.0 & side.bit() != 0
    }

    pub fn insert(&mut self, side: ModifierSide) {
        self.0 |= side.bit();
    }

    pub fn remove(&mut self, side: ModifierSide) {
        self.0 &= !side.bit();
    }

    #[must_use]
    pub const fn combined(self) -> ModifierMask {
        ModifierMask::new(
            self.0 & (ModifierSide::LeftCtrl.bit() | ModifierSide::RightCtrl.bit()) != 0,
            self.0 & (ModifierSide::LeftAlt.bit() | ModifierSide::RightAlt.bit()) != 0,
            self.0 & (ModifierSide::LeftShift.bit() | ModifierSide::RightShift.bit()) != 0,
            self.0 & (ModifierSide::LeftMeta.bit() | ModifierSide::RightMeta.bit()) != 0,
        )
    }

    #[must_use]
    pub const fn any_alt(self) -> bool {
        self.0 & (ModifierSide::LeftAlt.bit() | ModifierSide::RightAlt.bit()) != 0
    }

    #[must_use]
    pub const fn any_meta(self) -> bool {
        self.0 & (ModifierSide::LeftMeta.bit() | ModifierSide::RightMeta.bit()) != 0
    }
}

/// Key classes relevant to activation ordering. Escape and Enter deliberately
/// remain opaque to this core so the independent session reducer can process a
/// passed event after activation cancellation has completed.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum KeyIdentity {
    Letter(ActivationKey),
    Modifier(ModifierSide),
    Escape,
    Enter,
    Other(u16),
}

/// Explicit source classification. Only Talking Quill's privately tagged
/// replay/paste/dummy traffic bypasses the core. Untagged external automation
/// is intentionally input-equivalent to a physical keypress.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct InputSource(InputSourceKind);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum InputSourceKind {
    Physical,
    HelperReplay,
    HelperPaste,
    HelperDummy,
    External,
    #[cfg(feature = "native-test-input")]
    TestPhysical,
}

macro_rules! redacted_debug {
    ($type:ty, $name:literal) => {
        impl fmt::Debug for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!($name, "(<redacted>)"))
            }
        }
    };
}

redacted_debug!(PhysicalPhase, "PhysicalPhase");
redacted_debug!(ModifierSide, "ModifierSide");
redacted_debug!(ModifierSides, "ModifierSides");
redacted_debug!(KeyIdentity, "KeyIdentity");
redacted_debug!(InputSource, "InputSource");

impl InputSource {
    #[allow(non_upper_case_globals)]
    pub const Physical: Self = Self(InputSourceKind::Physical);
    #[allow(non_upper_case_globals)]
    pub const HelperReplay: Self = Self(InputSourceKind::HelperReplay);
    #[allow(non_upper_case_globals)]
    pub const HelperPaste: Self = Self(InputSourceKind::HelperPaste);
    #[allow(non_upper_case_globals)]
    pub const HelperDummy: Self = Self(InputSourceKind::HelperDummy);
    #[allow(non_upper_case_globals)]
    pub const External: Self = Self(InputSourceKind::External);

    /// Test seams receive physical authority only when the explicit core test
    /// feature is selected. Optimized builds reject that feature at compile time;
    /// feature-free builds classify this request as external.
    #[must_use]
    pub const fn test_physical() -> Self {
        #[cfg(feature = "native-test-input")]
        {
            Self(InputSourceKind::TestPhysical)
        }
        #[cfg(not(feature = "native-test-input"))]
        {
            Self::External
        }
    }

    #[must_use]
    pub const fn is_physical(self) -> bool {
        match self.0 {
            InputSourceKind::Physical | InputSourceKind::External => true,
            #[cfg(feature = "native-test-input")]
            InputSourceKind::TestPhysical => true,
            InputSourceKind::HelperReplay
            | InputSourceKind::HelperPaste
            | InputSourceKind::HelperDummy => false,
        }
    }

    #[must_use]
    pub const fn is_test_physical(self) -> bool {
        #[cfg(feature = "native-test-input")]
        {
            matches!(self.0, InputSourceKind::TestPhysical)
        }
        #[cfg(not(feature = "native-test-input"))]
        {
            false
        }
    }
}

/// State of the terminal callback/admission gate observed with an event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GateState {
    Open,
    Closed,
}

/// Exact side-specific physical snapshot after the normalized event. Native
/// adapters must obtain this from their owner-thread tracker or OS recovery
/// snapshot; the reducer never infers startup state from a partial edge stream.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct PhysicalSnapshot {
    pub held_letters: u32,
    pub modifiers: ModifierSides,
    pub alt_gr_active: bool,
}

impl fmt::Debug for PhysicalSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PhysicalSnapshot(<redacted>)")
    }
}

impl PhysicalSnapshot {
    const VALID_LETTER_MASK: u32 = (1_u32 << ACTIVATION_KEY_CAPACITY) - 1;

    /// Retains the exact native observation, including impossible high bits.
    /// The engine rejects such a snapshot fail-closed instead of silently
    /// masking evidence that the adapter/model boundary disagreed.
    #[must_use]
    pub const fn new(held_letters: u32, modifiers: ModifierSides, alt_gr_active: bool) -> Self {
        Self {
            held_letters,
            modifiers,
            alt_gr_active,
        }
    }

    #[must_use]
    pub const fn checked(
        held_letters: u32,
        modifiers: ModifierSides,
        alt_gr_active: bool,
    ) -> Option<Self> {
        if held_letters & !Self::VALID_LETTER_MASK == 0 {
            Some(Self::new(held_letters, modifiers, alt_gr_active))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.held_letters & !Self::VALID_LETTER_MASK == 0
    }
}

/// One normalized callback event. `phase` has a distinct repeat value,
/// `snapshot` is the exact physical state after the edge, and `native`
/// contains the lossless platform data required by a replay adapter.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct NormalizedEvent {
    pub key: KeyIdentity,
    pub phase: PhysicalPhase,
    pub source: InputSource,
    pub native: NativeKey,
    pub observed_at_ms: u64,
    pub config_revision: ConfigRevision,
    pub gate: GateState,
    pub snapshot: PhysicalSnapshot,
}

impl fmt::Debug for NormalizedEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NormalizedEvent(<redacted>)")
    }
}

impl NormalizedEvent {
    #[must_use]
    pub const fn physical(
        key: KeyIdentity,
        phase: PhysicalPhase,
        native: NativeKey,
        observed_at_ms: u64,
        config_revision: ConfigRevision,
        gate: GateState,
        snapshot: PhysicalSnapshot,
    ) -> Self {
        Self {
            key,
            phase,
            source: InputSource::Physical,
            native,
            observed_at_ms,
            config_revision,
            gate,
            snapshot,
        }
    }

    #[must_use]
    pub const fn with_source(mut self, source: InputSource) -> Self {
        self.source = source;
        self
    }

    #[must_use]
    pub const fn with_gate(mut self, gate: GateState) -> Self {
        self.gate = gate;
        self
    }

    #[must_use]
    pub const fn with_alt_gr(mut self, active: bool) -> Self {
        self.snapshot.alt_gr_active = active;
        self
    }
}

/// Inputs are owner-linearized. Controls therefore cannot race a pending
/// effect continuation with another physical event.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineInput {
    Event(NormalizedEvent),
    Control(Control),
}
