//! Directional transport sequence validation.
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub struct SequenceValidator {
    next: Option<u64>,
}

impl Default for SequenceValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceValidator {
    #[must_use]
    pub const fn new() -> Self {
        Self { next: Some(1) }
    }

    /// Restores a validator from a trusted local high-water mark. This is also
    /// useful for bounded model tests near `u64::MAX`; high-water is never read
    /// from an untrusted frame.
    #[must_use]
    pub(crate) const fn from_high_water(high_water: Option<u64>) -> Self {
        let next = match high_water {
            None => Some(1),
            Some(u64::MAX) => None,
            Some(value) => Some(value + 1),
        };
        Self { next }
    }

    pub fn check(&self, sequence: u64) -> Result<(), SequenceError> {
        let Some(expected) = self.next else {
            return Err(SequenceError::Wrapped);
        };
        if sequence == 0 {
            return Err(SequenceError::Wrapped);
        }
        if sequence < expected {
            return Err(SequenceError::Duplicate);
        }
        if sequence > expected {
            return Err(SequenceError::Skipped);
        }
        Ok(())
    }

    pub fn accept(&mut self, sequence: u64) -> Result<(), SequenceError> {
        self.check(sequence)?;
        self.next = sequence.checked_add(1);
        Ok(())
    }

    #[must_use]
    pub const fn next(&self) -> Option<u64> {
        self.next
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SequenceError {
    #[error("duplicate or stale owner-protocol sequence")]
    Duplicate,
    #[error("skipped owner-protocol sequence")]
    Skipped,
    #[error("zero or wrapped owner-protocol sequence")]
    Wrapped,
}
