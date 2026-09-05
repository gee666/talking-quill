//! Overflow preserves exposed balancing edges and external ordering.

use super::*;

impl RecoveryEdgeJournal {
    pub(super) fn retain_overflow_edge(&mut self, edge: injection::DeferredEvent) {
        if (edge.source != InputSource::External && !edge.foreground_balance)
            || self.overflow_balance_len == self.overflow_balances.len()
        {
            return;
        }
        let index =
            (self.overflow_balance_head + self.overflow_balance_len) % self.overflow_balances.len();
        self.overflow_balances[index] = edge;
        self.overflow_balance_len += 1;
    }

    pub(super) fn finalize_discarded_hidden_phases(&mut self) {
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            for key in 0..128 {
                if self.key_origins[slot][key] == 1 && self.held_keys[slot][key] {
                    self.discard_key_fences[slot][key] = true;
                    self.held_keys[slot][key] = false;
                    self.key_origins[slot][key] = 0;
                }
            }
            let hidden_mouse = self.held_mouse_buttons[slot] & !self.mouse_foreground_origins[slot];
            self.discard_mouse_fences[slot] |= hidden_mouse;
            self.held_mouse_buttons[slot] &= !hidden_mouse;
        }
    }

    pub(super) fn enter_overflow(&mut self) {
        if self.overflow {
            return;
        }
        self.overflow = true;
        if self.token.is_none() {
            for index in 0..self.pending_len {
                self.retain_overflow_edge(self.pending[index]);
                self.pending[index] = injection::DeferredEvent::EMPTY;
            }
            self.pending_len = 0;
        }
        for index in 0..self.tail_len {
            self.retain_overflow_edge(self.tail[index]);
            self.tail[index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len = 0;
        self.finalize_discarded_hidden_phases();
    }

    pub(super) fn observe_foreground_exposure(&mut self, edge: injection::DeferredEvent) {
        let Some(slot) = self.source_slot(edge.source, edge.source_pid) else {
            return;
        };
        if edge.is_mouse() {
            let Ok(button) = u32::try_from(edge.mouse_button) else {
                return;
            };
            let Some(bit) = 1_u32.checked_shl(button) else {
                return;
            };
            if edge.is_down {
                self.exposed_mouse_buttons[slot] |= bit;
            } else {
                self.exposed_mouse_buttons[slot] &= !bit;
            }
        } else {
            self.exposed_keys[slot][usize::from(edge.key_code)] = edge.is_down;
        }
    }

    pub(super) fn retain_abort_balance(&mut self, mut edge: injection::DeferredEvent) {
        let Some(slot) = self.existing_source_slot(edge.source, edge.source_pid) else {
            self.retain_overflow_edge(edge);
            return;
        };
        if edge.is_mouse() {
            let Ok(button) = u32::try_from(edge.mouse_button) else {
                return;
            };
            let Some(bit) = 1_u32.checked_shl(button) else {
                return;
            };
            if !edge.is_down && self.exposed_mouse_buttons[slot] & bit != 0 {
                edge.foreground_balance = true;
                self.exposed_mouse_buttons[slot] &= !bit;
            }
        } else if !edge.is_down && self.exposed_keys[slot][usize::from(edge.key_code)] {
            edge.foreground_balance = true;
            self.exposed_keys[slot][usize::from(edge.key_code)] = false;
        }
        self.retain_overflow_edge(edge);
    }

    pub(super) fn abort_submission_to_overflow(&mut self) {
        self.overflow = true;
        for index in self.observed..self.submitted_len {
            self.retain_abort_balance(self.pending[index]);
        }
        for entry in &mut self.pending[..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = 0;
        self.submitted_len = 0;
        self.observed = 0;
        self.token = None;
        self.submission_deadline = None;
        for index in 0..self.tail_len {
            self.retain_overflow_edge(self.tail[index]);
            self.tail[index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len = 0;
        self.refresh_external_collection_deadline();
    }
}
