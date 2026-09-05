//! Ordered binding validation, reserved owners, and wire serialization.
use super::{Shortcut, ShortcutValidationError};
use crate::{ActivationKey, ModifierMask, ProfileId, ShortcutModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeSeq};
use std::fmt;

/// One strict profile-owned shortcut binding.
#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivationBinding {
    profile_id: ProfileId,
    shortcut: Shortcut,
}

impl fmt::Debug for ActivationBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActivationBinding(<redacted>)")
    }
}

impl ActivationBinding {
    pub const fn new(profile_id: ProfileId, shortcut: Shortcut) -> Self {
        Self {
            profile_id,
            shortcut,
        }
    }

    #[must_use]
    pub const fn profile_id(self) -> ProfileId {
        self.profile_id
    }

    #[must_use]
    pub const fn shortcut(self) -> Shortcut {
        self.shortcut
    }
}

/// At most thirteen validated profile-owned shortcuts in deterministic wire order.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ActivationBindings {
    bindings: [ActivationBinding; Self::MAX],
    count: u8,
}

impl fmt::Debug for ActivationBindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActivationBindings(<redacted>)")
    }
}

impl ActivationBindings {
    pub const MAX: usize = 13;
    const EMPTY_BINDING: ActivationBinding =
        ActivationBinding::new(ProfileId::GENERAL, Shortcut::EMPTY);

    pub fn new(bindings: &[ActivationBinding]) -> Result<Self, ShortcutValidationError> {
        if bindings.len() > Self::MAX {
            return Err(ShortcutValidationError::TooManyBindings);
        }
        for (index, binding) in bindings.iter().copied().enumerate() {
            if reserved_binding_owner(binding.shortcut)
                .is_some_and(|owner| owner != binding.profile_id)
            {
                return Err(ShortcutValidationError::ReservedBuiltInFamily);
            }
            for prior in bindings[..index].iter().copied() {
                if binding.profile_id == prior.profile_id {
                    return Err(ShortcutValidationError::DuplicateProfileId);
                }
                if binding.shortcut == prior.shortcut {
                    return Err(ShortcutValidationError::DuplicateBinding);
                }
            }
        }
        let mut stored = [Self::EMPTY_BINDING; Self::MAX];
        stored[..bindings.len()].copy_from_slice(bindings);
        Ok(Self {
            bindings: stored,
            count: bindings.len() as u8,
        })
    }

    #[must_use]
    pub fn len(self) -> usize {
        usize::from(self.count)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.count == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = ActivationBinding> + '_ {
        self.bindings[..usize::from(self.count)].iter().copied()
    }

    #[must_use]
    pub fn find_exact(
        self,
        modifiers: ModifierMask,
        keys: &[ActivationKey],
    ) -> Option<ActivationBinding> {
        self.iter().find(|binding| {
            binding.shortcut.modifier_mask() == modifiers && binding.shortcut.keys() == keys
        })
    }

    #[must_use]
    pub(crate) fn has_longer_prefix(self, binding: ActivationBinding) -> bool {
        self.has_longer_sequence_prefix(binding.shortcut.modifier_mask(), binding.shortcut.keys())
    }

    #[must_use]
    pub(crate) fn has_longer_sequence_prefix(
        self,
        modifiers: ModifierMask,
        keys: &[ActivationKey],
    ) -> bool {
        self.iter().any(|candidate| {
            candidate.shortcut.modifier_mask() == modifiers
                && candidate.shortcut.keys().len() > keys.len()
                && candidate.shortcut.keys().starts_with(keys)
        })
    }
}

impl Default for ActivationBindings {
    fn default() -> Self {
        Self {
            bindings: [Self::EMPTY_BINDING; Self::MAX],
            count: 0,
        }
    }
}

impl Serialize for ActivationBindings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.len()))?;
        for binding in self.iter() {
            sequence.serialize_element(&binding)?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for ActivationBindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bindings = Vec::<ActivationBinding>::deserialize(deserializer)?;
        Self::new(&bindings).map_err(D::Error::custom)
    }
}

fn reserved_binding_owner(shortcut: Shortcut) -> Option<ProfileId> {
    if shortcut.modifiers
        != (ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        })
    {
        return None;
    }
    match shortcut.keys() {
        [ActivationKey::X] => Some(ProfileId::GENERAL),
        [ActivationKey::X, ActivationKey::P] => Some(ProfileId::PROMPT),
        [ActivationKey::X, ActivationKey::Q] => Some(ProfileId::PROMPT_TO_ENGLISH),
        [ActivationKey::X, ActivationKey::M] => Some(ProfileId::MARKDOWN),
        [ActivationKey::X, ActivationKey::T] => Some(ProfileId::TRANSLATE_TO_ENGLISH),
        _ => None,
    }
}
