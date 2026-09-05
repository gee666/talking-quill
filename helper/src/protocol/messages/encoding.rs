use super::{Outbound, OutboundEncodingError, RequestId, RpcResponse};
use crate::framing::MAX_FRAME_BYTES;
use serde::Serialize;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, EventPhase, KeyboardEvent, SessionKey,
};

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum SerializableOutbound<'a> {
    Response(&'a RpcResponse),
    Notification(RpcNotification<'a>),
}

#[derive(Debug, Serialize)]
struct RpcNotification<'a> {
    jsonrpc: &'static str,
    method: &'static str,
    params: NotificationParams<'a>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum NotificationParams<'a> {
    Activation(ActivationEventParams),
    ActivationComplete(ActivationCompleteParams),
    Session(SessionKeyEventParams<'a>),
    RegisteredObservation(RegisteredObservationParams),
    PasteCommitted(PasteCommittedParams<'a>),
    InputDevicesChanged(EmptyNotificationParams),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteCommittedParams<'a> {
    request_id: &'a RequestId,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct EmptyNotificationParams {}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisteredObservationParams {
    generation: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ActivationEventParams {
    phase: EventPhase,
    #[serde(flatten)]
    binding: ActivationBinding,
    #[serde(flatten)]
    context: ActivationContext,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivationCompleteParams {
    phase: &'static str,
    #[serde(flatten)]
    binding: ActivationBinding,
    #[serde(flatten)]
    context: ActivationContext,
    held_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionKeyEventParams<'a> {
    key: &'a SessionKey,
    phase: EventPhase,
}

pub(super) fn encode_outbound(message: &Outbound) -> Result<Vec<u8>, OutboundEncodingError> {
    let serializable = match message {
        Outbound::Response(response) => SerializableOutbound::Response(response),
        Outbound::Event(KeyboardEvent::Activation {
            binding,
            context,
            phase,
        }) => SerializableOutbound::Notification(RpcNotification {
            jsonrpc: "2.0",
            method: "activation.event",
            params: NotificationParams::Activation(ActivationEventParams {
                phase: *phase,
                binding: *binding,
                context: *context,
            }),
        }),
        Outbound::Event(KeyboardEvent::ActivationComplete {
            binding,
            context,
            held_ms,
        }) => SerializableOutbound::Notification(RpcNotification {
            jsonrpc: "2.0",
            method: "activation.event",
            params: NotificationParams::ActivationComplete(ActivationCompleteParams {
                phase: "complete",
                binding: *binding,
                context: *context,
                held_ms: *held_ms,
            }),
        }),
        Outbound::Event(KeyboardEvent::SessionKey { key, phase }) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "session.key",
                params: NotificationParams::Session(SessionKeyEventParams { key, phase: *phase }),
            })
        }
        Outbound::RegisteredObservation(generation) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "registered_input.observed",
                params: NotificationParams::RegisteredObservation(RegisteredObservationParams {
                    generation: *generation,
                }),
            })
        }
        Outbound::PasteCommitted(request_id) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "paste.committed",
                params: NotificationParams::PasteCommitted(PasteCommittedParams { request_id }),
            })
        }
        Outbound::InputDevicesChanged => SerializableOutbound::Notification(RpcNotification {
            jsonrpc: "2.0",
            method: "audio.input_devices_changed",
            params: NotificationParams::InputDevicesChanged(EmptyNotificationParams {}),
        }),
    };
    let payload = serde_json::to_vec(&serializable)?;
    if payload.len() > MAX_FRAME_BYTES {
        Err(OutboundEncodingError::FrameTooLarge(payload.len()))
    } else {
        Ok(payload)
    }
}
