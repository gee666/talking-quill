//! Exact image identities shared by the unprivileged Windows gateway and owner.

use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use subtle::ConstantTimeEq;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsArchitecture {
    X64,
    Arm64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageRole {
    Gateway,
    Owner,
}

#[derive(Clone, Copy, Eq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl PartialEq for Sha256Digest {
    fn eq(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl Hash for Sha256Digest {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Sha256Digest([REDACTED])")
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_hex(&String::deserialize(deserializer)?)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(TABLE[(byte >> 4) as usize] as char);
        output.push(TABLE[(byte & 15) as usize] as char);
    }
    output
}

fn parse_hex(value: &str) -> Result<[u8; 32], &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("non-canonical SHA-256 digest");
    }
    let mut output = [0_u8; 32];
    for (byte, pair) in output.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let text = std::str::from_utf8(pair).map_err(|_| "invalid SHA-256 digest")?;
        *byte = u8::from_str_radix(text, 16).map_err(|_| "invalid SHA-256 digest")?;
    }
    Ok(output)
}
