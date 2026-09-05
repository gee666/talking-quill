//! Capture and health event payloads and validation.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKey {
    Escape,
    Enter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasteCommitState {
    Committed,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalDegradedReason {
    NativeFault,
    OwnershipUnknown,
    CallbackDelivery,
    Protocol,
}

result_struct!(ActivationEvent {
    capture_lease_epoch: U64String,
    owner_instance_id: Bytes32,
    profile_id: ProfileId,
    shortcut: BindingShortcut,
    activation_generation: U64String,
    #[serde(deserialize_with = "deserialize_required_option")]
    target_token: Option<WireToken>,
    phase: Phase,
    #[serde(deserialize_with = "deserialize_required_option")]
    held_ms: Option<U64String>
});
result_struct!(SessionKeyEvent {
    capture_lease_epoch: U64String,
    key: SessionKey,
    phase: Phase
});
result_struct!(RegisteredObservationEvent {
    capture_lease_epoch: U64String,
    generation: U64String
});
result_struct!(PasteCommittedEvent {
    capture_lease_epoch: U64String,
    operation_id: Bytes32,
    state: PasteCommitState
});
result_struct!(AudioDevicesChangedEvent {
    capture_lease_epoch: U64String
});
result_struct!(TerminalDegradedEvent {
    reason: TerminalDegradedReason
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Activation(ActivationEvent),
    SessionKey(SessionKeyEvent),
    RegisteredObservation(RegisteredObservationEvent),
    PasteCommitted(PasteCommittedEvent),
    AudioDevicesChanged(AudioDevicesChangedEvent),
    HealthChanged(HealthResult),
    TerminalDegraded(TerminalDegradedEvent),
}

impl Event {
    #[must_use]
    pub const fn allowed_for(&self, purpose: Purpose) -> bool {
        match self {
            Self::Activation(_)
            | Self::SessionKey(_)
            | Self::RegisteredObservation(_)
            | Self::PasteCommitted(_)
            | Self::AudioDevicesChanged(_) => matches!(purpose, Purpose::Capture),
            Self::HealthChanged(_) | Self::TerminalDegraded(_) => true,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        #[derive(Serialize)]
        struct Wire<'a, T> {
            event: &'static str,
            params: &'a T,
        }
        fn encode<T: Serialize>(event: &'static str, params: &T) -> Result<Vec<u8>, SchemaError> {
            serde_json::to_vec(&Wire { event, params }).map_err(|_| SchemaError::Json)
        }
        let bytes = match self {
            Self::Activation(value) => encode("activation", value),
            Self::SessionKey(value) => encode("session_key", value),
            Self::RegisteredObservation(value) => encode("registered_observation", value),
            Self::PasteCommitted(value) => encode("paste_committed", value),
            Self::AudioDevicesChanged(value) => encode("audio_devices_changed", value),
            Self::HealthChanged(value) => encode("health_changed", value),
            Self::TerminalDegraded(value) => encode("terminal_degraded", value),
        }?;
        parse_event_json(&bytes)?;
        Ok(bytes)
    }
}

pub fn parse_event_json(bytes: &[u8]) -> Result<Event, SchemaError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        event: String,
        params: Box<RawValue>,
    }
    let wire: Wire = strict_json(bytes)?;
    let raw = wire.params.get().as_bytes();
    let event = match wire.event.as_str() {
        "activation" => Event::Activation(strict_json(raw)?),
        "session_key" => Event::SessionKey(strict_json(raw)?),
        "registered_observation" => Event::RegisteredObservation(strict_json(raw)?),
        "paste_committed" => Event::PasteCommitted(strict_json(raw)?),
        "audio_devices_changed" => Event::AudioDevicesChanged(strict_json(raw)?),
        "health_changed" => Event::HealthChanged(strict_json(raw)?),
        "terminal_degraded" => Event::TerminalDegraded(strict_json(raw)?),
        _ => return Err(SchemaError::UnknownMessage),
    };
    if let Event::Activation(ActivationEvent { phase, held_ms, .. }) = &event
        && *phase == Phase::Down
        && held_ms.is_some()
    {
        return Err(SchemaError::InvalidEvent);
    }
    Ok(event)
}
