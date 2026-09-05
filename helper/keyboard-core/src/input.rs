//! Physical key tracking and legacy activation/session notifications.
use crate::{
    ACTIVATION_KEY_CAPACITY, ActivationBinding, ActivationContext, ActivationKey,
    COMBINED_PHYSICAL_DRAIN_CAPACITY, ModifierMask, transactional,
};
use serde::{Deserialize, Serialize};
use std::fmt;

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
