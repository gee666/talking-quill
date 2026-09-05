//! Reconcile physical state and track foreground-visible input.
use super::*;

impl TransactionEngine {
    /// Applies the required exact post-event snapshot and reports whether it
    /// agrees with the edge predicted from the prior reducer state.
    pub(super) fn observe_physical(&mut self, event: NormalizedEvent) -> bool {
        debug_assert!(event.source.is_physical());
        if !event.snapshot.is_valid() {
            self.physical_letters = event.snapshot.held_letters;
            self.physical_modifiers = event.snapshot.modifiers;
            self.alt_gr_active = event.snapshot.alt_gr_active;
            return false;
        }
        let mut predicted = PhysicalSnapshot::new(
            self.physical_letters,
            self.physical_modifiers,
            event.snapshot.alt_gr_active,
        );
        let phase_valid = match event.key {
            KeyIdentity::Letter(key) => {
                let was_held = self.physical_letters & letter_bit(key) != 0;
                match event.phase {
                    PhysicalPhase::Down => predicted.held_letters |= letter_bit(key),
                    PhysicalPhase::Repeat => predicted.held_letters |= letter_bit(key),
                    PhysicalPhase::Up => predicted.held_letters &= !letter_bit(key),
                }
                matches!(event.phase, PhysicalPhase::Down) != was_held
            }
            KeyIdentity::Modifier(side) => {
                let was_held = self.physical_modifiers.contains(side);
                match event.phase {
                    PhysicalPhase::Down => predicted.modifiers.insert(side),
                    PhysicalPhase::Repeat => predicted.modifiers.insert(side),
                    PhysicalPhase::Up => predicted.modifiers.remove(side),
                }
                matches!(event.phase, PhysicalPhase::Down) != was_held
            }
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => true,
        };
        let consistent = phase_valid && predicted == event.snapshot;
        self.physical_letters = event.snapshot.held_letters;
        self.physical_modifiers = event.snapshot.modifiers;
        self.alt_gr_active = event.snapshot.alt_gr_active;
        self.fenced_letters &= self.physical_letters;
        self.fenced_modifiers =
            ModifierSides::from_bits(self.fenced_modifiers.bits() & self.physical_modifiers.bits());
        if !self.physical_modifiers.any_alt() {
            self.alt_cycle_neutralized = false;
        }
        if !self.physical_modifiers.any_meta() {
            self.meta_cycle_neutralized = false;
        }
        consistent
    }

    pub(super) fn handle_physical_state_mismatch(mut self, event: NormalizedEvent) -> Turn {
        self.admission_open = false;
        self.terminal = true;
        self.fence_current_physical(true);
        match self.state {
            ActivationState::Candidate(_) => self.request_candidate_cancellation(
                event,
                EventDisposition::PassCurrent,
                CancelReason::PhysicalStateMismatch,
                true,
                true,
            ),
            ActivationState::Committed(committed) => {
                let current_was_owned = matches!(event.key, KeyIdentity::Letter(key)
                    if committed.owned_letters & letter_bit(key) != 0);
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    if current_was_owned {
                        EventDisposition::CaptureCurrent
                    } else {
                        EventDisposition::PassCurrent
                    },
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
            ActivationState::DrainOnly(drain) => {
                let current_was_owned = matches!(event.key, KeyIdentity::Letter(key)
                    if drain.owned_letters & letter_bit(key) != 0);
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    if current_was_owned {
                        EventDisposition::CaptureCurrent
                    } else {
                        EventDisposition::PassCurrent
                    },
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
            ActivationState::Idle => {
                self.apply_reconciliation(event.snapshot, true);
                self.complete_event(
                    event,
                    EventDisposition::PassCurrent,
                    Some(CancelReason::PhysicalStateMismatch),
                )
            }
        }
    }

    pub(super) fn apply_observed_reconciliation(&mut self, snapshot: PhysicalSnapshot) {
        let owned = self.owned_letters() & snapshot.held_letters;
        self.physical_letters = snapshot.held_letters;
        self.physical_modifiers = snapshot.modifiers;
        self.alt_gr_active = snapshot.alt_gr_active;
        self.foreground_letters = snapshot.held_letters & !owned;
        self.foreground_modifiers = snapshot.modifiers;
        self.fenced_letters &= snapshot.held_letters;
        self.fenced_modifiers =
            ModifierSides::from_bits(self.fenced_modifiers.bits() & snapshot.modifiers.bits());
        if !snapshot.modifiers.any_alt() {
            self.alt_cycle_neutralized = false;
        }
        if !snapshot.modifiers.any_meta() {
            self.meta_cycle_neutralized = false;
        }
        self.state = match self.state {
            ActivationState::Committed(mut committed) => {
                committed.owned_letters = owned;
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::Committed(committed)
                }
            }
            ActivationState::DrainOnly(_) => {
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(DrainOnly {
                        owned_letters: owned,
                    })
                }
            }
            ActivationState::Idle => ActivationState::Idle,
            // A candidate reaches this path only when activation could not
            // commit after the deferred native effect. Its journal lacks any
            // callbacks that raced that effect, so replay is no longer a
            // balanced representation. Discard it and retain only physically
            // held ownership for exact-up draining.
            ActivationState::Candidate(_) => {
                if owned == 0 {
                    ActivationState::Idle
                } else {
                    ActivationState::DrainOnly(DrainOnly {
                        owned_letters: owned,
                    })
                }
            }
        };
    }

    pub(super) fn apply_reconciliation(
        &mut self,
        snapshot: PhysicalSnapshot,
        close_admission: bool,
    ) {
        let owned = self.owned_letters() & snapshot.held_letters;
        self.physical_letters = snapshot.held_letters;
        self.physical_modifiers = snapshot.modifiers;
        self.alt_gr_active = snapshot.alt_gr_active;
        self.foreground_letters = snapshot.held_letters & !owned;
        self.foreground_modifiers = snapshot.modifiers;
        self.fenced_letters = snapshot.held_letters;
        self.fenced_modifiers = snapshot.modifiers;
        self.alt_cycle_neutralized = false;
        self.meta_cycle_neutralized = false;
        if close_admission {
            self.admission_open = false;
            self.terminal = true;
            self.state = ActivationState::DrainOnly(DrainOnly {
                owned_letters: owned,
            });
        } else {
            self.state = ActivationState::Idle;
        }
    }

    pub(super) fn satisfy_pending_cleanup_with_unowned_physical_up(
        &mut self,
        event: NormalizedEvent,
    ) {
        let (KeyIdentity::Letter(key), PhysicalPhase::Up, Some(cleanup)) =
            (event.key, event.phase, self.pending_injected_cleanup)
        else {
            return;
        };
        if self.owned_letters() & letter_bit(key) != 0
            || cleanup.letter_bits() & letter_bit(key) == 0
        {
            return;
        }
        let remaining = cleanup.without_letter(key);
        self.pending_injected_cleanup = (!remaining.is_empty()).then_some(remaining);
    }

    pub(super) fn request_retry_cleanup(self) -> Turn {
        if let Some(menu) = self.pending_menu_cleanup {
            return Turn::NeedEffect {
                effect: EffectRequest::CleanupMenuNeutralization(menu),
                continuation: Continuation {
                    pending: PendingEffect::MenuCleanup {
                        engine: self,
                        event: None,
                    },
                },
            };
        }
        if let Some(cleanup) = self.pending_injected_cleanup {
            let newly_held = self.physical_letters & !self.owned_letters();
            let (ready, blocked) = cleanup.partition_blocked(newly_held);
            if ready.is_empty() {
                return self.complete_control(false, Some(CancelReason::ReplayFailed));
            }
            return Turn::NeedEffect {
                effect: EffectRequest::CleanupInjected(ready),
                continuation: Continuation {
                    pending: PendingEffect::Cleanup {
                        engine: self,
                        requested: ready,
                        deferred: blocked,
                        completion: CleanupCompletion::RetryControl,
                    },
                },
            };
        }
        self.complete_control(true, None)
    }

    pub(super) fn apply_foreground_event(&mut self, event: NormalizedEvent) {
        match event.key {
            KeyIdentity::Letter(key) => match event.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_letters |= letter_bit(key);
                }
                PhysicalPhase::Up => self.foreground_letters &= !letter_bit(key),
            },
            KeyIdentity::Modifier(side) => match event.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_modifiers.insert(side);
                }
                PhysicalPhase::Up => self.foreground_modifiers.remove(side),
            },
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
        }
    }

    pub(super) fn apply_replay(&mut self, batch: ReplayBatch) {
        self.apply_replay_prefix(batch, batch.len());
    }

    pub(super) fn apply_replay_prefix(&mut self, batch: ReplayBatch, accepted: usize) {
        for record in batch.entries()[..accepted].iter().copied() {
            self.apply_injected_record(record);
        }
    }

    pub(super) fn apply_cleanup_prefix(&mut self, cleanup: CleanupBatch, accepted: usize) {
        for record in cleanup.entries()[..accepted].iter().copied() {
            self.apply_injected_record(record);
        }
    }

    pub(super) fn mark_cleanup_pending_visible(&mut self, cleanup: CleanupBatch) {
        for record in cleanup.entries().iter().copied() {
            if let KeyIdentity::Letter(key) = record.key {
                self.foreground_letters |= letter_bit(key);
            }
        }
    }

    pub(super) fn apply_injected_record(&mut self, record: ReplayRecord) {
        match record.key {
            KeyIdentity::Letter(key) => match record.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_letters |= letter_bit(key);
                }
                PhysicalPhase::Up => self.foreground_letters &= !letter_bit(key),
            },
            KeyIdentity::Modifier(side) => match record.phase {
                PhysicalPhase::Down | PhysicalPhase::Repeat => {
                    self.foreground_modifiers.insert(side);
                }
                PhysicalPhase::Up => self.foreground_modifiers.remove(side),
            },
            KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
        }
    }

    pub(super) fn install_config(&mut self, config: CompiledActivationConfig) {
        self.config = config;
        // The owner installs an enabled configuration when it explicitly opens
        // admission after a pause or binding change. Terminal faults and shutdown
        // remain latched; an ordinary close must not disable this engine forever.
        self.admission_open = self.config.enabled() && !self.terminal && !self.shutting_down;
        self.fence_current_physical(true);
    }

    pub(super) fn fence_current_physical(&mut self, include_modifiers: bool) {
        self.fenced_letters |= self.physical_letters;
        if include_modifiers {
            self.fenced_modifiers = ModifierSides::from_bits(
                self.fenced_modifiers.bits() | self.physical_modifiers.bits(),
            );
        }
    }
}
