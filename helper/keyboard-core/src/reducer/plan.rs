//! Delivery-dependent plans and frozen native target attachment.
use super::*;

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

impl DecisionPlan {
    pub(super) fn unchanged(state: &KeyboardReducer, swallow: bool) -> Self {
        Self::same(state.snapshot(), swallow)
    }

    pub(super) fn same(state: KeyboardReducer, swallow: bool) -> Self {
        Self {
            delivered_state: state.snapshot(),
            failed_state: state,
            event: None,
            swallow_if_delivered: swallow,
            swallow_if_failed: swallow,
        }
    }
}
