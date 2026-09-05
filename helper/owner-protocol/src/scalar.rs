use std::fmt;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;

use p256::elliptic_curve::sec1::ToSec1Point;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use subtle::ConstantTimeEq;
use thiserror::Error;

const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, PartialEq, Eq, Error)]
pub enum ScalarError {
    #[error("invalid canonical base64url value")]
    Base64Url,
    #[error("invalid 32-byte value")]
    Bytes32,
    #[error("invalid P-256 public key")]
    P256PublicKey,
    #[error("invalid canonical nonzero u64 string")]
    U64,
    #[error("invalid counter string")]
    Counter,
    #[error("invalid feature-bit string")]
    FeatureBits,
    #[error("operating-system randomness unavailable")]
    Random,
}

impl fmt::Debug for ScalarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// An opaque 32-byte wire value. Debug formatting is deliberately redacted.
#[derive(Clone, Copy, Eq)]
pub struct Bytes32([u8; 32]);

impl PartialEq for Bytes32 {
    fn eq(&self, other: &Self) -> bool {
        self.constant_time_eq(other)
    }
}

impl Hash for Bytes32 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl Bytes32 {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn random() -> Result<Self, ScalarError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| ScalarError::Random)?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl fmt::Debug for Bytes32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Bytes32([REDACTED])")
    }
}

impl Serialize for Bytes32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_base64url(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Bytes32Visitor;

        impl Visitor<'_> for Bytes32Visitor {
            type Value = Bytes32;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an unpadded canonical base64url 32-byte value")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let decoded = decode_base64url(value).map_err(E::custom)?;
                let bytes: [u8; 32] = decoded
                    .try_into()
                    .map_err(|_| E::custom(ScalarError::Bytes32))?;
                Ok(Bytes32(bytes))
            }
        }

        deserializer.deserialize_str(Bytes32Visitor)
    }
}

/// A canonical decimal JSON string in `1..=u64::MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct U64String(NonZeroU64);

impl U64String {
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl TryFrom<u64> for U64String {
    type Error = ScalarError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(ScalarError::U64)
    }
}

impl Serialize for U64String {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for U64String {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_canonical_u64(&value, false)
            .and_then(Self::try_from)
            .map_err(de::Error::custom)
    }
}

/// A canonical decimal aggregate counter safe to expose to JavaScript.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Counter(u64);

impl Counter {
    pub fn new(value: u64) -> Result<Self, ScalarError> {
        if value <= JS_MAX_SAFE_INTEGER {
            Ok(Self(value))
        } else {
            Err(ScalarError::Counter)
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for Counter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Counter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_canonical_u64(&value, true)
            .and_then(Self::new)
            .map_err(de::Error::custom)
    }
}

/// Lowercase `0x` plus exactly sixteen hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FeatureBits(u64);

impl FeatureBits {
    #[must_use]
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for FeatureBits {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:016x}", self.0))
    }
}

impl<'de> Deserialize<'de> for FeatureBits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() != 18
            || !value.starts_with("0x")
            || !value.as_bytes()[2..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(de::Error::custom(ScalarError::FeatureBits));
        }
        u64::from_str_radix(&value[2..], 16)
            .map(Self)
            .map_err(|_| de::Error::custom(ScalarError::FeatureBits))
    }
}

/// Validated SEC1 uncompressed P-256 public point.
#[derive(Clone, PartialEq, Eq)]
pub struct P256PublicKey([u8; 65]);

impl P256PublicKey {
    pub fn from_sec1_bytes(bytes: [u8; 65]) -> Result<Self, ScalarError> {
        if bytes[0] != 4 || p256::PublicKey::from_sec1_bytes(&bytes).is_err() {
            return Err(ScalarError::P256PublicKey);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 65] {
        &self.0
    }

    pub(crate) fn parsed(&self) -> p256::PublicKey {
        // Construction and deserialization validate this exact byte array.
        p256::PublicKey::from_sec1_bytes(&self.0).expect("validated P-256 point")
    }

    pub(crate) fn from_key(key: &p256::PublicKey) -> Self {
        let encoded = key.to_sec1_point(false);
        let bytes: [u8; 65] = encoded
            .as_bytes()
            .try_into()
            .expect("uncompressed P-256 point");
        Self(bytes)
    }
}

impl fmt::Debug for P256PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("P256PublicKey([REDACTED])")
    }
}

impl Serialize for P256PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_base64url(&self.0))
    }
}

impl<'de> Deserialize<'de> for P256PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let decoded = decode_base64url(&value).map_err(de::Error::custom)?;
        let bytes: [u8; 65] = decoded
            .try_into()
            .map_err(|_| de::Error::custom(ScalarError::P256PublicKey))?;
        Self::from_sec1_bytes(bytes).map_err(de::Error::custom)
    }
}

fn parse_canonical_u64(value: &str, allow_zero: bool) -> Result<u64, ScalarError> {
    let valid_zero = allow_zero && value == "0";
    let valid_positive = !value.is_empty()
        && value.len() <= 20
        && value.as_bytes()[0].is_ascii_digit()
        && value.as_bytes()[0] != b'0'
        && value.as_bytes().iter().all(u8::is_ascii_digit);
    if !(valid_zero || valid_positive) {
        return Err(if allow_zero {
            ScalarError::Counter
        } else {
            ScalarError::U64
        });
    }
    value.parse().map_err(|_| {
        if allow_zero {
            ScalarError::Counter
        } else {
            ScalarError::U64
        }
    })
}

mod base64url;
pub(crate) use base64url::{decode_base64url, encode_base64url};
