use std::fmt;

use super::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, ActivationKey,
    EventPhase, KeyInput, KeyPhase, KeyboardEvent, ModifierMask, NativeTargetToken, PhysicalKey,
    ProfileId, SessionCaptureMode, SessionKey, Shortcut,
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
    context: ActivationContext,
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
/// equal one configured shortcut. Any exact binding with longer descendants
/// remains pending and emits one atomic completion when its trigger is released.
/// After successful down delivery for every unambiguous
/// chord, only that trigger's down/repeats/up are swallowed; its up emits the
/// accepted shortcut snapshot even if modifiers, prefixes, or configuration
/// changed meanwhile.
///
/// A callback first calls [`KeyboardReducer::plan`], attempts optional
/// nonblocking delivery, and then calls [`KeyboardReducer::apply`]. Failed
/// initial delivery is fail-open. Escape/Enter session capture retains its
/// independent down/up behavior.
#[derive(Eq, PartialEq)]
pub struct KeyboardReducer {
    active_activation: Option<ActiveActivation>,
    pending_activation: Option<PendingActivation>,
    next_activation_generation: Option<ActivationGeneration>,
    held_letters: HeldLetters,
    modifiers: ModifierMask,
    activation_sequence_modifiers: Option<ModifierMask>,
    activation_sequence_fenced: bool,
    escape: SequenceState,
    enter: SequenceState,
}

pub struct DecisionPlan {
    delivered_state: KeyboardReducer,
    failed_state: KeyboardReducer,
    event: Option<KeyboardEvent>,
    swallow_if_delivered: bool,
    swallow_if_failed: bool,
}

impl fmt::Debug for KeyboardReducer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyboardReducer(<redacted>)")
    }
}

impl fmt::Debug for DecisionPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecisionPlan(<redacted>)")
    }
}

impl DecisionPlan {
    #[must_use]
    pub const fn event(&self) -> Option<KeyboardEvent> {
        self.event
    }

    /// Attaches a freshly captured native target token to an initial activation
    /// plan. Later platform packages call this before attempting notification
    /// delivery. Matching up events retain the same frozen context automatically.
    ///
    /// Returns false for non-activation plans and for an already-active up.
    pub fn attach_native_target(&mut self, target_token: NativeTargetToken) -> bool {
        let Some(event) = self.event else {
            return false;
        };
        match event {
            KeyboardEvent::Activation {
                binding,
                context,
                phase: EventPhase::Down,
            } if context.target_token().is_none() => {
                let context = context.with_target_token(target_token);
                self.event = Some(KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: EventPhase::Down,
                });
                if let Some(active) = self.delivered_state.active_activation.as_mut()
                    && active.context.activation_generation() == context.activation_generation()
                {
                    active.context = context;
                }
                true
            }
            KeyboardEvent::ActivationComplete {
                binding,
                context,
                held_ms,
            } if context.target_token().is_none() => {
                self.event = Some(KeyboardEvent::ActivationComplete {
                    binding,
                    context: context.with_target_token(target_token),
                    held_ms,
                });
                true
            }
            _ => false,
        }
    }
}

impl Default for KeyboardReducer {
    fn default() -> Self {
        Self {
            active_activation: None,
            pending_activation: None,
            next_activation_generation: Some(ActivationGeneration::FIRST),
            held_letters: HeldLetters::default(),
            modifiers: ModifierMask::default(),
            activation_sequence_modifiers: None,
            activation_sequence_fenced: false,
            escape: SequenceState::Idle,
            enter: SequenceState::Idle,
        }
    }
}

impl KeyboardReducer {
    /// Copies reducer state for plan/apply branching without exposing a public
    /// `Clone` implementation that could duplicate one process owner's
    /// activation-generation stream.
    fn snapshot(&self) -> Self {
        Self {
            active_activation: self.active_activation,
            pending_activation: self.pending_activation,
            next_activation_generation: self.next_activation_generation,
            held_letters: self.held_letters.clone(),
            modifiers: self.modifiers,
            activation_sequence_modifiers: self.activation_sequence_modifiers,
            activation_sequence_fenced: self.activation_sequence_fenced,
            escape: self.escape,
            enter: self.enter,
        }
    }

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
                let mut observed = self.snapshot();
                observed.observe_modifiers(input.modifiers);
                observed.plan_letter(input, key, bindings, activation_enabled, observed_at_ms)
            }
            PhysicalKey::Escape => {
                let mut observed = self.snapshot();
                if input.phase == KeyPhase::Down || self.escape != SequenceState::Idle {
                    observed.observe_modifiers(input.modifiers);
                }
                observed.plan_control(input, SessionKey::Escape, session_capture)
            }
            PhysicalKey::Enter => {
                let mut observed = self.snapshot();
                if input.phase == KeyPhase::Down || self.enter != SequenceState::Idle {
                    observed.observe_modifiers(input.modifiers);
                }
                observed.plan_control(input, SessionKey::Enter, session_capture)
            }
            PhysicalKey::Other => DecisionPlan::unchanged(self, false),
        }
    }

    /// Resets protocol/UI sequence state while returning synthetic balancing
    /// notifications for downs already delivered. This does not prove a native
    /// physical release: adapters that suppressed Escape/Enter downs retain
    /// their independent native-up ownership until a matching up or explicit
    /// HID-gap plus ordered-barrier proof.
    pub fn fail_open_balancing_events(&mut self) -> [Option<KeyboardEvent>; 3] {
        let activation = self
            .active_activation
            .map(|active| KeyboardEvent::Activation {
                binding: active.binding,
                context: active.context,
                phase: EventPhase::Up,
            });
        let escape =
            (self.escape == SequenceState::Suppressed).then_some(KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            });
        let enter =
            (self.enter == SequenceState::Suppressed).then_some(KeyboardEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            });
        let next_activation_generation = self.next_activation_generation;
        *self = Self::default();
        self.next_activation_generation = next_activation_generation;
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

    /// Records an exact modifier transition. Any combined-mask change while passive letters
    /// are held fences that physical sequence and cancels pending completion until every letter
    /// is released. Accepted activations remain intact solely for balancing up.
    pub fn observe_modifiers(&mut self, modifiers: ModifierMask) {
        if !self.held_letters.as_slice().is_empty()
            && self.activation_sequence_modifiers != Some(modifiers)
        {
            self.activation_sequence_fenced = true;
            self.pending_activation = None;
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

    /// Clears only session-key ownership whose physical up was proven hidden by
    /// a native callback gap. Still-held downs remain owned so their future ups
    /// stay suppressed. The returned flags identify the cleared keys for a
    /// platform's short queued-edge tombstone window.
    pub fn reconcile_hidden_session_releases(
        &mut self,
        escape_is_down: bool,
        enter_is_down: bool,
    ) -> (bool, bool) {
        let escape_cleared = self.escape == SequenceState::Suppressed && !escape_is_down;
        let enter_cleared = self.enter == SequenceState::Suppressed && !enter_is_down;
        if escape_cleared {
            self.escape = SequenceState::Idle;
        }
        if enter_cleared {
            self.enter = SequenceState::Idle;
        }
        (escape_cleared, enter_cleared)
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
                let mut next = self.snapshot();
                if input.phase == KeyPhase::Up {
                    next.set_control_state(key, SequenceState::Idle);
                }
                DecisionPlan::same(next, false)
            }
            SequenceState::Suppressed => {
                if input.phase == KeyPhase::Down {
                    return DecisionPlan::unchanged(self, true);
                }

                let mut next = self.snapshot();
                next.set_control_state(key, SequenceState::Idle);
                DecisionPlan {
                    delivered_state: next.snapshot(),
                    failed_state: next,
                    event: Some(KeyboardEvent::SessionKey {
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

                let mut delivered = self.snapshot();
                delivered.set_control_state(key, SequenceState::Suppressed);
                let mut failed = self.snapshot();
                failed.set_control_state(key, SequenceState::PassThrough);
                DecisionPlan {
                    delivered_state: delivered,
                    failed_state: failed,
                    event: Some(KeyboardEvent::SessionKey {
                        key,
                        phase: EventPhase::Down,
                    }),
                    swallow_if_delivered: true,
                    swallow_if_failed: false,
                }
            }
        }
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

    fn set_control_state(&mut self, key: SessionKey, state: SequenceState) {
        match key {
            SessionKey::Escape => self.escape = state,
            SessionKey::Enter => self.enter = state,
        }
    }
}

impl DecisionPlan {
    fn unchanged(state: &KeyboardReducer, swallow: bool) -> Self {
        Self::same(state.snapshot(), swallow)
    }

    fn same(state: KeyboardReducer, swallow: bool) -> Self {
        Self {
            delivered_state: state.snapshot(),
            failed_state: state,
            event: None,
            swallow_if_delivered: swallow,
            swallow_if_failed: swallow,
        }
    }
}
