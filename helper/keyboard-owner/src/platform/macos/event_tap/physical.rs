//! Physical key tables, side-specific modifiers, and HID/time conversion.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MacPhysicalTracker {
    pub(super) held: [bool; 128],
}

impl MacPhysicalTracker {
    pub(super) fn observe(&mut self, key_code: u16, phase: KeyPhase) -> bool {
        let Some(held) = self.held.get_mut(usize::from(key_code)) else {
            return false;
        };
        match phase {
            KeyPhase::Down => {
                let repeat = *held;
                *held = true;
                repeat
            }
            KeyPhase::Up => {
                *held = false;
                false
            }
        }
    }

    pub(super) fn seed(&mut self, key_code: u16) {
        if let Some(held) = self.held.get_mut(usize::from(key_code)) {
            *held = true;
        }
    }

    pub(super) fn is_held(&self, key_code: u16) -> bool {
        self.held
            .get(usize::from(key_code))
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn held_letter_bits(&self) -> u32 {
        LETTER_KEY_CODES
            .iter()
            .enumerate()
            .fold(0_u32, |bits, (index, key_code)| {
                bits | if self.is_held(*key_code) {
                    1_u32 << index
                } else {
                    0
                }
            })
    }

    pub(super) fn native_state_is_consistent_except(
        &self,
        excluded_key_code: u16,
        mut is_down: impl FnMut(u16) -> bool,
    ) -> bool {
        LETTER_KEY_CODES
            .into_iter()
            .chain([ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE])
            .filter(|key_code| *key_code != excluded_key_code)
            .all(|key_code| self.is_held(key_code) == is_down(key_code))
    }
}

impl Default for MacPhysicalTracker {
    fn default() -> Self {
        Self { held: [false; 128] }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PreheldLetters(pub(super) u32);

impl PreheldLetters {
    pub(super) fn insert(&mut self, key: ActivationKey) {
        self.0 |= 1_u32 << u32::from(key.index());
    }

    #[cfg(test)]
    pub(super) fn remove(&mut self, key: ActivationKey) {
        self.0 &= !(1_u32 << u32::from(key.index()));
    }

    #[cfg(test)]
    pub(super) const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct MacModifierTracker(pub(super) u8);

impl MacModifierTracker {
    pub(super) fn observe_flags_changed(&mut self, key_code: u16, is_down: bool) -> bool {
        let Some(index) = MODIFIER_KEY_CODES
            .iter()
            .position(|candidate| *candidate == key_code)
        else {
            return false;
        };
        let bit = 1_u8 << index;
        if is_down {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
        true
    }

    pub(super) fn seed(&mut self, key_code: u16) {
        let _ = self.observe_flags_changed(key_code, true);
    }

    pub(super) const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.0 & 0b0000_0011 != 0,
            self.0 & 0b0000_1100 != 0,
            self.0 & 0b0011_0000 != 0,
            self.0 & 0b1100_0000 != 0,
        )
    }

    pub(super) const fn sides(self) -> ModifierSides {
        let mut bits = 0_u8;
        if self.0 & 0b0000_0001 != 0 {
            bits |= ModifierSide::LeftCtrl.bit();
        }
        if self.0 & 0b0000_0010 != 0 {
            bits |= ModifierSide::RightCtrl.bit();
        }
        if self.0 & 0b0000_0100 != 0 {
            bits |= ModifierSide::LeftAlt.bit();
        }
        if self.0 & 0b0000_1000 != 0 {
            bits |= ModifierSide::RightAlt.bit();
        }
        if self.0 & 0b0001_0000 != 0 {
            bits |= ModifierSide::LeftShift.bit();
        }
        if self.0 & 0b0010_0000 != 0 {
            bits |= ModifierSide::RightShift.bit();
        }
        if self.0 & 0b0100_0000 != 0 {
            bits |= ModifierSide::LeftMeta.bit();
        }
        if self.0 & 0b1000_0000 != 0 {
            bits |= ModifierSide::RightMeta.bit();
        }
        ModifierSides::from_bits(bits)
    }
}
pub(super) fn physical_snapshot(keyboard: &CallbackKeyboard) -> PhysicalSnapshot {
    PhysicalSnapshot::new(
        keyboard.physical.held_letter_bits(),
        keyboard.modifiers.sides(),
        false,
    )
}

pub(super) fn transactional_key_identity(key_code: u16) -> KeyIdentity {
    match map_key_code(key_code) {
        PhysicalKey::Letter(key) => KeyIdentity::Letter(key),
        PhysicalKey::Escape => KeyIdentity::Escape,
        PhysicalKey::Enter => KeyIdentity::Enter,
        PhysicalKey::Other => KeyIdentity::Other(key_code),
    }
}

pub(super) const fn modifier_side_for_key_code(key_code: u16) -> Option<ModifierSide> {
    match key_code {
        LEFT_CONTROL_KEY_CODE => Some(ModifierSide::LeftCtrl),
        RIGHT_CONTROL_KEY_CODE => Some(ModifierSide::RightCtrl),
        LEFT_OPTION_KEY_CODE => Some(ModifierSide::LeftAlt),
        RIGHT_OPTION_KEY_CODE => Some(ModifierSide::RightAlt),
        LEFT_SHIFT_KEY_CODE => Some(ModifierSide::LeftShift),
        RIGHT_SHIFT_KEY_CODE => Some(ModifierSide::RightShift),
        LEFT_COMMAND_KEY_CODE => Some(ModifierSide::LeftMeta),
        RIGHT_COMMAND_KEY_CODE => Some(ModifierSide::RightMeta),
        _ => None,
    }
}
pub(super) const fn modifier_mask_from_flags(flags: u64) -> ModifierMask {
    ModifierMask::new(
        flags & ffi::K_CG_EVENT_FLAG_MASK_CONTROL != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_SHIFT != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_COMMAND != 0,
    )
}

pub(super) fn mach_ticks_to_nanoseconds(ticks: u64, numer: u32, denom: u32) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    let value = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    u64::try_from(value).ok()
}

pub(super) fn event_timestamp_now() -> u64 {
    let mut timebase = ffi::MachTimebaseInfo::default();
    // SAFETY: timebase is valid writable storage and mach_absolute_time has no
    // pointer preconditions. CGEvent timestamps use nanoseconds since startup.
    let status = unsafe { ffi::mach_timebase_info(&raw mut timebase) };
    if status != 0 {
        return u64::MAX;
    }
    let ticks = unsafe { ffi::mach_absolute_time() };
    mach_ticks_to_nanoseconds(ticks, timebase.numer, timebase.denom).unwrap_or(u64::MAX)
}

pub(super) fn native_mouse_button_is_down(button: u32) -> bool {
    // SAFETY: HID-system button state accepts a bounded CGMouseButton and does
    // not allocate, retain, or call Accessibility APIs.
    unsafe { ffi::CGEventSourceButtonState(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, button) }
}

pub(super) fn native_key_is_down(key_code: u16) -> bool {
    #[cfg(feature = "transactional-shortcuts-dev")]
    if let Some(held) = crate::platform::macos::macos_test_physical_key_state(key_code) {
        return held;
    }
    // SAFETY: HID-system key state is the authoritative physical device state
    // and accepts every bounded CGKeyCode. Synthetic/logical session state must
    // not satisfy neutral checks or strict-drain reconciliation.
    unsafe { ffi::CGEventSourceKeyState(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, key_code) }
}

pub(super) fn map_key_code(code: u16) -> PhysicalKey {
    if let Some(index) = LETTER_KEY_CODES
        .iter()
        .position(|candidate| *candidate == code)
    {
        return PhysicalKey::Letter(
            ActivationKey::from_index(index as u8).expect("key table has exactly A-Z entries"),
        );
    }
    match code {
        ESCAPE_KEY_CODE => PhysicalKey::Escape,
        RETURN_KEY_CODE | KEYPAD_ENTER_KEY_CODE => PhysicalKey::Enter,
        _ => PhysicalKey::Other,
    }
}
