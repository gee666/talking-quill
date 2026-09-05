//! Readiness and physical discard fences for retained recovery edges.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn queued_ordering_pending(&self) -> bool {
        self.pending_len != 0
            || self.tail_len != 0
            || self.overflow_balance_len != 0
            || self.token.is_some()
            || self.overflow
    }

    pub(super) fn physical_fences_pending(&self) -> bool {
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).any(|slot| {
            self.discard_key_fences[slot].iter().any(|fenced| *fenced)
                || self.discard_mouse_fences[slot] != 0
        })
    }

    pub(super) fn reconcile_physical_fences(
        &mut self,
        mut key_is_down: impl FnMut(u16) -> bool,
        mut button_is_down: impl FnMut(u32) -> bool,
    ) {
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            for key_code in 0..128_u16 {
                if self.discard_key_fences[slot][usize::from(key_code)] && !key_is_down(key_code) {
                    self.discard_key_fences[slot][usize::from(key_code)] = false;
                }
            }
            for button in 0..32_u32 {
                let bit = 1_u32 << button;
                if self.discard_mouse_fences[slot] & bit != 0 && !button_is_down(button) {
                    self.discard_mouse_fences[slot] &= !bit;
                }
            }
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        self.queued_ordering_pending()
            || self.physical_fences_pending()
            || self
                .owned_release_generation
                .iter()
                .any(|value| *value != 0)
    }

    pub(super) fn ordering_pending(&self) -> bool {
        self.queued_ordering_pending() || self.physical_fences_pending()
    }

    #[cfg(test)]
    pub(super) fn hidden_balanced(&self) -> bool {
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).all(|slot| {
            self.key_origins[slot].iter().all(|origin| *origin != 1)
                && self.held_mouse_buttons[slot] & !self.mouse_foreground_origins[slot] == 0
        })
    }

    pub(super) fn ready_len(&self) -> usize {
        if self.token.is_some() || self.pending_len == 0 {
            return 0;
        }
        let mut hidden_keys = [[false; 128]; RECOVERY_PHYSICAL_SOURCE_SLOTS];
        let mut hidden_mouse = [0_u32; RECOVERY_PHYSICAL_SOURCE_SLOTS];
        let mut ready = 0;
        for (index, edge) in self.pending[..self.pending_len].iter().copied().enumerate() {
            if edge.uncertain_owned {
                break;
            }
            let physical_slot = if edge.source == InputSource::Physical {
                Some(0)
            } else if edge.source.is_test_physical() {
                Some(1)
            } else {
                None
            };
            if edge.hidden_generation
                && let Some(slot) = physical_slot
            {
                if edge.is_mouse() {
                    let Ok(button) = u32::try_from(edge.mouse_button) else {
                        break;
                    };
                    let Some(bit) = 1_u32.checked_shl(button) else {
                        break;
                    };
                    if edge.is_down {
                        hidden_mouse[slot] |= bit;
                    } else {
                        hidden_mouse[slot] &= !bit;
                    }
                } else if !edge.repeat {
                    hidden_keys[slot][usize::from(edge.key_code)] = edge.is_down;
                }
            }
            if hidden_keys.iter().flatten().all(|held| !*held)
                && hidden_mouse.iter().all(|held| *held == 0)
            {
                ready = index + 1;
            }
        }
        ready
    }
}
