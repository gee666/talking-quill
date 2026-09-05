//! Candidate cancellation, deferred replay, and terminal ownership drains.
use super::*;

impl TransactionEngine {
    pub(super) fn request_candidate_cancellation(
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

    pub(super) fn discard_candidate_into_terminal_drain(
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

    pub(super) fn close_candidate_event_for_later_cancellation(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        reason: CancelReason,
    ) -> Turn {
        self.admission_open = false;
        self.complete_event(event, disposition, Some(reason))
    }

    pub(super) fn close_candidate_control_for_later_cancellation(
        mut self,
        reason: CancelReason,
    ) -> Turn {
        self.admission_open = false;
        self.complete_control(false, Some(reason))
    }

    pub(super) fn discard_candidate_control_into_terminal_drain(
        mut self,
        reason: CancelReason,
    ) -> Turn {
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
}
