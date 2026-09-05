//! Bounded UTF-8 profile identifiers and canonical built-in names.
use crate::ShortcutValidationError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;

/// A validated profile identifier stored without heap allocation.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ProfileId {
    bytes: [u8; Self::MAX_BYTES],
    len: u8,
}

impl fmt::Debug for ProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProfileId(<redacted>)")
    }
}

impl ProfileId {
    pub const MAX_BYTES: usize = 36;
    pub const GENERAL: Self = Self::built_in(b"general");
    pub const PROMPT: Self = Self::built_in(b"prompt");
    pub const PROMPT_TO_ENGLISH: Self = Self::built_in(b"prompt-to-english");
    pub const MARKDOWN: Self = Self::built_in(b"markdown");
    pub const TRANSLATE_TO_ENGLISH: Self = Self::built_in(b"translate-to-english");

    const fn built_in(value: &[u8]) -> Self {
        let mut bytes = [0; Self::MAX_BYTES];
        let mut index = 0;
        while index < value.len() {
            bytes[index] = value[index];
            index += 1;
        }
        Self {
            bytes,
            len: value.len() as u8,
        }
    }

    pub fn new(value: &str) -> Result<Self, ShortcutValidationError> {
        if value.is_empty() || value.len() > Self::MAX_BYTES {
            return Err(ShortcutValidationError::InvalidProfileId);
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
            .expect("validated profile IDs are ASCII")
    }
}

impl Serialize for ProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(D::Error::custom)
    }
}
