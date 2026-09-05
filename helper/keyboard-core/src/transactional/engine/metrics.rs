//! Saturating transaction counters.
use super::CancelReason;

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
    pub(super) fn increment(value: &mut u64) {
        *value = value.saturating_add(1);
    }

    pub(super) fn observe_journal(&mut self, len: usize) {
        self.journal_high_water = self.journal_high_water.max(len as u64);
    }

    pub(super) fn record_cancellation(&mut self, reason: CancelReason) {
        Self::increment(&mut self.cancelled);
        Self::increment(&mut self.cancellation_reasons[reason.index()]);
    }

    pub(super) fn record_replay(&mut self, submitted: usize, requested: usize) {
        Self::increment(&mut self.replay_attempted);
        if submitted == requested {
            Self::increment(&mut self.replay_succeeded);
        } else if submitted == 0 || submitted > requested {
            Self::increment(&mut self.replay_failed);
        } else {
            Self::increment(&mut self.replay_partial);
        }
    }

    pub(super) fn record_dummy(&mut self, accepted: usize) {
        Self::increment(&mut self.dummy_attempted);
        match accepted {
            2 => Self::increment(&mut self.dummy_succeeded),
            1 => Self::increment(&mut self.dummy_partial),
            _ => Self::increment(&mut self.dummy_failed),
        }
    }
}
