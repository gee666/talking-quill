//! Candidate matching, commitment, and owned-key drain states.
use super::*;

impl TransactionEngine {
    pub(super) fn handle_closed_gate_event(mut self, event: NormalizedEvent) -> Turn {
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

    pub(super) fn handle_revision_mismatch(mut self, event: NormalizedEvent) -> Turn {
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

    pub(super) fn handle_idle(
        mut self,
        event: NormalizedEvent,
        prior_physical_letters: u32,
    ) -> Turn {
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

    pub(super) fn handle_candidate(
        mut self,
        event: NormalizedEvent,
        mut candidate: Candidate,
    ) -> Turn {
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
        let already_owned = candidate.owned_letters & bit != 0;
        let valid_phase = match event.phase {
            PhysicalPhase::Down => !already_owned,
            PhysicalPhase::Repeat | PhysicalPhase::Up => already_owned,
        };
        if !valid_phase {
            self.state = ActivationState::Candidate(candidate);
            return self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::InvalidContinuation,
                false,
                false,
            );
        }
        // Validate ownership before appending; every accepted letter phase
        // records exactly one edge before changing ownership or matching.
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

        match event.phase {
            PhysicalPhase::Repeat => {
                self.state = ActivationState::Candidate(candidate);
                self.complete_event(event, EventDisposition::CaptureCurrent, None)
            }
            PhysicalPhase::Down => {
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

    pub(super) fn handle_committed(
        mut self,
        event: NormalizedEvent,
        mut committed: Committed,
    ) -> Turn {
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

    pub(super) fn handle_drain(mut self, event: NormalizedEvent, mut drain: DrainOnly) -> Turn {
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
}
