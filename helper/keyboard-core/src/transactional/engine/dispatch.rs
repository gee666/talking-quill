//! Input and control dispatch, including delayed observed releases.
use super::*;

impl TransactionEngine {
    pub(super) fn begin_event(mut self, event: NormalizedEvent) -> Turn {
        if matches!(
            event.source,
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
        ) {
            return self.complete_event(event, EventDisposition::PassCurrent, None);
        }
        let prior_letters = self.physical_letters;
        let snapshot_consistent = self.observe_physical(event);
        if !snapshot_consistent {
            return self.handle_physical_state_mismatch(event);
        }
        self.satisfy_pending_cleanup_with_unowned_physical_up(event);

        if event.gate == GateState::Closed {
            return self.handle_closed_gate_event(event);
        }

        if event.config_revision != self.config.revision() {
            return self.handle_revision_mismatch(event);
        }

        match self.state {
            ActivationState::Idle => self.handle_idle(event, prior_letters),
            ActivationState::Candidate(candidate) => self.handle_candidate(event, candidate),
            ActivationState::Committed(committed) => self.handle_committed(event, committed),
            ActivationState::DrainOnly(drain) => self.handle_drain(event, drain),
        }
    }

    pub(super) fn begin_control(mut self, control: Control) -> Turn {
        match control {
            Control::ReplaceConfig(config) => {
                if !config.revision().is_newer_than(self.config.revision()) {
                    return self.complete_control(false, Some(CancelReason::RevisionMismatch));
                }
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::Candidate(candidate);
                        self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Install(
                            config,
                        )))
                    }
                    ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                        self.install_config(config);
                        self.complete_control(true, Some(CancelReason::ConfigurationReplaced))
                    }
                    ActivationState::Idle => {
                        self.install_config(config);
                        self.complete_control(true, Some(CancelReason::ConfigurationReplaced))
                    }
                }
            }
            Control::CloseAdmission(reason) => {
                self.admission_open = false;
                // Closure is only the fresh-admission barrier. Candidate
                // cancellation/replay is deliberately a later ordered action.
                self.complete_control(true, Some(reason))
            }
            Control::CancelCandidate(reason) => match self.state {
                ActivationState::Candidate(candidate) => {
                    self.state = ActivationState::Candidate(candidate);
                    self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Cancel(
                        reason,
                    )))
                }
                ActivationState::Idle => self.complete_control(true, Some(reason)),
                ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                    self.complete_control(false, Some(reason))
                }
            },
            Control::Cancel(CancelReason::Shutdown) => self.begin_control(Control::Shutdown),
            Control::Cancel(reason) if reason.closes_admission() => {
                // Legacy atomic terminal cancellation remains available for
                // unrecoverable core faults. W1 owner loss ordering uses the
                // explicit CloseAdmission -> CancelCandidate pair instead.
                self.admission_open = false;
                self.terminal = true;
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: candidate.owned_letters,
                        });
                        self.metrics.record_cancellation(reason);
                    }
                    ActivationState::Committed(committed) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: committed.owned_letters,
                        });
                    }
                    ActivationState::Idle | ActivationState::DrainOnly(_) => {}
                }
                self.complete_control(true, Some(reason))
            }
            Control::Cancel(reason) if reason.is_nonterminal_control_cancellation() => {
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        self.state = ActivationState::Candidate(candidate);
                        self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Cancel(
                            reason,
                        )))
                    }
                    ActivationState::Idle => self.complete_control(true, Some(reason)),
                    ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                        self.complete_control(false, Some(reason))
                    }
                }
            }
            Control::Cancel(reason) => self.complete_control(false, Some(reason)),
            Control::Reconcile(snapshot) => match self.state {
                ActivationState::Candidate(candidate) => {
                    self.physical_letters = snapshot.held_letters;
                    self.physical_modifiers = snapshot.modifiers;
                    self.alt_gr_active = snapshot.alt_gr_active;
                    self.fenced_letters &= snapshot.held_letters;
                    self.fenced_modifiers = ModifierSides::from_bits(
                        self.fenced_modifiers.bits() & snapshot.modifiers.bits(),
                    );
                    self.state = ActivationState::Candidate(candidate);
                    self.request_replay(ReplayCompletion::Control(ControlAfterReplay::Reconcile(
                        snapshot,
                    )))
                }
                ActivationState::Committed(_) | ActivationState::DrainOnly(_) => {
                    self.apply_reconciliation(snapshot, true);
                    self.complete_control(true, Some(CancelReason::PhysicalStateMismatch))
                }
                ActivationState::Idle => {
                    self.apply_reconciliation(snapshot, false);
                    self.complete_control(true, Some(CancelReason::PhysicalStateMismatch))
                }
            },
            Control::ReconcileObserved {
                snapshot,
                observed_at_ms,
            } => {
                let release = match &mut self.state {
                    ActivationState::Committed(committed)
                        if committed.activation_up_pending
                            && snapshot.held_letters & letter_bit(committed.trigger) == 0 =>
                    {
                        committed.activation_up_pending = false;
                        Some(ActivationNotice::Up {
                            binding: committed.binding,
                            held_ms: observed_at_ms.saturating_sub(committed.trigger_down_at_ms),
                        })
                    }
                    _ => None,
                };
                self.apply_observed_reconciliation(snapshot);
                if let Some(notice) = release {
                    return Turn::NeedEffect {
                        effect: EffectRequest::DeliverActivation(notice),
                        continuation: Continuation {
                            pending: PendingEffect::ObservedActivationUp { engine: self },
                        },
                    };
                }
                self.complete_control(true, None)
            }
            Control::RetryCleanup => self.request_retry_cleanup(),
            Control::Shutdown => {
                self.admission_open = false;
                self.shutting_down = true;
                match self.state {
                    ActivationState::Candidate(candidate) => {
                        // The candidate journal contains physical edges hidden
                        // from the foreground. Shutdown must not replay a held
                        // down into a possibly changed target. Retain ownership
                        // and drain only on authoritative physical releases.
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: candidate.owned_letters,
                        });
                        self.metrics.record_cancellation(CancelReason::Shutdown);
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                    ActivationState::Committed(committed) => {
                        self.state = ActivationState::DrainOnly(DrainOnly {
                            owned_letters: committed.owned_letters,
                        });
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                    ActivationState::Idle | ActivationState::DrainOnly(_) => {
                        self.complete_control(true, Some(CancelReason::Shutdown))
                    }
                }
            }
        }
    }
}
