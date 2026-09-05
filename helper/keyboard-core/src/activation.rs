//! Frozen activation identity and bounded native target capability.
use serde::{Serialize, Serializer};
use std::fmt;
use thiserror::Error;

/// Process-scoped identity for one accepted activation boundary.
///
/// Process-scoped helper generation. The gateway-facing helper protocol keeps
/// this legacy value in JavaScript's exact integer range; owner v1 uses its
/// separate full-width decimal-string generation type.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ActivationGeneration(u64);

impl fmt::Debug for ActivationGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActivationGeneration(<redacted>)")
    }
}

impl ActivationGeneration {
    pub const FIRST: Self = Self(1);
    pub const MAX: Self = Self(9_007_199_254_740_991);

    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value >= Self::FIRST.0 && value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Self::new(value),
            None => None,
        }
    }
}

/// Opaque bounded token captured by a native adapter for later target
/// revalidation. Electron may retain and return the serialized value but must
/// never interpret it. `None` means no sufficiently strong target was captured.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct NativeTargetToken {
    bytes: [u8; Self::MAX_BYTES],
    len: u8,
}

impl NativeTargetToken {
    pub const MAX_BYTES: usize = 64;

    pub fn new(value: &str) -> Result<Self, NativeTargetTokenError> {
        if value.is_empty() {
            return Err(NativeTargetTokenError::Empty);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(NativeTargetTokenError::TooLong);
        }
        let mut bytes = [0; Self::MAX_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("native target tokens are constructed from UTF-8")
    }
}

impl fmt::Debug for NativeTargetToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeTargetToken(<redacted>)")
    }
}

impl Serialize for NativeTargetToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum NativeTargetTokenError {
    #[error("a native target token must not be empty")]
    Empty,
    #[error("a native target token must not exceed 64 UTF-8 bytes")]
    TooLong,
}

/// Frozen identity and native target capability for one activation.
#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationContext {
    activation_generation: ActivationGeneration,
    target_token: Option<NativeTargetToken>,
}

impl fmt::Debug for ActivationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActivationContext(<redacted>)")
    }
}

impl ActivationContext {
    #[must_use]
    pub const fn target_unavailable(activation_generation: ActivationGeneration) -> Self {
        Self {
            activation_generation,
            target_token: None,
        }
    }

    #[must_use]
    pub const fn activation_generation(self) -> ActivationGeneration {
        self.activation_generation
    }

    #[must_use]
    pub const fn target_token(self) -> Option<NativeTargetToken> {
        self.target_token
    }

    #[must_use]
    pub const fn with_target_token(self, target_token: NativeTargetToken) -> Self {
        Self {
            activation_generation: self.activation_generation,
            target_token: Some(target_token),
        }
    }
}
