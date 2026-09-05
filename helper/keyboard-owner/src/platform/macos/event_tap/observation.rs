//! Exact replay cursors and authenticated two-edge barriers.

use super::*;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy)]
pub(super) enum ExpectedReplayBatch {
    Replay(ReplayBatch),
    Cleanup(CleanupBatch),
}

impl ExpectedReplayBatch {
    pub(super) const fn len(self) -> usize {
        match self {
            Self::Replay(batch) => batch.len(),
            Self::Cleanup(batch) => batch.len(),
        }
    }

    pub(super) fn record(self, index: usize) -> Option<ReplayRecord> {
        match self {
            Self::Replay(batch) => batch.entries().get(index).copied(),
            Self::Cleanup(batch) => batch.entries().get(index).copied(),
        }
    }
}

pub(super) struct ReplayObservation {
    pub(super) batch: ExpectedReplayBatch,
    pub(super) token: injection::OperationToken,
    pub(super) next: usize,
    pub(super) deadline: Instant,
}

#[cfg(test)]
pub(super) struct SubmittedReplayTerminalSuffix {
    pub(super) suffix: [ReplayRecord; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY],
    pub(super) suffix_len: usize,
}

#[cfg(test)]
impl SubmittedReplayTerminalSuffix {
    pub(super) fn capture(observation: &ReplayObservation) -> Self {
        let mut suffix = [ReplayRecord {
            key: KeyIdentity::Other(0),
            native: NativeKey {
                virtual_key: 0,
                scan_code: 0,
                extended: false,
                platform_flags: 0,
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 0,
        }; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY];
        let mut suffix_len = 0;
        for index in observation.next..observation.batch.len() {
            if let Some(record) = observation.batch.record(index) {
                suffix[suffix_len] = record;
                suffix_len += 1;
            }
        }
        Self { suffix, suffix_len }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GapBarrierObservation {
    Down,
    Complete,
    Forged,
}
pub(super) fn classify_exact_pair(
    state: u8,
    event_type: u32,
    key_code: u16,
    expected_key_code: u16,
    repeat: bool,
    flags: u64,
    expected_flags: u64,
) -> GapBarrierObservation {
    if key_code != expected_key_code || repeat || flags != expected_flags {
        return GapBarrierObservation::Forged;
    }
    match (state, event_type) {
        (0, ffi::K_CG_EVENT_KEY_DOWN) => GapBarrierObservation::Down,
        (1, ffi::K_CG_EVENT_KEY_UP) => GapBarrierObservation::Complete,
        _ => GapBarrierObservation::Forged,
    }
}

pub(super) fn observe_exact_pair(
    state: &mut u8,
    event_type: u32,
    key_code: u16,
    expected_key_code: u16,
    repeat: bool,
    flags: u64,
    expected_flags: u64,
) -> GapBarrierObservation {
    let observation = classify_exact_pair(
        *state,
        event_type,
        key_code,
        expected_key_code,
        repeat,
        flags,
        expected_flags,
    );
    match observation {
        GapBarrierObservation::Down => *state = 1,
        GapBarrierObservation::Complete => *state = 2,
        GapBarrierObservation::Forged => {}
    }
    observation
}
