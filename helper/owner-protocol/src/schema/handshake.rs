//! Strict handshake discrimination and authenticated finish payloads.
use super::*;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authenticate {
    #[serde(rename = "type")]
    kind: AuthenticateTag,
    #[serde(rename = "clientProof")]
    pub client_proof: Bytes32,
}

impl Authenticate {
    #[must_use]
    pub const fn new(client_proof: Bytes32) -> Self {
        Self {
            kind: AuthenticateTag::Authenticate,
            client_proof,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_handshake_json(&bytes)?;
        Ok(bytes)
    }
}

impl fmt::Debug for Authenticate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Authenticate([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AuthenticateTag {
    Authenticate,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authenticated {
    #[serde(rename = "type")]
    kind: AuthenticatedTag,
    #[serde(rename = "selectedProtocol")]
    pub selected_protocol: SelectedProtocol,
    pub purpose: Purpose,
    #[serde(rename = "authorityCeiling")]
    pub authority_ceiling: AuthorityCeiling,
    #[serde(rename = "ownerProof")]
    pub owner_proof: Bytes32,
}

impl Authenticated {
    pub fn new(
        selected_protocol: SelectedProtocol,
        purpose: Purpose,
        authority_ceiling: AuthorityCeiling,
        owner_proof: Bytes32,
    ) -> Result<Self, SchemaError> {
        let value = Self {
            kind: AuthenticatedTag::Authenticated,
            selected_protocol,
            purpose,
            authority_ceiling,
            owner_proof,
        };
        value.to_json()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        match parse_handshake_json(&bytes)? {
            HandshakeMessage::Authenticated(_) => Ok(bytes),
            _ => Err(SchemaError::UnknownMessage),
        }
    }
}

impl fmt::Debug for Authenticated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Authenticated([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AuthenticatedTag {
    Authenticated,
}

#[derive(Clone, PartialEq, Eq)]
pub enum HandshakeMessage {
    Hello(Hello),
    Challenge(Challenge),
    Authenticate(Authenticate),
    Authenticated(Authenticated),
}

impl fmt::Debug for HandshakeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandshakeMessage([REDACTED])")
    }
}

pub fn parse_handshake_json(bytes: &[u8]) -> Result<HandshakeMessage, SchemaError> {
    // Parse once solely to select an exact concrete schema. The concrete parse
    // rejects every unknown/duplicate field; source bytes never enter errors.
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| SchemaError::Json)?;
    let kind = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(serde_json::Value::as_str)
        .ok_or(SchemaError::Json)?;
    match kind {
        "hello" => {
            let value: Hello = strict_json(bytes)?;
            value.validate()?;
            Ok(HandshakeMessage::Hello(value))
        }
        "challenge" => {
            let value: Challenge = strict_json(bytes)?;
            value.validate()?;
            Ok(HandshakeMessage::Challenge(value))
        }
        "authenticate" => strict_json(bytes).map(HandshakeMessage::Authenticate),
        "authenticated" => {
            let value: Authenticated = strict_json(bytes)?;
            if value.selected_protocol.major != PROTOCOL_MAJOR
                || value.selected_protocol.compatibility_epoch != COMPATIBILITY_EPOCH
                || value.selected_protocol.feature_bits.get() & BASE_V1 == 0
                || purpose_ceiling(value.purpose) != value.authority_ceiling
            {
                return Err(SchemaError::AuthorityCeiling);
            }
            Ok(HandshakeMessage::Authenticated(value))
        }
        _ => Err(SchemaError::UnknownMessage),
    }
}

fn validate_platform_key(
    platform: Platform,
    public_key: Option<&P256PublicKey>,
) -> Result<(), SchemaError> {
    match (platform, public_key) {
        (Platform::Windows, Some(_)) | (Platform::Macos, None) => Ok(()),
        _ => Err(SchemaError::PlatformKey),
    }
}

const fn purpose_ceiling(purpose: Purpose) -> AuthorityCeiling {
    match purpose {
        Purpose::Observe => AuthorityCeiling::Observer,
        Purpose::Capture => AuthorityCeiling::Capture,
        Purpose::Maintenance => AuthorityCeiling::Maintenance,
    }
}

mod challenge;
mod hello;
pub use challenge::Challenge;
pub use hello::Hello;
