//! Capture lease generations, activation pairing, and pending paste events.
use super::*;

#[derive(Clone)]
struct ActiveActivationRoute {
    event: ActivationEvent,
}

pub(super) struct CaptureEventRoute {
    lease_epoch: u64,
    revoked: bool,
    generation_high_water: u64,
    observation_generation_high_water: u64,
    active_activation: Option<ActiveActivationRoute>,
    session_keys: u8,
    paste_operations: Vec<Bytes32>,
}

impl CaptureEventRoute {
    pub(super) fn new(lease_epoch: u64) -> Self {
        Self {
            lease_epoch,
            revoked: false,
            generation_high_water: 0,
            observation_generation_high_water: 0,
            active_activation: None,
            session_keys: 0,
            paste_operations: Vec::new(),
        }
    }

    pub(super) const fn is_revoked(&self) -> bool {
        self.revoked
    }

    pub(super) fn revoke(&mut self) {
        self.revoked = true;
    }

    pub(super) fn wait_for_paste(&mut self, operation: Bytes32) -> Result<(), SessionCodecError> {
        if self.paste_operations.len() >= crate::envelope::MAX_OUTSTANDING_REQUESTS {
            return Err(SessionCodecError::EventRoute);
        }
        self.paste_operations.push(operation);
        Ok(())
    }

    pub(super) fn clear_ephemeral(&mut self) {
        self.active_activation = None;
        self.session_keys = 0;
    }

    pub(super) fn accept_event(
        &mut self,
        event: &Event,
        owner_instance: Option<Bytes32>,
    ) -> Result<(), SessionCodecError> {
        if self.revoked {
            return Err(SessionCodecError::EventRoute);
        }
        match event {
            Event::Activation(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || owner_instance != Some(actual.owner_instance_id)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                let generation = actual.activation_generation.get();
                match (actual.phase, actual.held_ms, &self.active_activation) {
                    (Phase::Down, None, None) if generation > self.generation_high_water => {
                        self.generation_high_water = generation;
                        self.active_activation = Some(ActiveActivationRoute {
                            event: actual.clone(),
                        });
                    }
                    (Phase::Up, None, Some(expected))
                        if same_activation_route(&expected.event, actual) =>
                    {
                        self.active_activation = None;
                    }
                    (Phase::Up, Some(_), None) if generation > self.generation_high_water => {
                        self.generation_high_water = generation;
                    }
                    _ => return Err(SessionCodecError::EventRoute),
                }
            }
            Event::RegisteredObservation(actual) => {
                let generation = actual.generation.get();
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || generation <= self.observation_generation_high_water
                {
                    return Err(SessionCodecError::EventRoute);
                }
                self.observation_generation_high_water = generation;
            }
            Event::SessionKey(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch {
                    return Err(SessionCodecError::EventRoute);
                }
                let bit = match actual.key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                match actual.phase {
                    Phase::Down if self.session_keys & bit == 0 => self.session_keys |= bit,
                    Phase::Up if self.session_keys & bit != 0 => self.session_keys &= !bit,
                    _ => return Err(SessionCodecError::EventRoute),
                }
            }
            Event::PasteCommitted(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || !self
                        .paste_operations
                        .iter()
                        .any(|operation| operation == &actual.operation_id)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                self.paste_operations
                    .retain(|operation| operation != &actual.operation_id);
            }
            Event::AudioDevicesChanged(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch {
                    return Err(SessionCodecError::EventRoute);
                }
            }
            Event::HealthChanged(_) | Event::TerminalDegraded(_) => {}
        }
        Ok(())
    }
}

fn same_activation_route(expected: &ActivationEvent, actual: &ActivationEvent) -> bool {
    expected.capture_lease_epoch == actual.capture_lease_epoch
        && expected.owner_instance_id == actual.owner_instance_id
        && expected.profile_id == actual.profile_id
        && expected.shortcut == actual.shortcut
        && expected.activation_generation == actual.activation_generation
        && expected.target_token == actual.target_token
}

pub(super) const fn is_capture_scoped_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Activation(_)
            | Event::RegisteredObservation(_)
            | Event::SessionKey(_)
            | Event::PasteCommitted(_)
            | Event::AudioDevicesChanged(_)
    )
}
