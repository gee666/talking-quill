use super::{ParseRequest, Request, RequestId, RpcError, RpcResponse};
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use serde_json::value::RawValue;
use std::fmt;

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

pub(super) fn parse_request(payload: &[u8]) -> ParseRequest {
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
