use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    InstalledUnobserved,
    PhysicalObserved,
    PermissionRequired,
    Unavailable,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub accessibility: PermissionState,
    pub input_monitoring: PermissionState,
    pub event_post: PermissionState,
}

impl Permissions {
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            accessibility: PermissionState::Unknown,
            input_monitoring: PermissionState::Unknown,
            event_post: PermissionState::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontApp {
    pub process_name: String,
    pub window_title: String,
    pub window_bounds: Option<WindowBounds>,
}

const MAX_FRONT_APP_FIELD_ESCAPED_BYTES: usize = 7 * 1024;

impl FrontApp {
    pub(crate) fn bounded(self) -> Self {
        Self {
            process_name: bound_json_string(self.process_name),
            window_title: bound_json_string(self.window_title),
            window_bounds: self.window_bounds,
        }
    }
}

fn bound_json_string(mut value: String) -> String {
    let mut escaped_bytes = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let character_bytes = if character <= '\u{001f}' {
            6
        } else if character == '"' || character == '\\' {
            2
        } else {
            character.len_utf8()
        };
        if escaped_bytes + character_bytes > MAX_FRONT_APP_FIELD_ESCAPED_BYTES {
            break;
        }
        escaped_bytes += character_bytes;
        end = index + character.len_utf8();
    }
    value.truncate(end);
    value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteFailure {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    OsRejected,
    Unavailable,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteResult {
    pub submitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PasteFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardTextHash([u8; 32]);

impl ClipboardTextHash {
    pub(crate) fn from_lower_hex(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = decode_lower_hex(pair[0])?;
            let low = decode_lower_hex(pair[1])?;
            bytes[index] = (high << 4) | low;
        }
        Some(Self(bytes))
    }

    #[must_use]
    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

const fn decode_lower_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PlatformError {
    #[error("keyboard owner is unavailable")]
    OwnerUnavailable,
    #[error("keyboard owner authentication failed")]
    OwnerAuthentication,
    #[error("keyboard owner is incompatible")]
    OwnerIncompatible,
    #[error("keyboard owner is busy")]
    OwnerBusy,
    #[error("keyboard owner singleton collision")]
    OwnerSingletonCollision,
    #[error("keyboard owner is draining")]
    OwnerDraining,
    #[error("keyboard owner rollback is latched")]
    OwnerRollback,
    #[error("keyboard owner reported a security fault")]
    OwnerSecurityFault,
    #[error("owner operation result is indeterminate")]
    Indeterminate,
    #[error("native operation failed")]
    NativeFailure,
}
