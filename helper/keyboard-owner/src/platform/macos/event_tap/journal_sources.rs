//! Bounded source identities and side-specific modifier transitions.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn external_pid_referenced(&self, source_pid: i64) -> bool {
        self.pending[..self.pending_len]
            .iter()
            .chain(&self.tail[..self.tail_len])
            .any(|edge| edge.source == InputSource::External && edge.source_pid == source_pid)
            || (0..self.overflow_balance_len).any(|offset| {
                let index = (self.overflow_balance_head + offset) % self.overflow_balances.len();
                let edge = self.overflow_balances[index];
                edge.source == InputSource::External && edge.source_pid == source_pid
            })
    }

    pub(super) fn external_slot_pinned(&self, index: usize) -> bool {
        let slot = RECOVERY_PHYSICAL_SOURCE_SLOTS + index;
        let pid = self.external_source_pids[index];
        pid != 0
            && (self.external_pid_referenced(pid)
                || self.modifier_bits[slot] != 0
                || self.held_keys[slot].iter().any(|held| *held)
                || self.held_mouse_buttons[slot] != 0
                || self.exposed_keys[slot].iter().any(|held| *held)
                || self.exposed_mouse_buttons[slot] != 0
                || self.discard_key_fences[slot].iter().any(|fenced| *fenced)
                || self.discard_mouse_fences[slot] != 0)
    }

    #[allow(unreachable_patterns)]
    pub(super) fn source_slot(&mut self, source: InputSource, source_pid: i64) -> Option<usize> {
        if source.is_test_physical() {
            return Some(1);
        }
        match source {
            InputSource::Physical => Some(0),
            InputSource::External => {
                if let Some(index) = self
                    .external_source_pids
                    .iter()
                    .position(|pid| *pid == source_pid && *pid != 0)
                {
                    return Some(RECOVERY_PHYSICAL_SOURCE_SLOTS + index);
                }
                let index = self
                    .external_source_pids
                    .iter()
                    .position(|pid| *pid == 0)
                    .or_else(|| {
                        (0..RECOVERY_EXTERNAL_SOURCE_SLOTS)
                            .find(|index| !self.external_slot_pinned(*index))
                    })?;
                let slot = RECOVERY_PHYSICAL_SOURCE_SLOTS + index;
                self.external_source_pids[index] = source_pid;
                self.modifier_bits[slot] = 0;
                self.modifier_known[slot] = 0;
                self.held_keys[slot] = [false; 128];
                self.key_origins[slot] = [0; 128];
                self.key_generations[slot] = [0; 128];
                self.held_mouse_buttons[slot] = 0;
                self.mouse_foreground_origins[slot] = 0;
                self.exposed_keys[slot] = [false; 128];
                self.exposed_mouse_buttons[slot] = 0;
                self.discard_key_fences[slot] = [false; 128];
                self.discard_mouse_fences[slot] = 0;
                Some(slot)
            }
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy => None,
            _ => None,
        }
    }

    #[allow(unreachable_patterns)]
    pub(super) fn existing_source_slot(
        &self,
        source: InputSource,
        source_pid: i64,
    ) -> Option<usize> {
        if source.is_test_physical() {
            return Some(1);
        }
        match source {
            InputSource::Physical => Some(0),
            InputSource::External => self
                .external_source_pids
                .iter()
                .position(|pid| *pid == source_pid && *pid != 0)
                .map(|index| RECOVERY_PHYSICAL_SOURCE_SLOTS + index),
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy => None,
            _ => None,
        }
    }

    pub(super) fn seed_modifier_side(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
    ) {
        let Some(side) = modifier_side_for_key_code(key_code) else {
            return;
        };
        let Some(slot) = self.source_slot(source, source_pid) else {
            return;
        };
        let bit = side.bit();
        if self.modifier_known[slot] & bit != 0 {
            return;
        }
        self.modifier_known[slot] |= bit;
        if is_down {
            self.modifier_bits[slot] |= bit;
        }
    }

    pub(super) fn modifier_side_transition(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        physical_is_down: Option<bool>,
    ) -> Option<(bool, bool)> {
        let side = modifier_side_for_key_code(key_code)?;
        let slot = self.source_slot(source, source_pid)?;
        let bit = side.bit();
        let was_down = self.modifier_bits[slot] & bit != 0;
        let is_down = if source == InputSource::Physical {
            physical_is_down?
        } else {
            // Test-physical and external streams have no authoritative HID
            // state. Their bounded, source-specific side model advances only
            // the exact keycode named by this flagsChanged edge.
            !was_down
        };
        self.modifier_known[slot] |= bit;
        if is_down {
            self.modifier_bits[slot] |= bit;
        } else {
            self.modifier_bits[slot] &= !bit;
        }
        Some((is_down, was_down))
    }

    pub(super) fn observe_normal_external_modifier(
        &mut self,
        source_pid: i64,
        key_code: u16,
    ) -> Option<bool> {
        self.modifier_side_transition(InputSource::External, source_pid, key_code, None)
            .map(|(is_down, _)| is_down)
    }
}
