//! Request native effects and finish activation transactions.
use super::*;

impl TransactionEngine {
    pub(super) fn request_activation(
        self,
        event: NormalizedEvent,
        notice: ActivationNotice,
    ) -> Turn {
        // A user commonly releases Alt just before the pending prefix key. The
        // low-level callback has already applied that release to physical state,
        // but the accepted shortcut still owns the Alt menu cycle recorded at
        // candidate admission.
        let activation_modifiers = match self.state {
            ActivationState::Candidate(candidate) => candidate.expected_modifiers,
            _ => self.physical_modifiers.combined(),
        };
        let menu = MenuModifiers {
            alt: activation_modifiers.alt() && !self.alt_cycle_neutralized,
            meta: activation_modifiers.meta() && !self.meta_cycle_neutralized,
        };
        if self.menu_neutralization_policy == MenuNeutralizationPolicy::Required && menu.any() {
            Turn::NeedEffect {
                effect: EffectRequest::NeutralizeMenu(menu),
                continuation: Continuation {
                    pending: PendingEffect::Neutralize {
                        engine: self,
                        event,
                        notice,
                        menu,
                    },
                },
            }
        } else {
            self.request_activation_delivery(event, notice)
        }
    }

    pub(super) fn request_activation_delivery(
        self,
        event: NormalizedEvent,
        notice: ActivationNotice,
    ) -> Turn {
        Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation: Continuation {
                pending: PendingEffect::ActivationDownOrComplete {
                    engine: self,
                    event,
                    notice,
                },
            },
        }
    }

    pub(super) fn request_activation_up(
        self,
        event: NormalizedEvent,
        notice: ActivationNotice,
    ) -> Turn {
        Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation: Continuation {
                pending: PendingEffect::ActivationUp {
                    engine: self,
                    event,
                    notice,
                },
            },
        }
    }

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

    pub(super) fn request_replay(self, completion: ReplayCompletion) -> Turn {
        let ActivationState::Candidate(candidate) = self.state else {
            return finish_terminal_completion(
                self,
                completion,
                CancelReason::EffectProtocolViolation,
            );
        };
        let batch = candidate
            .journal
            .replay_batch()
            .expect("an active candidate has an open journal");
        Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            continuation: Continuation {
                pending: PendingEffect::Replay {
                    engine: self,
                    batch,
                    completion,
                },
            },
        }
    }

    pub(super) fn finish_successful_activation(
        mut self,
        event: NormalizedEvent,
        notice: ActivationNotice,
    ) -> Turn {
        let ActivationState::Candidate(mut candidate) = self.state else {
            return self.terminal_event(
                event,
                EventDisposition::CaptureCurrent,
                CancelReason::EffectProtocolViolation,
            );
        };
        candidate
            .journal
            .commit()
            .expect("activation candidate journal is open");
        self.metrics.observe_journal(candidate.journal.len());
        TransactionMetrics::increment(&mut self.metrics.committed);
        let (binding, trigger, trigger_down_at_ms, activation_up_pending) = match notice {
            ActivationNotice::Down { binding } => (
                binding,
                binding.shortcut().trigger(),
                event.observed_at_ms,
                true,
            ),
            ActivationNotice::Complete { binding, held_ms } => (
                binding,
                binding.shortcut().trigger(),
                event.observed_at_ms.saturating_sub(held_ms),
                false,
            ),
            ActivationNotice::Up { .. } => {
                return self.terminal_event(
                    event,
                    EventDisposition::CaptureCurrent,
                    CancelReason::EffectProtocolViolation,
                );
            }
        };
        let committed = Committed {
            binding,
            trigger,
            trigger_down_at_ms,
            owned_letters: candidate.owned_letters,
            activation_up_pending,
        };
        self.state = if committed.owned_letters == 0 {
            ActivationState::Idle
        } else {
            ActivationState::Committed(committed)
        };
        let disposition = if matches!(notice, ActivationNotice::Complete { .. })
            && matches!(event.key, KeyIdentity::Modifier(_))
        {
            // Modifier down was never owned. Pass its physical up after the
            // dummy menu neutralization while retaining every candidate letter
            // until its matching physical release.
            EventDisposition::PassCurrent
        } else {
            EventDisposition::CaptureCurrent
        };
        self.complete_event(event, disposition, None)
    }

    pub(super) fn finish_activation_up(mut self, event: NormalizedEvent, delivered: bool) -> Turn {
        let ActivationState::Committed(committed) = self.state else {
            return self.terminal_event(
                event,
                EventDisposition::CaptureCurrent,
                CancelReason::EffectProtocolViolation,
            );
        };
        if delivered {
            self.state = if committed.owned_letters == 0 {
                ActivationState::Idle
            } else {
                ActivationState::Committed(committed)
            };
            self.complete_event(event, EventDisposition::CaptureCurrent, None)
        } else {
            self.admission_open = false;
            self.terminal = true;
            self.state = ActivationState::DrainOnly(DrainOnly {
                owned_letters: committed.owned_letters,
            });
            self.complete_event(
                event,
                EventDisposition::CaptureCurrent,
                Some(CancelReason::ActivationDeliveryFailed),
            )
        }
    }

    pub(super) fn complete_event(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        cancellation: Option<CancelReason>,
    ) -> Turn {
        if disposition == EventDisposition::PassCurrent && event.source.is_physical() {
            self.apply_foreground_event(event);
        }
        Turn::Complete {
            completion: Completion::Event(EventOutcome {
                disposition,
                cancellation,
                terminal: self.terminal,
            }),
            engine: self,
        }
    }

    pub(super) fn complete_control(
        self,
        applied: bool,
        cancellation: Option<CancelReason>,
    ) -> Turn {
        let shutdown = self.shutdown_state();
        Turn::Complete {
            engine: self,
            completion: Completion::Control(ControlOutcome {
                applied,
                cancellation,
                shutdown,
            }),
        }
    }

    pub(super) fn terminal_event(
        mut self,
        event: NormalizedEvent,
        disposition: EventDisposition,
        reason: CancelReason,
    ) -> Turn {
        self.admission_open = false;
        self.terminal = true;
        self.move_owned_to_drain();
        self.complete_event(event, disposition, Some(reason))
    }

    pub(super) fn move_owned_to_drain(&mut self) {
        let owned_letters = self.owned_letters() & self.physical_letters;
        self.state = ActivationState::DrainOnly(DrainOnly { owned_letters });
    }
}
