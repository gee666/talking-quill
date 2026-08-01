use super::{
    ActivationBinding, ActivationBindings, ActivationKey, EventPhase, HelperEvent, KeyInput,
    KeyPhase, ModifierMask, PhysicalKey, ProfileId, SessionCaptureMode, SessionKey, Shortcut,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SequenceState {
    #[default]
    Idle,
    Suppressed,
    PassThrough,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveActivation {
    binding: ActivationBinding,
    trigger: ActivationKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingActivation {
    binding: ActivationBinding,
    started_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeldLetters {
    keys: [ActivationKey; Shortcut::MAX_KEYS],
    count: u8,
}

impl HeldLetters {
    fn as_slice(&self) -> &[ActivationKey] {
        &self.keys[..usize::from(self.count)]
    }

    fn contains(&self, key: ActivationKey) -> bool {
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

/// Pure keyboard state used by native callbacks and platform-neutral tests.
///
/// Fresh A-Z downs are retained in physical order while the keys remain held.
/// Prefix letters always pass through. A fresh final-key down is accepted only
/// when the exact four-modifier mask and complete ordered held-key sequence
/// equal one configured shortcut. The canonical one-key General prefix remains
/// pending while a longer built-in chord can complete and emits one atomic
/// completion on release. After successful down delivery for every unambiguous
/// chord, only that trigger's down/repeats/up are swallowed; its up emits the
/// accepted shortcut snapshot even if modifiers, prefixes, or configuration
/// changed meanwhile.
///
/// A callback first calls [`KeyboardReducer::plan`], attempts optional
/// nonblocking delivery, and then calls [`KeyboardReducer::apply`]. Failed
/// initial delivery is fail-open. Escape/Enter session capture retains its
/// independent down/up behavior.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyboardReducer {
    active_activation: Option<ActiveActivation>,
    pending_activation: Option<PendingActivation>,
    held_letters: HeldLetters,
    modifiers: ModifierMask,
    activation_sequence_modifiers: Option<ModifierMask>,
    activation_sequence_fenced: bool,
    escape: SequenceState,
    enter: SequenceState,
}

#[derive(Clone, Debug)]
pub struct DecisionPlan {
    delivered_state: KeyboardReducer,
    failed_state: KeyboardReducer,
    event: Option<HelperEvent>,
    swallow_if_delivered: bool,
    swallow_if_failed: bool,
}

impl DecisionPlan {
    #[must_use]
    pub const fn event(&self) -> Option<HelperEvent> {
        self.event
    }
}

impl KeyboardReducer {
    /// Compatibility entry point for the current one-letter native paths.
    #[must_use]
    pub fn plan(
        &self,
        input: KeyInput,
        activation_key: ActivationKey,
        activation_enabled: bool,
        session_capture: SessionCaptureMode,
    ) -> DecisionPlan {
        let shortcut = Shortcut::legacy_alt_letter(activation_key, input.modifiers.shift());
        let bindings =
            ActivationBindings::new(&[ActivationBinding::new(ProfileId::GENERAL, shortcut)])
                .expect("one shortcut is bounded");
        self.plan_bindings(input, bindings, activation_enabled, session_capture)
    }

    #[must_use]
    pub fn plan_bindings(
        &self,
        input: KeyInput,
        bindings: ActivationBindings,
        activation_enabled: bool,
        session_capture: SessionCaptureMode,
    ) -> DecisionPlan {
        self.plan_bindings_at(input, bindings, activation_enabled, session_capture, 0)
    }

    #[must_use]
    pub fn plan_bindings_at(
        &self,
        input: KeyInput,
        bindings: ActivationBindings,
        activation_enabled: bool,
        session_capture: SessionCaptureMode,
        observed_at_ms: u64,
    ) -> DecisionPlan {
        self.plan_input(
            input,
            bindings,
            activation_enabled,
            session_capture,
            observed_at_ms,
        )
    }

    fn plan_input(
        &self,
        input: KeyInput,
        bindings: ActivationBindings,
        activation_enabled: bool,
        session_capture: SessionCaptureMode,
        observed_at_ms: u64,
    ) -> DecisionPlan {
        if input.injected {
            return DecisionPlan::unchanged(self, false);
        }

        match input.key {
            PhysicalKey::Letter(key) => {
                let active_trigger = self
                    .active_activation
                    .is_some_and(|active| active.trigger == key);
                if input.phase == KeyPhase::Up
                    && !active_trigger
                    && !self.held_letters.contains(key)
                {
                    return DecisionPlan::unchanged(self, false);
                }
                let mut observed = self.clone();
                observed.observe_modifiers(input.modifiers);
                observed.plan_letter(input, key, bindings, activation_enabled, observed_at_ms)
            }
            PhysicalKey::Escape => {
                let mut observed = self.clone();
                if input.phase == KeyPhase::Down || self.escape != SequenceState::Idle {
                    observed.observe_modifiers(input.modifiers);
                }
                observed.plan_control(input, SessionKey::Escape, session_capture)
            }
            PhysicalKey::Enter => {
                let mut observed = self.clone();
                if input.phase == KeyPhase::Down || self.enter != SequenceState::Idle {
                    observed.observe_modifiers(input.modifiers);
                }
                observed.plan_control(input, SessionKey::Enter, session_capture)
            }
            PhysicalKey::Other => DecisionPlan::unchanged(self, false),
        }
    }

    /// Abandons every captured sequence while returning synthetic balancing
    /// notifications for downs that were already delivered.
    pub fn fail_open_balancing_events(&mut self) -> [Option<HelperEvent>; 3] {
        let activation = self
            .active_activation
            .map(|active| HelperEvent::Activation {
                binding: active.binding,
                phase: EventPhase::Up,
            });
        let escape =
            (self.escape == SequenceState::Suppressed).then_some(HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            });
        let enter = (self.enter == SequenceState::Suppressed).then_some(HelperEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Up,
        });
        *self = Self::default();
        [activation, escape, enter]
    }

    /// Applies a planned transition and returns whether the native event must
    /// be swallowed. A failed initial down passes through. A matching up for an
    /// already delivered down remains swallowed even if its delivery fails.
    pub fn apply(&mut self, plan: DecisionPlan, delivered: bool) -> bool {
        if plan.event.is_none() || delivered {
            *self = plan.delivered_state;
            plan.swallow_if_delivered
        } else {
            *self = plan.failed_state;
            plan.swallow_if_failed
        }
    }

    #[must_use]
    pub fn held_letters(&self) -> &[ActivationKey] {
        self.held_letters.as_slice()
    }

    #[must_use]
    pub const fn modifiers(&self) -> ModifierMask {
        self.modifiers
    }

    /// Records an exact modifier transition. Any change while passive letters
    /// are held fences that physical sequence until all of those letters are
    /// released. A pending General may survive release of its configured
    /// Alt/Option modifier, but adding or re-adding any modifier cancels it.
    /// Accepted activations remain intact solely for balancing up.
    pub fn observe_modifiers(&mut self, modifiers: ModifierMask) {
        let added_modifier = (modifiers.ctrl() && !self.modifiers.ctrl())
            || (modifiers.alt() && !self.modifiers.alt())
            || (modifiers.shift() && !self.modifiers.shift())
            || (modifiers.meta() && !self.modifiers.meta());
        if !self.held_letters.as_slice().is_empty() {
            if self.activation_sequence_modifiers != Some(modifiers) {
                self.activation_sequence_fenced = true;
            }
            if added_modifier {
                self.pending_activation = None;
            }
        }
        self.modifiers = modifiers;
    }

    /// Fences passive letters across an activation binding revision without
    /// disturbing an accepted activation snapshot.
    pub fn fence_activation_revision(&mut self) {
        if !self.held_letters.as_slice().is_empty() {
            self.activation_sequence_fenced = true;
            self.pending_activation = None;
        }
    }

    /// Returns whether any sequence has an initial down that was already
    /// delivered and suppressed.
    #[must_use]
    pub fn has_captured_sequence(&self) -> bool {
        self.active_activation.is_some()
            || self.escape == SequenceState::Suppressed
            || self.enter == SequenceState::Suppressed
    }

    /// Returns whether this physical key belongs to a sequence whose initial
    /// down was already delivered and suppressed. Native hooks use this only to
    /// finish balancing repeats/ups after their callback gate closes.
    #[must_use]
    pub fn is_capturing(&self, key: PhysicalKey) -> bool {
        match key {
            PhysicalKey::Letter(letter) => self
                .active_activation
                .is_some_and(|active| active.trigger == letter),
            PhysicalKey::Escape => self.escape == SequenceState::Suppressed,
            PhysicalKey::Enter => self.enter == SequenceState::Suppressed,
            PhysicalKey::Other => false,
        }
    }

    fn plan_letter(
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

                let mut next = self.clone();
                next.active_activation = None;
                next.held_letters.release(key);
                next.reset_activation_sequence_if_released();
                return DecisionPlan {
                    delivered_state: next.clone(),
                    failed_state: next,
                    event: Some(HelperEvent::Activation {
                        binding: active.binding,
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
                let Some(pending) = self
                    .pending_activation
                    .filter(|pending| pending.binding.shortcut().trigger() == key)
                else {
                    return self.pass_letter(input, key);
                };
                let mut next = self.clone();
                next.pending_activation = None;
                next.held_letters.release(key);
                next.reset_activation_sequence_if_released();
                DecisionPlan {
                    delivered_state: next.clone(),
                    failed_state: next,
                    event: Some(HelperEvent::ActivationComplete {
                        binding: pending.binding,
                        held_ms: observed_at_ms.saturating_sub(pending.started_at_ms),
                    }),
                    // The pending prefix down passed through, so its up must also pass through.
                    swallow_if_delivered: false,
                    swallow_if_failed: false,
                }
            }
            KeyPhase::Down if input.repeat => DecisionPlan::unchanged(self, false),
            KeyPhase::Down => {
                let mut next = self.clone();
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
                let accepted = (!next.activation_sequence_fenced && activation_enabled)
                    .then(|| bindings.find_exact(input.modifiers, next.held_letters.as_slice()));
                let Some(binding) = accepted
                    .flatten()
                    .filter(|binding| binding.shortcut().trigger() == key)
                else {
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
                let mut delivered = next.clone();
                delivered.active_activation = Some(ActiveActivation {
                    binding,
                    trigger: key,
                });
                DecisionPlan {
                    delivered_state: delivered,
                    failed_state: next,
                    event: Some(HelperEvent::Activation {
                        binding,
                        phase: EventPhase::Down,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: false,
                }
            }
        }
    }

    fn pass_letter(&self, input: KeyInput, key: ActivationKey) -> DecisionPlan {
        let mut next = self.clone();
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

    fn plan_control(
        &self,
        input: KeyInput,
        key: SessionKey,
        session_capture: SessionCaptureMode,
    ) -> DecisionPlan {
        let state = match key {
            SessionKey::Escape => self.escape,
            SessionKey::Enter => self.enter,
        };

        match state {
            SequenceState::PassThrough => {
                let mut next = self.clone();
                if input.phase == KeyPhase::Up {
                    next.set_control_state(key, SequenceState::Idle);
                }
                DecisionPlan::same(next, false)
            }
            SequenceState::Suppressed => {
                if input.phase == KeyPhase::Down {
                    return DecisionPlan::unchanged(self, true);
                }

                let mut next = self.clone();
                next.set_control_state(key, SequenceState::Idle);
                DecisionPlan {
                    delivered_state: next.clone(),
                    failed_state: next,
                    event: Some(HelperEvent::SessionKey {
                        key,
                        phase: EventPhase::Up,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: true,
                }
            }
            SequenceState::Idle => {
                if input.repeat || !session_capture.allows(key) || input.phase != KeyPhase::Down {
                    return DecisionPlan::unchanged(self, false);
                }

                let mut delivered = self.clone();
                delivered.set_control_state(key, SequenceState::Suppressed);
                let mut failed = self.clone();
                failed.set_control_state(key, SequenceState::PassThrough);
                DecisionPlan {
                    delivered_state: delivered,
                    failed_state: failed,
                    event: Some(HelperEvent::SessionKey {
                        key,
                        phase: EventPhase::Down,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: false,
                }
            }
        }
    }

    fn reset_activation_sequence_if_released(&mut self) {
        if self.held_letters.as_slice().is_empty() {
            self.activation_sequence_modifiers = None;
            self.activation_sequence_fenced = false;
        }
    }

    fn set_control_state(&mut self, key: SessionKey, state: SequenceState) {
        match key {
            SessionKey::Escape => self.escape = state,
            SessionKey::Enter => self.enter = state,
        }
    }
}

impl DecisionPlan {
    fn unchanged(state: &KeyboardReducer, swallow: bool) -> Self {
        Self::same(state.clone(), swallow)
    }

    fn same(state: KeyboardReducer, swallow: bool) -> Self {
        Self {
            delivered_state: state.clone(),
            failed_state: state,
            event: None,
            swallow_if_delivered: swallow,
            swallow_if_failed: swallow,
        }
    }
}
