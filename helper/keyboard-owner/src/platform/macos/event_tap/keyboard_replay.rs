//! Replay observation and foreground-visible cleanup accounting.

use super::*;

impl CallbackKeyboard {
    pub(super) fn has_submitted_replay_authority(&self) -> bool {
        self.submitted_replay_authority().is_some()
    }

    pub(super) fn begin_replay_observation(
        &mut self,
        batch: ExpectedReplayBatch,
        token: injection::OperationToken,
    ) -> bool {
        if self.replay_observation.is_some() || batch.len() == 0 {
            return false;
        }
        self.replay_observation = Some(ReplayObservation {
            batch,
            token,
            next: 0,
            deadline: Instant::now() + Duration::from_millis(250),
        });
        if matches!(batch, ExpectedReplayBatch::Replay(_)) {
            self.replay_target_changed = false;
            self.replay_visible_downs = 0;
            self.replay_visible_down_records =
                [None; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY];
            self.replay_visible_down_len = 0;
            self.replay_target_cleanup_pending = false;
        }
        true
    }

    pub(super) fn current_replay_record(&self) -> Option<ReplayRecord> {
        let observation = self.replay_observation.as_ref()?;
        observation.batch.record(observation.next)
    }

    pub(super) fn replay_disposition_after_target_check(
        &mut self,
        target_is_current: bool,
    ) -> CurrentEdgeDisposition {
        if !target_is_current {
            self.replay_target_changed = true;
        }
        let Some(record) = self.current_replay_record() else {
            return CurrentEdgeDisposition::Owned;
        };
        let letter_bit = match record.key {
            KeyIdentity::Letter(letter) => 1_u32 << u32::from(letter.index()),
            _ => 0,
        };
        let balancing_modifier_up =
            matches!(record.key, KeyIdentity::Modifier(_)) && record.phase == PhysicalPhase::Up;
        let disposition = if !self.replay_target_changed
            || balancing_modifier_up
            || (record.phase == PhysicalPhase::Up && self.replay_visible_downs & letter_bit != 0)
        {
            CurrentEdgeDisposition::Pass
        } else {
            CurrentEdgeDisposition::Owned
        };
        if disposition == CurrentEdgeDisposition::Pass {
            match record.phase {
                PhysicalPhase::Down => {
                    if letter_bit != 0 && self.replay_visible_downs & letter_bit == 0 {
                        self.replay_visible_down_records[self.replay_visible_down_len] =
                            Some(record);
                        self.replay_visible_down_len += 1;
                    }
                    self.replay_visible_downs |= letter_bit;
                }
                PhysicalPhase::Up => {
                    self.replay_visible_downs &= !letter_bit;
                    if let Some(index) = self.replay_visible_down_records
                        [..self.replay_visible_down_len]
                        .iter()
                        .rposition(|visible| {
                            visible.is_some_and(|visible| visible.key == record.key)
                        })
                    {
                        self.replay_visible_down_records[index] = None;
                    }
                }
                PhysicalPhase::Repeat => {}
            }
        }
        disposition
    }

    pub(super) fn visible_replay_cleanup(&self) -> CleanupBatch {
        CleanupBatch::from_visible_downs(
            &self.replay_visible_down_records[..self.replay_visible_down_len],
        )
    }

    pub(super) fn classify_replay_event(
        &self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        let Some(observation) = self.replay_observation.as_ref() else {
            return GapBarrierObservation::Forged;
        };
        let Some(record) = observation.batch.record(observation.next) else {
            return GapBarrierObservation::Forged;
        };
        let expected_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if record.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let expected_repeat = record.phase == PhysicalPhase::Repeat;
        if event_type != expected_type
            || key_code != record.native.virtual_key
            || repeat != expected_repeat
            || flags != record.native.platform_flags
        {
            GapBarrierObservation::Forged
        } else if observation.next + 1 == observation.batch.len() {
            GapBarrierObservation::Complete
        } else {
            GapBarrierObservation::Down
        }
    }

    pub(super) fn observe_replay_event(
        &mut self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        let classified = self.classify_replay_event(event_type, key_code, repeat, flags);
        if classified == GapBarrierObservation::Forged {
            return classified;
        }
        let observation = self
            .replay_observation
            .as_mut()
            .expect("classified replay observation remains installed");
        observation.next += 1;
        if classified == GapBarrierObservation::Complete {
            self.replay_observation = None;
        }
        classified
    }

    pub(super) fn observe_gap_barrier_event(
        &mut self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        if !self.gap_barrier_pending || key_code != 127 || repeat || flags != 0 {
            return GapBarrierObservation::Forged;
        }
        match (self.gap_barrier_observed_down, event_type) {
            (false, ffi::K_CG_EVENT_KEY_DOWN) => {
                self.gap_barrier_observed_down = true;
                GapBarrierObservation::Down
            }
            (true, ffi::K_CG_EVENT_KEY_UP) => {
                self.clear_gap_tombstones();
                GapBarrierObservation::Complete
            }
            _ => GapBarrierObservation::Forged,
        }
    }
}
