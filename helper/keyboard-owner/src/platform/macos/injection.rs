//! Prebuilt native event injection. Pool ranges and permission gates stay shared;
//! preparation modules configure batches without changing posting order or ownership.

#![cfg_attr(all(test, feature = "local-unsigned-owner"), allow(dead_code))]

use std::ptr::{null, null_mut};

use super::ffi;
use crate::platform::PlatformError;
use talking_quill_keyboard_core::transactional::{
    CleanupBatch, InputSource, JOURNAL_CAPACITY, PhysicalPhase, ReplayBatch, ReplayRecord,
};

const BARRIER_EVENT_COUNT: usize = 2;
const PASTE_BARRIER_EVENT_COUNT: usize = 2;
const REPLAY_POOL_START: usize = 0;
const BARRIER_POOL_START: usize = REPLAY_POOL_START + JOURNAL_CAPACITY;
const PASTE_BARRIER_POOL_START: usize = BARRIER_POOL_START + BARRIER_EVENT_COUNT;
pub(super) const DEFERRED_EDGE_CAPACITY: usize = 64;
pub(super) const DEFERRED_POOL_BANKS: usize = 2;
const DEFERRED_POOL_START: usize = PASTE_BARRIER_POOL_START + PASTE_BARRIER_EVENT_COUNT;
pub(super) const NATIVE_EVENT_POOL_CAPACITY: usize = JOURNAL_CAPACITY
    + BARRIER_EVENT_COUNT
    + PASTE_BARRIER_EVENT_COUNT
    + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventDescriptor {
    event_type: u32,
    key_code: u16,
    key_down: bool,
    repeat: bool,
    flags: u64,
    marker: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventMutation {
    event_type: u32,
    key_code: i64,
    repeat: i64,
    flags: u64,
    marker: i64,
}

fn event_mutation(descriptor: EventDescriptor) -> EventMutation {
    EventMutation {
        event_type: descriptor.event_type,
        key_code: i64::from(descriptor.key_code),
        repeat: i64::from(descriptor.repeat),
        flags: descriptor.flags,
        marker: descriptor.marker,
    }
}

fn posting_is_available() -> bool {
    // SAFETY: permission and Secure Event Input probes take no pointers and do
    // not prompt. Replay must not post while Secure Event Input is active.
    unsafe { ffi::CGPreflightPostEventAccess() && ffi::IsSecureEventInputEnabled() == 0 }
}

mod identity;
pub(super) use identity::InjectionIdentity;
pub(super) use identity::OperationToken;
pub(super) use identity::Submission;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use identity::TEST_PERMISSION_LOSS_MARKER;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use identity::TEST_PHYSICAL_MARKER;
pub(super) use identity::replay_shape_is_valid;
pub(super) use identity::token_matches;
pub(super) use identity::unmarked_source;
mod pool;
pub(super) use pool::NativeEventPool;
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
use pool::*;
mod deferred;
pub(super) use deferred::DeferredEvent;
#[allow(unused_imports)] // Preserve the named prepared-operation API for callers.
pub(super) use deferred::PreparedDeferredEvents;
pub(super) use deferred::prepare_deferred_events;
mod barriers;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use barriers::post_prepared_paste_barrier_up;
pub(super) use barriers::prepare_gap_barrier;
pub(super) use barriers::prepare_paste_barrier;
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
use barriers::*;
#[allow(unused_imports)] // Callers currently infer these prepared-operation types.
pub(super) use barriers::{PreparedGapBarrier, PreparedPasteBarrier};
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
pub(super) use barriers::{post_gap_barrier, post_paste_barrier};
mod replay;
#[allow(unused_imports)]
pub(super) use replay::PreparedReplay;
pub(super) use replay::prepare_cleanup;
pub(super) use replay::prepare_replay;
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
use replay::*;
#[cfg(feature = "transactional-shortcuts-dev")]
mod test_seams;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use test_seams::is_test_physical_marker;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use test_seams::post_test_permission_loss;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use test_seams::post_test_physical_key;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use test_seams::post_test_physical_mouse_down;

#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
mod tests;
