use std::collections::HashSet;
use std::fmt;

use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use thiserror::Error;

use crate::release_policy::{PolicyBlob, PolicySignature};
use crate::scalar::{Bytes32, Counter, FeatureBits, P256PublicKey, U64String};
use crate::{BASE_V1, COMPATIBILITY_EPOCH, PROTOCOL_MAJOR};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Observe,
    Capture,
    Maintenance,
}

impl Purpose {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Observe => 1,
            Self::Capture => 2,
            Self::Maintenance => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorityCeiling {
    Observer,
    Capture,
    Maintenance,
}

impl AuthorityCeiling {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Observer => 1,
            Self::Capture => 2,
            Self::Maintenance => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
}

impl Platform {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Windows => 1,
            Self::Macos => 2,
        }
    }

    pub(crate) const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Windows),
            2 => Some(Self::Macos),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x64")]
    X64,
    #[serde(rename = "arm64")]
    Arm64,
}

impl Architecture {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::X64 => 1,
            Self::Arm64 => 2,
        }
    }

    pub(crate) const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::X64),
            2 => Some(Self::Arm64),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerReportedState {
    Starting,
    IdleNeutral,
    LeaseDisabled,
    LeaseEnabled,
    LeaseDraining,
    OrphanCancelling,
    OrphanDraining,
    MaintenanceDraining,
    DegradedDraining,
    MaintenanceReady,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Healthy,
    RollbackLatched,
    Degraded,
    StoppingNative,
    FlushingResponse,
    Exiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
    NotRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    Off,
    Recording,
    CancelOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolHeader {
    pub major: u16,
    pub minor: u16,
    #[serde(rename = "compatibilityEpoch")]
    pub compatibility_epoch: u32,
    #[serde(rename = "supportedFeatureBits")]
    pub supported_feature_bits: FeatureBits,
    #[serde(rename = "requiredFeatureBits")]
    pub required_feature_bits: FeatureBits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedProtocol {
    pub major: u16,
    pub minor: u16,
    #[serde(rename = "compatibilityEpoch")]
    pub compatibility_epoch: u32,
    #[serde(rename = "featureBits")]
    pub feature_bits: FeatureBits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ProtocolSelectionError {
    #[error("invalid owner-protocol header")]
    InvalidHeader,
    #[error("owner-protocol major mismatch")]
    MajorMismatch,
    #[error("owner-protocol compatibility epoch mismatch")]
    EpochMismatch,
    #[error("owner-protocol required features are unavailable")]
    MissingRequiredFeature,
    #[error("owner-protocol has unknown required features")]
    UnknownRequiredFeature,
}

impl ProtocolHeader {
    pub fn validate(self) -> Result<(), ProtocolSelectionError> {
        let supported = self.supported_feature_bits.get();
        let required = self.required_feature_bits.get();
        if self.major != PROTOCOL_MAJOR
            || self.compatibility_epoch != COMPATIBILITY_EPOCH
            || supported & BASE_V1 == 0
            || required & BASE_V1 == 0
            || required & !supported != 0
        {
            return Err(ProtocolSelectionError::InvalidHeader);
        }
        if required & !BASE_V1 != 0 {
            return Err(ProtocolSelectionError::UnknownRequiredFeature);
        }
        Ok(())
    }

    pub fn negotiate(self, owner: Self) -> Result<SelectedProtocol, ProtocolSelectionError> {
        self.validate()?;
        owner.validate()?;
        if self.major != owner.major {
            return Err(ProtocolSelectionError::MajorMismatch);
        }
        if self.compatibility_epoch != owner.compatibility_epoch {
            return Err(ProtocolSelectionError::EpochMismatch);
        }
        let selected = self.supported_feature_bits.get() & owner.supported_feature_bits.get();
        if selected & (self.required_feature_bits.get() | owner.required_feature_bits.get())
            != (self.required_feature_bits.get() | owner.required_feature_bits.get())
        {
            return Err(ProtocolSelectionError::MissingRequiredFeature);
        }
        Ok(SelectedProtocol {
            major: self.major,
            minor: self.minor.min(owner.minor),
            compatibility_epoch: self.compatibility_epoch,
            feature_bits: FeatureBits::new(selected),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    #[serde(rename = "type")]
    kind: HelloTag,
    pub purpose: Purpose,
    pub protocol: ProtocolHeader,
    #[serde(rename = "clientNonce")]
    pub client_nonce: Bytes32,
    pub platform: Platform,
    pub architecture: Architecture,
    #[serde(rename = "releaseBuildDigest")]
    pub release_build_digest: Bytes32,
    #[serde(rename = "executableSha256")]
    pub executable_sha256: Bytes32,
    #[serde(rename = "installationIdentityDigest")]
    pub installation_identity_digest: Bytes32,
    #[serde(rename = "signerPolicyDigest")]
    pub signer_policy_digest: Bytes32,
    #[serde(rename = "osSessionBindingDigest")]
    pub os_session_binding_digest: Bytes32,
    #[serde(rename = "clientReleasePolicy")]
    pub client_release_policy: PolicyBlob,
    #[serde(rename = "clientReleasePolicySignature")]
    pub client_release_policy_signature: PolicySignature,
    #[serde(rename = "clientReleasePolicyDigest")]
    pub client_release_policy_digest: Bytes32,
    #[serde(rename = "platformCredentialBindingDigest")]
    pub platform_credential_binding_digest: Bytes32,
    #[serde(
        rename = "clientEphemeralPublicKey",
        deserialize_with = "deserialize_required_option"
    )]
    pub client_ephemeral_public_key: Option<P256PublicKey>,
}

impl Hello {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        purpose: Purpose,
        protocol: ProtocolHeader,
        client_nonce: Bytes32,
        platform: Platform,
        architecture: Architecture,
        release_build_digest: Bytes32,
        executable_sha256: Bytes32,
        installation_identity_digest: Bytes32,
        signer_policy_digest: Bytes32,
        os_session_binding_digest: Bytes32,
        client_release_policy: PolicyBlob,
        client_release_policy_signature: PolicySignature,
        platform_credential_binding_digest: Bytes32,
        client_ephemeral_public_key: Option<P256PublicKey>,
    ) -> Result<Self, SchemaError> {
        let client_release_policy_digest = client_release_policy.digest();
        let value = Self {
            kind: HelloTag::Hello,
            purpose,
            protocol,
            client_nonce,
            platform,
            architecture,
            release_build_digest,
            executable_sha256,
            installation_identity_digest,
            signer_policy_digest,
            os_session_binding_digest,
            client_release_policy,
            client_release_policy_signature,
            client_release_policy_digest,
            platform_credential_binding_digest,
            client_ephemeral_public_key,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_handshake_json(&bytes)?;
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        self.protocol.validate()?;
        if !self
            .client_release_policy_digest
            .constant_time_eq(&self.client_release_policy.digest())
        {
            return Err(SchemaError::PolicyDigest);
        }
        validate_platform_key(self.platform, self.client_ephemeral_public_key.as_ref())
    }
}

impl fmt::Debug for Hello {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Hello([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HelloTag {
    Hello,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    #[serde(rename = "type")]
    kind: ChallengeTag,
    #[serde(rename = "ownerProtocol")]
    pub owner_protocol: ProtocolHeader,
    #[serde(rename = "selectedProtocol")]
    pub selected_protocol: SelectedProtocol,
    pub purpose: Purpose,
    #[serde(rename = "authorityCeiling")]
    pub authority_ceiling: AuthorityCeiling,
    #[serde(rename = "ownerNonce")]
    pub owner_nonce: Bytes32,
    #[serde(rename = "sessionId")]
    pub session_id: Bytes32,
    #[serde(rename = "ownerInstanceId")]
    pub owner_instance_id: Bytes32,
    pub platform: Platform,
    pub architecture: Architecture,
    #[serde(rename = "releaseBuildDigest")]
    pub release_build_digest: Bytes32,
    #[serde(rename = "executableSha256")]
    pub executable_sha256: Bytes32,
    #[serde(rename = "installationIdentityDigest")]
    pub installation_identity_digest: Bytes32,
    #[serde(rename = "signerPolicyDigest")]
    pub signer_policy_digest: Bytes32,
    #[serde(rename = "osSessionBindingDigest")]
    pub os_session_binding_digest: Bytes32,
    #[serde(rename = "ownerReleasePolicy")]
    pub owner_release_policy: PolicyBlob,
    #[serde(rename = "ownerReleasePolicySignature")]
    pub owner_release_policy_signature: PolicySignature,
    #[serde(rename = "ownerReleasePolicyDigest")]
    pub owner_release_policy_digest: Bytes32,
    #[serde(rename = "platformCredentialBindingDigest")]
    pub platform_credential_binding_digest: Bytes32,
    #[serde(
        rename = "ownerEphemeralPublicKey",
        deserialize_with = "deserialize_required_option"
    )]
    pub owner_ephemeral_public_key: Option<P256PublicKey>,
}

impl Challenge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_protocol: ProtocolHeader,
        selected_protocol: SelectedProtocol,
        purpose: Purpose,
        authority_ceiling: AuthorityCeiling,
        owner_nonce: Bytes32,
        session_id: Bytes32,
        owner_instance_id: Bytes32,
        platform: Platform,
        architecture: Architecture,
        release_build_digest: Bytes32,
        executable_sha256: Bytes32,
        installation_identity_digest: Bytes32,
        signer_policy_digest: Bytes32,
        os_session_binding_digest: Bytes32,
        owner_release_policy: PolicyBlob,
        owner_release_policy_signature: PolicySignature,
        platform_credential_binding_digest: Bytes32,
        owner_ephemeral_public_key: Option<P256PublicKey>,
    ) -> Result<Self, SchemaError> {
        let owner_release_policy_digest = owner_release_policy.digest();
        let value = Self {
            kind: ChallengeTag::Challenge,
            owner_protocol,
            selected_protocol,
            purpose,
            authority_ceiling,
            owner_nonce,
            session_id,
            owner_instance_id,
            platform,
            architecture,
            release_build_digest,
            executable_sha256,
            installation_identity_digest,
            signer_policy_digest,
            os_session_binding_digest,
            owner_release_policy,
            owner_release_policy_signature,
            owner_release_policy_digest,
            platform_credential_binding_digest,
            owner_ephemeral_public_key,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_handshake_json(&bytes)?;
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        self.owner_protocol.validate()?;
        if self.selected_protocol.major != self.owner_protocol.major
            || self.selected_protocol.compatibility_epoch != self.owner_protocol.compatibility_epoch
            || self.selected_protocol.minor > self.owner_protocol.minor
            || self.selected_protocol.feature_bits.get()
                & !self.owner_protocol.supported_feature_bits.get()
                != 0
            || self.selected_protocol.feature_bits.get()
                & self.owner_protocol.required_feature_bits.get()
                != self.owner_protocol.required_feature_bits.get()
        {
            return Err(SchemaError::Protocol(
                ProtocolSelectionError::MissingRequiredFeature,
            ));
        }
        if !self
            .owner_release_policy_digest
            .constant_time_eq(&self.owner_release_policy.digest())
        {
            return Err(SchemaError::PolicyDigest);
        }
        validate_platform_key(self.platform, self.owner_ephemeral_public_key.as_ref())?;
        if purpose_ceiling(self.purpose) != self.authority_ceiling {
            return Err(SchemaError::AuthorityCeiling);
        }
        Ok(())
    }
}

impl fmt::Debug for Challenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Challenge([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ChallengeTag {
    Challenge,
}

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

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BoundedText<const MAX: usize>(String);

impl<const MAX: usize> BoundedText<MAX> {
    pub fn new(value: String) -> Result<Self, SchemaError> {
        if value.is_empty() || value.len() > MAX {
            Err(SchemaError::Bounds)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MAX: usize> fmt::Debug for BoundedText<MAX> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BoundedText([REDACTED])")
    }
}

impl<const MAX: usize> Serialize for BoundedText<MAX> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedText<MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProfileId(String);

impl ProfileId {
    pub fn new(value: String) -> Result<Self, SchemaError> {
        let built_in = matches!(
            value.as_str(),
            "general" | "prompt" | "prompt-to-english" | "markdown" | "translate-to-english"
        );
        if built_in || valid_profile_uuid(value.as_bytes()) {
            Ok(Self(value))
        } else {
            Err(SchemaError::Binding)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProfileId([REDACTED])")
    }
}

impl Serialize for ProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

pub type WireToken = BoundedText<64>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Letter {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
}

impl Serialize for Letter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let byte = b'A' + *self as u8;
        serializer.serialize_str(std::str::from_utf8(&[byte]).expect("ASCII letter"))
    }
}

impl<'de> Deserialize<'de> for Letter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let [byte] = value.as_bytes() else {
            return Err(de::Error::custom(SchemaError::Binding));
        };
        if !byte.is_ascii_uppercase() {
            return Err(de::Error::custom(SchemaError::Binding));
        }
        const LETTERS: [Letter; 26] = [
            Letter::A,
            Letter::B,
            Letter::C,
            Letter::D,
            Letter::E,
            Letter::F,
            Letter::G,
            Letter::H,
            Letter::I,
            Letter::J,
            Letter::K,
            Letter::L,
            Letter::M,
            Letter::N,
            Letter::O,
            Letter::P,
            Letter::Q,
            Letter::R,
            Letter::S,
            Letter::T,
            Letter::U,
            Letter::V,
            Letter::W,
            Letter::X,
            Letter::Y,
            Letter::Z,
        ];
        Ok(LETTERS[usize::from(*byte - b'A')])
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifiers {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    #[must_use]
    pub const fn values(self) -> (bool, bool, bool, bool) {
        (self.ctrl, self.alt, self.shift, self.meta)
    }

    const fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Modifiers([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindingShortcut {
    modifiers: Modifiers,
    keys: Vec<Letter>,
}

impl BindingShortcut {
    pub fn new(modifiers: Modifiers, keys: Vec<Letter>) -> Result<Self, SchemaError> {
        if !modifiers.any()
            || keys.is_empty()
            || keys.len() > 26
            || keys.iter().copied().collect::<HashSet<_>>().len() != keys.len()
        {
            return Err(SchemaError::Binding);
        }
        Ok(Self { modifiers, keys })
    }

    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    #[must_use]
    pub fn keys(&self) -> &[Letter] {
        &self.keys
    }
}

impl fmt::Debug for BindingShortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BindingShortcut([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for BindingShortcut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            modifiers: Modifiers,
            keys: Vec<Letter>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.modifiers, wire.keys).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    #[serde(rename = "profileId")]
    profile_id: ProfileId,
    shortcut: BindingShortcut,
}

impl Binding {
    #[must_use]
    pub const fn new(profile_id: ProfileId, shortcut: BindingShortcut) -> Self {
        Self {
            profile_id,
            shortcut,
        }
    }

    #[must_use]
    pub const fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    #[must_use]
    pub const fn shortcut(&self) -> &BindingShortcut {
        &self.shortcut
    }
}

impl fmt::Debug for Binding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Binding([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Bindings(Vec<Binding>);

impl fmt::Debug for Bindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Bindings([REDACTED])")
    }
}

impl Bindings {
    pub fn new(bindings: Vec<Binding>) -> Result<Self, SchemaError> {
        if bindings.len() > 13
            || bindings.iter().any(|binding| {
                reserved_profile_owner(&binding.shortcut)
                    .is_some_and(|owner| owner != binding.profile_id.as_str())
            })
            || bindings
                .iter()
                .map(|binding| &binding.profile_id)
                .collect::<HashSet<_>>()
                .len()
                != bindings.len()
            || bindings
                .iter()
                .map(|binding| &binding.shortcut)
                .collect::<HashSet<_>>()
                .len()
                != bindings.len()
        {
            return Err(SchemaError::Binding);
        }
        Ok(Self(bindings))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Binding] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Bindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<Binding>::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

fn valid_profile_uuid(value: &[u8]) -> bool {
    if value.len() != 36 {
        return false;
    }
    for (index, byte) in value.iter().copied().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    let without_hyphens = || value.iter().copied().filter(|byte| *byte != b'-');
    let nil = without_hyphens().all(|byte| byte == b'0');
    let max = without_hyphens().all(|byte| byte == b'f');
    nil || max
        || (matches!(value[14].to_ascii_lowercase(), b'1'..=b'8')
            && matches!(value[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b'))
}

fn reserved_profile_owner(shortcut: &BindingShortcut) -> Option<&'static str> {
    if shortcut.modifiers.values() != (false, true, false, false) {
        return None;
    }
    match shortcut.keys.as_slice() {
        [Letter::X] => Some("general"),
        [Letter::X, Letter::P] => Some("prompt"),
        [Letter::X, Letter::Q] => Some("prompt-to-english"),
        [Letter::X, Letter::M] => Some("markdown"),
        [Letter::X, Letter::T] => Some("translate-to-english"),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    LeaseAcquire,
    MaintenanceAcquire,
    HealthGet,
    PermissionsGet,
    ObservabilityGet,
    FrontAppGet,
    FrontAppMetadataGet,
    LeaseRenew,
    SessionReconcileOff,
    SessionSetMode,
    CaptureReplaceConfiguration,
    CaptureSetEnabled,
    PasteInject,
    LeaseRelease,
    OwnerExitWhenNeutral,
    RuntimeRollback,
    MaintenanceRenew,
    MaintenancePrepare,
}

impl Method {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeaseAcquire => "lease.acquire",
            Self::MaintenanceAcquire => "maintenance.acquire",
            Self::HealthGet => "health.get",
            Self::PermissionsGet => "permissions.get",
            Self::ObservabilityGet => "observability.get",
            Self::FrontAppGet => "front_app.get",
            Self::FrontAppMetadataGet => "front_app.metadata.get",
            Self::LeaseRenew => "lease.renew",
            Self::SessionReconcileOff => "session.reconcile_off",
            Self::SessionSetMode => "session.set_mode",
            Self::CaptureReplaceConfiguration => "capture.replace_configuration",
            Self::CaptureSetEnabled => "capture.set_enabled",
            Self::PasteInject => "paste.inject",
            Self::LeaseRelease => "lease.release",
            Self::OwnerExitWhenNeutral => "owner.exit_when_neutral",
            Self::RuntimeRollback => "runtime.rollback",
            Self::MaintenanceRenew => "maintenance.renew",
            Self::MaintenancePrepare => "maintenance.prepare",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "lease.acquire" => Self::LeaseAcquire,
            "maintenance.acquire" => Self::MaintenanceAcquire,
            "health.get" => Self::HealthGet,
            "permissions.get" => Self::PermissionsGet,
            "observability.get" => Self::ObservabilityGet,
            "front_app.get" => Self::FrontAppGet,
            "front_app.metadata.get" => Self::FrontAppMetadataGet,
            "lease.renew" => Self::LeaseRenew,
            "session.reconcile_off" => Self::SessionReconcileOff,
            "session.set_mode" => Self::SessionSetMode,
            "capture.replace_configuration" => Self::CaptureReplaceConfiguration,
            "capture.set_enabled" => Self::CaptureSetEnabled,
            "paste.inject" => Self::PasteInject,
            "lease.release" => Self::LeaseRelease,
            "owner.exit_when_neutral" => Self::OwnerExitWhenNeutral,
            "runtime.rollback" => Self::RuntimeRollback,
            "maintenance.renew" => Self::MaintenanceRenew,
            "maintenance.prepare" => Self::MaintenancePrepare,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn is_capability_mutation(self) -> bool {
        !matches!(
            self,
            Self::LeaseAcquire
                | Self::MaintenanceAcquire
                | Self::HealthGet
                | Self::PermissionsGet
                | Self::ObservabilityGet
                | Self::FrontAppGet
                | Self::FrontAppMetadataGet
        )
    }

    #[must_use]
    pub const fn allowed_for(self, purpose: Purpose) -> bool {
        match purpose {
            Purpose::Observe => matches!(
                self,
                Self::HealthGet
                    | Self::PermissionsGet
                    | Self::ObservabilityGet
                    | Self::FrontAppGet
                    | Self::FrontAppMetadataGet
            ),
            Purpose::Capture => !matches!(
                self,
                Self::MaintenanceAcquire | Self::MaintenanceRenew | Self::MaintenancePrepare
            ),
            Purpose::Maintenance => matches!(
                self,
                Self::HealthGet
                    | Self::ObservabilityGet
                    | Self::MaintenanceAcquire
                    | Self::MaintenanceRenew
                    | Self::MaintenancePrepare
            ),
        }
    }
}

impl Serialize for Method {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

macro_rules! capture_params {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            pub capture_lease_id: Bytes32,
            pub capture_lease_epoch: U64String,
            pub command_sequence: U64String,
            $($(#[$meta])* pub $field: $type),*
        }
    };
}

macro_rules! maintenance_params {
    ($name:ident { $($field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            pub maintenance_capability_id: Bytes32,
            pub maintenance_capability_epoch: U64String,
            pub command_sequence: U64String,
            $(pub $field: $type),*
        }
    };
}

capture_params!(CaptureCommandParams {});
capture_params!(SessionSetModeParams { mode: SessionMode });
capture_params!(ReplaceConfigurationParams {
    revision: U64String,
    bindings: Bindings
});
capture_params!(SetEnabledParams { enabled: bool });
capture_params!(PasteInjectParams {
    operation_id: Bytes32,
    owner_instance_id: Bytes32,
    activation_generation: U64String,
    #[serde(deserialize_with = "deserialize_required_option")]
    target_token: Option<WireToken>,
    fallback_text_sha256: Bytes32
});
maintenance_params!(MaintenanceCommandParams {});
maintenance_params!(MaintenancePrepareParams {
    transaction_id: Bytes32,
    operation: MaintenanceOperation
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase", deny_unknown_fields)]
pub enum MaintenanceAcquireParams {
    Update {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
        #[serde(rename = "targetBuildDigest")]
        target_build_digest: Bytes32,
        #[serde(rename = "targetOwnerSha256")]
        target_owner_sha256: Bytes32,
    },
    Rollback {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
        #[serde(rename = "targetBuildDigest")]
        target_build_digest: Bytes32,
        #[serde(rename = "targetOwnerSha256")]
        target_owner_sha256: Bytes32,
    },
    Uninstall {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
    },
}

impl MaintenanceAcquireParams {
    #[must_use]
    pub const fn operation(&self) -> MaintenanceOperation {
        match self {
            Self::Update { .. } => MaintenanceOperation::Update,
            Self::Rollback { .. } => MaintenanceOperation::Rollback,
            Self::Uninstall { .. } => MaintenanceOperation::Uninstall,
        }
    }

    #[must_use]
    pub const fn transaction_id(&self) -> Bytes32 {
        match self {
            Self::Update { transaction_id, .. }
            | Self::Rollback { transaction_id, .. }
            | Self::Uninstall { transaction_id, .. } => *transaction_id,
        }
    }

    #[must_use]
    pub const fn source_build_digest(&self) -> Bytes32 {
        match self {
            Self::Update {
                source_build_digest,
                ..
            }
            | Self::Rollback {
                source_build_digest,
                ..
            }
            | Self::Uninstall {
                source_build_digest,
                ..
            } => *source_build_digest,
        }
    }

    #[must_use]
    pub const fn target_build_digest(&self) -> Option<Bytes32> {
        match self {
            Self::Update {
                target_build_digest,
                ..
            }
            | Self::Rollback {
                target_build_digest,
                ..
            } => Some(*target_build_digest),
            Self::Uninstall { .. } => None,
        }
    }

    #[must_use]
    pub const fn target_owner_sha256(&self) -> Option<Bytes32> {
        match self {
            Self::Update {
                target_owner_sha256,
                ..
            }
            | Self::Rollback {
                target_owner_sha256,
                ..
            } => Some(*target_owner_sha256),
            Self::Uninstall { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    LeaseAcquire(Empty),
    MaintenanceAcquire(MaintenanceAcquireParams),
    HealthGet(Empty),
    PermissionsGet(Empty),
    ObservabilityGet(Empty),
    FrontAppGet(Empty),
    FrontAppMetadataGet(Empty),
    LeaseRenew(CaptureCommandParams),
    SessionReconcileOff(CaptureCommandParams),
    SessionSetMode(SessionSetModeParams),
    CaptureReplaceConfiguration(ReplaceConfigurationParams),
    CaptureSetEnabled(SetEnabledParams),
    PasteInject(PasteInjectParams),
    LeaseRelease(CaptureCommandParams),
    OwnerExitWhenNeutral(CaptureCommandParams),
    RuntimeRollback(CaptureCommandParams),
    MaintenanceRenew(MaintenanceCommandParams),
    MaintenancePrepare(MaintenancePrepareParams),
}

impl Request {
    #[must_use]
    pub const fn method(&self) -> Method {
        match self {
            Self::LeaseAcquire(_) => Method::LeaseAcquire,
            Self::MaintenanceAcquire(_) => Method::MaintenanceAcquire,
            Self::HealthGet(_) => Method::HealthGet,
            Self::PermissionsGet(_) => Method::PermissionsGet,
            Self::ObservabilityGet(_) => Method::ObservabilityGet,
            Self::FrontAppGet(_) => Method::FrontAppGet,
            Self::FrontAppMetadataGet(_) => Method::FrontAppMetadataGet,
            Self::LeaseRenew(_) => Method::LeaseRenew,
            Self::SessionReconcileOff(_) => Method::SessionReconcileOff,
            Self::SessionSetMode(_) => Method::SessionSetMode,
            Self::CaptureReplaceConfiguration(_) => Method::CaptureReplaceConfiguration,
            Self::CaptureSetEnabled(_) => Method::CaptureSetEnabled,
            Self::PasteInject(_) => Method::PasteInject,
            Self::LeaseRelease(_) => Method::LeaseRelease,
            Self::OwnerExitWhenNeutral(_) => Method::OwnerExitWhenNeutral,
            Self::RuntimeRollback(_) => Method::RuntimeRollback,
            Self::MaintenanceRenew(_) => Method::MaintenanceRenew,
            Self::MaintenancePrepare(_) => Method::MaintenancePrepare,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        fn encode<T: Serialize>(method: Method, params: &T) -> Result<Vec<u8>, SchemaError> {
            #[derive(Serialize)]
            struct Wire<'a, T> {
                method: Method,
                params: &'a T,
            }
            serde_json::to_vec(&Wire { method, params }).map_err(|_| SchemaError::Json)
        }
        let encoded = match self {
            Self::LeaseAcquire(v) => encode(self.method(), v),
            Self::MaintenanceAcquire(v) => encode(self.method(), v),
            Self::HealthGet(v) => encode(self.method(), v),
            Self::PermissionsGet(v) => encode(self.method(), v),
            Self::ObservabilityGet(v) => encode(self.method(), v),
            Self::FrontAppGet(v) => encode(self.method(), v),
            Self::FrontAppMetadataGet(v) => encode(self.method(), v),
            Self::LeaseRenew(v) => encode(self.method(), v),
            Self::SessionReconcileOff(v) => encode(self.method(), v),
            Self::SessionSetMode(v) => encode(self.method(), v),
            Self::CaptureReplaceConfiguration(v) => encode(self.method(), v),
            Self::CaptureSetEnabled(v) => encode(self.method(), v),
            Self::PasteInject(v) => encode(self.method(), v),
            Self::LeaseRelease(v) => encode(self.method(), v),
            Self::OwnerExitWhenNeutral(v) => encode(self.method(), v),
            Self::RuntimeRollback(v) => encode(self.method(), v),
            Self::MaintenanceRenew(v) => encode(self.method(), v),
            Self::MaintenancePrepare(v) => encode(self.method(), v),
        }?;
        parse_request_json(&encoded)?;
        Ok(encoded)
    }
}

pub fn parse_request_json(bytes: &[u8]) -> Result<Request, SchemaError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        method: String,
        params: Box<RawValue>,
    }
    let wire: Wire = strict_json(bytes)?;
    let method = Method::parse(&wire.method).ok_or(SchemaError::UnknownMethod)?;
    macro_rules! parse {
        ($variant:ident, $type:ty) => {
            strict_json::<$type>(wire.params.get().as_bytes()).map(Request::$variant)
        };
    }
    match method {
        Method::LeaseAcquire => parse!(LeaseAcquire, Empty),
        Method::MaintenanceAcquire => parse!(MaintenanceAcquire, MaintenanceAcquireParams),
        Method::HealthGet => parse!(HealthGet, Empty),
        Method::PermissionsGet => parse!(PermissionsGet, Empty),
        Method::ObservabilityGet => parse!(ObservabilityGet, Empty),
        Method::FrontAppGet => parse!(FrontAppGet, Empty),
        Method::FrontAppMetadataGet => parse!(FrontAppMetadataGet, Empty),
        Method::LeaseRenew => parse!(LeaseRenew, CaptureCommandParams),
        Method::SessionReconcileOff => parse!(SessionReconcileOff, CaptureCommandParams),
        Method::SessionSetMode => parse!(SessionSetMode, SessionSetModeParams),
        Method::CaptureReplaceConfiguration => {
            parse!(CaptureReplaceConfiguration, ReplaceConfigurationParams)
        }
        Method::CaptureSetEnabled => parse!(CaptureSetEnabled, SetEnabledParams),
        Method::PasteInject => parse!(PasteInject, PasteInjectParams),
        Method::LeaseRelease => parse!(LeaseRelease, CaptureCommandParams),
        Method::OwnerExitWhenNeutral => parse!(OwnerExitWhenNeutral, CaptureCommandParams),
        Method::RuntimeRollback => parse!(RuntimeRollback, CaptureCommandParams),
        Method::MaintenanceRenew => parse!(MaintenanceRenew, MaintenanceCommandParams),
        Method::MaintenancePrepare => parse!(MaintenancePrepare, MaintenancePrepareParams),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Busy,
    Draining,
    Incompatible,
    Rollback,
    SecurityFault,
    InvalidState,
    NativeFailure,
    Indeterminate,
    Unavailable,
}

impl ErrorCode {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Busy => "owner busy",
            Self::Draining => "owner draining",
            Self::Incompatible => "incompatible owner",
            Self::Rollback => "rollback latched",
            Self::SecurityFault => "security fault",
            Self::InvalidState => "invalid owner state",
            Self::NativeFailure => "native operation failed",
            Self::Indeterminate => "operation indeterminate",
            Self::Unavailable => "owner unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    code: ErrorCode,
    message: &'static str,
}

impl ErrorBody {
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.message(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl<'de> Deserialize<'de> for ErrorBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: ErrorCode,
            message: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.message != wire.code.message() {
            return Err(de::Error::custom(SchemaError::ErrorMessage));
        }
        Ok(Self::new(wire.code))
    }
}

macro_rules! result_struct {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name { $($(#[$meta])* pub $field: $type),* }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcquireState {
    Disabled,
}
result_struct!(LeaseAcquireResult {
    capture_lease_id: Bytes32,
    capture_lease_epoch: U64String,
    state: AcquireState
});
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintenanceAcquireState {
    Sealed,
    Draining,
}
result_struct!(MaintenanceAcquireResult {
    maintenance_capability_id: Bytes32,
    maintenance_capability_epoch: U64String,
    state: MaintenanceAcquireState
});
result_struct!(RenewResult { renewed: bool });
result_struct!(SessionModeResult { mode: SessionMode });
result_struct!(ConfigurationResult {
    revision: U64String
});
result_struct!(EnabledResult { enabled: bool });
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseDisposition {
    Neutral,
    Draining,
}
result_struct!(ReleaseResult {
    disposition: LeaseDisposition
});
result_struct!(RollbackResult {
    latched: bool,
    disposition: LeaseDisposition
});
result_struct!(MaintenancePrepareResult {
    ready_to_exit: bool,
    owner_handoff: Bytes32
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteRefusalReason {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    TargetUnavailable,
    ClipboardChanged,
    NativeUnavailable,
    NativeRejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PasteResult {
    ClipboardOnly {
        reason: PasteRefusalReason,
    },
    Waiting {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
    Committed {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
    Indeterminate {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
}

result_struct!(HealthResult {
    owner_instance_id: Bytes32,
    reported_state: OwnerReportedState,
    process_state: ProcessState,
    rollback_latched: bool,
    native_state_unknown: bool,
    maintenance_sealed: bool,
    keyboard_build_eligible: bool,
    paste_ready: bool,
    permissions_eligible: bool,
    hook_healthy: bool
});
result_struct!(PermissionsResult {
    accessibility: PermissionState,
    input_monitoring: PermissionState,
    event_post: PermissionState
});
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FrontAppWindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

result_struct!(FrontAppResult {
    available: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    application_token: Option<WireToken>
});
result_struct!(FrontAppMetadataResult {
    available: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    process_name: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    window_title: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    window_bounds: Option<FrontAppWindowBounds>
});

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthFailures {
    pub cross_user: Counter,
    pub wrong_session: Counter,
    pub code_identity: Counter,
    pub mac: Counter,
    pub protocol: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OwnerCounters {
    pub starts: Counter,
    pub clean_exits: Counter,
    pub abnormal_exits: Counter,
    pub singleton_collisions: Counter,
    pub auth_attempts: Counter,
    pub auth_failures: AuthFailures,
    pub lease_acquired: Counter,
    pub lease_renewed: Counter,
    pub lease_expired: Counter,
    pub lease_disconnected: Counter,
    pub lease_released_neutral: Counter,
    pub lease_released_draining: Counter,
    pub drain_duration_ms_total: Counter,
    pub drain_duration_ms_max: Counter,
    pub maintenance_postponed: Counter,
    pub handoff_succeeded: Counter,
    pub handoff_failed: Counter,
    pub degraded: Counter,
    pub hook_recoveries: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancellationReasons {
    pub invalid_continuation: Counter,
    pub modifier_changed: Counter,
    pub alt_gr: Counter,
    pub journal_overflow: Counter,
    pub configuration_replaced: Counter,
    pub revision_mismatch: Counter,
    pub gate_closed: Counter,
    pub shutdown: Counter,
    pub helper_disconnected: Counter,
    pub secure_desktop: Counter,
    pub timeout: Counter,
    pub activation_delivery_failed: Counter,
    pub neutralization_failed: Counter,
    pub replay_failed: Counter,
    pub effect_protocol_violation: Counter,
    pub physical_state_mismatch: Counter,
    pub target_changed: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TransactionCounters {
    pub started: Counter,
    pub committed: Counter,
    pub replayed: Counter,
    pub cancelled: Counter,
    pub journal_high_water: Counter,
    pub cancellation_reasons: CancellationReasons,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectCounters {
    pub attempted: Counter,
    pub succeeded: Counter,
    pub partial: Counter,
    pub failed: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInputCounters {
    pub hook_installed: Counter,
    pub pump_alive: Counter,
    pub hc_action_callbacks: Counter,
    pub physical_callbacks: Counter,
    pub physical_callbacks_filtered: Counter,
    pub registered_candidate_callbacks: Counter,
    pub registered_match_callbacks: Counter,
    pub registered_release_callbacks: Counter,
    pub callback_channel_accepted: Counter,
    pub callback_channel_rejected: Counter,
    pub adapter_dequeued: Counter,
    pub owner_admitted: Counter,
    pub owner_flushed: Counter,
    pub owner_rejected: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativePasteCounters {
    pub target_validation_fallbacks: Counter,
    pub modifier_wait_duration_ms_total: Counter,
    pub modifier_wait_duration_ms_max: Counter,
    pub modifier_timeouts: Counter,
    pub shutdown_ownership_deadlines: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityResult {
    pub owner: OwnerCounters,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "registeredInput"
    )]
    pub registered_input: Option<RegisteredInputCounters>,
    pub transactions: TransactionCounters,
    pub replay: EffectCounters,
    pub dummy: EffectCounters,
    #[serde(rename = "nativePaste")]
    pub native_paste: NativePasteCounters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuccessResult {
    LeaseAcquire(LeaseAcquireResult),
    MaintenanceAcquire(MaintenanceAcquireResult),
    Health(HealthResult),
    Permissions(PermissionsResult),
    Observability(Box<ObservabilityResult>),
    FrontApp(FrontAppResult),
    FrontAppMetadata(FrontAppMetadataResult),
    Renew(RenewResult),
    SessionMode(SessionModeResult),
    Configuration(ConfigurationResult),
    Enabled(EnabledResult),
    Paste(PasteResult),
    Release(ReleaseResult),
    Rollback(RollbackResult),
    MaintenancePrepare(MaintenancePrepareResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Success(SuccessResult),
    Error(ErrorBody),
}

pub fn parse_response_json(method: Method, bytes: &[u8]) -> Result<Response, SchemaError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SuccessWire {
        ok: bool,
        result: Box<RawValue>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ErrorWire {
        ok: bool,
        error: ErrorBody,
    }
    // `RawValue` cannot be buffered through Serde's untagged-enum content
    // representation. Select only the boolean discriminator first, then parse
    // the exact deny-unknown-fields union arm from the original bytes. The
    // concrete parse still rejects duplicate `ok` and every extra field.
    let discriminator: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SchemaError::Json)?;
    let ok = discriminator
        .as_object()
        .and_then(|object| object.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .ok_or(SchemaError::ResponseUnion)?;
    let result = if ok {
        let SuccessWire { ok: true, result } = strict_json::<SuccessWire>(bytes)? else {
            return Err(SchemaError::ResponseUnion);
        };
        result
    } else {
        let ErrorWire { ok: false, error } = strict_json::<ErrorWire>(bytes)? else {
            return Err(SchemaError::ResponseUnion);
        };
        return Ok(Response::Error(error));
    };
    let raw_text = result.get();
    let raw = raw_text.as_bytes();
    let success = match method {
        Method::LeaseAcquire => SuccessResult::LeaseAcquire(strict_json(raw)?),
        Method::MaintenanceAcquire => SuccessResult::MaintenanceAcquire(strict_json(raw)?),
        Method::HealthGet => bounded_result(raw_text, 1024).map(SuccessResult::Health)?,
        Method::PermissionsGet => bounded_result(raw_text, 512).map(SuccessResult::Permissions)?,
        Method::ObservabilityGet => bounded_result(raw_text, 2048)
            .map(Box::new)
            .map(SuccessResult::Observability)?,
        Method::FrontAppGet => bounded_result(raw_text, 1024).map(SuccessResult::FrontApp)?,
        Method::FrontAppMetadataGet => {
            bounded_result(raw_text, 1024).map(SuccessResult::FrontAppMetadata)?
        }
        Method::LeaseRenew | Method::MaintenanceRenew => SuccessResult::Renew(strict_json(raw)?),
        Method::SessionReconcileOff | Method::SessionSetMode => {
            SuccessResult::SessionMode(strict_json(raw)?)
        }
        Method::CaptureReplaceConfiguration => SuccessResult::Configuration(strict_json(raw)?),
        Method::CaptureSetEnabled => SuccessResult::Enabled(strict_json(raw)?),
        Method::PasteInject => SuccessResult::Paste(strict_json(raw)?),
        Method::LeaseRelease | Method::OwnerExitWhenNeutral => {
            SuccessResult::Release(strict_json(raw)?)
        }
        Method::RuntimeRollback => SuccessResult::Rollback(strict_json(raw)?),
        Method::MaintenancePrepare => SuccessResult::MaintenancePrepare(strict_json(raw)?),
    };
    validate_success(&success)?;
    Ok(Response::Success(success))
}

impl Response {
    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum Wire<'a, T> {
            Success { ok: bool, result: &'a T },
            Error { ok: bool, error: &'a ErrorBody },
        }
        fn success<T: Serialize>(result: &T, maximum: usize) -> Result<Vec<u8>, SchemaError> {
            let raw = serde_json::to_vec(result).map_err(|_| SchemaError::Json)?;
            if raw.len() > maximum {
                return Err(SchemaError::Bounds);
            }
            serde_json::to_vec(&Wire::Success { ok: true, result }).map_err(|_| SchemaError::Json)
        }
        match self {
            Self::Error(error) => serde_json::to_vec(&Wire::<()>::Error { ok: false, error })
                .map_err(|_| SchemaError::Json),
            Self::Success(result) => {
                validate_success(result)?;
                match result {
                    SuccessResult::LeaseAcquire(v) => success(v, usize::MAX),
                    SuccessResult::MaintenanceAcquire(v) => success(v, usize::MAX),
                    SuccessResult::Health(v) => success(v, 1024),
                    SuccessResult::Permissions(v) => success(v, 512),
                    SuccessResult::Observability(v) => success(v, 2048),
                    SuccessResult::FrontApp(v) => success(v, 1024),
                    SuccessResult::FrontAppMetadata(v) => success(v, 1024),
                    SuccessResult::Renew(v) => success(v, usize::MAX),
                    SuccessResult::SessionMode(v) => success(v, usize::MAX),
                    SuccessResult::Configuration(v) => success(v, usize::MAX),
                    SuccessResult::Enabled(v) => success(v, usize::MAX),
                    SuccessResult::Paste(v) => success(v, usize::MAX),
                    SuccessResult::Release(v) => success(v, usize::MAX),
                    SuccessResult::Rollback(v) => success(v, usize::MAX),
                    SuccessResult::MaintenancePrepare(v) => success(v, usize::MAX),
                }
            }
        }
    }
}

fn validate_success(result: &SuccessResult) -> Result<(), SchemaError> {
    match result {
        SuccessResult::LeaseAcquire(LeaseAcquireResult {
            capture_lease_id, ..
        }) if capture_lease_id.as_bytes().iter().all(|byte| *byte == 0) => {
            Err(SchemaError::InvalidSuccess)
        }
        SuccessResult::MaintenanceAcquire(MaintenanceAcquireResult {
            maintenance_capability_id,
            ..
        }) if maintenance_capability_id
            .as_bytes()
            .iter()
            .all(|byte| *byte == 0) =>
        {
            Err(SchemaError::InvalidSuccess)
        }
        SuccessResult::Renew(RenewResult { renewed: true })
        | SuccessResult::Rollback(RollbackResult { latched: true, .. }) => Ok(()),
        SuccessResult::MaintenancePrepare(MaintenancePrepareResult {
            ready_to_exit: true,
            owner_handoff,
        }) if owner_handoff.as_bytes().iter().any(|byte| *byte != 0) => Ok(()),
        SuccessResult::Renew(_)
        | SuccessResult::MaintenancePrepare(_)
        | SuccessResult::Rollback(_) => Err(SchemaError::InvalidSuccess),
        SuccessResult::FrontApp(FrontAppResult {
            available,
            application_token,
        }) if *available == application_token.is_some() => Ok(()),
        SuccessResult::FrontApp(_) => Err(SchemaError::InvalidSuccess),
        SuccessResult::FrontAppMetadata(FrontAppMetadataResult {
            available,
            process_name,
            window_title,
            window_bounds,
        }) if if *available {
            process_name.is_some() && window_title.is_some()
        } else {
            process_name.is_none() && window_title.is_none() && window_bounds.is_none()
        } =>
        {
            Ok(())
        }
        SuccessResult::FrontAppMetadata(_) => Err(SchemaError::InvalidSuccess),
        _ => Ok(()),
    }
}

fn bounded_result<T: DeserializeOwned>(raw: &str, maximum: usize) -> Result<T, SchemaError> {
    if raw.len() > maximum {
        return Err(SchemaError::Bounds);
    }
    strict_json(raw.as_bytes())
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevocationReason {
    Eof,
    Heartbeat,
    Maintenance,
    Release,
    Protocol,
    Rollback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipKind {
    Candidate,
    Activation,
    Session,
    ReplayCleanup,
    Paste,
    Multiple,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
pub enum PredecessorTerminalEvent {
    #[serde(rename = "lease.revoked")]
    LeaseRevoked {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        reason: RevocationReason,
    },
    #[serde(rename = "lease.draining")]
    LeaseDraining {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        ownership: OwnershipKind,
    },
    #[serde(rename = "lease.neutral")]
    LeaseNeutral {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        disposition: LeaseDisposition,
    },
    #[serde(rename = "lease.unavailable")]
    LeaseUnavailable {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        reason: TerminalUnavailableReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalUnavailableReason {
    NativeFault,
    OwnershipUnknown,
}

impl PredecessorTerminalEvent {
    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_predecessor_terminal_json(&bytes)?;
        Ok(bytes)
    }
}

pub fn parse_predecessor_terminal_json(
    bytes: &[u8],
) -> Result<PredecessorTerminalEvent, SchemaError> {
    let event: PredecessorTerminalEvent = strict_json(bytes)?;
    if matches!(
        event,
        PredecessorTerminalEvent::LeaseNeutral {
            disposition: LeaseDisposition::Draining,
            ..
        }
    ) {
        return Err(SchemaError::InvalidEvent);
    }
    Ok(event)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SchemaError {
    #[error("owner-protocol JSON does not match its strict schema")]
    Json,
    #[error("owner-protocol message discriminator is unknown")]
    UnknownMessage,
    #[error("owner-protocol method is unknown")]
    UnknownMethod,
    #[error("owner-protocol value exceeds its bound")]
    Bounds,
    #[error("owner-protocol binding snapshot is invalid")]
    Binding,
    #[error("owner-protocol release-policy digest mismatch")]
    PolicyDigest,
    #[error("owner-protocol platform key mode is invalid")]
    PlatformKey,
    #[error("owner-protocol authority ceiling does not match purpose")]
    AuthorityCeiling,
    #[error("owner-protocol response union is invalid")]
    ResponseUnion,
    #[error("owner-protocol fixed error message is invalid")]
    ErrorMessage,
    #[error("owner-protocol success result is semantically invalid")]
    InvalidSuccess,
    #[error("owner-protocol event is semantically invalid")]
    InvalidEvent,
    #[error(transparent)]
    Protocol(#[from] ProtocolSelectionError),
}

fn strict_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, SchemaError> {
    serde_json::from_slice(bytes).map_err(|_| SchemaError::Json)
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
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
