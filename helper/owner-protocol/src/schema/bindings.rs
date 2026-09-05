//! Ordered shortcut grammar and reserved profile owners.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Letter {
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

impl Serialize for Letter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let byte = b'A' + *self as u8;
        serializer.serialize_str(std::str::from_utf8(&[byte]).expect("ASCII letter"))
    }
}

impl<'de> Deserialize<'de> for Letter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let [byte] = value.as_bytes() else {
            return Err(de::Error::custom(SchemaError::Binding));
        };
        if !byte.is_ascii_uppercase() {
            return Err(de::Error::custom(SchemaError::Binding));
        }
        const LETTERS: [Letter; 26] = [
            Letter::A,
            Letter::B,
            Letter::C,
            Letter::D,
            Letter::E,
            Letter::F,
            Letter::G,
            Letter::H,
            Letter::I,
            Letter::J,
            Letter::K,
            Letter::L,
            Letter::M,
            Letter::N,
            Letter::O,
            Letter::P,
            Letter::Q,
            Letter::R,
            Letter::S,
            Letter::T,
            Letter::U,
            Letter::V,
            Letter::W,
            Letter::X,
            Letter::Y,
            Letter::Z,
        ];
        Ok(LETTERS[usize::from(*byte - b'A')])
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifiers {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

impl Modifiers {
    #[must_use]
    pub const fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            meta,
        }
    }

    #[must_use]
    pub const fn values(self) -> (bool, bool, bool, bool) {
        (self.ctrl, self.alt, self.shift, self.meta)
    }

    const fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Modifiers([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindingShortcut {
    modifiers: Modifiers,
    keys: Vec<Letter>,
}

impl BindingShortcut {
    pub fn new(modifiers: Modifiers, keys: Vec<Letter>) -> Result<Self, SchemaError> {
        if !modifiers.any()
            || keys.is_empty()
            || keys.len() > 26
            || keys.iter().copied().collect::<HashSet<_>>().len() != keys.len()
        {
            return Err(SchemaError::Binding);
        }
        Ok(Self { modifiers, keys })
    }

    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    #[must_use]
    pub fn keys(&self) -> &[Letter] {
        &self.keys
    }
}

impl fmt::Debug for BindingShortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BindingShortcut([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for BindingShortcut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            modifiers: Modifiers,
            keys: Vec<Letter>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.modifiers, wire.keys).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    #[serde(rename = "profileId")]
    profile_id: ProfileId,
    shortcut: BindingShortcut,
}

impl Binding {
    #[must_use]
    pub const fn new(profile_id: ProfileId, shortcut: BindingShortcut) -> Self {
        Self {
            profile_id,
            shortcut,
        }
    }

    #[must_use]
    pub const fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    #[must_use]
    pub const fn shortcut(&self) -> &BindingShortcut {
        &self.shortcut
    }
}

impl fmt::Debug for Binding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Binding([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Bindings(Vec<Binding>);

impl fmt::Debug for Bindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Bindings([REDACTED])")
    }
}

impl Bindings {
    pub fn new(bindings: Vec<Binding>) -> Result<Self, SchemaError> {
        if bindings.len() > 13
            || bindings.iter().any(|binding| {
                reserved_profile_owner(&binding.shortcut)
                    .is_some_and(|owner| owner != binding.profile_id.as_str())
            })
            || bindings
                .iter()
                .map(|binding| &binding.profile_id)
                .collect::<HashSet<_>>()
                .len()
                != bindings.len()
            || bindings
                .iter()
                .map(|binding| &binding.shortcut)
                .collect::<HashSet<_>>()
                .len()
                != bindings.len()
        {
            return Err(SchemaError::Binding);
        }
        Ok(Self(bindings))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Binding] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Bindings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<Binding>::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

fn reserved_profile_owner(shortcut: &BindingShortcut) -> Option<&'static str> {
    if shortcut.modifiers.values() != (false, true, false, false) {
        return None;
    }
    match shortcut.keys.as_slice() {
        [Letter::X] => Some("general"),
        [Letter::X, Letter::P] => Some("prompt"),
        [Letter::X, Letter::Q] => Some("prompt-to-english"),
        [Letter::X, Letter::M] => Some("markdown"),
        [Letter::X, Letter::T] => Some("translate-to-english"),
        _ => None,
    }
}
