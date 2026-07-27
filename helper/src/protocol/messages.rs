use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, Visitor},
};
use serde_json::{Value, value::RawValue};

use crate::{
    framing::MAX_FRAME_BYTES,
    keyboard::{ActivationBinding, EventPhase, HelperEvent, SessionKey},
};

pub const INBOUND_METHODS: [&str; 8] = [
    "initialize",
    "activation.configure",
    "session.set_capture",
    "paste.inject",
    "front_app.get",
    "permissions.get",
    "ping",
    "shutdown",
];

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_STRING_ID_BYTES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

impl RequestId {
    fn is_valid(&self) -> bool {
        match self {
            Self::Number(value) => *value <= MAX_SAFE_INTEGER,
            Self::String(value) => !value.is_empty() && value.len() <= MAX_STRING_ID_BYTES,
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(value: u64) -> Self {
        Self::Number(value)
    }
}

/// Strict JSON-RPC 2.0 request ID. Commands require an ID; an absent ID marks
/// an otherwise valid envelope as a notification. Explicit `null` is invalid.
#[derive(Debug, Default)]
enum IdField {
    #[default]
    Missing,
    Null,
    Value(RequestId),
}

impl<'de> Deserialize<'de> for IdField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor;

        impl<'de> Visitor<'de> for IdVisitor {
            type Value = IdField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-null string or nonnegative safe-integer request ID")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(IdField::Null)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(IdField::Null)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(IdField::Value(RequestId::Number(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u64::try_from(value)
                    .map(RequestId::Number)
                    .map(IdField::Value)
                    .map_err(|_| E::custom("request ID must be nonnegative"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(IdField::Value(RequestId::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(IdField::Value(RequestId::String(value)))
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
}

/// Typed second-pass envelope. Required field types, unknown fields, and
/// duplicate fields are rejected before notification classification.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    jsonrpc: String,
    #[serde(default)]
    id: IdField,
    method: String,
    params: Box<RawValue>,
}

#[derive(Debug)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    pub params: Box<RawValue>,
}

#[derive(Debug)]
pub enum ParseRequest {
    Request(Request),
    IgnoreNotification,
    Error(RpcResponse),
}

pub fn parse_request(payload: &[u8]) -> ParseRequest {
    // Establish that the payload is exactly one syntactically valid JSON value
    // before typed decoding can stop early on a schema error.
    let raw = match serde_json::from_slice::<Box<RawValue>>(payload) {
        Ok(raw) => raw,
        Err(_) => {
            return ParseRequest::Error(RpcResponse::error(None, RpcError::parse_error()));
        }
    };

    match serde_json::from_str::<RequestEnvelope>(raw.get()) {
        Ok(envelope) => {
            // A missing ID is a notification only after every other envelope
            // invariant, including object-shaped params, has passed.
            let id_is_valid = match &envelope.id {
                IdField::Missing => true,
                IdField::Null => false,
                IdField::Value(id) => id.is_valid(),
            };
            if envelope.jsonrpc != "2.0"
                || !id_is_valid
                || envelope.method.is_empty()
                || envelope.method.len() > 64
                || !raw_value_is_object(&envelope.params)
            {
                return ParseRequest::Error(RpcResponse::error(None, RpcError::invalid_request()));
            }

            let id = match envelope.id {
                IdField::Missing => return ParseRequest::IgnoreNotification,
                IdField::Null => unreachable!("null ID rejected above"),
                IdField::Value(id) => id,
            };
            ParseRequest::Request(Request {
                id,
                method: envelope.method,
                params: envelope.params,
            })
        }
        Err(_) => ParseRequest::Error(RpcResponse::error(None, RpcError::invalid_request())),
    }
}

fn raw_value_is_object(value: &RawValue) -> bool {
    value
        .get()
        .as_bytes()
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(b'{')
}

#[derive(Debug)]
pub enum Outbound {
    Response(RpcResponse),
    Event(HelperEvent),
    PasteCommitted(RequestId),
}

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
    PasteCommitted(PasteCommittedParams<'a>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteCommittedParams<'a> {
    request_id: &'a RequestId,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ActivationEventParams {
    phase: EventPhase,
    #[serde(flatten)]
    binding: ActivationBinding,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivationCompleteParams {
    phase: &'static str,
    #[serde(flatten)]
    binding: ActivationBinding,
    held_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionKeyEventParams<'a> {
    key: &'a SessionKey,
    phase: EventPhase,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboundEncodingError {
    #[error("outbound JSON serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("outbound JSON payload is too large: {0} bytes")]
    FrameTooLarge(usize),
}

pub fn encode_outbound(message: &Outbound) -> Result<Vec<u8>, OutboundEncodingError> {
    let serializable = match message {
        Outbound::Response(response) => SerializableOutbound::Response(response),
        Outbound::Event(HelperEvent::Activation { binding, phase }) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "activation.event",
                params: NotificationParams::Activation(ActivationEventParams {
                    phase: *phase,
                    binding: *binding,
                }),
            })
        }
        Outbound::Event(HelperEvent::ActivationComplete { binding, held_ms }) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "activation.event",
                params: NotificationParams::ActivationComplete(ActivationCompleteParams {
                    phase: "complete",
                    binding: *binding,
                    held_ms: *held_ms,
                }),
            })
        }
        Outbound::Event(HelperEvent::SessionKey { key, phase }) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "session.key",
                params: NotificationParams::Session(SessionKeyEventParams { key, phase: *phase }),
            })
        }
        Outbound::PasteCommitted(request_id) => {
            SerializableOutbound::Notification(RpcNotification {
                jsonrpc: "2.0",
                method: "paste.committed",
                params: NotificationParams::PasteCommitted(PasteCommittedParams { request_id }),
            })
        }
    };
    let payload = serde_json::to_vec(&serializable)?;
    if payload.len() > MAX_FRAME_BYTES {
        Err(OutboundEncodingError::FrameTooLarge(payload.len()))
    } else {
        Ok(payload)
    }
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    jsonrpc: &'static str,
    id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

impl RpcResponse {
    pub fn success<T: Serialize>(id: RequestId, result: T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            jsonrpc: "2.0",
            id: Some(id),
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub const fn error(id: Option<RequestId>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct RpcError {
    code: i32,
    message: &'static str,
}

impl RpcError {
    pub const fn parse_error() -> Self {
        Self {
            code: -32_700,
            message: "Parse error",
        }
    }

    pub const fn invalid_request() -> Self {
        Self {
            code: -32_600,
            message: "Invalid Request",
        }
    }

    pub const fn method_not_found() -> Self {
        Self {
            code: -32_601,
            message: "Method not found",
        }
    }

    pub const fn invalid_params() -> Self {
        Self {
            code: -32_602,
            message: "Invalid params",
        }
    }

    pub const fn internal_error() -> Self {
        Self {
            code: -32_603,
            message: "Internal error",
        }
    }

    pub const fn incompatible_protocol() -> Self {
        Self {
            code: -32_001,
            message: "Incompatible protocol version",
        }
    }

    pub const fn invalid_state() -> Self {
        Self {
            code: -32_002,
            message: "Invalid helper state",
        }
    }

    pub const fn native_unavailable() -> Self {
        Self {
            code: -32_003,
            message: "Native operation unavailable",
        }
    }

    pub const fn response_too_large() -> Self {
        Self {
            code: -32_004,
            message: "Response too large",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::{ActivationKey, ProfileId};
    use crate::platform::FrontApp;

    fn string_response(length: usize) -> Outbound {
        Outbound::Response(
            RpcResponse::success(RequestId::for_test(1), "a".repeat(length)).unwrap(),
        )
    }

    #[test]
    fn outbound_encoding_accepts_exact_max_and_rejects_max_plus_one() {
        let overhead = encode_outbound(&string_response(0)).unwrap().len();
        let exact = encode_outbound(&string_response(MAX_FRAME_BYTES - overhead)).unwrap();
        assert_eq!(exact.len(), MAX_FRAME_BYTES);
        assert!(matches!(
            encode_outbound(&string_response(MAX_FRAME_BYTES - overhead + 1)),
            Err(OutboundEncodingError::FrameTooLarge(size)) if size == MAX_FRAME_BYTES + 1
        ));
    }

    #[test]
    fn worst_case_front_app_escaping_stays_inside_one_frame() {
        let front_app = FrontApp {
            process_name: "\u{0001}".repeat(10_000),
            window_title: "\u{0001}".repeat(10_000),
            window_bounds: None,
        }
        .bounded();
        let outbound = Outbound::Response(
            RpcResponse::success(RequestId::String("\u{0001}".repeat(64)), front_app).unwrap(),
        );
        let payload = encode_outbound(&outbound).unwrap();
        assert!(payload.len() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn every_keyboard_notification_is_frame_bounded() {
        for event in [
            HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    crate::keyboard::Shortcut::legacy_alt_letter(ActivationKey::Z, false),
                ),
                phase: EventPhase::Down,
            },
            HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::PROMPT,
                    crate::keyboard::Shortcut::legacy_alt_letter(ActivationKey::Z, true),
                ),
                phase: EventPhase::Up,
            },
            HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            },
            HelperEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            },
        ] {
            assert!(encode_outbound(&Outbound::Event(event)).is_ok());
        }
    }
}
