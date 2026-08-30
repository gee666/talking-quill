use std::{cmp::Ordering, fmt, num::NonZeroU64};

use thiserror::Error;

use super::JOURNAL_CAPACITY;
use crate::{
    ACTIVATION_KEY_CAPACITY, ActivationBinding, ActivationBindings, ActivationKey, ModifierMask,
    ProfileId, Shortcut,
};

const KEY_COUNT: usize = ACTIVATION_KEY_CAPACITY;
const MODIFIER_MASK_COUNT: usize = 16;
const LENGTH_COUNT: usize = Shortcut::MAX_KEYS + 1;

/// Owner-linearized configuration identity.
///
/// The legacy `new(revision)` constructor uses capture scope one so existing
/// platform adapters remain source compatible. Owner transport integration
/// must use [`ConfigRevision::scoped`] so a new capture epoch may restart at
/// revision one without colliding with a prior epoch. Revision zero is the
/// explicit unconfigured identity and is never a wire revision.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct ConfigRevision {
    capture_scope: Option<NonZeroU64>,
    revision: u64,
}

/// Descriptive alias used by owner state and new integrations.
pub type ConfigIdentity = ConfigRevision;

impl ConfigRevision {
    #[must_use]
    pub const fn new(revision: u64) -> Self {
        Self {
            capture_scope: if revision == 0 {
                None
            } else {
                Some(NonZeroU64::MIN)
            },
            revision,
        }
    }

    #[must_use]
    pub const fn scoped(capture_scope: u64, revision: u64) -> Option<Self> {
        match (NonZeroU64::new(capture_scope), NonZeroU64::new(revision)) {
            (Some(capture_scope), Some(revision)) => Some(Self {
                capture_scope: Some(capture_scope),
                revision: revision.get(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn capture_scope(self) -> Option<u64> {
        match self.capture_scope {
            Some(scope) => Some(scope.get()),
            None => None,
        }
    }

    #[must_use]
    pub const fn is_configured(self) -> bool {
        self.capture_scope.is_some()
    }

    /// Returns whether this identity can replace `current` without allowing an
    /// old capture epoch or a non-increasing same-epoch revision.
    #[must_use]
    pub const fn is_newer_than(self, current: Self) -> bool {
        match (self.capture_scope, current.capture_scope) {
            (Some(next_scope), Some(current_scope)) => {
                next_scope.get() > current_scope.get()
                    || (next_scope.get() == current_scope.get() && self.revision > current.revision)
            }
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Revision exhaustion is rejected rather than wrapping and reinterpreting
    /// physically held input as a new configuration.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match (self.capture_scope, self.revision.checked_add(1)) {
            (Some(capture_scope), Some(revision)) => Some(Self {
                capture_scope: Some(capture_scope),
                revision,
            }),
            (None, Some(1)) => Some(Self::new(1)),
            _ => None,
        }
    }
}

impl fmt::Debug for ConfigRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfigIdentity(<redacted>)")
    }
}

impl Ord for ConfigRevision {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.capture_scope, self.revision).cmp(&(other.capture_scope, other.revision))
    }
}

impl PartialOrd for ConfigRevision {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum CompileError {
    #[error("shortcut replay bound exceeds the fixed journal capacity")]
    JournalCapacityExceeded,
}

/// A compact cursor into a compiled prefix family. One bit represents one of
/// the at most thirteen validated bindings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MatchCursor {
    candidates: u16,
    depth: u8,
}

impl MatchCursor {
    #[must_use]
    pub const fn candidate_bits(self) -> u16 {
        self.candidates
    }

    #[must_use]
    pub const fn depth(self) -> u8 {
        self.depth
    }
}

/// Classification after one fresh ordered key-down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchClass {
    NoCandidate,
    Prefix(MatchCursor),
    Exact {
        cursor: MatchCursor,
        binding: ActivationBinding,
    },
    ExactWithLonger {
        cursor: MatchCursor,
        binding: ActivationBinding,
    },
}

impl MatchClass {
    #[must_use]
    pub const fn cursor(self) -> Option<MatchCursor> {
        match self {
            Self::NoCandidate => None,
            Self::Prefix(cursor)
            | Self::Exact { cursor, .. }
            | Self::ExactWithLonger { cursor, .. } => Some(cursor),
        }
    }
}

/// Allocation-free compiled prefix matcher. Compilation is intended for the
/// native owner/configuration path, never a low-level callback.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CompiledMatcher {
    bindings: [ActivationBinding; ActivationBindings::MAX],
    binding_count: u8,
    modifier_sets: [u16; MODIFIER_MASK_COUNT],
    position_sets: [[u16; KEY_COUNT]; Shortcut::MAX_KEYS],
    length_sets: [u16; LENGTH_COUNT],
}

impl CompiledMatcher {
    pub fn compile(bindings: ActivationBindings) -> Result<Self, CompileError> {
        let empty = ActivationBinding::new(ProfileId::GENERAL, Shortcut::EMPTY);
        let mut compiled = Self {
            bindings: [empty; ActivationBindings::MAX],
            binding_count: bindings.len() as u8,
            modifier_sets: [0; MODIFIER_MASK_COUNT],
            position_sets: [[0; KEY_COUNT]; Shortcut::MAX_KEYS],
            length_sets: [0; LENGTH_COUNT],
        };

        for (binding_index, binding) in bindings.iter().enumerate() {
            let shortcut = binding.shortcut();
            // Two edges per grammar key plus two bounded terminating records.
            // Repeats are guarded separately by callback-time cancellation.
            if shortcut.keys().len().saturating_mul(2).saturating_add(2) > JOURNAL_CAPACITY {
                return Err(CompileError::JournalCapacityExceeded);
            }
            let bit = 1_u16 << binding_index;
            compiled.bindings[binding_index] = binding;
            compiled.modifier_sets[modifier_index(shortcut.modifier_mask())] |= bit;
            compiled.length_sets[shortcut.keys().len()] |= bit;
            for (position, key) in shortcut.keys().iter().copied().enumerate() {
                compiled.position_sets[position][usize::from(key.index())] |= bit;
            }
        }
        Ok(compiled)
    }

    #[must_use]
    pub fn start(&self, modifiers: ModifierMask, key: ActivationKey) -> MatchClass {
        let candidates = self.modifier_sets[modifier_index(modifiers)]
            & self.position_sets[0][usize::from(key.index())];
        self.classify(MatchCursor {
            candidates,
            depth: 1,
        })
    }

    #[must_use]
    pub fn advance(&self, cursor: MatchCursor, key: ActivationKey) -> MatchClass {
        let position = usize::from(cursor.depth);
        if position >= Shortcut::MAX_KEYS {
            return MatchClass::NoCandidate;
        }
        let candidates = cursor.candidates & self.position_sets[position][usize::from(key.index())];
        self.classify(MatchCursor {
            candidates,
            depth: cursor.depth + 1,
        })
    }

    #[must_use]
    pub const fn binding_count(&self) -> usize {
        self.binding_count as usize
    }

    fn classify(&self, cursor: MatchCursor) -> MatchClass {
        if cursor.candidates == 0 {
            return MatchClass::NoCandidate;
        }
        let exact = cursor.candidates & self.length_sets[usize::from(cursor.depth)];
        if exact == 0 {
            return MatchClass::Prefix(cursor);
        }
        debug_assert!(exact.is_power_of_two(), "validated bindings are distinct");
        let binding = self.bindings[exact.trailing_zeros() as usize];
        if cursor.candidates & !exact == 0 {
            MatchClass::Exact { cursor, binding }
        } else {
            MatchClass::ExactWithLonger { cursor, binding }
        }
    }
}

impl Default for CompiledMatcher {
    fn default() -> Self {
        Self::compile(ActivationBindings::default()).expect("empty bindings fit fixed bounds")
    }
}

/// Immutable, copyable callback snapshot compiled outside the callback.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CompiledActivationConfig {
    revision: ConfigRevision,
    enabled: bool,
    bindings: ActivationBindings,
    matcher: CompiledMatcher,
}

impl CompiledActivationConfig {
    pub fn compile(
        revision: ConfigRevision,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<Self, CompileError> {
        Ok(Self {
            revision,
            enabled,
            bindings,
            matcher: CompiledMatcher::compile(bindings)?,
        })
    }

    #[must_use]
    pub const fn revision(self) -> ConfigRevision {
        self.revision
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn bindings(self) -> ActivationBindings {
        self.bindings
    }

    #[must_use]
    pub const fn matcher(&self) -> &CompiledMatcher {
        &self.matcher
    }
}

impl fmt::Debug for CompiledMatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CompiledMatcher(<redacted>)")
    }
}

impl fmt::Debug for CompiledActivationConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CompiledActivationConfig(<redacted>)")
    }
}

impl Default for CompiledActivationConfig {
    fn default() -> Self {
        Self::compile(
            ConfigRevision::default(),
            false,
            ActivationBindings::default(),
        )
        .expect("empty configuration fits fixed bounds")
    }
}

const fn modifier_index(mask: ModifierMask) -> usize {
    (if mask.ctrl() { 1 } else { 0 })
        | (if mask.alt() { 1 << 1 } else { 0 })
        | (if mask.shift() { 1 << 2 } else { 0 })
        | (if mask.meta() { 1 << 3 } else { 0 })
}
