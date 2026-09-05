//! Independent Escape/Enter capture transitions.
use super::*;

impl KeyboardReducer {
    pub(super) fn plan_session_input(
        &self,
        input: KeyInput,
        key: SessionKey,
        session_capture: SessionCaptureMode,
    ) -> DecisionPlan {
        let mut observed = self.snapshot();
        if input.phase == KeyPhase::Down || self.control_state(key) != SequenceState::Idle {
            observed.observe_modifiers(input.modifiers);
        }
        observed.plan_control(input, key, session_capture)
    }

    fn control_state(&self, key: SessionKey) -> SequenceState {
        match key {
            SessionKey::Escape => self.escape,
            SessionKey::Enter => self.enter,
        }
    }

    fn plan_control(
        &self,
        input: KeyInput,
        key: SessionKey,
        session_capture: SessionCaptureMode,
    ) -> DecisionPlan {
        match self.control_state(key) {
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

    fn set_control_state(&mut self, key: SessionKey, state: SequenceState) {
        match key {
            SessionKey::Escape => self.escape = state,
            SessionKey::Enter => self.enter = state,
        }
    }
}
