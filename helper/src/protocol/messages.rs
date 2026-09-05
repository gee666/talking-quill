use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use talking_quill_keyboard_core::KeyboardEvent;

mod encoding;
mod parsing;
#[cfg(test)]
mod tests;

#[cfg(not(feature = "windows-installed-acceptance"))]
pub const INBOUND_METHODS: [&str; 11] = BASE_INBOUND_METHODS;

#[cfg(feature = "windows-installed-acceptance")]
pub const INBOUND_METHODS: [&str; 13] = [
    "initialize",
    "activation.configure",
    "session.set_capture",
    "paste.inject",
    "front_app.get",
    "permissions.get",
    "runtime.observability",
    "acceptance.endpoint_observability",
    "acceptance.pause_lease_renewal",
    "ping",
    "owner.prepare_maintenance",
    "diagnostic.ack",
    "shutdown",
];

#[cfg(not(feature = "windows-installed-acceptance"))]
const BASE_INBOUND_METHODS: [&str; 11] = [
    "initialize",
    "activation.configure",
    "session.set_capture",
    "paste.inject",
    "front_app.get",
    "permissions.get",
    "runtime.observability",
    "ping",
    "owner.prepare_maintenance",
    "diagnostic.ack",
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
    parsing::parse_request(payload)
}

#[derive(Debug)]
pub enum Outbound {
    Response(RpcResponse),
    Event(KeyboardEvent),
    RegisteredObservation(u64),
    PasteCommitted(RequestId),
    InputDevicesChanged,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboundEncodingError {
    #[error("outbound JSON serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("outbound JSON payload is too large: {0} bytes")]
    FrameTooLarge(usize),
}

pub fn encode_outbound(message: &Outbound) -> Result<Vec<u8>, OutboundEncodingError> {
    encoding::encode_outbound(message)
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
    pub const fn owner_authentication() -> Self {
        Self {
            code: -32_005,
            message: "Keyboard owner authentication failed",
        }
    }
    pub const fn owner_incompatible() -> Self {
        Self {
            code: -32_006,
            message: "Keyboard owner incompatible",
        }
    }
    pub const fn owner_busy() -> Self {
        Self {
            code: -32_007,
            message: "Keyboard owner busy",
        }
    }
    pub const fn owner_draining() -> Self {
        Self {
            code: -32_008,
            message: "Keyboard owner draining",
        }
    }
    pub const fn owner_rollback() -> Self {
        Self {
            code: -32_009,
            message: "Keyboard owner rollback latched",
        }
    }
    pub const fn owner_security_fault() -> Self {
        Self {
            code: -32_010,
            message: "Keyboard owner security fault",
        }
    }
    pub const fn indeterminate() -> Self {
        Self {
            code: -32_011,
            message: "Operation result indeterminate",
        }
    }
    pub const fn owner_singleton_collision() -> Self {
        Self {
            code: -32_012,
            message: "Keyboard owner singleton collision",
        }
    }
}
