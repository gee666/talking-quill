//! Prepared gap and paste barriers in disjoint native pool ranges.

use super::*;

#[derive(Clone, Copy)]
pub(in crate::platform::macos) struct PreparedGapBarrier {
    pub(super) token: OperationToken,
}

impl PreparedGapBarrier {
    pub(in crate::platform::macos) const fn token(self) -> OperationToken {
        self.token
    }

    pub(in crate::platform::macos) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(BARRIER_POOL_START, BARRIER_EVENT_COUNT);
    }
}

pub(in crate::platform::macos) fn prepare_gap_barrier(
    pool: &mut NativeEventPool,
) -> Option<PreparedGapBarrier> {
    if !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, descriptor) in gap_barrier_descriptors(token).into_iter().enumerate() {
        pool.configure(BARRIER_POOL_START + offset, descriptor);
    }
    Some(PreparedGapBarrier { token })
}

#[cfg(test)]
pub(in crate::platform::macos) fn post_gap_barrier(
    pool: &mut NativeEventPool,
) -> Option<OperationToken> {
    let prepared = prepare_gap_barrier(pool)?;
    prepared.post(pool);
    Some(prepared.token())
}

#[derive(Clone, Copy)]
pub(in crate::platform::macos) struct PreparedPasteBarrier {
    pub(super) token: OperationToken,
}

impl PreparedPasteBarrier {
    pub(in crate::platform::macos) const fn token(self) -> OperationToken {
        self.token
    }

    pub(in crate::platform::macos) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(PASTE_BARRIER_POOL_START, PASTE_BARRIER_EVENT_COUNT);
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(in crate::platform::macos) fn post_down(self, pool: &mut NativeEventPool) {
        pool.post_range(PASTE_BARRIER_POOL_START, 1);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform::macos) fn post_prepared_paste_barrier_up(pool: &mut NativeEventPool) {
    pool.post_range(PASTE_BARRIER_POOL_START + 1, 1);
}

pub(in crate::platform::macos) fn prepare_paste_barrier(
    pool: &mut NativeEventPool,
) -> Option<PreparedPasteBarrier> {
    if !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, descriptor) in paste_barrier_descriptors(token).into_iter().enumerate() {
        pool.configure(PASTE_BARRIER_POOL_START + offset, descriptor);
    }
    Some(PreparedPasteBarrier { token })
}

#[cfg(test)]
pub(in crate::platform::macos) fn post_paste_barrier(
    pool: &mut NativeEventPool,
) -> Option<OperationToken> {
    let prepared = prepare_paste_barrier(pool)?;
    prepared.post(pool);
    Some(prepared.token())
}

pub(super) const fn gap_barrier_descriptors(
    token: OperationToken,
) -> [EventDescriptor; BARRIER_EVENT_COUNT] {
    [
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 127,
            key_down: true,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code: 127,
            key_down: false,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
    ]
}

pub(super) const fn paste_barrier_descriptors(
    token: OperationToken,
) -> [EventDescriptor; PASTE_BARRIER_EVENT_COUNT] {
    // Identical wire shape, but preparation still uses distinct tokens and slots.
    gap_barrier_descriptors(token)
}
