//! Layout-stable letters and exact aggregate modifiers.
use serde::{Deserialize, Serialize};
use std::fmt;

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
