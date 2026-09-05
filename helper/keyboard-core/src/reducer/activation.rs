//! Ordered activation transitions.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HeldLetters {
    keys: [ActivationKey; Shortcut::MAX_KEYS],
    count: u8,
}

impl HeldLetters {
    pub(super) fn as_slice(&self) -> &[ActivationKey] {
        &self.keys[..usize::from(self.count)]
    }

    pub(super) fn contains(&self, key: ActivationKey) -> bool {
        self.as_slice().contains(&key)
    }

    fn push_fresh(&mut self, key: ActivationKey) -> bool {
        if self.contains(key) || usize::from(self.count) == Shortcut::MAX_KEYS {
            return false;
        }
        self.keys[usize::from(self.count)] = key;
        self.count += 1;
        true
    }

    fn release(&mut self, key: ActivationKey) {
        let Some(index) = self.as_slice().iter().position(|held| *held == key) else {
            return;
        };
        let count = usize::from(self.count);
        self.keys.copy_within(index + 1..count, index);
        self.count -= 1;
    }
}

impl Default for HeldLetters {
    fn default() -> Self {
        Self {
            keys: [ActivationKey::A; Shortcut::MAX_KEYS],
            count: 0,
        }
    }
}

impl KeyboardReducer {
    pub(super) fn plan_letter(
        &self,
        input: KeyInput,
        key: ActivationKey,
        bindings: ActivationBindings,
        activation_enabled: bool,
        observed_at_ms: u64,
    ) -> DecisionPlan {
        if let Some(active) = self.active_activation {
            if active.trigger == key {
                if input.phase == KeyPhase::Down {
                    return DecisionPlan::unchanged(self, true);
                }

                let mut next = self.snapshot();
                next.active_activation = None;
                next.held_letters.release(key);
                next.reset_activation_sequence_if_released();
                return DecisionPlan {
                    delivered_state: next.snapshot(),
                    failed_state: next,
                    event: Some(KeyboardEvent::Activation {
                        binding: active.binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: true,
                };
            }
            return self.pass_letter(input, key);
        }

        match input.phase {
            KeyPhase::Up => {
                if let Some(pending) = self
                    .pending_activation
                    .filter(|pending| pending.binding.shortcut().trigger() == key)
                {
                    let mut next = self.snapshot();
                    next.pending_activation = None;
                    next.held_letters.release(key);
                    next.reset_activation_sequence_if_released();
                    let Some(context) = next.take_activation_context() else {
                        return DecisionPlan::same(next, false);
                    };
                    return DecisionPlan {
                        delivered_state: next.snapshot(),
                        failed_state: next,
                        event: Some(KeyboardEvent::ActivationComplete {
                            binding: pending.binding,
                            context,
                            held_ms: observed_at_ms.saturating_sub(pending.started_at_ms),
                        }),
                        // The pending prefix down passed through, so its up must also pass through.
                        swallow_if_delivered: false,
                        swallow_if_failed: false,
                    };
                }

                let abandons_pending_exact =
                    self.pending_activation.is_some() && self.held_letters.contains(key);
                let abandons_prefix_only_continuation = bindings
                    .find_exact(input.modifiers, self.held_letters.as_slice())
                    .is_none()
                    && bindings
                        .has_longer_sequence_prefix(input.modifiers, self.held_letters.as_slice());
                if !abandons_pending_exact && !abandons_prefix_only_continuation {
                    return self.pass_letter(input, key);
                }
                let mut next = self.snapshot();
                next.pending_activation = None;
                next.activation_sequence_fenced = true;
                next.held_letters.release(key);
                next.reset_activation_sequence_if_released();
                DecisionPlan::same(next, false)
            }
            KeyPhase::Down if input.repeat => DecisionPlan::unchanged(self, false),
            KeyPhase::Down => {
                let mut next = self.snapshot();
                let begins_sequence = next.held_letters.as_slice().is_empty();
                if !next.held_letters.push_fresh(key) {
                    return DecisionPlan::same(next, false);
                }
                if begins_sequence {
                    next.activation_sequence_modifiers = Some(input.modifiers);
                    if !input.modifiers.any() {
                        next.activation_sequence_fenced = true;
                    }
                }
                let matching_enabled = !next.activation_sequence_fenced && activation_enabled;
                let accepted = matching_enabled
                    .then(|| bindings.find_exact(input.modifiers, next.held_letters.as_slice()))
                    .flatten()
                    .filter(|binding| binding.shortcut().trigger() == key);
                let Some(binding) = accepted else {
                    if matching_enabled
                        && bindings.has_longer_sequence_prefix(
                            input.modifiers,
                            next.held_letters.as_slice(),
                        )
                    {
                        // Entering a longer candidate commits the attempt away from an earlier
                        // shorter exact. If this prefix-only continuation is abandoned, neither
                        // binding may activate from the stale held-key state.
                        next.pending_activation = None;
                        return DecisionPlan::same(next, false);
                    }
                    if next.pending_activation.is_some() {
                        next.pending_activation = None;
                        next.activation_sequence_fenced = true;
                    }
                    return DecisionPlan::same(next, false);
                };

                if bindings.has_longer_prefix(binding) {
                    next.pending_activation = Some(PendingActivation {
                        binding,
                        started_at_ms: observed_at_ms,
                    });
                    return DecisionPlan::same(next, false);
                }

                next.pending_activation = None;
                let Some(context) = next.take_activation_context() else {
                    next.activation_sequence_fenced = true;
                    return DecisionPlan::same(next, false);
                };
                let mut delivered = next.snapshot();
                delivered.active_activation = Some(ActiveActivation {
                    binding,
                    context,
                    trigger: key,
                });
                DecisionPlan {
                    delivered_state: delivered,
                    failed_state: next,
                    event: Some(KeyboardEvent::Activation {
                        binding,
                        context,
                        phase: EventPhase::Down,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: false,
                }
            }
        }
    }

    fn pass_letter(&self, input: KeyInput, key: ActivationKey) -> DecisionPlan {
        let mut next = self.snapshot();
        match input.phase {
            KeyPhase::Down if !input.repeat => {
                next.held_letters.push_fresh(key);
            }
            KeyPhase::Up => {
                next.held_letters.release(key);
                next.reset_activation_sequence_if_released();
            }
            KeyPhase::Down => {}
        }
        DecisionPlan::same(next, false)
    }

    fn take_activation_context(&mut self) -> Option<ActivationContext> {
        let generation = self.next_activation_generation?;
        self.next_activation_generation = generation.checked_next();
        Some(ActivationContext::target_unavailable(generation))
    }

    fn reset_activation_sequence_if_released(&mut self) {
        if self.held_letters.as_slice().is_empty() {
            self.activation_sequence_modifiers = None;
            self.activation_sequence_fenced = false;
        }
    }
}
