#[cfg(all(feature = "native-test-input", not(debug_assertions)))]
compile_error!("native-test-input cannot grant physical authority in an optimized core build");

mod reducer;
pub mod transactional;

use std::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::Error as _,
    ser::{SerializeSeq, SerializeStruct},
};
use thiserror::Error;

pub use reducer::{DecisionPlan, KeyboardReducer};

/// Shared fixed capacities derived from the v1 keyboard grammar.
///
/// Owner state, callback tracking, matcher storage, replay cleanup, and tests
/// must use these definitions rather than independently repeating literals.
pub mod bounds {
    /// Unique physical A-Z generations that activation may own.
    pub const ACTIVATION_KEY_CAPACITY: usize = 26;
    /// Unique Escape/Enter generations that session capture may own.
    pub const SESSION_KEY_CAPACITY: usize = 2;
    /// Combined physical drain capacity across activation and session keys.
    pub const COMBINED_PHYSICAL_DRAIN_CAPACITY: usize =
        ACTIVATION_KEY_CAPACITY + SESSION_KEY_CAPACITY;
    /// At most one balancing cleanup release per unique activation letter.
    pub const REPLAY_CLEANUP_EDGE_CAPACITY: usize = ACTIVATION_KEY_CAPACITY;
    /// Fixed owner callback-admission/effect queue budget.
    pub const OWNER_ADMITTED_EFFECT_CAPACITY: usize = 8;
}

pub use bounds::{
    ACTIVATION_KEY_CAPACITY, COMBINED_PHYSICAL_DRAIN_CAPACITY, OWNER_ADMITTED_EFFECT_CAPACITY,
    REPLAY_CLEANUP_EDGE_CAPACITY, SESSION_KEY_CAPACITY,
};

/// Layout-stable letter keys accepted by the helper protocol.
///
/// Native backends map these values to physical A-Z key positions. No
/// arbitrary virtual key can enter the native paste path.
#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
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

impl fmt::Debug for ActivationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActivationKey(<redacted>)")
    }
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
#[derive(Clone, Copy, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl fmt::Debug for ShortcutModifiers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ShortcutModifiers(<redacted>)")
    }
}

impl ShortcutModifiers {
    #[must_use]
    pub const fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

/// Compact exact modifier state recorded with every physical letter event.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct ModifierMask(u8);

impl fmt::Debug for ModifierMask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ModifierMask(<redacted>)")
    }
}

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

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PhysicalKey {
    Letter(ActivationKey),
    Escape,
    Enter,
    Other,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum KeyPhase {
    Down,
    Up,
}

#[derive(Clone, Copy, Eq, PartialEq)]
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
#[derive(Clone, Default, Eq, PartialEq)]
pub struct PhysicalKeyTracker {
    held: [bool; COMBINED_PHYSICAL_DRAIN_CAPACITY],
}

impl fmt::Debug for PhysicalKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PhysicalKey(<redacted>)")
    }
}

impl fmt::Debug for KeyInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyInput(<redacted>)")
    }
}

impl fmt::Debug for PhysicalKeyTracker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PhysicalKeyTracker(<redacted>)")
    }
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
        self.held[..ACTIVATION_KEY_CAPACITY]
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
        PhysicalKey::Escape => Some(ACTIVATION_KEY_CAPACITY),
        PhysicalKey::Enter => Some(ACTIVATION_KEY_CAPACITY + 1),
        PhysicalKey::Other => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventPhase {
    Down,
    Up,
}

impl fmt::Debug for KeyPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyPhase(<redacted>)")
    }
}

impl From<KeyPhase> for EventPhase {
    fn from(value: KeyPhase) -> Self {
        match value {
            KeyPhase::Down => Self::Down,
            KeyPhase::Up => Self::Up,
        }
    }
}

impl From<KeyPhase> for transactional::PhysicalPhase {
    fn from(value: KeyPhase) -> Self {
        match value {
            KeyPhase::Down => Self::Down,
            KeyPhase::Up => Self::Up,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
#[serde(rename_all = "kebab-case")]
pub enum SessionCaptureMode {
    #[default]
    Off,
    Recording,
    CancelOnly,
}

impl SessionCaptureMode {
    #[must_use]
    pub const fn allows(self, key: SessionKey) -> bool {
        match key {
            SessionKey::Escape => !matches!(self, Self::Off),
            SessionKey::Enter => matches!(self, Self::Recording),
        }
    }

    #[must_use]
    #[doc(hidden)]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    #[must_use]
    #[doc(hidden)]
    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Recording,
            2 => Self::CancelOnly,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKey {
    Escape,
    Enter,
}

impl fmt::Debug for SessionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionKey(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum KeyboardEvent {
    Activation {
        binding: ActivationBinding,
        context: ActivationContext,
        phase: EventPhase,
    },
    ActivationComplete {
        binding: ActivationBinding,
        context: ActivationContext,
        held_ms: u64,
    },
    SessionKey {
        key: SessionKey,
        phase: EventPhase,
    },
}

impl fmt::Debug for KeyboardEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Activation { .. } => "KeyboardEvent::Activation(<redacted>)",
            Self::ActivationComplete { .. } => "KeyboardEvent::ActivationComplete(<redacted>)",
            Self::SessionKey { .. } => "KeyboardEvent::SessionKey(<redacted>)",
        })
    }
}
