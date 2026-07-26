use super::{
    ActivationBindings, ActivationKey, EventPhase, HelperEvent, KeyInput, KeyPhase, PhysicalKey,
    SessionKey,
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
    key: ActivationKey,
    shift: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivationInputMode {
    NativeKey,
    PassiveHook,
    RegisteredHotKey,
}

/// Pure keyboard state used by both native callbacks.
///
/// A callback first calls [`KeyboardReducer::plan`], attempts the optional
/// nonblocking event delivery, and then calls [`KeyboardReducer::apply`]. The
/// failure branch passes the current physical key sequence through, preventing
/// later repeats or key-up from being captured after its first key-down escaped.
///
/// Alt/Option and optional Shift are activation context, not captured
/// sequences. Their native down/up events always pass through so unrelated
/// modifier input is never swallowed and does not require risky synthetic
/// reinjection. Ctrl/Control, Command, or either Windows key disallow a new
/// activation but likewise pass through. Only an explicitly enabled configured
/// letter's down, repeats, and matching up are swallowed after a successfully
/// delivered activation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyboardReducer {
    active_activation: Option<ActiveActivation>,
    activation_passthrough: [bool; 26],
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
    #[must_use]
    pub fn plan(
        &self,
        input: KeyInput,
        activation_key: ActivationKey,
        activation_enabled: bool,
        session_capture: bool,
    ) -> DecisionPlan {
        self.plan_with_activation_mode(
            input,
            activation_key,
            activation_enabled,
            session_capture,
            ActivationInputMode::NativeKey,
        )
    }

    #[must_use]
    pub fn plan_bindings(
        &self,
        input: KeyInput,
        bindings: ActivationBindings,
        activation_enabled: bool,
        session_capture: bool,
    ) -> DecisionPlan {
        let (configured, enabled) = match input.key {
            PhysicalKey::Letter(key) => (
                key,
                activation_enabled && bindings.contains(key, input.shift),
            ),
            _ => (ActivationKey::DEFAULT, activation_enabled),
        };
        self.plan(input, configured, enabled, session_capture)
    }

    /// Plans a Windows low-level-hook event. Letter down/repeat events are
    /// observation-only because RegisterHotKey owns full-chord down blocking;
    /// a matching up for an established activation is still balanced here.
    #[must_use]
    pub fn plan_passive_hook(
        &self,
        input: KeyInput,
        activation_key: ActivationKey,
        activation_enabled: bool,
        session_capture: bool,
    ) -> DecisionPlan {
        self.plan_with_activation_mode(
            input,
            activation_key,
            activation_enabled,
            session_capture,
            ActivationInputMode::PassiveHook,
        )
    }

    #[must_use]
    pub fn plan_passive_bindings(
        &self,
        input: KeyInput,
        bindings: ActivationBindings,
        activation_enabled: bool,
        session_capture: bool,
    ) -> DecisionPlan {
        let configured = match input.key {
            PhysicalKey::Letter(key) => key,
            _ => ActivationKey::DEFAULT,
        };
        self.plan_with_activation_mode(
            input,
            configured,
            activation_enabled && matches!(input.key, PhysicalKey::Letter(key) if bindings.contains(key, input.shift)),
            session_capture,
            ActivationInputMode::PassiveHook,
        )
    }

    /// Plans one validated, nonrepeating Windows WM_HOTKEY activation down.
    #[must_use]
    pub fn plan_registered_hotkey(
        &self,
        key: ActivationKey,
        shift: bool,
        activation_key: ActivationKey,
        activation_enabled: bool,
    ) -> DecisionPlan {
        self.plan_with_activation_mode(
            KeyInput {
                key: PhysicalKey::Letter(key),
                phase: KeyPhase::Down,
                alt: true,
                shift,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            activation_key,
            activation_enabled,
            false,
            ActivationInputMode::RegisteredHotKey,
        )
    }

    #[must_use]
    pub fn plan_registered_binding(
        &self,
        key: ActivationKey,
        shift: bool,
        bindings: ActivationBindings,
        activation_enabled: bool,
    ) -> DecisionPlan {
        self.plan_registered_hotkey(
            key,
            shift,
            key,
            activation_enabled && bindings.contains(key, shift),
        )
    }

    fn plan_with_activation_mode(
        &self,
        input: KeyInput,
        activation_key: ActivationKey,
        activation_enabled: bool,
        session_capture: bool,
        activation_mode: ActivationInputMode,
    ) -> DecisionPlan {
        if input.injected {
            return DecisionPlan::unchanged(self, false);
        }

        match input.key {
            PhysicalKey::Letter(key) => self.plan_letter(
                input,
                key,
                activation_key,
                activation_enabled,
                activation_mode,
            ),
            PhysicalKey::Escape => self.plan_control(input, SessionKey::Escape, session_capture),
            PhysicalKey::Enter => self.plan_control(input, SessionKey::Enter, session_capture),
            PhysicalKey::Other => DecisionPlan::unchanged(self, false),
        }
    }

    /// Abandons every captured native sequence while returning synthetic
    /// balancing notifications for downs that were already delivered. Native
    /// backends use this when a policy transaction makes the current event pass
    /// through before normal reducer planning can run.
    pub fn fail_open_balancing_events(&mut self) -> [Option<HelperEvent>; 3] {
        let activation = self
            .active_activation
            .map(|active| HelperEvent::Activation {
                key: active.key,
                phase: EventPhase::Up,
                shift: active.shift,
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
    /// be swallowed. `delivered` is ignored when the plan has no notification.
    /// A failed initial down remains fail-open, while the matching up of an
    /// already delivered/swallowed down remains swallowed to balance the
    /// current physical sequence before later input fails open.
    pub fn apply(&mut self, plan: DecisionPlan, delivered: bool) -> bool {
        if plan.event.is_none() || delivered {
            *self = plan.delivered_state;
            plan.swallow_if_delivered
        } else {
            *self = plan.failed_state;
            plan.swallow_if_failed
        }
    }

    fn plan_letter(
        &self,
        input: KeyInput,
        key: ActivationKey,
        configured: ActivationKey,
        activation_enabled: bool,
        activation_mode: ActivationInputMode,
    ) -> DecisionPlan {
        if activation_mode == ActivationInputMode::PassiveHook && input.phase == KeyPhase::Down {
            return DecisionPlan::unchanged(self, false);
        }

        let key_index = usize::from(key.index());
        if self.activation_passthrough[key_index] {
            let mut next = self.clone();
            if input.phase == KeyPhase::Up {
                next.activation_passthrough[key_index] = false;
            }
            return DecisionPlan::same(next, false);
        }

        if let Some(active) = self.active_activation {
            if active.key != key {
                return self.pass_letter_sequence(input, key);
            }

            // Once captured, the configured letter's complete physical
            // sequence remains captured even if configuration or modifiers
            // change before its matching up.
            if input.phase == KeyPhase::Down {
                return DecisionPlan::unchanged(self, true);
            }

            let mut next = self.clone();
            next.active_activation = None;
            return DecisionPlan {
                delivered_state: next.clone(),
                failed_state: next,
                event: Some(HelperEvent::Activation {
                    key: active.key,
                    phase: EventPhase::Up,
                    shift: active.shift,
                }),
                swallow_if_delivered: true,
                // This down was already delivered and swallowed. Balance that
                // physical sequence even if only its up notification fails;
                // the callback makes all subsequent input fail open.
                swallow_if_failed: true,
            };
        }

        if input.repeat
            || key != configured
            || input.phase != KeyPhase::Down
            || !activation_enabled
            || !input.alt
            || input.disallowed_modifiers
        {
            return self.pass_letter_sequence(input, key);
        }

        let shift = input.shift;
        let mut delivered = self.clone();
        delivered.active_activation = Some(ActiveActivation { key, shift });
        let mut failed = self.clone();
        failed.activation_passthrough[key_index] = true;

        DecisionPlan {
            delivered_state: delivered,
            failed_state: failed,
            event: Some(HelperEvent::Activation {
                key,
                phase: EventPhase::Down,
                shift,
            }),
            swallow_if_delivered: true,
            swallow_if_failed: false,
        }
    }

    fn pass_letter_sequence(&self, input: KeyInput, key: ActivationKey) -> DecisionPlan {
        let mut next = self.clone();
        next.activation_passthrough[usize::from(key.index())] = input.phase == KeyPhase::Down;
        DecisionPlan::same(next, false)
    }

    fn plan_control(
        &self,
        input: KeyInput,
        key: SessionKey,
        session_capture: bool,
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
                    // The matching down was already delivered and swallowed.
                    // Swallow this up even when its notification fails.
                    swallow_if_failed: true,
                }
            }
            SequenceState::Idle => {
                if input.repeat || !session_capture || input.phase != KeyPhase::Down {
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
