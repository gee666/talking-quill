//! Validated ordered chords and their profile-owned binding collections.
use crate::{ACTIVATION_KEY_CAPACITY, ActivationKey, ModifierMask, ShortcutModifiers};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeStruct,
};
use std::fmt;
use thiserror::Error;

mod bindings;
pub use bindings::{ActivationBinding, ActivationBindings};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ShortcutValidationError {
    #[error("a shortcut must contain between 1 and 26 keys")]
    InvalidKeyCount,
    #[error("a shortcut must contain at least one modifier")]
    MissingModifier,
    #[error("shortcut keys must be unique")]
    DuplicateKey,
    #[error("profile ID must contain 1 to 36 UTF-8 bytes")]
    InvalidProfileId,
    #[error("activation supports at most 13 bindings")]
    TooManyBindings,
    #[error("activation profile IDs must be distinct")]
    DuplicateProfileId,
    #[error("activation shortcuts must be distinct")]
    DuplicateBinding,
    #[error("the canonical built-in shortcuts are reserved for their exact owners")]
    ReservedBuiltInFamily,
}

/// A bounded, allocation-free shortcut chord.
///
/// The key slice preserves physical down order. Its final key is the trigger.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Shortcut {
    modifiers: ShortcutModifiers,
    keys: [ActivationKey; Self::MAX_KEYS],
    key_count: u8,
}

impl fmt::Debug for Shortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Shortcut(<redacted>)")
    }
}

impl Shortcut {
    pub const MAX_KEYS: usize = ACTIVATION_KEY_CAPACITY;
    // Storage sentinel for binding collections and the compiled matcher only.
    // Never expose it as a validated chord or call trigger() on it.
    pub(crate) const EMPTY: Self = Self {
        modifiers: ShortcutModifiers {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
        },
        keys: [ActivationKey::A; Self::MAX_KEYS],
        key_count: 0,
    };

    pub fn new(
        modifiers: ShortcutModifiers,
        keys: &[ActivationKey],
    ) -> Result<Self, ShortcutValidationError> {
        if keys.is_empty() || keys.len() > Self::MAX_KEYS {
            return Err(ShortcutValidationError::InvalidKeyCount);
        }
        if !modifiers.any() {
            return Err(ShortcutValidationError::MissingModifier);
        }
        let mut seen = 0_u32;
        let mut stored = [ActivationKey::A; Self::MAX_KEYS];
        for (index, key) in keys.iter().copied().enumerate() {
            let bit = 1_u32 << u32::from(key.index());
            if seen & bit != 0 {
                return Err(ShortcutValidationError::DuplicateKey);
            }
            seen |= bit;
            stored[index] = key;
        }
        Ok(Self {
            modifiers,
            keys: stored,
            key_count: keys.len() as u8,
        })
    }

    /// Compatibility constructor for the reducer's one-letter entry point.
    #[must_use]
    #[doc(hidden)]
    pub fn legacy_alt_letter(key: ActivationKey, shift: bool) -> Self {
        Self::new(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift,
                meta: false,
            },
            &[key],
        )
        .expect("one unique key is a valid shortcut")
    }

    #[must_use]
    pub const fn modifiers(self) -> ShortcutModifiers {
        self.modifiers
    }

    #[must_use]
    pub fn modifier_mask(self) -> ModifierMask {
        self.modifiers.into()
    }

    #[must_use]
    pub fn keys(&self) -> &[ActivationKey] {
        &self.keys[..usize::from(self.key_count)]
    }

    #[must_use]
    pub fn trigger(self) -> ActivationKey {
        self.keys[usize::from(self.key_count - 1)]
    }
}

impl Serialize for Shortcut {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut value = serializer.serialize_struct("Shortcut", 2)?;
        value.serialize_field("modifiers", &self.modifiers)?;
        value.serialize_field("keys", self.keys())?;
        value.end()
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ShortcutWire {
            modifiers: ShortcutModifiers,
            keys: Vec<ActivationKey>,
        }

        let wire = ShortcutWire::deserialize(deserializer)?;
        Self::new(wire.modifiers, &wire.keys).map_err(D::Error::custom)
    }
}
