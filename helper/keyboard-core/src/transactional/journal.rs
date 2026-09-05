use std::fmt;

use thiserror::Error;

use super::{KeyIdentity, PhysicalPhase};
use crate::ACTIVATION_KEY_CAPACITY;

mod batches;
pub use batches::{CleanupBatch, ReplayBatch};

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
