//! Bounded platform proof bytes and complete DER sequence validation.
use super::*;

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
