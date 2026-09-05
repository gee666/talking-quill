//! Scalar cache handles connect the event-tap owner to the AX worker. Retained
//! evidence, observers, and insertion remain worker-owned; shared epochs fence use.

#![cfg_attr(all(test, feature = "local-unsigned-owner"), allow(dead_code))]

use std::{
    collections::HashMap,
    ffi::{CStr, c_void},
    ptr::{null, null_mut},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded};

use super::{
    accessibility::permission_snapshot,
    cf::{OwnedCf, cf_string_sha256_bounded, create_cf_string},
    ffi, secure_input_active,
};
use crate::platform::{
    ClipboardTextHash, PasteFailure, PermissionState, PlatformError, permissions_allow_native_input,
};
use talking_quill_keyboard_core::{ActivationContext, ActivationGeneration, NativeTargetToken};

pub(super) const TARGET_REGISTRY_CAPACITY: usize = 32;
const TARGET_CACHE_REFRESH_INTERVAL: Duration = Duration::from_millis(5);
const TARGET_CACHE_MAX_AGE: Duration = Duration::from_millis(20);
const VALIDATION_QUEUE_CAPACITY: usize = 8;

const AX_FOCUSED_APPLICATION: &CStr = c"AXFocusedApplication";
const AX_FOCUSED_WINDOW: &CStr = c"AXFocusedWindow";
const AX_FOCUSED_UI_ELEMENT: &CStr = c"AXFocusedUIElement";
const AX_WINDOW: &CStr = c"AXWindow";
const AX_SELECTED_TEXT: &CStr = c"AXSelectedText";
const AX_SELECTED_TEXT_RANGE: &CStr = c"AXSelectedTextRange";
const AX_FOCUSED_WINDOW_CHANGED: &CStr = c"AXFocusedWindowChanged";
const AX_FOCUSED_UI_ELEMENT_CHANGED: &CStr = c"AXFocusedUIElementChanged";
const AX_SELECTED_TEXT_CHANGED: &CStr = c"AXSelectedTextChanged";
const AX_SELECTED_TEXT_RANGE_CHANGED: &CStr = c"AXSelectedTextRangeChanged";
const AX_MESSAGING_TIMEOUT_SECONDS: f32 = 0.2;
const AX_MESSAGING_TIMEOUT: Duration = Duration::from_millis(200);
const INSERTION_RESULT_MARGIN: Duration = Duration::from_millis(50);
const WORKSPACE_OBSERVER_CLASS: &CStr = c"TalkingQuillWorkspaceFocusObserverV8";
const WORKSPACE_CALLBACK_SELECTOR: &CStr = c"tqWorkspaceDidActivate:";

struct CachedTarget {
    evidence: TargetEvidence,
    captured_at: Instant,
    notification_epoch: u64,
    boundary_epoch: u64,
    selected_range_epoch: u64,
    publication_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActivationReservation {
    notification_epoch: u64,
    boundary_epoch: u64,
    selected_range_epoch: u64,
    publication_id: u64,
}

impl ActivationReservation {
    #[cfg(test)]
    pub(super) fn confirms_after_boundary(
        self,
        observed_epoch: u64,
        observed_boundary_epoch: u64,
        current_epoch: u64,
        current_boundary_epoch: u64,
        worker_confirmed: bool,
    ) -> bool {
        worker_confirmed
            && self.notification_epoch == observed_epoch
            && observed_epoch == current_epoch
            && self.boundary_epoch.checked_add(1) == Some(observed_boundary_epoch)
            && observed_boundary_epoch == current_boundary_epoch
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TargetHandle {
    publication_id: u64,
}

#[cfg(test)]
pub(super) const fn target_handle_for_test(publication_id: u64) -> TargetHandle {
    TargetHandle { publication_id }
}

struct ConfirmedTarget {
    evidence: TargetEvidence,
    notification_epoch: u64,
    boundary_epoch: u64,
    selected_range_epoch: u64,
}

struct CacheShared {
    slot: Mutex<Option<CachedTarget>>,
    notification_epoch: AtomicU64,
    boundary_epoch: AtomicU64,
    selected_range_epoch: AtomicU64,
    stopping: AtomicBool,
    published_id: AtomicU64,
}

impl CacheShared {
    fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            notification_epoch: AtomicU64::new(1),
            boundary_epoch: AtomicU64::new(1),
            selected_range_epoch: AtomicU64::new(1),
            stopping: AtomicBool::new(false),
            published_id: AtomicU64::new(0),
        }
    }

    fn invalidate_notification(&self) {
        self.notification_epoch.fetch_add(1, Ordering::AcqRel);
        self.invalidate_boundary();
    }

    fn invalidate_selected_range(&self) {
        self.selected_range_epoch.fetch_add(1, Ordering::AcqRel);
        self.invalidate_notification();
    }

    fn invalidate_boundary(&self) {
        self.boundary_epoch.fetch_add(1, Ordering::AcqRel);
        // Never clear retained CF evidence from an event callback. Epochs make
        // it unusable immediately; the AX worker replaces or releases it.
    }

    fn current_notification_epoch(&self) -> u64 {
        self.notification_epoch.load(Ordering::Acquire)
    }

    fn current_boundary_epoch(&self) -> u64 {
        self.boundary_epoch.load(Ordering::Acquire)
    }

    fn current_selected_range_epoch(&self) -> u64 {
        self.selected_range_epoch.load(Ordering::Acquire)
    }
}

mod capture;
pub(super) use capture::TargetEvidence;
pub(super) use capture::same_target;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use capture::test_target_caret_identity_contract;
use capture::*;
mod validation;
pub(super) use validation::ValidationRequest;
pub(super) use validation::ValidationResponse;
pub(super) use validation::ValidationTicket;
use validation::*;
mod cache;
pub(super) use cache::TargetCache;
mod workspace;
use workspace::*;
mod observer;
use observer::*;
mod worker;
use worker::*;
mod clipboard;
use clipboard::*;
mod insertion_state;
pub(super) use insertion_state::InsertionRequest;
pub(super) use insertion_state::InsertionStatus;
use insertion_state::*;
mod insertion;
use insertion::*;
#[cfg(feature = "transactional-shortcuts-dev")]
mod test_seams;
#[cfg(feature = "transactional-shortcuts-dev")]
use test_seams::*;
mod registry;
pub(super) use registry::TargetRegistry;
#[cfg(test)]
use registry::*;
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
pub(super) use registry::{activation_reservation_for_test, validation_request_for_test};

#[cfg(test)]
mod tests;
