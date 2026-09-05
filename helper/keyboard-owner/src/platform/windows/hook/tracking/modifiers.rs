//! Track generic and sided modifiers without conflating physical transitions.
use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::windows::hook) struct ModifierSides {
    pub(in crate::platform::windows::hook) left: bool,
    pub(in crate::platform::windows::hook) right: bool,
    pub(in crate::platform::windows::hook) generic: bool,
}

impl ModifierSides {
    pub(in crate::platform::windows::hook) fn from_state(
        left: u16,
        right: u16,
        generic: Option<u16>,
        is_down: &mut impl FnMut(u16) -> bool,
    ) -> Self {
        let left_down = is_down(left);
        let right_down = is_down(right);
        Self {
            left: left_down,
            right: right_down,
            generic: !left_down && !right_down && generic.is_some_and(is_down),
        }
    }

    pub(in crate::platform::windows::hook) fn observe_left(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.left = phase == KeyPhase::Down;
    }

    pub(in crate::platform::windows::hook) fn observe_right(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.right = phase == KeyPhase::Down;
    }

    pub(in crate::platform::windows::hook) fn observe_generic(&mut self, phase: KeyPhase) {
        self.generic = phase == KeyPhase::Down;
    }

    pub(in crate::platform::windows::hook) const fn is_down(self) -> bool {
        self.left || self.right || self.generic
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::windows::hook) struct ModifierTracker {
    pub(in crate::platform::windows::hook) ctrl: ModifierSides,
    pub(in crate::platform::windows::hook) alt: ModifierSides,
    pub(in crate::platform::windows::hook) shift: ModifierSides,
    pub(in crate::platform::windows::hook) meta: ModifierSides,
}

impl ModifierTracker {
    pub(in crate::platform::windows::hook) fn from_state(
        mut is_down: impl FnMut(u16) -> bool,
    ) -> Self {
        Self {
            ctrl: ModifierSides::from_state(
                VK_LCONTROL,
                VK_RCONTROL,
                Some(VK_CONTROL),
                &mut is_down,
            ),
            alt: ModifierSides::from_state(VK_LMENU, VK_RMENU, Some(VK_MENU), &mut is_down),
            shift: ModifierSides::from_state(VK_LSHIFT, VK_RSHIFT, Some(VK_SHIFT), &mut is_down),
            meta: ModifierSides::from_state(VK_LWIN, VK_RWIN, None, &mut is_down),
        }
    }

    pub(in crate::platform::windows::hook) fn observe(
        &mut self,
        virtual_key: u16,
        scan_code: u32,
        extended: bool,
        phase: KeyPhase,
    ) -> bool {
        match virtual_key {
            VK_LCONTROL => self.ctrl.observe_left(phase),
            VK_RCONTROL => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D && extended => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D => self.ctrl.observe_left(phase),
            VK_CONTROL => self.ctrl.observe_generic(phase),
            VK_LMENU => self.alt.observe_left(phase),
            VK_RMENU => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 && extended => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 => self.alt.observe_left(phase),
            VK_MENU => self.alt.observe_generic(phase),
            VK_LSHIFT => self.shift.observe_left(phase),
            VK_RSHIFT => self.shift.observe_right(phase),
            VK_SHIFT if scan_code == 0x2A => self.shift.observe_left(phase),
            VK_SHIFT if scan_code == 0x36 => self.shift.observe_right(phase),
            VK_SHIFT => self.shift.observe_generic(phase),
            VK_LWIN => self.meta.observe_left(phase),
            VK_RWIN => self.meta.observe_right(phase),
            _ => return false,
        }
        true
    }

    pub(in crate::platform::windows::hook) const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.ctrl.is_down(),
            self.alt.is_down(),
            self.shift.is_down(),
            self.meta.is_down(),
        )
    }

    pub(in crate::platform::windows::hook) const fn transactional_sides(
        self,
    ) -> TransactionalModifierSides {
        let mut bits = 0_u8;
        if self.ctrl.left || self.ctrl.generic {
            bits |= ModifierSide::LeftCtrl.bit();
        }
        if self.ctrl.right {
            bits |= ModifierSide::RightCtrl.bit();
        }
        if self.alt.left || self.alt.generic {
            bits |= ModifierSide::LeftAlt.bit();
        }
        if self.alt.right {
            bits |= ModifierSide::RightAlt.bit();
        }
        if self.shift.left || self.shift.generic {
            bits |= ModifierSide::LeftShift.bit();
        }
        if self.shift.right {
            bits |= ModifierSide::RightShift.bit();
        }
        if self.meta.left {
            bits |= ModifierSide::LeftMeta.bit();
        }
        if self.meta.right {
            bits |= ModifierSide::RightMeta.bit();
        }
        TransactionalModifierSides::from_bits(bits)
    }

    pub(in crate::platform::windows::hook) const fn is_neutral(self) -> bool {
        !self.mask().any()
    }
}
