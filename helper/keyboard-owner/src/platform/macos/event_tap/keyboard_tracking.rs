//! Preheld state and ordered gap tombstone reconciliation.

use super::*;

impl CallbackKeyboard {
    pub(super) fn seed_from_state(&mut self, mut is_down: impl FnMut(u16) -> bool) {
        self.physical = MacPhysicalTracker::default();
        self.modifiers = MacModifierTracker::default();
        self.preheld_letters = PreheldLetters::default();
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if is_down(key_code) {
                self.physical.seed(key_code);
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
        for key_code in [ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE] {
            if is_down(key_code) {
                self.physical.seed(key_code);
            }
        }
        for key_code in MODIFIER_KEY_CODES {
            if is_down(key_code) {
                self.modifiers.seed(key_code);
            }
        }
    }

    pub(super) fn merge_current_state_as_preheld(&mut self, mut is_down: impl FnMut(u16) -> bool) {
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if is_down(key_code) {
                self.physical.seed(key_code);
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
        for key_code in [ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE] {
            if is_down(key_code) {
                self.physical.seed(key_code);
            }
        }
        for key_code in MODIFIER_KEY_CODES {
            if is_down(key_code) {
                self.modifiers.seed(key_code);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn tracked_native_state_is_consistent(&self, excluded_key_code: u16) -> bool {
        self.physical
            .native_state_is_consistent_except(excluded_key_code, native_key_is_down)
    }

    pub(super) fn fence_current_letters(&mut self) {
        self.preheld_letters = PreheldLetters::default();
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if self.physical.is_held(key_code) {
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
    }

    pub(super) fn handle_gap_tombstone(
        &mut self,
        key_code: u16,
        phase: KeyPhase,
        is_repeat: bool,
    ) -> bool {
        if phase == KeyPhase::Down && is_repeat {
            // Autorepeat records queued before the hidden physical up remain
            // owned. They neither prove a fresh down nor retire the barrier.
            return if let PhysicalKey::Letter(key) = map_key_code(key_code) {
                self.gap_reconciled_letters & (1_u32 << u32::from(key.index())) != 0
            } else {
                (key_code == ESCAPE_KEY_CODE && self.gap_reconciled_escape)
                    || self.gap_reconciled_enter_key_code == Some(key_code)
            };
        }
        let tombstone = if let PhysicalKey::Letter(key) = map_key_code(key_code) {
            let bit = 1_u32 << u32::from(key.index());
            let present = self.gap_reconciled_letters & bit != 0;
            self.gap_reconciled_letters &= !bit;
            present
        } else if key_code == ESCAPE_KEY_CODE && self.gap_reconciled_escape {
            self.gap_reconciled_escape = false;
            true
        } else if self.gap_reconciled_enter_key_code == Some(key_code) {
            self.gap_reconciled_enter_key_code = None;
            true
        } else {
            false
        };
        if tombstone && !self.gap_tombstones_pending() && self.gap_barrier_token.is_none() {
            // Stream order retired the final uncertain source edge before a
            // barrier was submitted. An already-posted exact pair must remain
            // installed and drain in order even after a fresh down.
            self.gap_barrier_pending = false;
        }
        let captured = tombstone && phase == KeyPhase::Up;
        if captured {
            let _ = self.physical.observe(key_code, KeyPhase::Up);
        }
        captured
    }

    pub(super) fn gap_tombstones_pending(&self) -> bool {
        self.gap_reconciled_letters != 0
            || self.gap_reconciled_escape
            || self.gap_reconciled_enter_key_code.is_some()
    }

    pub(super) fn strict_drain_recovery_needed(&self) -> bool {
        self.transactional.journal_len() != 0
            || self.transactional.owned_letters() != 0
            || self.transactional.pending_injected_cleanup().is_some()
            || self.transactional.pending_menu_cleanup().is_some()
            || has_session_ownership(self)
            || self.gap_tombstones_pending()
            || self.gap_barrier_pending
            || self.gap_barrier_token.is_some()
            || self.replay_observation.is_some()
            || self.inflight_effect.is_some()
    }

    pub(super) fn clear_gap_tombstones(&mut self) {
        self.gap_reconciled_letters = 0;
        self.gap_reconciled_escape = false;
        self.gap_reconciled_enter_key_code = None;
        self.gap_barrier_pending = false;
        self.gap_barrier_token = None;
        self.gap_barrier_observed_down = false;
    }

    pub(super) fn submitted_replay_authority(&self) -> Option<&ReplayObservation> {
        let observation = self.replay_observation.as_ref()?;
        self.inflight_effect
            .as_ref()
            .is_some_and(|inflight| inflight.outcome.is_none())
            .then_some(observation)
    }
}
