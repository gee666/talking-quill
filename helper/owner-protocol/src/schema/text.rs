//! Validated bounded text and profile identifiers.
use super::*;

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
