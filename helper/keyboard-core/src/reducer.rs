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

use activation::HeldLetters;

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
                self.plan_session_input(input, SessionKey::Escape, session_capture)
            }
            PhysicalKey::Enter => {
                self.plan_session_input(input, SessionKey::Enter, session_capture)
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
}

mod activation;
mod plan;
mod session;
