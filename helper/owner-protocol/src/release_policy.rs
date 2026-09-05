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

#[cfg(test)]
mod tests;

mod signature;
pub use signature::PolicySignature;
