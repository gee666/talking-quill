//! Batch deadlines, submission authority, and exact cursor advancement.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn refresh_external_collection_deadline(&mut self) {
        let has_external = self.pending[..self.pending_len]
            .iter()
            .chain(&self.tail[..self.tail_len])
            .any(|edge| edge.source == InputSource::External)
            || (0..self.overflow_balance_len).any(|offset| {
                let index = (self.overflow_balance_head + offset) % self.overflow_balances.len();
                self.overflow_balances[index].source == InputSource::External
            });
        if has_external {
            self.external_collection_deadline
                .get_or_insert_with(|| Instant::now() + EXTERNAL_DEFERRED_COLLECTION_TIMEOUT);
        } else {
            self.external_collection_deadline = None;
        }
    }

    pub(super) fn expire_external_collection(&mut self, now: Instant) -> bool {
        if self.token.is_some()
            || !self
                .external_collection_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            return false;
        }
        self.enter_overflow();
        self.materialize_overflow_batch();
        true
    }

    pub(super) fn append(&mut self, edge: injection::DeferredEvent) -> bool {
        if edge.source == InputSource::External && self.external_collection_deadline.is_none() {
            self.external_collection_deadline =
                Some(Instant::now() + EXTERNAL_DEFERRED_COLLECTION_TIMEOUT);
        }
        if self.overflow {
            self.retain_overflow_edge(edge);
            self.materialize_overflow_batch();
            return false;
        }
        let (entries, len) = if self.token.is_some() {
            (&mut self.tail, &mut self.tail_len)
        } else {
            (&mut self.pending, &mut self.pending_len)
        };
        if *len == entries.len() {
            self.enter_overflow();
            self.retain_overflow_edge(edge);
            self.materialize_overflow_batch();
            return false;
        }
        entries[*len] = edge;
        *len += 1;
        true
    }

    pub(super) fn materialize_overflow_batch(&mut self) {
        if !self.overflow || self.token.is_some() || self.pending_len != 0 {
            return;
        }
        self.finalize_discarded_hidden_phases();
        let count = self
            .overflow_balance_len
            .min(injection::DEFERRED_EDGE_CAPACITY);
        for index in 0..count {
            let balance_index = (self.overflow_balance_head + index) % self.overflow_balances.len();
            self.pending[index] = self.overflow_balances[balance_index];
            self.overflow_balances[balance_index] = injection::DeferredEvent::EMPTY;
        }
        self.overflow_balance_head =
            (self.overflow_balance_head + count) % self.overflow_balances.len();
        self.overflow_balance_len -= count;
        self.pending_len = count;
        if count == 0 {
            self.overflow = false;
        }
        self.refresh_external_collection_deadline();
    }

    pub(super) fn ready_slice(&self) -> Option<&[injection::DeferredEvent]> {
        let ready = self.ready_len();
        (ready != 0).then_some(&self.pending[..ready])
    }

    pub(super) fn begin_submission(
        &mut self,
        token: injection::OperationToken,
        bank: usize,
        count: usize,
    ) -> bool {
        if self.token.is_some() || count == 0 || count > self.pending_len {
            return false;
        }
        let suffix = self.pending_len - count;
        if self.tail_len + suffix > self.tail.len() {
            return false;
        }
        for index in 0..suffix {
            self.tail[self.tail_len + index] = self.pending[count + index];
            self.pending[count + index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len += suffix;
        self.pending_len = count;
        self.submitted_len = count;
        self.observed = 0;
        self.token = Some(token);
        self.next_pool_bank = (bank + 1) % injection::DEFERRED_POOL_BANKS;
        self.submission_deadline = Some(Instant::now() + Duration::from_millis(250));
        self.refresh_external_collection_deadline();
        true
    }

    pub(super) fn expected(&self) -> Option<injection::DeferredEvent> {
        self.token
            .and_then(|_| self.pending.get(self.observed).copied())
            .filter(|_| self.observed < self.submitted_len)
    }

    pub(super) fn advance_observation(&mut self) -> GapBarrierObservation {
        if self.token.is_none() || self.observed >= self.submitted_len {
            return GapBarrierObservation::Forged;
        }
        self.observed += 1;
        if self.observed != self.submitted_len {
            return GapBarrierObservation::Down;
        }
        for entry in &mut self.pending[..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = 0;
        self.submitted_len = 0;
        self.observed = 0;
        self.token = None;
        self.submission_deadline = None;
        if self.overflow {
            self.materialize_overflow_batch();
        } else {
            for index in 0..self.tail_len {
                self.pending[index] = self.tail[index];
                self.tail[index] = injection::DeferredEvent::EMPTY;
            }
            self.pending_len = self.tail_len;
            self.tail_len = 0;
        }
        self.refresh_external_collection_deadline();
        GapBarrierObservation::Complete
    }

    pub(super) fn settle_overflow(&mut self) {
        self.materialize_overflow_batch();
    }
}
