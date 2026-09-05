//! Replay and cleanup preparation from transactional records.

use super::*;

#[derive(Clone, Copy)]
pub(in crate::platform::macos) struct PreparedReplay {
    pub(super) count: usize,
    pub(super) token: OperationToken,
}

impl PreparedReplay {
    pub(in crate::platform::macos) const fn submission(self) -> Submission {
        Submission {
            count: self.count,
            token: Some(self.token),
        }
    }

    pub(in crate::platform::macos) fn post(
        self,
        pool: &mut NativeEventPool,
        proxy: Option<ffi::CGEventTapProxy>,
    ) {
        if let Some(proxy) = proxy {
            pool.post_range_at_proxy(REPLAY_POOL_START, self.count, proxy);
        } else {
            pool.post_range(REPLAY_POOL_START, self.count);
        }
    }
}

pub(in crate::platform::macos) fn prepare_replay(
    pool: &mut NativeEventPool,
    batch: ReplayBatch,
) -> Option<PreparedReplay> {
    prepare_records(pool, batch.entries())
}

pub(in crate::platform::macos) fn prepare_cleanup(
    pool: &mut NativeEventPool,
    batch: CleanupBatch,
) -> Option<PreparedReplay> {
    prepare_records(pool, batch.entries())
}

pub(super) fn prepare_records(
    pool: &mut NativeEventPool,
    records: &[ReplayRecord],
) -> Option<PreparedReplay> {
    if records.is_empty() || records.len() > JOURNAL_CAPACITY || !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, record) in records.iter().copied().enumerate() {
        pool.configure(
            REPLAY_POOL_START + offset,
            replay_descriptor(record, token.marker),
        );
    }
    Some(PreparedReplay {
        count: records.len(),
        token,
    })
}

pub(super) fn replay_descriptor(record: ReplayRecord, marker: i64) -> EventDescriptor {
    EventDescriptor {
        event_type: if matches!(
            record.key,
            talking_quill_keyboard_core::transactional::KeyIdentity::Modifier(_)
        ) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if record.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        },
        key_code: record.native.virtual_key,
        key_down: record.phase != PhysicalPhase::Up,
        repeat: record.phase == PhysicalPhase::Repeat,
        flags: record.native.platform_flags,
        marker,
    }
}
