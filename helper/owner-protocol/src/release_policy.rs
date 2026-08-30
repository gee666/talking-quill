use std::fmt;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::scalar::{Bytes32, decode_base64url, encode_base64url};
use crate::schema::{Architecture, Platform, ProtocolHeader};

pub const RELEASE_POLICY_BYTES: usize = 328;
const RELEASE_POLICY_MAGIC: &[u8; 8] = b"TQKOPOL1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerMode {
    SafeDisabled,
    EnabledCandidate,
}

impl OwnerMode {
    const fn tag(self) -> u8 {
        match self {
            Self::SafeDisabled => 1,
            Self::EnabledCandidate => 2,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, ReleasePolicyError> {
        match tag {
            1 => Ok(Self::SafeDisabled),
            2 => Ok(Self::EnabledCandidate),
            _ => Err(ReleasePolicyError::InvalidTag),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReleasePolicyPredecessor {
    pub release_build_digest: Bytes32,
    pub gateway_sha256: Bytes32,
    pub owner_sha256: Bytes32,
    pub platform: Platform,
    pub architecture: Architecture,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReleasePolicy {
    pub platform: Platform,
    pub architecture: Architecture,
    pub owner_mode: OwnerMode,
    pub release_build_digest: Bytes32,
    pub gateway_sha256: Bytes32,
    pub owner_sha256: Bytes32,
    pub gateway_signer_policy_digest: Bytes32,
    pub owner_signer_policy_digest: Bytes32,
    pub gateway_protocol: ProtocolHeader,
    pub owner_protocol: ProtocolHeader,
    pub predecessor: Option<ReleasePolicyPredecessor>,
}

impl fmt::Debug for ReleasePolicyPredecessor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReleasePolicyPredecessor([REDACTED])")
    }
}

impl fmt::Debug for ReleasePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReleasePolicy([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PolicyBlob([u8; RELEASE_POLICY_BYTES]);

impl PolicyBlob {
    pub fn from_bytes(bytes: [u8; RELEASE_POLICY_BYTES]) -> Result<Self, ReleasePolicyError> {
        let blob = Self(bytes);
        blob.decode()?;
        Ok(blob)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; RELEASE_POLICY_BYTES] {
        &self.0
    }

    #[must_use]
    pub fn digest(&self) -> Bytes32 {
        Bytes32::new(Sha256::digest(self.0).into())
    }

    pub fn decode(&self) -> Result<ReleasePolicy, ReleasePolicyError> {
        ReleasePolicy::decode(&self.0)
    }
}

impl fmt::Debug for PolicyBlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PolicyBlob([REDACTED])")
    }
}

impl Serialize for PolicyBlob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_base64url(&self.0))
    }
}

impl<'de> Deserialize<'de> for PolicyBlob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = decode_base64url(&encoded).map_err(de::Error::custom)?;
        let bytes: [u8; RELEASE_POLICY_BYTES] = bytes
            .try_into()
            .map_err(|_| de::Error::custom(ReleasePolicyError::InvalidLength))?;
        let blob = Self(bytes);
        blob.decode().map_err(de::Error::custom)?;
        Ok(blob)
    }
}

/// Opaque detached DER CMS bytes. Signature trust is a platform policy check.
#[derive(Clone, PartialEq, Eq)]
pub struct PolicySignature(Vec<u8>);

impl PolicySignature {
    pub fn from_windows_manifest_proof(bytes: Vec<u8>) -> Result<Self, ReleasePolicyError> {
        if bytes.len() != 136 || &bytes[..8] != b"TQKOWPR1" {
            return Err(ReleasePolicyError::InvalidSignature);
        }
        Ok(Self(bytes))
    }

    pub fn from_der(bytes: Vec<u8>) -> Result<Self, ReleasePolicyError> {
        if !(1..=4096).contains(&bytes.len()) || !is_complete_der_sequence(&bytes) {
            return Err(ReleasePolicyError::InvalidSignature);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_proof_bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn as_der(&self) -> &[u8] {
        self.as_proof_bytes()
    }
}

impl fmt::Debug for PolicySignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PolicySignature([REDACTED])")
    }
}

impl Serialize for PolicySignature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_base64url(&self.0))
    }
}

impl<'de> Deserialize<'de> for PolicySignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let bytes = decode_base64url(&encoded).map_err(de::Error::custom)?;
        if bytes.starts_with(b"TQKOWPR1") {
            Self::from_windows_manifest_proof(bytes).map_err(de::Error::custom)
        } else {
            Self::from_der(bytes).map_err(de::Error::custom)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ReleasePolicyError {
    #[error("release policy has the wrong length")]
    InvalidLength,
    #[error("release policy magic or version is invalid")]
    InvalidHeader,
    #[error("release policy contains an invalid tag")]
    InvalidTag,
    #[error("release policy contains nonzero reserved bytes")]
    NonzeroReserved,
    #[error("release policy predecessor encoding is inconsistent")]
    InvalidPredecessor,
    #[error("release policy protocol header is invalid")]
    InvalidProtocol,
    #[error("release policy proof is invalid for the selected platform")]
    InvalidSignature,
}

impl ReleasePolicy {
    pub fn encode(&self) -> Result<PolicyBlob, ReleasePolicyError> {
        self.validate()?;
        let mut bytes = [0_u8; RELEASE_POLICY_BYTES];
        bytes[0..8].copy_from_slice(RELEASE_POLICY_MAGIC);
        bytes[8..10].copy_from_slice(&1_u16.to_be_bytes());
        bytes[10] = self.platform.tag();
        bytes[11] = self.architecture.tag();
        bytes[12] = self.owner_mode.tag();
        bytes[13] = u8::from(self.predecessor.is_some());
        copy_digest(&mut bytes[16..48], self.release_build_digest);
        copy_digest(&mut bytes[48..80], self.gateway_sha256);
        copy_digest(&mut bytes[80..112], self.owner_sha256);
        copy_digest(&mut bytes[112..144], self.gateway_signer_policy_digest);
        copy_digest(&mut bytes[144..176], self.owner_signer_policy_digest);
        encode_protocol(&mut bytes[176..200], self.gateway_protocol);
        encode_protocol(&mut bytes[200..224], self.owner_protocol);
        if let Some(predecessor) = &self.predecessor {
            copy_digest(&mut bytes[224..256], predecessor.release_build_digest);
            copy_digest(&mut bytes[256..288], predecessor.gateway_sha256);
            copy_digest(&mut bytes[288..320], predecessor.owner_sha256);
            bytes[320] = predecessor.platform.tag();
            bytes[321] = predecessor.architecture.tag();
        }
        Ok(PolicyBlob(bytes))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ReleasePolicyError> {
        let bytes: &[u8; RELEASE_POLICY_BYTES] = bytes
            .try_into()
            .map_err(|_| ReleasePolicyError::InvalidLength)?;
        if &bytes[0..8] != RELEASE_POLICY_MAGIC || bytes[8..10] != 1_u16.to_be_bytes() {
            return Err(ReleasePolicyError::InvalidHeader);
        }
        if bytes[14..16].iter().any(|byte| *byte != 0)
            || bytes[322..328].iter().any(|byte| *byte != 0)
        {
            return Err(ReleasePolicyError::NonzeroReserved);
        }
        let platform = Platform::from_tag(bytes[10]).ok_or(ReleasePolicyError::InvalidTag)?;
        let architecture =
            Architecture::from_tag(bytes[11]).ok_or(ReleasePolicyError::InvalidTag)?;
        let owner_mode = OwnerMode::from_tag(bytes[12])?;
        let predecessor = match bytes[13] {
            0 => {
                if bytes[224..328].iter().any(|byte| *byte != 0) {
                    return Err(ReleasePolicyError::InvalidPredecessor);
                }
                None
            }
            1 => Some(ReleasePolicyPredecessor {
                release_build_digest: read_digest(&bytes[224..256]),
                gateway_sha256: read_digest(&bytes[256..288]),
                owner_sha256: read_digest(&bytes[288..320]),
                platform: Platform::from_tag(bytes[320])
                    .ok_or(ReleasePolicyError::InvalidPredecessor)?,
                architecture: Architecture::from_tag(bytes[321])
                    .ok_or(ReleasePolicyError::InvalidPredecessor)?,
            }),
            _ => return Err(ReleasePolicyError::InvalidPredecessor),
        };
        let policy = Self {
            platform,
            architecture,
            owner_mode,
            release_build_digest: read_digest(&bytes[16..48]),
            gateway_sha256: read_digest(&bytes[48..80]),
            owner_sha256: read_digest(&bytes[80..112]),
            gateway_signer_policy_digest: read_digest(&bytes[112..144]),
            owner_signer_policy_digest: read_digest(&bytes[144..176]),
            gateway_protocol: decode_protocol(&bytes[176..200])?,
            owner_protocol: decode_protocol(&bytes[200..224])?,
            predecessor,
        };
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<(), ReleasePolicyError> {
        self.gateway_protocol
            .validate()
            .and_then(|()| self.owner_protocol.validate())
            .map_err(|_| ReleasePolicyError::InvalidProtocol)?;
        if self.predecessor.as_ref().is_some_and(|value| {
            value.platform != self.platform || value.architecture != self.architecture
        }) {
            return Err(ReleasePolicyError::InvalidPredecessor);
        }
        Ok(())
    }
}

fn encode_protocol(output: &mut [u8], protocol: ProtocolHeader) {
    output[0..2].copy_from_slice(&protocol.major.to_be_bytes());
    output[2..4].copy_from_slice(&protocol.minor.to_be_bytes());
    output[4..8].copy_from_slice(&protocol.compatibility_epoch.to_be_bytes());
    output[8..16].copy_from_slice(&protocol.supported_feature_bits.get().to_be_bytes());
    output[16..24].copy_from_slice(&protocol.required_feature_bits.get().to_be_bytes());
}

fn decode_protocol(bytes: &[u8]) -> Result<ProtocolHeader, ReleasePolicyError> {
    let protocol = ProtocolHeader {
        major: u16::from_be_bytes(bytes[0..2].try_into().expect("two bytes")),
        minor: u16::from_be_bytes(bytes[2..4].try_into().expect("two bytes")),
        compatibility_epoch: u32::from_be_bytes(bytes[4..8].try_into().expect("four bytes")),
        supported_feature_bits: crate::scalar::FeatureBits::new(u64::from_be_bytes(
            bytes[8..16].try_into().expect("eight bytes"),
        )),
        required_feature_bits: crate::scalar::FeatureBits::new(u64::from_be_bytes(
            bytes[16..24].try_into().expect("eight bytes"),
        )),
    };
    protocol
        .validate()
        .map(|()| protocol)
        .map_err(|_| ReleasePolicyError::InvalidProtocol)
}

fn copy_digest(output: &mut [u8], digest: Bytes32) {
    output.copy_from_slice(digest.as_bytes());
}

fn read_digest(input: &[u8]) -> Bytes32 {
    Bytes32::new(input.try_into().expect("32-byte digest field"))
}

fn is_complete_der_sequence(bytes: &[u8]) -> bool {
    if bytes.first() != Some(&0x30) || bytes.len() < 2 {
        return false;
    }
    let first_length = bytes[1];
    if first_length < 0x80 {
        return usize::from(first_length) + 2 == bytes.len();
    }
    let length_bytes = usize::from(first_length & 0x7f);
    if length_bytes == 0 || length_bytes > 4 || bytes.len() < 2 + length_bytes || bytes[2] == 0 {
        return false;
    }
    let mut content_length = 0_usize;
    for byte in &bytes[2..2 + length_bytes] {
        content_length = match content_length
            .checked_mul(256)
            .and_then(|v| v.checked_add(usize::from(*byte)))
        {
            Some(value) => value,
            None => return false,
        };
    }
    content_length >= 128 && 2 + length_bytes + content_length == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::{OwnerMode, ReleasePolicy, ReleasePolicyPredecessor};
    use crate::scalar::Bytes32;
    use crate::schema::{Architecture, Platform, ProtocolHeader};

    #[test]
    fn all_frozen_release_policy_vectors_conform() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/compatibility/keyboard-owner-v1/release-policy-vectors.json"
        ))
        .expect("valid fixture JSON");
        assert_eq!(fixture["fixtureVersion"], 1);
        for vector in fixture["vectors"].as_array().expect("vectors") {
            let fields = &vector["fields"];
            let policy_platform = platform(text(fields, "platform"));
            let policy_architecture = architecture(text(fields, "architecture"));
            let predecessor = fields["predecessor"].as_object().map(|_| {
                let value = &fields["predecessor"];
                ReleasePolicyPredecessor {
                    release_build_digest: digest(text(value, "releaseBuildDigest")),
                    gateway_sha256: digest(text(value, "gatewaySha256")),
                    owner_sha256: digest(text(value, "ownerSha256")),
                    platform: platform(text(value, "platform")),
                    architecture: architecture(text(value, "architecture")),
                }
            });
            let policy = ReleasePolicy {
                platform: policy_platform,
                architecture: policy_architecture,
                owner_mode: match text(fields, "ownerMode") {
                    "safe_disabled" => OwnerMode::SafeDisabled,
                    "enabled_candidate" => OwnerMode::EnabledCandidate,
                    _ => panic!("fixture owner mode"),
                },
                release_build_digest: digest(text(fields, "releaseBuildDigest")),
                gateway_sha256: digest(text(fields, "gatewaySha256")),
                owner_sha256: digest(text(fields, "ownerSha256")),
                gateway_signer_policy_digest: digest(text(fields, "gatewaySignerPolicyDigest")),
                owner_signer_policy_digest: digest(text(fields, "ownerSignerPolicyDigest")),
                gateway_protocol: protocol(&fields["gatewayProtocol"]),
                owner_protocol: protocol(&fields["ownerProtocol"]),
                predecessor,
            };
            let encoded = policy.encode().expect("policy encodes");
            assert_eq!(
                encoded.as_bytes().as_slice(),
                hex(text(vector, "expectedHex"))
            );
            assert_eq!(encoded.digest(), digest(text(vector, "expectedSha256")));
            assert_eq!(encoded.decode().expect("policy decodes"), policy);
        }
    }

    fn protocol(value: &serde_json::Value) -> ProtocolHeader {
        serde_json::from_value(value.clone()).expect("protocol")
    }

    fn platform(value: &str) -> Platform {
        match value {
            "windows" => Platform::Windows,
            "macos" => Platform::Macos,
            _ => panic!("fixture platform"),
        }
    }

    fn architecture(value: &str) -> Architecture {
        match value {
            "x64" => Architecture::X64,
            "arm64" => Architecture::Arm64,
            _ => panic!("fixture architecture"),
        }
    }

    fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
        value[field].as_str().expect("fixture string")
    }

    fn digest(value: &str) -> Bytes32 {
        Bytes32::new(hex(value).try_into().expect("32-byte digest"))
    }

    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex")
            })
            .collect()
    }
}
