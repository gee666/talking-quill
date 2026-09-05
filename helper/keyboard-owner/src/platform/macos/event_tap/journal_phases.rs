//! Key and mouse generations, foreground origins, and discard fences.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn key_was_down(&self, source: InputSource, source_pid: i64, key_code: u16) -> bool {
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.held_keys[slot][usize::from(key_code)])
    }

    pub(super) fn key_is_discard_fenced(
        &self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
    ) -> bool {
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.discard_key_fences[slot][usize::from(key_code)])
    }

    pub(super) fn mouse_is_discard_fenced(
        &self,
        source: InputSource,
        source_pid: i64,
        button: u32,
    ) -> bool {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return false;
        };
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.discard_mouse_fences[slot] & bit != 0)
    }

    pub(super) fn consume_key_discard_fence(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
    ) {
        if !is_down && let Some(slot) = self.existing_source_slot(source, source_pid) {
            self.discard_key_fences[slot][usize::from(key_code)] = false;
        }
    }

    pub(super) fn consume_mouse_discard_fence(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
    ) {
        if is_down {
            return;
        }
        let Some(bit) = 1_u32.checked_shl(button) else {
            return;
        };
        if let Some(slot) = self.existing_source_slot(source, source_pid) {
            self.discard_mouse_fences[slot] &= !bit;
        }
    }

    pub(super) fn observe_key_phase(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
        repeat: bool,
        foreground_was_down: bool,
    ) -> Option<(u32, bool, bool)> {
        let index = usize::from(key_code);
        let slot = self.source_slot(source, source_pid)?;
        let generation = &mut self.key_generations[slot][index];
        if *generation == 0 {
            *generation = 1;
        }
        if self.key_origins[slot][index] == 0 && foreground_was_down {
            self.key_origins[slot][index] = 2;
            self.held_keys[slot][index] = true;
        }
        let mut foreground_balance = false;
        let hidden_generation;
        if is_down && !repeat {
            if !self.held_keys[slot][index] {
                *generation = generation.checked_add(1).unwrap_or(u32::MAX);
                self.key_origins[slot][index] = 1;
            }
            self.held_keys[slot][index] = true;
            hidden_generation = self.key_origins[slot][index] == 1;
        } else if repeat {
            if !self.held_keys[slot][index] {
                self.key_origins[slot][index] = 2;
            }
            self.held_keys[slot][index] = true;
            hidden_generation = self.key_origins[slot][index] == 1;
        } else {
            hidden_generation = self.key_origins[slot][index] == 1;
            foreground_balance = self.key_origins[slot][index] == 2
                || (self.key_origins[slot][index] == 0 && foreground_was_down);
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
        Some((*generation, foreground_balance, hidden_generation))
    }

    pub(super) fn observe_mouse_phase(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
        foreground_was_down: bool,
    ) -> Option<(bool, bool)> {
        let bit = 1_u32.checked_shl(button)?;
        let slot = self.source_slot(source, source_pid)?;
        if foreground_was_down && self.held_mouse_buttons[slot] & bit == 0 {
            self.held_mouse_buttons[slot] |= bit;
            self.mouse_foreground_origins[slot] |= bit;
        }
        let foreground_balance;
        let hidden_generation;
        if is_down {
            if self.held_mouse_buttons[slot] & bit == 0 {
                self.mouse_foreground_origins[slot] &= !bit;
            }
            self.held_mouse_buttons[slot] |= bit;
            foreground_balance = false;
            hidden_generation = self.mouse_foreground_origins[slot] & bit == 0;
        } else {
            hidden_generation = self.held_mouse_buttons[slot] & bit != 0
                && self.mouse_foreground_origins[slot] & bit == 0;
            foreground_balance = self.mouse_foreground_origins[slot] & bit != 0
                || (self.held_mouse_buttons[slot] & bit == 0 && foreground_was_down);
            self.held_mouse_buttons[slot] &= !bit;
            self.mouse_foreground_origins[slot] &= !bit;
        }
        Some((foreground_balance, hidden_generation))
    }

    pub(super) fn normal_external_key_transition(
        &mut self,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
        repeat: bool,
    ) {
        let Some(slot) = self.source_slot(InputSource::External, source_pid) else {
            return;
        };
        let index = usize::from(key_code);
        if is_down {
            if !repeat && !self.held_keys[slot][index] {
                self.key_generations[slot][index] =
                    self.key_generations[slot][index].saturating_add(1).max(1);
            }
            self.held_keys[slot][index] = true;
            self.key_origins[slot][index] = 2;
        } else {
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
    }

    pub(super) fn normal_mouse_transition(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
    ) {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return;
        };
        let Some(slot) = self.source_slot(source, source_pid) else {
            return;
        };
        if is_down {
            self.held_mouse_buttons[slot] |= bit;
            self.mouse_foreground_origins[slot] |= bit;
        } else {
            self.held_mouse_buttons[slot] &= !bit;
            self.mouse_foreground_origins[slot] &= !bit;
        }
    }

    pub(super) fn foreground_mouse_was_down(
        &self,
        source: InputSource,
        source_pid: i64,
        button: u32,
    ) -> bool {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return false;
        };
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.held_mouse_buttons[slot] & bit != 0)
    }
}
