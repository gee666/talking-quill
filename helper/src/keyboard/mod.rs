mod reducer;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::Error as _,
    ser::{SerializeSeq, SerializeStruct},
};
use thiserror::Error;

pub use reducer::{DecisionPlan, KeyboardReducer};

/// Layout-stable letter keys accepted by the helper protocol.
///
/// Native backends map these values to physical A-Z key positions. No
/// arbitrary virtual key can enter the native paste path.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(u8)]
pub enum ActivationKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
}

impl ActivationKey {
    pub const DEFAULT: Self = Self::Z;

    #[must_use]
    pub const fn index(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::A),
            1 => Some(Self::B),
            2 => Some(Self::C),
            3 => Some(Self::D),
            4 => Some(Self::E),
            5 => Some(Self::F),
            6 => Some(Self::G),
            7 => Some(Self::H),
            8 => Some(Self::I),
            9 => Some(Self::J),
            10 => Some(Self::K),
            11 => Some(Self::L),
            12 => Some(Self::M),
            13 => Some(Self::N),
            14 => Some(Self::O),
            15 => Some(Self::P),
            16 => Some(Self::Q),
            17 => Some(Self::R),
            18 => Some(Self::S),
            19 => Some(Self::T),
            20 => Some(Self::U),
            21 => Some(Self::V),
            22 => Some(Self::W),
            23 => Some(Self::X),
            24 => Some(Self::Y),
            25 => Some(Self::Z),
            _ => None,
        }
    }
}

/// The exact four-modifier shortcut wire object.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl ShortcutModifiers {
    #[must_use]
    pub const fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

/// Compact exact modifier state recorded with every physical letter event.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ModifierMask(u8);

impl ModifierMask {
    const CTRL: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const META: u8 = 1 << 3;

    #[must_use]
    pub const fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self(
            if ctrl { Self::CTRL } else { 0 }
                | if alt { Self::ALT } else { 0 }
                | if shift { Self::SHIFT } else { 0 }
                | if meta { Self::META } else { 0 },
        )
    }

    #[must_use]
    pub const fn ctrl(self) -> bool {
        self.0 & Self::CTRL != 0
    }

    #[must_use]
    pub const fn alt(self) -> bool {
        self.0 & Self::ALT != 0
    }

    #[must_use]
    pub const fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    #[must_use]
    pub const fn meta(self) -> bool {
        self.0 & Self::META != 0
    }

    #[must_use]
    pub const fn any(self) -> bool {
        self.0 != 0
    }
}

impl From<ShortcutModifiers> for ModifierMask {
    fn from(value: ShortcutModifiers) -> Self {
        Self::new(value.ctrl, value.alt, value.shift, value.meta)
    }
}

impl From<ModifierMask> for ShortcutModifiers {
    fn from(value: ModifierMask) -> Self {
        Self {
            ctrl: value.ctrl(),
            alt: value.alt(),
            shift: value.shift(),
            meta: value.meta(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ShortcutValidationError {
    #[error("a shortcut must contain between 1 and 26 keys")]
    InvalidKeyCount,
    #[error("a shortcut must contain at least one modifier")]
    MissingModifier,
    #[error("shortcut keys must be unique")]
    DuplicateKey,
    #[error("profile ID must be general, prompt, or an RFC UUID")]
    InvalidProfileId,
    #[error("activation supports at most 10 bindings")]
    TooManyBindings,
    #[error("activation profile IDs must be distinct")]
    DuplicateProfileId,
    #[error("activation shortcuts must be distinct")]
    DuplicateBinding,
    #[error("activation bindings with the same modifiers must not prefix one another")]
    PrefixConflict,
}

/// A bounded, allocation-free shortcut chord.
///
/// The key slice preserves physical down order. Its final key is the trigger.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Shortcut {
    modifiers: ShortcutModifiers,
    keys: [ActivationKey; Self::MAX_KEYS],
    key_count: u8,
}

impl Shortcut {
    pub const MAX_KEYS: usize = 26;
    const EMPTY: Self = Self {
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
    pub(crate) fn legacy_alt_letter(key: ActivationKey, shift: bool) -> Self {
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

/// A validated profile identifier stored without heap allocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProfileId {
    bytes: [u8; Self::MAX_BYTES],
    len: u8,
}

impl ProfileId {
    pub const MAX_BYTES: usize = 36;
    pub const GENERAL: Self = Self::built_in(b"general");
    pub const PROMPT: Self = Self::built_in(b"prompt");

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
        if value != "general" && value != "prompt" && !valid_uuid(value.as_bytes()) {
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

fn valid_uuid(value: &[u8]) -> bool {
    if value.len() != ProfileId::MAX_BYTES {
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
    let nil = value
        .iter()
        .copied()
        .filter(|byte| *byte != b'-')
        .all(|byte| byte == b'0');
    let max = value
        .iter()
        .copied()
        .filter(|byte| *byte != b'-')
        .all(|byte| byte == b'f');
    nil || max
        || (matches!(value[14].to_ascii_lowercase(), b'1'..=b'8')
            && matches!(value[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b'))
}

/// One strict profile-owned shortcut binding.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivationBinding {
    profile_id: ProfileId,
    shortcut: Shortcut,
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

/// At most ten validated profile-owned shortcuts in deterministic wire order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivationBindings {
    bindings: [ActivationBinding; Self::MAX],
    count: u8,
}

impl ActivationBindings {
    pub const MAX: usize = 10;
    const EMPTY_BINDING: ActivationBinding =
        ActivationBinding::new(ProfileId::GENERAL, Shortcut::EMPTY);

    pub fn new(bindings: &[ActivationBinding]) -> Result<Self, ShortcutValidationError> {
        if bindings.len() > Self::MAX {
            return Err(ShortcutValidationError::TooManyBindings);
        }
        for (index, binding) in bindings.iter().copied().enumerate() {
            for prior in bindings[..index].iter().copied() {
                if binding.profile_id == prior.profile_id {
                    return Err(ShortcutValidationError::DuplicateProfileId);
                }
                if binding.shortcut == prior.shortcut {
                    return Err(ShortcutValidationError::DuplicateBinding);
                }
                if binding.shortcut.modifiers == prior.shortcut.modifiers
                    && ordered_prefix(binding.shortcut.keys(), prior.shortcut.keys())
                {
                    return Err(ShortcutValidationError::PrefixConflict);
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

fn ordered_prefix(left: &[ActivationKey], right: &[ActivationKey]) -> bool {
    let shared = left.len().min(right.len());
    left[..shared] == right[..shared]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalKey {
    Letter(ActivationKey),
    Escape,
    Enter,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyPhase {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyInput {
    pub key: PhysicalKey,
    pub phase: KeyPhase,
    pub modifiers: ModifierMask,
    pub repeat: bool,
    pub injected: bool,
}

/// Tracks physical down/up state for keys whose sequences the helper may
/// capture. Windows low-level hook records do not expose an autorepeat bit, so
/// a second down before the matching up is the only reliable repeat signal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PhysicalKeyTracker {
    held: [bool; 28],
}

impl PhysicalKeyTracker {
    /// Records an event and returns true only for a repeated key-down.
    pub fn observe(&mut self, key: PhysicalKey, phase: KeyPhase) -> bool {
        let Some(index) = tracked_key_index(key) else {
            return false;
        };
        match phase {
            KeyPhase::Down => {
                let repeat = self.held[index];
                self.held[index] = true;
                repeat
            }
            KeyPhase::Up => {
                self.held[index] = false;
                false
            }
        }
    }

    /// Returns a compact snapshot of currently held A-Z keys.
    #[must_use]
    pub fn held_letter_bits(&self) -> u32 {
        self.held[..26]
            .iter()
            .enumerate()
            .fold(0_u32, |bits, (index, held)| {
                bits | if *held { 1_u32 << index } else { 0 }
            })
    }
}

const fn tracked_key_index(key: PhysicalKey) -> Option<usize> {
    match key {
        PhysicalKey::Letter(letter) => Some(letter.index() as usize),
        PhysicalKey::Escape => Some(26),
        PhysicalKey::Enter => Some(27),
        PhysicalKey::Other => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventPhase {
    Down,
    Up,
}

impl From<KeyPhase> for EventPhase {
    fn from(value: KeyPhase) -> Self {
        match value {
            KeyPhase::Down => Self::Down,
            KeyPhase::Up => Self::Up,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKey {
    Escape,
    Enter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelperEvent {
    Activation {
        binding: ActivationBinding,
        phase: EventPhase,
    },
    SessionKey {
        key: SessionKey,
        phase: EventPhase,
    },
}
