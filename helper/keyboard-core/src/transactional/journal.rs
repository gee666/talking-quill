use std::fmt;

use thiserror::Error;

use super::{KeyIdentity, PhysicalPhase};
use crate::{ACTIVATION_KEY_CAPACITY, REPLAY_CLEANUP_EDGE_CAPACITY};

/// Two edges for every A-Z grammar key plus two bounded terminating records.
/// An unbounded repeat stream is cancelled before the next append overflows.
pub const JOURNAL_CAPACITY: usize = 2 * ACTIVATION_KEY_CAPACITY + 2;

/// Lossless native reconstruction fields shared by platform adapters.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct NativeKey {
    pub virtual_key: u16,
    pub scan_code: u32,
    pub extended: bool,
    pub platform_flags: u64,
}

/// One captured original in callback order.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ReplayRecord {
    pub key: KeyIdentity,
    pub native: NativeKey,
    pub phase: PhysicalPhase,
    pub observed_at_ms: u64,
}

impl ReplayRecord {
    const EMPTY: Self = Self {
        key: KeyIdentity::Other(0),
        native: NativeKey {
            virtual_key: 0,
            scan_code: 0,
            extended: false,
            platform_flags: 0,
        },
        phase: PhysicalPhase::Up,
        observed_at_ms: 0,
    };
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JournalDisposition {
    #[default]
    Open,
    Committed,
    Replayed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum JournalError {
    #[error("captured-event journal is full")]
    Full,
    #[error("captured-event journal is already finalized")]
    Finalized,
    #[error("accepted replay count exceeds batch length")]
    InvalidAcceptedCount,
}

/// Fixed-capacity captured-event journal. No callback operation allocates.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EventJournal {
    entries: [ReplayRecord; JOURNAL_CAPACITY],
    len: u8,
    disposition: JournalDisposition,
}

impl EventJournal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: [ReplayRecord::EMPTY; JOURNAL_CAPACITY],
            len: 0,
            disposition: JournalDisposition::Open,
        }
    }

    pub fn push(&mut self, record: ReplayRecord) -> Result<(), JournalError> {
        if self.disposition != JournalDisposition::Open {
            return Err(JournalError::Finalized);
        }
        if self.len() == JOURNAL_CAPACITY {
            return Err(JournalError::Full);
        }
        self.entries[self.len()] = record;
        self.len += 1;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), JournalError> {
        self.finalize(JournalDisposition::Committed)
    }

    pub fn mark_replayed(&mut self) -> Result<(), JournalError> {
        self.finalize(JournalDisposition::Replayed)
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Candidate processing reserves the final slot for the callback edge that
    /// causes replay. This prevents a full repeat stream from forcing the
    /// current repeat/terminator to be suppressed without a replacement.
    #[must_use]
    pub const fn has_reserved_current_slot(&self) -> bool {
        self.len() < JOURNAL_CAPACITY
    }

    #[must_use]
    pub const fn must_replay_before_next_capture(&self) -> bool {
        self.len() + 1 >= JOURNAL_CAPACITY
    }

    #[must_use]
    pub const fn disposition(&self) -> JournalDisposition {
        self.disposition
    }

    #[must_use]
    pub fn entries(&self) -> &[ReplayRecord] {
        &self.entries[..self.len()]
    }

    pub fn replay_batch(&self) -> Result<ReplayBatch, JournalError> {
        if self.disposition != JournalDisposition::Open {
            return Err(JournalError::Finalized);
        }
        Ok(ReplayBatch {
            entries: self.entries,
            len: self.len,
        })
    }

    fn finalize(&mut self, disposition: JournalDisposition) -> Result<(), JournalError> {
        if self.disposition != JournalDisposition::Open {
            return Err(JournalError::Finalized);
        }
        self.disposition = disposition;
        Ok(())
    }
}

impl Default for EventJournal {
    fn default() -> Self {
        Self::new()
    }
}

/// Copyable ordered replay request passed to a platform injector.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ReplayBatch {
    entries: [ReplayRecord; JOURNAL_CAPACITY],
    len: u8,
}

impl ReplayBatch {
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn entries(&self) -> &[ReplayRecord] {
        &self.entries[..self.len()]
    }

    /// Builds releases for exactly those helper-injected downs accepted in a
    /// partial replay prefix and not balanced by an accepted up. Releases are
    /// emitted in reverse down order and can never include a physical modifier.
    pub fn cleanup_for_accepted(&self, accepted: usize) -> Result<CleanupBatch, JournalError> {
        if accepted > self.len() {
            return Err(JournalError::InvalidAcceptedCount);
        }
        let mut held = [None; REPLAY_CLEANUP_EDGE_CAPACITY];
        let mut held_len = 0_usize;
        for record in self.entries()[..accepted].iter().copied() {
            let KeyIdentity::Letter(_) = record.key else {
                continue;
            };
            match record.phase {
                PhysicalPhase::Down => {
                    if held[..held_len]
                        .iter()
                        .flatten()
                        .all(|prior: &ReplayRecord| prior.key != record.key)
                    {
                        held[held_len] = Some(record);
                        held_len += 1;
                    }
                }
                PhysicalPhase::Repeat => {}
                PhysicalPhase::Up => {
                    if let Some(index) = held[..held_len]
                        .iter()
                        .position(|prior| prior.is_some_and(|prior| prior.key == record.key))
                    {
                        held[index] = None;
                    }
                }
            }
        }

        let mut cleanup = CleanupBatch::new();
        for record in held[..held_len].iter().rev().flatten().copied() {
            cleanup.entries[cleanup.len()] = ReplayRecord {
                phase: PhysicalPhase::Up,
                ..record
            };
            cleanup.len += 1;
        }
        Ok(cleanup)
    }
}

/// Bounded best-effort cleanup for helper-owned injected downs after a partial
/// native replay. These are injected releases, never synthetic physical ups.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CleanupBatch {
    entries: [ReplayRecord; REPLAY_CLEANUP_EDGE_CAPACITY],
    len: u8,
}

impl CleanupBatch {
    const fn new() -> Self {
        Self {
            entries: [ReplayRecord::EMPTY; REPLAY_CLEANUP_EDGE_CAPACITY],
            len: 0,
        }
    }

    /// Builds exact balancing ups for replay downs an observing adapter
    /// actually exposed. Entries are released in reverse exposure order.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[doc(hidden)]
    pub fn from_visible_downs(visible: &[Option<ReplayRecord>]) -> Self {
        let mut cleanup = Self::new();
        let mut seen_letters = 0_u32;
        for record in visible.iter().rev().flatten().copied() {
            let KeyIdentity::Letter(key) = record.key else {
                continue;
            };
            let bit = 1_u32 << key.index();
            if record.phase != PhysicalPhase::Down || seen_letters & bit != 0 {
                continue;
            }
            seen_letters |= bit;
            cleanup.entries[cleanup.len()] = ReplayRecord {
                phase: PhysicalPhase::Up,
                ..record
            };
            cleanup.len += 1;
        }
        cleanup
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn entries(&self) -> &[ReplayRecord] {
        &self.entries[..self.len()]
    }

    pub(crate) fn suffix(&self, accepted: usize) -> Result<Self, JournalError> {
        if accepted > self.len() {
            return Err(JournalError::InvalidAcceptedCount);
        }
        let mut remaining = Self::new();
        for entry in self.entries()[accepted..].iter().copied() {
            remaining.entries[remaining.len()] = entry;
            remaining.len += 1;
        }
        Ok(remaining)
    }

    pub(crate) fn for_missing_letters(&self, held_letters: u32) -> Self {
        let mut missing = Self::new();
        for entry in self.entries().iter().copied() {
            let KeyIdentity::Letter(key) = entry.key else {
                continue;
            };
            if held_letters & (1_u32 << key.index()) == 0 {
                missing.entries[missing.len()] = entry;
                missing.len += 1;
            }
        }
        missing
    }

    pub(crate) fn letter_bits(&self) -> u32 {
        self.entries().iter().fold(0_u32, |bits, entry| {
            bits | match entry.key {
                KeyIdentity::Letter(key) => 1_u32 << key.index(),
                _ => 0,
            }
        })
    }

    pub(crate) fn without_letter(&self, removed: crate::ActivationKey) -> Self {
        let mut retained = Self::new();
        for entry in self.entries().iter().copied() {
            if entry.key != KeyIdentity::Letter(removed) {
                retained.entries[retained.len()] = entry;
                retained.len += 1;
            }
        }
        retained
    }

    pub(crate) fn partition_blocked(self, blocked_letters: u32) -> (Self, Self) {
        let mut ready = Self::new();
        let mut blocked = Self::new();
        for entry in self.entries().iter().copied() {
            let is_blocked = matches!(entry.key, KeyIdentity::Letter(key)
                if blocked_letters & (1_u32 << key.index()) != 0);
            let destination = if is_blocked { &mut blocked } else { &mut ready };
            destination.entries[destination.len()] = entry;
            destination.len += 1;
        }
        (ready, blocked)
    }

    pub(crate) fn followed_by(self, suffix: Self) -> Self {
        let mut combined = Self::new();
        let mut seen_letters = 0_u32;
        for entry in self
            .entries()
            .iter()
            .chain(suffix.entries().iter())
            .copied()
        {
            let KeyIdentity::Letter(key) = entry.key else {
                continue;
            };
            let bit = 1_u32 << key.index();
            if seen_letters & bit != 0 {
                continue;
            }
            seen_letters |= bit;
            combined.entries[combined.len()] = entry;
            combined.len += 1;
        }
        combined
    }
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

redacted_debug!(NativeKey, "NativeKey");
redacted_debug!(ReplayRecord, "ReplayRecord");
redacted_debug!(EventJournal, "EventJournal");
redacted_debug!(ReplayBatch, "ReplayBatch");
redacted_debug!(CleanupBatch, "CleanupBatch");

impl Default for CleanupBatch {
    fn default() -> Self {
        Self::new()
    }
}
