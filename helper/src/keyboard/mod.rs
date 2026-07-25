mod reducer;

use serde::{Deserialize, Serialize};

pub use reducer::{DecisionPlan, KeyboardReducer};

/// Layout-stable activation keys accepted by the helper protocol.
///
/// The native backends map these values to physical A-Z key positions. No
/// arbitrary virtual key or key sequence can enter the native paste path.
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ActivationBindings(u64);

impl ActivationBindings {
    pub const MAX: usize = 10;

    #[must_use]
    pub fn from_exact(bindings: &[(ActivationKey, bool)]) -> Option<Self> {
        if bindings.len() > Self::MAX {
            return None;
        }
        let mut value = Self::default();
        for &(key, shift) in bindings {
            let bit = 1_u64 << (u32::from(key.index()) * 2 + u32::from(shift));
            if value.0 & bit != 0 {
                return None;
            }
            value.0 |= bit;
        }
        Some(value)
    }

    #[must_use]
    pub const fn contains(self, key: ActivationKey, shift: bool) -> bool {
        self.0 & (1_u64 << (key.index() as u32 * 2 + shift as u32)) != 0
    }

    pub(crate) const fn bits(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_bits(bits: u64) -> Self {
        Self(bits & ((1_u64 << 52) - 1))
    }

    pub fn iter(self) -> impl Iterator<Item = (ActivationKey, bool)> {
        (0_u8..26).flat_map(move |index| {
            [false, true].into_iter().filter_map(move |shift| {
                let key = ActivationKey::from_index(index)?;
                self.contains(key, shift).then_some((key, shift))
            })
        })
    }
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
    pub alt: bool,
    pub shift: bool,
    pub disallowed_modifiers: bool,
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
        key: ActivationKey,
        phase: EventPhase,
        shift: bool,
    },
    SessionKey {
        key: SessionKey,
        phase: EventPhase,
    },
}
