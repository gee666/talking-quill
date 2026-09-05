//! Deferred keyboard and mouse snapshots and bank preparation.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::platform::macos) struct DeferredEvent {
    pub(in crate::platform::macos) event_type: u32,
    pub(in crate::platform::macos) key_code: u16,
    pub(in crate::platform::macos) repeat: bool,
    pub(in crate::platform::macos) is_down: bool,
    pub(in crate::platform::macos) flags: u64,
    pub(in crate::platform::macos) keyboard_type: i64,
    pub(in crate::platform::macos) original_timestamp: ffi::CGEventTimestamp,
    pub(in crate::platform::macos) source: InputSource,
    pub(in crate::platform::macos) source_pid: i64,
    pub(in crate::platform::macos) original_marker: i64,
    pub(in crate::platform::macos) generation: u32,
    pub(in crate::platform::macos) uncertain_owned: bool,
    pub(in crate::platform::macos) foreground_balance: bool,
    pub(in crate::platform::macos) hidden_generation: bool,
    pub(in crate::platform::macos) location: ffi::CGPoint,
    pub(in crate::platform::macos) mouse_number: i64,
    pub(in crate::platform::macos) mouse_click_state: i64,
    pub(in crate::platform::macos) mouse_pressure: f64,
    pub(in crate::platform::macos) mouse_button: i64,
    pub(in crate::platform::macos) mouse_delta_x: i64,
    pub(in crate::platform::macos) mouse_delta_y: i64,
    pub(in crate::platform::macos) mouse_instant_mouser: i64,
    pub(in crate::platform::macos) mouse_subtype: i64,
}

impl DeferredEvent {
    pub(in crate::platform::macos) const EMPTY: Self = Self {
        event_type: 0,
        key_code: 0,
        repeat: false,
        is_down: false,
        flags: 0,
        keyboard_type: 0,
        original_timestamp: 0,
        source: InputSource::External,
        source_pid: 0,
        original_marker: 0,
        generation: 0,
        uncertain_owned: false,
        foreground_balance: false,
        hidden_generation: false,
        location: ffi::CGPoint { x: 0.0, y: 0.0 },
        mouse_number: 0,
        mouse_click_state: 0,
        mouse_pressure: 0.0,
        mouse_button: 0,
        mouse_delta_x: 0,
        mouse_delta_y: 0,
        mouse_instant_mouser: 0,
        mouse_subtype: 0,
    };

    pub(in crate::platform::macos) const fn is_mouse(self) -> bool {
        matches!(
            self.event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
                | ffi::K_CG_EVENT_LEFT_MOUSE_UP
                | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
                | ffi::K_CG_EVENT_RIGHT_MOUSE_UP
                | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
                | ffi::K_CG_EVENT_OTHER_MOUSE_UP
        )
    }
}

#[derive(Clone, Copy)]
pub(in crate::platform::macos) struct PreparedDeferredEvents {
    pub(super) start: usize,
    pub(super) count: usize,
    pub(super) token: OperationToken,
}

impl PreparedDeferredEvents {
    pub(in crate::platform::macos) const fn token(self) -> OperationToken {
        self.token
    }

    pub(in crate::platform::macos) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(self.start, self.count);
    }
}

pub(in crate::platform::macos) fn prepare_deferred_events(
    pool: &mut NativeEventPool,
    bank: usize,
    edges: &[DeferredEvent],
) -> Option<PreparedDeferredEvents> {
    if bank >= DEFERRED_POOL_BANKS
        || edges.is_empty()
        || edges.len() > DEFERRED_EDGE_CAPACITY
        || !posting_is_available()
    {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, edge) in edges.iter().copied().enumerate() {
        pool.configure_deferred(bank, offset, edge, token.marker);
    }
    Some(PreparedDeferredEvents {
        start: DEFERRED_POOL_START + bank * DEFERRED_EDGE_CAPACITY,
        count: edges.len(),
        token,
    })
}
