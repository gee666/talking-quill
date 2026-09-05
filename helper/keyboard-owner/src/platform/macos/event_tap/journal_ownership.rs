//! Retired native releases and resolution of uncertain captured edges.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn record_owned_release_at(&mut self, key_code: u16, generation: u32) {
        let index = usize::from(key_code);
        let generation = generation.max(1);
        self.owned_release_generation[index] = generation;
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            self.key_generations[slot][index] = self.key_generations[slot][index].max(generation);
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
    }

    pub(super) fn record_owned_release(&mut self, key_code: u16) {
        let index = usize::from(key_code);
        let generation = self.key_generations[0][index]
            .max(self.key_generations[1][index])
            .max(1);
        self.record_owned_release_at(key_code, generation);
    }

    pub(super) fn owned_release_recorded(&self, key_code: u16) -> bool {
        self.owned_release_generation[usize::from(key_code)] != 0
    }

    pub(super) fn force_old_generation_released(&self, key_code: u16) -> bool {
        let index = usize::from(key_code);
        let released = self.owned_release_generation[index];
        if released == 0 {
            return false;
        }
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            if self.held_keys[slot][index] && self.key_generations[slot][index] > released {
                return false;
            }
        }
        true
    }

    pub(super) fn physical_newer_generation_held(&self, key_code: u16) -> bool {
        let index = usize::from(key_code);
        let released = self.owned_release_generation[index];
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).any(|slot| {
            self.held_keys[slot][index]
                && (released == 0 || self.key_generations[slot][index] > released)
        })
    }

    pub(super) fn released_letter_bits(&self) -> u32 {
        LETTER_KEY_CODES
            .iter()
            .enumerate()
            .fold(0_u32, |bits, (index, key_code)| {
                bits | if self.owned_release_recorded(*key_code) {
                    1_u32 << index
                } else {
                    0
                }
            })
    }

    pub(super) fn clear_owned_releases(&mut self) {
        self.owned_release_generation = [0; 128];
    }

    pub(super) fn resolve_uncertain_ownership(&mut self, keyboard: &mut CallbackKeyboard) {
        if self.submitted_len != 0 {
            return;
        }
        let mut retained = 0;
        for index in 0..self.pending_len {
            let mut edge = self.pending[index];
            let physical = edge.source.is_physical();
            let transactional_owned = physical
                && matches!(map_key_code(edge.key_code), PhysicalKey::Letter(letter)
                    if keyboard.transactional.owned_letters()
                        & (1_u32 << u32::from(letter.index())) != 0);
            let session_owned = physical
                && match map_key_code(edge.key_code) {
                    PhysicalKey::Escape => keyboard.session_escape_native_owned,
                    PhysicalKey::Enter => {
                        keyboard.session_enter_native_owned == Some(edge.key_code)
                    }
                    PhysicalKey::Letter(_) | PhysicalKey::Other => false,
                };
            let belongs_to_old_transaction = transactional_owned
                && !(self.owned_release_recorded(edge.key_code)
                    && edge.generation > self.owned_release_generation[usize::from(edge.key_code)]);
            if edge.uncertain_owned && (belongs_to_old_transaction || session_owned) {
                if edge.event_type == ffi::K_CG_EVENT_KEY_UP {
                    if belongs_to_old_transaction {
                        self.record_owned_release_at(edge.key_code, edge.generation);
                    }
                    if session_owned {
                        match map_key_code(edge.key_code) {
                            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
                            PhysicalKey::Enter => {
                                keyboard.session_enter_native_owned = None;
                                keyboard.captured_enter_key_code = None;
                            }
                            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
                        }
                    }
                }
                continue;
            }
            edge.uncertain_owned = false;
            self.pending[retained] = edge;
            retained += 1;
        }
        for entry in &mut self.pending[retained..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = retained;
        let _ = keyboard.reducer.reconcile_hidden_session_releases(
            keyboard.session_escape_native_owned,
            keyboard.session_enter_native_owned.is_some(),
        );
    }
}
