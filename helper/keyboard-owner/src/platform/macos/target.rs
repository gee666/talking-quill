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

pub(super) struct TargetEvidence {
    process_id: i32,
    application: OwnedCf,
    window: OwnedCf,
    focused_control: OwnedCf,
    selected_text_range_value: OwnedCf,
    selected_text_range: ffi::CFRange,
}

// SAFETY: AXUIElement and Core Foundation references may be retained, released,
// and compared across threads. AX messaging itself is confined to the cache
// worker; event-tap and owner code only move evidence and call local CFEqual.
unsafe impl Send for TargetEvidence {}

impl TargetEvidence {
    fn retained_clone(&self) -> Result<Self, PlatformError> {
        Ok(Self {
            process_id: self.process_id,
            application: self.application.retained_clone()?,
            window: self.window.retained_clone()?,
            focused_control: self.focused_control.retained_clone()?,
            selected_text_range_value: self.selected_text_range_value.retained_clone()?,
            selected_text_range: self.selected_text_range,
        })
    }
}

struct TargetCaptureResources {
    system: OwnedCf,
    focused_application: OwnedCf,
    focused_window: OwnedCf,
    focused_ui_element: OwnedCf,
    window: OwnedCf,
    selected_text: OwnedCf,
    selected_text_range: OwnedCf,
    focused_window_changed: OwnedCf,
    focused_ui_element_changed: OwnedCf,
    selected_text_changed: OwnedCf,
    selected_text_range_changed: OwnedCf,
}

impl TargetCaptureResources {
    fn new() -> Result<Self, PlatformError> {
        let focused_application = create_cf_string(AX_FOCUSED_APPLICATION)?;
        let focused_window = create_cf_string(AX_FOCUSED_WINDOW)?;
        let focused_ui_element = create_cf_string(AX_FOCUSED_UI_ELEMENT)?;
        let window = create_cf_string(AX_WINDOW)?;
        let selected_text = create_cf_string(AX_SELECTED_TEXT)?;
        let selected_text_range = create_cf_string(AX_SELECTED_TEXT_RANGE)?;
        let focused_window_changed = create_cf_string(AX_FOCUSED_WINDOW_CHANGED)?;
        let focused_ui_element_changed = create_cf_string(AX_FOCUSED_UI_ELEMENT_CHANGED)?;
        let selected_text_changed = create_cf_string(AX_SELECTED_TEXT_CHANGED)?;
        let selected_text_range_changed = create_cf_string(AX_SELECTED_TEXT_RANGE_CHANGED)?;
        // SAFETY: the system-wide element follows the Create rule. This runs on
        // the dedicated AX worker, never the event-tap owner.
        let system = OwnedCf::from_created(
            unsafe { ffi::AXUIElementCreateSystemWide() }
                .cast_const()
                .cast(),
        )?;
        set_ax_messaging_timeout(&system)?;
        Ok(Self {
            system,
            focused_application,
            focused_window,
            focused_ui_element,
            window,
            selected_text,
            selected_text_range,
            focused_window_changed,
            focused_ui_element_changed,
            selected_text_changed,
            selected_text_range_changed,
        })
    }
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ValidationTicket {
    request_id: u64,
    start_epoch: u64,
    start_boundary_epoch: u64,
}

impl ValidationTicket {
    pub(super) const fn start_epoch(self) -> u64 {
        self.start_epoch
    }

    pub(super) const fn start_boundary_epoch(self) -> u64 {
        self.start_boundary_epoch
    }
}

const VALIDATION_SLOT_FREE: u64 = 0;
const VALIDATION_SLOT_PENDING: u64 = 1;
const VALIDATION_SLOT_READY: u64 = 2;
const VALIDATION_SLOT_STATE_BITS: u32 = 2;
const VALIDATION_SLOT_STATE_MASK: u64 = (1 << VALIDATION_SLOT_STATE_BITS) - 1;
const VALIDATION_SLOT_MAX_GENERATION: u64 = u64::MAX >> VALIDATION_SLOT_STATE_BITS;

fn validation_slot_word(generation: u64, state: u64) -> u64 {
    (generation << VALIDATION_SLOT_STATE_BITS) | state
}

fn validation_slot_generation(word: u64) -> u64 {
    word >> VALIDATION_SLOT_STATE_BITS
}

fn validation_slot_state(word: u64) -> u64 {
    word & VALIDATION_SLOT_STATE_MASK
}

struct ValidationSlot {
    word: AtomicU64,
    response: Mutex<Option<ValidationResponse>>,
}

impl ValidationSlot {
    fn new() -> Self {
        Self {
            word: AtomicU64::new(validation_slot_word(0, VALIDATION_SLOT_FREE)),
            response: Mutex::new(None),
        }
    }
}

struct ValidationPool {
    slots: [ValidationSlot; VALIDATION_QUEUE_CAPACITY],
    stopped: AtomicBool,
}

impl ValidationPool {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| ValidationSlot::new()),
            stopped: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn acquire(
        self: &Arc<Self>,
        ticket: ValidationTicket,
    ) -> Option<(ValidationRequest, ValidationWork)> {
        self.acquire_expected(ticket, 0)
    }

    fn acquire_expected(
        self: &Arc<Self>,
        ticket: ValidationTicket,
        expected_publication_id: u64,
    ) -> Option<(ValidationRequest, ValidationWork)> {
        if self.stopped.load(Ordering::Acquire) {
            return None;
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let current = slot.word.load(Ordering::Acquire);
            if validation_slot_state(current) != VALIDATION_SLOT_FREE {
                continue;
            }
            let generation = validation_slot_generation(current);
            if generation == VALIDATION_SLOT_MAX_GENERATION {
                continue;
            }
            let generation = generation + 1;
            if slot
                .word
                .compare_exchange(
                    current,
                    validation_slot_word(generation, VALIDATION_SLOT_PENDING),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                if self.stopped.load(Ordering::Acquire) {
                    let _ = slot.word.compare_exchange(
                        validation_slot_word(generation, VALIDATION_SLOT_PENDING),
                        validation_slot_word(generation, VALIDATION_SLOT_FREE),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    return None;
                }
                let index = u8::try_from(index).expect("validation slot capacity fits u8");
                return Some((
                    ValidationRequest {
                        ticket,
                        slot_index: index,
                        slot_generation: generation,
                        pool: Arc::clone(self),
                        active: true,
                    },
                    ValidationWork {
                        ticket,
                        slot_index: index,
                        slot_generation: generation,
                        expected_publication_id,
                    },
                ));
            }
        }
        None
    }

    fn is_pending(&self, work: ValidationWork) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        self.slots[usize::from(work.slot_index)]
            .word
            .load(Ordering::Acquire)
            == validation_slot_word(work.slot_generation, VALIDATION_SLOT_PENDING)
    }

    fn publish(&self, work: ValidationWork, response: ValidationResponse) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        let slot = &self.slots[usize::from(work.slot_index)];
        let pending = validation_slot_word(work.slot_generation, VALIDATION_SLOT_PENDING);
        if slot.word.load(Ordering::Acquire) != pending {
            return false;
        }
        let mut stored = slot
            .response
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.word.load(Ordering::Acquire) != pending {
            return false;
        }
        *stored = Some(response);
        slot.word
            .compare_exchange(
                pending,
                validation_slot_word(work.slot_generation, VALIDATION_SLOT_READY),
                Ordering::Release,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn release(&self, slot_index: u8, slot_generation: u64) {
        let slot = &self.slots[usize::from(slot_index)];
        for state in [VALIDATION_SLOT_PENDING, VALIDATION_SLOT_READY] {
            if slot
                .word
                .compare_exchange(
                    validation_slot_word(slot_generation, state),
                    validation_slot_word(slot_generation, VALIDATION_SLOT_FREE),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub(super) struct ValidationRequest {
    ticket: ValidationTicket,
    slot_index: u8,
    slot_generation: u64,
    pool: Arc<ValidationPool>,
    active: bool,
}

impl ValidationRequest {
    pub(super) const fn ticket(&self) -> ValidationTicket {
        self.ticket
    }

    pub(super) fn try_response(&mut self) -> Option<ValidationResponse> {
        if !self.active {
            return None;
        }
        let slot = &self.pool.slots[usize::from(self.slot_index)];
        let ready = validation_slot_word(self.slot_generation, VALIDATION_SLOT_READY);
        if slot.word.load(Ordering::Acquire) != ready {
            return None;
        }
        let mut stored = match slot.response.try_lock() {
            Ok(stored) => stored,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if slot.word.load(Ordering::Acquire) != ready {
            return None;
        }
        let response = stored.take()?;
        if slot
            .word
            .compare_exchange(
                ready,
                validation_slot_word(self.slot_generation, VALIDATION_SLOT_FREE),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            *stored = Some(response);
            return None;
        }
        self.active = false;
        Some(response)
    }
}

impl Drop for ValidationRequest {
    fn drop(&mut self) {
        if self.active {
            self.pool.release(self.slot_index, self.slot_generation);
            self.active = false;
        }
    }
}

#[derive(Clone, Copy)]
struct ValidationWork {
    ticket: ValidationTicket,
    slot_index: u8,
    slot_generation: u64,
    expected_publication_id: u64,
}

const INSERTION_PENDING: u8 = 0;
const INSERTION_CLAIMED: u8 = 1;
const INSERTION_SUCCEEDED: u8 = 2;
const INSERTION_FAILED_UNAVAILABLE: u8 = 3;
const INSERTION_FAILED_PERMISSION: u8 = 4;
const INSERTION_FAILED_SECURE_INPUT: u8 = 5;
const INSERTION_FAILED_OS_REJECTED: u8 = 6;
const INSERTION_CANCELLED: u8 = 7;
const INSERTION_AMBIGUOUS: u8 = 8;
const INSERTION_TARGET_INVALID: u8 = 9;

struct InsertionWork {
    publication_id: u64,
    notification_epoch: u64,
    boundary_epoch: u64,
    selected_range_epoch: u64,
    expected_clipboard_sha256: ClipboardTextHash,
    deadline: Instant,
    state: Arc<AtomicU8>,
}

pub(super) struct InsertionRequest {
    work: Option<InsertionWork>,
    state: Arc<AtomicU8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InsertionStatus {
    Pending,
    Claimed,
    Succeeded,
    Failed(PasteFailure),
    TargetInvalid,
    Ambiguous,
}

fn insertion_status(state: u8) -> InsertionStatus {
    match state {
        INSERTION_CLAIMED => InsertionStatus::Claimed,
        INSERTION_SUCCEEDED => InsertionStatus::Succeeded,
        INSERTION_FAILED_PERMISSION => InsertionStatus::Failed(PasteFailure::PermissionDenied),
        INSERTION_FAILED_SECURE_INPUT => InsertionStatus::Failed(PasteFailure::SecureInput),
        INSERTION_FAILED_OS_REJECTED => InsertionStatus::Failed(PasteFailure::OsRejected),
        INSERTION_FAILED_UNAVAILABLE | INSERTION_CANCELLED => {
            InsertionStatus::Failed(PasteFailure::Unavailable)
        }
        INSERTION_AMBIGUOUS => InsertionStatus::Ambiguous,
        INSERTION_TARGET_INVALID => InsertionStatus::TargetInvalid,
        _ => InsertionStatus::Pending,
    }
}

fn insertion_failure_state(reason: PasteFailure) -> u8 {
    match reason {
        PasteFailure::PermissionDenied => INSERTION_FAILED_PERMISSION,
        PasteFailure::SecureInput => INSERTION_FAILED_SECURE_INPUT,
        PasteFailure::OsRejected => INSERTION_FAILED_OS_REJECTED,
        PasteFailure::ConflictingModifiers
        | PasteFailure::Unavailable
        | PasteFailure::Indeterminate => INSERTION_FAILED_UNAVAILABLE,
    }
}

fn insertion_completion_budget_available(deadline: Instant, now: Instant) -> bool {
    deadline.saturating_duration_since(now) >= AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN
}

impl InsertionRequest {
    pub(super) fn status(&self) -> InsertionStatus {
        insertion_status(self.state.load(Ordering::Acquire))
    }

    fn submit(&mut self, sender: &Sender<InsertionWork>) -> bool {
        let Some(work) = self.work.take() else {
            return false;
        };
        match sender.try_send(work) {
            Ok(()) => true,
            Err(error) => {
                self.work = Some(error.into_inner());
                false
            }
        }
    }

    pub(super) fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                INSERTION_PENDING,
                INSERTION_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Drop for InsertionRequest {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) struct ValidationResponse {
    ticket: ValidationTicket,
    handle: Option<TargetHandle>,
    observed_epoch: u64,
    observed_boundary_epoch: u64,
    observed_selected_range_epoch: u64,
}

impl ValidationResponse {
    pub(super) fn into_current_handle(
        self,
        expected: ValidationTicket,
        current_epoch: u64,
        current_boundary_epoch: u64,
        current_selected_range_epoch: u64,
    ) -> Option<(TargetHandle, u64, u64, u64, bool)> {
        (self.ticket == expected
            && self.ticket.start_epoch == self.observed_epoch
            && self.observed_epoch == current_epoch
            && self.ticket.start_boundary_epoch == self.observed_boundary_epoch
            && self.observed_boundary_epoch == current_boundary_epoch
            && self.observed_selected_range_epoch == current_selected_range_epoch)
            .then_some(self.handle.map(|handle| {
                (
                    handle,
                    self.observed_epoch,
                    self.observed_boundary_epoch,
                    self.observed_selected_range_epoch,
                    true,
                )
            }))
            .flatten()
    }
}

/// A cache populated and owned exclusively by the dedicated AX worker. Event
/// callbacks reserve only a nondestructive scalar publication ID plus epochs;
/// they never move, retain, compare, or release CF evidence. Validation returns
/// scalar handles, and all retained evidence teardown stays on the AX worker.
pub(super) struct TargetCache {
    shared: Arc<CacheShared>,
    validation_pool: Arc<ValidationPool>,
    requests: Sender<ValidationWork>,
    insertions: Sender<InsertionWork>,
    next_request_id: AtomicU64,
    worker: Option<JoinHandle<()>>,
    worker_completion: Receiver<()>,
    #[cfg(test)]
    _test_request_receiver: Option<Receiver<ValidationWork>>,
}

impl TargetCache {
    pub(super) fn start() -> Result<Self, PlatformError> {
        let shared = Arc::new(CacheShared::new());
        let validation_pool = Arc::new(ValidationPool::new());
        let (requests, request_receiver) = bounded(VALIDATION_QUEUE_CAPACITY);
        let (insertions, insertion_receiver) = bounded(1);
        let worker_shared = Arc::clone(&shared);
        let worker_pool = Arc::clone(&validation_pool);
        let (worker_completion_tx, worker_completion) = bounded(1);
        let worker = thread::Builder::new()
            .name("talking-quill-helper-macos-ax-cache".into())
            .spawn(move || {
                target_cache_worker(
                    worker_shared,
                    worker_pool,
                    request_receiver,
                    insertion_receiver,
                );
                let _ = worker_completion_tx.try_send(());
            })
            .map_err(|_| PlatformError::ThreadStopped)?;
        Ok(Self {
            shared,
            validation_pool,
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: Some(worker),
            worker_completion,
            #[cfg(test)]
            _test_request_receiver: None,
        })
    }

    pub(super) fn invalidate_boundary(&self) {
        self.shared.invalidate_boundary();
    }

    pub(super) fn reserve_activation(&self) -> Option<ActivationReservation> {
        self.reserve_activation_at(Instant::now())
    }

    #[cfg(test)]
    fn request_validation(&self) -> Option<ValidationRequest> {
        self.request_validation_from(self.current_epoch(), 1)
    }

    pub(super) fn request_target_validation(
        &self,
        target: TargetHandle,
    ) -> Option<ValidationRequest> {
        self.request_validation_from(self.current_epoch(), target.publication_id)
    }

    pub(super) fn request_activation_validation(
        &self,
        reservation: &ActivationReservation,
    ) -> Option<ValidationRequest> {
        self.request_validation_from(reservation.notification_epoch, reservation.publication_id)
    }

    /// Callback-safe scalar fence for candidate-start evidence. Broad focus and
    /// selected-range observers invalidate these epochs before any replay or
    /// target-sensitive operation may use the retained publication.
    pub(super) fn reservation_is_current(&self, reservation: &ActivationReservation) -> bool {
        if self.current_epoch() != reservation.notification_epoch
            || self.current_selected_range_epoch() != reservation.selected_range_epoch
            || self.shared.published_id.load(Ordering::Acquire) != reservation.publication_id
            || self.shared.stopping.load(Ordering::Acquire)
        {
            return false;
        }
        let slot = match self.shared.slot.try_lock() {
            Ok(slot) => slot,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        slot.as_ref().is_some_and(|cached| {
            cached.publication_id == reservation.publication_id
                && cached.notification_epoch == reservation.notification_epoch
                && cached.selected_range_epoch == reservation.selected_range_epoch
                && Instant::now().saturating_duration_since(cached.captured_at)
                    <= TARGET_CACHE_MAX_AGE
        })
    }

    pub(super) fn prepare_insertion(
        &self,
        target: TargetHandle,
        notification_epoch: u64,
        boundary_epoch: u64,
        selected_range_epoch: u64,
        expected_clipboard_sha256: ClipboardTextHash,
        deadline: Instant,
    ) -> Option<InsertionRequest> {
        self.handle_is_current(
            target,
            notification_epoch,
            boundary_epoch,
            selected_range_epoch,
        )
        .then(|| {
            let state = Arc::new(AtomicU8::new(INSERTION_PENDING));
            InsertionRequest {
                work: Some(InsertionWork {
                    publication_id: target.publication_id,
                    notification_epoch,
                    boundary_epoch,
                    selected_range_epoch,
                    expected_clipboard_sha256,
                    deadline,
                    state: Arc::clone(&state),
                }),
                state,
            }
        })
    }

    pub(super) fn submit_insertion(&self, request: &mut InsertionRequest) -> bool {
        request.submit(&self.insertions)
    }

    fn request_validation_from(
        &self,
        start_epoch: u64,
        expected_publication_id: u64,
    ) -> Option<ValidationRequest> {
        // Callback-safe: slots and the work queue were allocated at startup.
        // Acquisition is a bounded atomic scan; Arc clone and try_send neither
        // allocate nor wait, and every failure recycles the claimed slot.
        let request_id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        let ticket = ValidationTicket {
            request_id,
            start_epoch,
            start_boundary_epoch: self.current_boundary_epoch(),
        };
        let (request, work) = self
            .validation_pool
            .acquire_expected(ticket, expected_publication_id)?;
        if self.requests.try_send(work).is_ok() {
            Some(request)
        } else {
            drop(request);
            None
        }
    }

    pub(super) fn request_stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        self.validation_pool.stop();
        self.shared.invalidate_notification();
    }

    pub(super) fn current_epoch(&self) -> u64 {
        self.shared.current_notification_epoch()
    }

    pub(super) fn current_boundary_epoch(&self) -> u64 {
        self.shared.current_boundary_epoch()
    }

    pub(super) fn current_selected_range_epoch(&self) -> u64 {
        self.shared.current_selected_range_epoch()
    }

    pub(super) fn handle_is_current(
        &self,
        target: TargetHandle,
        validated_epoch: u64,
        validated_boundary_epoch: u64,
        validated_selected_range_epoch: u64,
    ) -> bool {
        self.current_epoch() == validated_epoch
            && self.current_boundary_epoch() == validated_boundary_epoch
            && self.current_selected_range_epoch() == validated_selected_range_epoch
            && self.shared.published_id.load(Ordering::Acquire) == target.publication_id
            && !self.shared.stopping.load(Ordering::Acquire)
    }

    fn reserve_activation_at(&self, now: Instant) -> Option<ActivationReservation> {
        let notification_before = self.shared.current_notification_epoch();
        let boundary_before = self.shared.current_boundary_epoch();
        let selected_range_before = self.shared.current_selected_range_epoch();
        let slot = match self.shared.slot.try_lock() {
            Ok(slot) => slot,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let cached = slot.as_ref()?;
        let notification_after = self.shared.current_notification_epoch();
        let boundary_after = self.shared.current_boundary_epoch();
        let selected_range_after = self.shared.current_selected_range_epoch();
        if notification_before != notification_after
            || boundary_before != boundary_after
            || selected_range_before != selected_range_after
            || cached.notification_epoch != notification_after
            || cached.boundary_epoch != boundary_after
            || cached.selected_range_epoch != selected_range_after
            || cached.publication_id == 0
            || self.shared.published_id.load(Ordering::Acquire) != cached.publication_id
            || now.saturating_duration_since(cached.captured_at) > TARGET_CACHE_MAX_AGE
        {
            return None;
        }
        Some(ActivationReservation {
            notification_epoch: cached.notification_epoch,
            boundary_epoch: cached.boundary_epoch,
            selected_range_epoch: cached.selected_range_epoch,
            publication_id: cached.publication_id,
        })
    }

    #[cfg(test)]
    pub(super) fn install_current_handle_for_test(
        &self,
        target: TargetHandle,
        notification_epoch: u64,
        boundary_epoch: u64,
    ) {
        self.shared
            .notification_epoch
            .store(notification_epoch, Ordering::Release);
        self.shared
            .boundary_epoch
            .store(boundary_epoch, Ordering::Release);
        self.shared.selected_range_epoch.store(1, Ordering::Release);
        self.shared
            .published_id
            .store(target.publication_id, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn with_open_validation_queue_for_test() -> Self {
        let (requests, request_receiver) = bounded(VALIDATION_QUEUE_CAPACITY);
        let (insertions, _insertion_receiver) = bounded(1);
        Self {
            shared: Arc::new(CacheShared::new()),
            validation_pool: Arc::new(ValidationPool::new()),
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: None,
            worker_completion: bounded(1).1,
            _test_request_receiver: Some(request_receiver),
        }
    }

    #[cfg(test)]
    fn without_worker() -> Self {
        let (requests, request_receiver) = bounded(VALIDATION_QUEUE_CAPACITY);
        let (insertions, _insertion_receiver) = bounded(1);
        Self {
            shared: Arc::new(CacheShared::new()),
            validation_pool: Arc::new(ValidationPool::new()),
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: None,
            worker_completion: bounded(1).1,
            _test_request_receiver: Some(request_receiver),
        }
    }

    #[cfg(test)]
    fn publish_for_test(&self, evidence: TargetEvidence, captured_at: Instant) {
        let notification_epoch = self.shared.current_notification_epoch();
        let publication_id = self
            .shared
            .published_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *self.shared.slot.lock().unwrap() = Some(CachedTarget {
            evidence,
            captured_at,
            notification_epoch,
            boundary_epoch: self.shared.current_boundary_epoch(),
            selected_range_epoch: self.shared.current_selected_range_epoch(),
            publication_id,
        });
    }
}

impl Drop for TargetCache {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take()
            && (self
                .worker_completion
                .recv_timeout(Duration::from_millis(500))
                .is_ok()
                || worker.is_finished())
        {
            let _ = worker.join();
        }
        // A nonresponsive AX process cannot retain helper shutdown forever.
        // Every AX message is separately bounded; if the worker still misses
        // this outer deadline its Arc-owned evidence/refcons outlive this cache
        // safely and process exit terminates the detached worker.
    }
}

struct WorkspaceCallbackEntry {
    shared: Arc<CacheShared>,
    active: usize,
    retiring: bool,
}

#[derive(Default)]
struct WorkspaceCallbackRegistry {
    entries: HashMap<usize, WorkspaceCallbackEntry>,
}

fn workspace_callback_registry() -> &'static (Mutex<WorkspaceCallbackRegistry>, Condvar) {
    static REGISTRY: OnceLock<(Mutex<WorkspaceCallbackRegistry>, Condvar)> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        (
            Mutex::new(WorkspaceCallbackRegistry::default()),
            Condvar::new(),
        )
    })
}

struct WorkspaceInvocation {
    observer: usize,
    shared: Arc<CacheShared>,
}

impl WorkspaceInvocation {
    fn begin(observer: ffi::ObjcId) -> Option<Self> {
        let observer = observer as usize;
        let (registry, _) = workspace_callback_registry();
        let mut registry = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = registry.entries.get_mut(&observer)?;
        if entry.retiring {
            return None;
        }
        // Hold an Objective-C +1 before leaving registry admission. Teardown
        // cannot reach its final release until this active count drains, so
        // the receiver cannot deallocate during the selector body.
        unsafe {
            let _ = ffi::objc_msgSend(
                observer as ffi::ObjcId,
                ffi::sel_registerName(c"retain".as_ptr()),
            );
        }
        entry.active += 1;
        Some(Self {
            observer,
            shared: Arc::clone(&entry.shared),
        })
    }
}

impl Drop for WorkspaceInvocation {
    fn drop(&mut self) {
        let (registry, drained) = workspace_callback_registry();
        let mut registry = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = registry.entries.get_mut(&self.observer) {
            entry.active = entry.active.saturating_sub(1);
            if entry.active == 0 {
                drained.notify_all();
            }
        }
        drop(registry);
        // SAFETY: begin acquired this exact callback-owned +1 while teardown
        // was excluded by the registry lock.
        unsafe {
            let _ = ffi::objc_msgSend(
                self.observer as ffi::ObjcId,
                ffi::sel_registerName(c"release".as_ptr()),
            );
        }
    }
}

unsafe extern "C" fn workspace_activation_callback(
    observer: ffi::ObjcId,
    _selector: ffi::ObjcSel,
    _notification: ffi::ObjcId,
) {
    // The registry admission is the callback-lifetime synchronization point.
    // Teardown unregisters first, closes this admission, waits for every
    // admitted invocation, and only then releases the Objective-C receiver.
    if let Some(invocation) = WorkspaceInvocation::begin(observer) {
        invocation.shared.invalidate_notification();
    }
}

struct WorkspaceObserver {
    observer: ffi::ObjcId,
    notification_center: ffi::ObjcId,
}

impl WorkspaceObserver {
    fn install(shared: &Arc<CacheShared>) -> Result<Self, PlatformError> {
        // SAFETY: Objective-C runtime class and selector names are static C
        // strings. Registration happens once per helper process.
        let mut class = unsafe { ffi::objc_getClass(WORKSPACE_OBSERVER_CLASS.as_ptr()) };
        if class.is_null() {
            let superclass = unsafe { ffi::objc_getClass(c"NSObject".as_ptr()) };
            if superclass.is_null() {
                return Err(PlatformError::NativeFailure);
            }
            class = unsafe {
                ffi::objc_allocateClassPair(superclass, WORKSPACE_OBSERVER_CLASS.as_ptr(), 0)
            };
            let callback_selector =
                unsafe { ffi::sel_registerName(WORKSPACE_CALLBACK_SELECTOR.as_ptr()) };
            if class.is_null()
                || callback_selector.is_null()
                || !unsafe {
                    ffi::class_addMethod(
                        class,
                        callback_selector,
                        workspace_activation_callback as *const () as *const c_void,
                        c"v@:@".as_ptr(),
                    )
                }
            {
                return Err(PlatformError::NativeFailure);
            }
            unsafe { ffi::objc_registerClassPair(class) };
        }

        let observer =
            unsafe { ffi::objc_msgSend(class.cast(), ffi::sel_registerName(c"new".as_ptr())) };
        if observer.is_null() {
            return Err(PlatformError::NativeFailure);
        }
        let workspace_class = unsafe { ffi::objc_getClass(c"NSWorkspace".as_ptr()) };
        let shared_workspace = unsafe {
            ffi::objc_msgSend(
                workspace_class.cast(),
                ffi::sel_registerName(c"sharedWorkspace".as_ptr()),
            )
        };
        let notification_center = unsafe {
            ffi::objc_msgSend(
                shared_workspace,
                ffi::sel_registerName(c"notificationCenter".as_ptr()),
            )
        };
        if workspace_class.is_null() || shared_workspace.is_null() || notification_center.is_null()
        {
            unsafe {
                let _ = ffi::objc_msgSend(observer, ffi::sel_registerName(c"release".as_ptr()));
            }
            return Err(PlatformError::NativeFailure);
        }

        let (registry, _) = workspace_callback_registry();
        registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .insert(
                observer as usize,
                WorkspaceCallbackEntry {
                    shared: Arc::clone(shared),
                    active: 0,
                    retiring: false,
                },
            );
        unsafe {
            let _ = ffi::objc_msgSend(
                notification_center,
                ffi::sel_registerName(c"addObserver:selector:name:object:".as_ptr()),
                observer,
                ffi::sel_registerName(WORKSPACE_CALLBACK_SELECTOR.as_ptr()),
                ffi::NSWorkspaceDidActivateApplicationNotification,
                null_mut::<c_void>(),
            );
        }
        Ok(Self {
            observer,
            notification_center,
        })
    }
}

fn retire_workspace_callback(observer: usize) {
    let (registry, drained) = workspace_callback_registry();
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = registry.entries.get_mut(&observer) {
        entry.retiring = true;
    }
    while registry
        .entries
        .get(&observer)
        .is_some_and(|entry| entry.active != 0)
    {
        registry = drained
            .wait(registry)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    registry.entries.remove(&observer);
}

impl Drop for WorkspaceObserver {
    fn drop(&mut self) {
        // NSNotificationCenter removal closes future selector dispatch. Registry
        // retirement then drains every callback that already acquired its Arc;
        // the receiver's final +1 is released only after that drain completes.
        unsafe {
            let _ = ffi::objc_msgSend(
                self.notification_center,
                ffi::sel_registerName(c"removeObserver:".as_ptr()),
                self.observer,
            );
        }
        retire_workspace_callback(self.observer as usize);
        unsafe {
            let _ = ffi::objc_msgSend(self.observer, ffi::sel_registerName(c"release".as_ptr()));
        }
    }
}

struct TargetObserverContext {
    shared: Arc<CacheShared>,
    focused_control: OwnedCf,
}

unsafe extern "C" fn target_observer_callback(
    _observer: ffi::AXObserverRef,
    element: ffi::AXUIElementRef,
    _notification: ffi::CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: WorkerObserver owns this boxed context until its source and
    // observer are removed on the same AX worker.
    let context = unsafe { &*refcon.cast::<TargetObserverContext>() };
    // Selected-text/range notifications are registered only on the retained
    // focused control. They advance an independent epoch as well as the broad
    // target epoch; app/window/focus notifications arrive on the application.
    if !element.is_null()
        && unsafe {
            ffi::CFEqual(
                element.cast_const().cast(),
                context.focused_control.as_type_ref(),
            ) != 0
        }
    {
        context.shared.invalidate_selected_range();
    } else {
        context.shared.invalidate_notification();
    }
}

struct WorkerObserver {
    observer: ffi::AXObserverRef,
    source: ffi::CFRunLoopSourceRef,
    run_loop: ffi::CFRunLoopRef,
    context: *mut TargetObserverContext,
    process_id: i32,
}

fn observer_identity_matches(
    observer_process_id: i32,
    evidence_process_id: i32,
    focused_control_equal: bool,
) -> bool {
    observer_process_id == evidence_process_id && focused_control_equal
}

impl WorkerObserver {
    fn observes_control(&self, evidence: &TargetEvidence) -> bool {
        let focused_control_equal = unsafe {
            // SAFETY: the observer context and candidate evidence are both
            // retained on this AX worker for the complete comparison.
            ffi::CFEqual(
                (*self.context).focused_control.as_type_ref(),
                evidence.focused_control.as_type_ref(),
            ) != 0
        };
        observer_identity_matches(self.process_id, evidence.process_id, focused_control_equal)
    }

    fn install(
        run_loop: ffi::CFRunLoopRef,
        shared: &Arc<CacheShared>,
        resources: &TargetCaptureResources,
        evidence: &TargetEvidence,
    ) -> Result<Self, PlatformError> {
        let focused_control = evidence.focused_control.retained_clone()?;
        let context = Box::into_raw(Box::new(TargetObserverContext {
            shared: Arc::clone(shared),
            focused_control,
        }));
        let mut observer = null_mut();
        // SAFETY: observer output is writable and the boxed callback context
        // remains valid until WorkerObserver removes and releases the observer.
        if unsafe {
            ffi::AXObserverCreate(
                evidence.process_id,
                Some(target_observer_callback),
                &raw mut observer,
            )
        } != 0
            || observer.is_null()
        {
            unsafe { drop(Box::from_raw(context)) };
            return Err(PlatformError::NativeFailure);
        }
        let registrations = [
            (
                evidence.application.as_type_ref().cast_mut(),
                resources.focused_window_changed.as_type_ref(),
            ),
            (
                evidence.application.as_type_ref().cast_mut(),
                resources.focused_ui_element_changed.as_type_ref(),
            ),
            (
                evidence.focused_control.as_type_ref().cast_mut(),
                resources.selected_text_changed.as_type_ref(),
            ),
            (
                evidence.focused_control.as_type_ref().cast_mut(),
                resources.selected_text_range_changed.as_type_ref(),
            ),
        ];
        for (element, notification) in registrations {
            if unsafe {
                ffi::AXObserverAddNotification(
                    observer,
                    element,
                    notification,
                    context.cast::<c_void>(),
                )
            } != 0
            {
                unsafe {
                    ffi::CFRelease(observer.cast_const());
                    drop(Box::from_raw(context));
                }
                return Err(PlatformError::NativeFailure);
            }
        }
        let source = unsafe { ffi::AXObserverGetRunLoopSource(observer) };
        if source.is_null() {
            unsafe {
                ffi::CFRelease(observer.cast_const());
                drop(Box::from_raw(context));
            }
            return Err(PlatformError::NativeFailure);
        }
        unsafe { ffi::CFRunLoopAddSource(run_loop, source, ffi::kCFRunLoopDefaultMode) };
        Ok(Self {
            observer,
            source,
            run_loop,
            context,
            process_id: evidence.process_id,
        })
    }
}

impl Drop for WorkerObserver {
    fn drop(&mut self) {
        unsafe {
            ffi::CFRunLoopRemoveSource(self.run_loop, self.source, ffi::kCFRunLoopDefaultMode);
            ffi::CFRelease(self.observer.cast_const());
            drop(Box::from_raw(self.context));
        }
    }
}

fn service_worker_notifications(seconds: f64) {
    // SAFETY: called only by the AX worker that owns its current run loop.
    unsafe {
        let _ = ffi::CFRunLoopRunInMode(ffi::kCFRunLoopDefaultMode, seconds, false);
    }
}

fn capture_observer_confirmed(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    run_loop: ffi::CFRunLoopRef,
    observer: &mut Option<WorkerObserver>,
) -> Option<ConfirmedTarget> {
    // Flush already-signalled observer/workspace sources before establishing
    // the exact epoch that brackets the complete AX double sample.
    service_worker_notifications(0.000_1);
    let epoch_before = shared.current_notification_epoch();
    let boundary_before = shared.current_boundary_epoch();
    let selected_range_before = shared.current_selected_range_epoch();
    let evidence = capture_target(resources)?;
    if observer
        .as_ref()
        .is_none_or(|current| !current.observes_control(&evidence))
    {
        // PID/application/window continuity is not focused-control identity.
        // Retire the old control observer first, poison both broad and range
        // epochs, and require a later double capture under the new observer.
        *observer = None;
        shared.invalidate_selected_range();
        *observer = WorkerObserver::install(run_loop, shared, resources, &evidence).ok();
        return None;
    }
    // Drain notifications that raced any AX query. Evidence carries the exact
    // validated epoch; no caller may reload a newer epoch and relabel it.
    service_worker_notifications(0.001);
    let epoch_after = shared.current_notification_epoch();
    let boundary_after = shared.current_boundary_epoch();
    let selected_range_after = shared.current_selected_range_epoch();
    (epoch_before == epoch_after
        && boundary_before == boundary_after
        && selected_range_before == selected_range_after)
        .then_some(ConfirmedTarget {
            evidence,
            notification_epoch: epoch_before,
            boundary_epoch: boundary_before,
            selected_range_epoch: selected_range_before,
        })
}

fn publish_confirmed_target(
    shared: &Arc<CacheShared>,
    confirmed: ConfirmedTarget,
    last_target: &mut Option<TargetEvidence>,
) {
    if confirmed.notification_epoch != shared.current_notification_epoch()
        || confirmed.boundary_epoch != shared.current_boundary_epoch()
        || confirmed.selected_range_epoch != shared.current_selected_range_epoch()
    {
        return;
    }
    let changed = last_target
        .as_ref()
        .is_some_and(|previous| !same_target(previous, &confirmed.evidence));
    let Ok(retained_for_comparison) = confirmed.evidence.retained_clone() else {
        *last_target = None;
        shared.invalidate_notification();
        return;
    };
    if changed {
        // A tuple change without a delivered notification is itself focus
        // evidence. Advance the epoch and discard this pre-invalidation sample;
        // only a subsequent capture may publish under the new exact epoch.
        *last_target = None;
        shared.invalidate_notification();
        return;
    }
    let mut slot = shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if confirmed.notification_epoch == shared.current_notification_epoch()
        && confirmed.boundary_epoch == shared.current_boundary_epoch()
        && confirmed.selected_range_epoch == shared.current_selected_range_epoch()
    {
        let publication_id = slot
            .as_ref()
            .filter(|cached| {
                cached.notification_epoch == confirmed.notification_epoch
                    && same_target(&cached.evidence, &confirmed.evidence)
            })
            .map_or_else(
                || {
                    shared
                        .published_id
                        .fetch_add(1, Ordering::AcqRel)
                        .wrapping_add(1)
                },
                |cached| cached.publication_id,
            );
        if publication_id == 0 {
            shared.invalidate_notification();
            return;
        }
        *slot = Some(CachedTarget {
            evidence: confirmed.evidence,
            captured_at: Instant::now(),
            notification_epoch: confirmed.notification_epoch,
            boundary_epoch: confirmed.boundary_epoch,
            selected_range_epoch: confirmed.selected_range_epoch,
            publication_id,
        });
        *last_target = Some(retained_for_comparison);
    }
}

struct AutoreleasePool(*mut c_void);

impl AutoreleasePool {
    fn push() -> Result<Self, PlatformError> {
        // SAFETY: Objective-C runtime returns an opaque pool token for this
        // thread; it is popped exactly once by Drop on the same worker.
        let token = unsafe { ffi::objc_autoreleasePoolPush() };
        (!token.is_null())
            .then_some(Self(token))
            .ok_or(PlatformError::NativeFailure)
    }
}

impl Drop for AutoreleasePool {
    fn drop(&mut self) {
        // SAFETY: token came from push on this worker and remains unmatched.
        unsafe { ffi::objc_autoreleasePoolPop(self.0) };
    }
}

struct ClipboardTextSample {
    text: OwnedCf,
    hash: ClipboardTextHash,
    change_count: isize,
}

fn pasteboard_change_count(pasteboard: ffi::ObjcId) -> isize {
    // SAFETY: NSPasteboard changeCount returns NSInteger and takes no arguments.
    unsafe { ffi::objc_msgSend_isize(pasteboard, ffi::sel_registerName(c"changeCount".as_ptr())) }
}

fn clipboard_plain_text(conversion_deadline: Instant) -> Option<ClipboardTextSample> {
    // SAFETY: all Objective-C objects are used synchronously on the AX worker
    // under its current autorelease pool. The sampled immutable NSString is
    // retained before claim and fenced by NSPasteboard changeCount.
    unsafe {
        let pasteboard_class = ffi::objc_getClass(c"NSPasteboard".as_ptr());
        let string_class = ffi::objc_getClass(c"NSString".as_ptr());
        if pasteboard_class.is_null() || string_class.is_null() {
            return None;
        }
        let pasteboard = ffi::objc_msgSend(
            pasteboard_class.cast(),
            ffi::sel_registerName(c"generalPasteboard".as_ptr()),
        );
        let plain_text_type = ffi::objc_msgSend(
            string_class.cast(),
            ffi::sel_registerName(c"stringWithUTF8String:".as_ptr()),
            c"public.utf8-plain-text".as_ptr(),
        );
        if pasteboard.is_null() || plain_text_type.is_null() {
            return None;
        }
        let before = pasteboard_change_count(pasteboard);
        let value = ffi::objc_msgSend(
            pasteboard,
            ffi::sel_registerName(c"stringForType:".as_ptr()),
            plain_text_type,
        );
        if value.is_null() {
            return None;
        }
        let text = OwnedCf::from_created(ffi::CFRetain(value.cast_const().cast())).ok()?;
        let hash = cf_string_sha256_bounded(text.as_type_ref(), conversion_deadline).ok()?;
        let after = pasteboard_change_count(pasteboard);
        (before == after).then_some(ClipboardTextSample {
            text,
            hash,
            change_count: after,
        })
    }
}

fn clipboard_sample_is_authorized(
    sample_hash: ClipboardTextHash,
    sample_change_count: isize,
    expected_hash: ClipboardTextHash,
    current_change_count: isize,
) -> bool {
    sample_hash == expected_hash && sample_change_count == current_change_count
}

fn current_clipboard_change_count() -> Option<isize> {
    unsafe {
        let pasteboard_class = ffi::objc_getClass(c"NSPasteboard".as_ptr());
        if pasteboard_class.is_null() {
            return None;
        }
        let pasteboard = ffi::objc_msgSend(
            pasteboard_class.cast(),
            ffi::sel_registerName(c"generalPasteboard".as_ptr()),
        );
        (!pasteboard.is_null()).then(|| pasteboard_change_count(pasteboard))
    }
}

fn clipboard_change_count_is_current(expected: isize) -> bool {
    current_clipboard_change_count() == Some(expected)
}

const fn postclaim_clipboard_revision_is_authorized(expected: isize, current: isize) -> bool {
    expected == current
}

fn selected_range_for_control(
    resources: &TargetCaptureResources,
    control: &OwnedCf,
) -> Option<ffi::CFRange> {
    let value = ax_copy_attribute(
        control.as_type_ref().cast_mut(),
        resources.selected_text_range.as_type_ref(),
    )
    .ok()?;
    let value_type = unsafe { ffi::AXValueGetType(value.as_type_ref()) };
    let mut range = ffi::CFRange::default();
    let extracted = unsafe {
        ffi::AXValueGetValue(
            value.as_type_ref(),
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (&raw mut range).cast(),
        )
    } != 0;
    selected_text_range_is_valid(value_type, extracted, range).then_some(range)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertionFailure {
    TargetInvalid,
    Paste(PasteFailure),
}

const fn insertion_identity_failure(
    stopping: bool,
    identity_matches: bool,
) -> Option<InsertionFailure> {
    if stopping {
        Some(InsertionFailure::Paste(PasteFailure::Unavailable))
    } else if !identity_matches {
        Some(InsertionFailure::TargetInvalid)
    } else {
        None
    }
}

fn insertion_target_failure(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    work: &InsertionWork,
    cached: &CachedTarget,
) -> Option<InsertionFailure> {
    // Flush workspace/focus/range notifications around the AX range query. The
    // permission and real Secure Input preflights are deliberately last, so the
    // returned proof is immediately adjacent to claim or mutation.
    service_worker_notifications(0.000_1);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id
            && cached.publication_id == work.publication_id
            && cached.notification_epoch == work.notification_epoch
            && cached.boundary_epoch == work.boundary_epoch
            && cached.selected_range_epoch == work.selected_range_epoch,
    ) {
        return Some(failure);
    }
    let selected_range_matches =
        selected_range_for_control(resources, &cached.evidence.focused_control)
            == Some(cached.evidence.selected_text_range);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        selected_range_matches,
    ) {
        return Some(failure);
    }
    service_worker_notifications(0.000_1);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id,
    ) {
        return Some(failure);
    }
    if !permissions_allow_native_input(permission_snapshot()) {
        return Some(InsertionFailure::Paste(PasteFailure::PermissionDenied));
    }
    if secure_input_active() {
        return Some(InsertionFailure::Paste(PasteFailure::SecureInput));
    }
    if !insertion_completion_budget_available(work.deadline, Instant::now()) {
        return Some(InsertionFailure::Paste(PasteFailure::Unavailable));
    }
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id,
    ) {
        return Some(failure);
    }
    None
}

fn fail_insertion(state: &AtomicU8, failure: InsertionFailure) {
    match failure {
        InsertionFailure::TargetInvalid => {
            let _ = state.compare_exchange(
                INSERTION_PENDING,
                INSERTION_TARGET_INVALID,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        InsertionFailure::Paste(reason) => fail_pending_insertion(state, reason),
    }
}

fn fail_pending_insertion(state: &AtomicU8, reason: PasteFailure) {
    let _ = state.compare_exchange(
        INSERTION_PENDING,
        insertion_failure_state(reason),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

fn mark_claimed_insertion_ambiguous(state: &AtomicU8) {
    let _ = state.compare_exchange(
        INSERTION_CLAIMED,
        INSERTION_AMBIGUOUS,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

fn complete_claimed_insertion(state: &AtomicU8, inserted: bool) {
    let _ = state.compare_exchange(
        INSERTION_CLAIMED,
        if inserted {
            INSERTION_SUCCEEDED
        } else {
            INSERTION_FAILED_OS_REJECTED
        },
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AxSetOutcome {
    Succeeded,
    DefinitiveFailure,
    Ambiguous,
}

fn classify_ax_set_error(error: i32) -> AxSetOutcome {
    match error {
        ffi::K_AX_ERROR_SUCCESS => AxSetOutcome::Succeeded,
        ffi::K_AX_ERROR_ILLEGAL_ARGUMENT
        | ffi::K_AX_ERROR_INVALID_UI_ELEMENT
        | ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED
        | ffi::K_AX_ERROR_API_DISABLED => AxSetOutcome::DefinitiveFailure,
        // kAXErrorCannotComplete is the documented result when messaging the
        // target times out. Unknown results also lack acceptance proof.
        ffi::K_AX_ERROR_CANNOT_COMPLETE => AxSetOutcome::Ambiguous,
        _ => AxSetOutcome::Ambiguous,
    }
}

fn settle_claimed_ax_error(state: &AtomicU8, error: i32) -> bool {
    match classify_ax_set_error(error) {
        AxSetOutcome::Succeeded => true,
        AxSetOutcome::DefinitiveFailure => {
            complete_claimed_insertion(state, false);
            false
        }
        AxSetOutcome::Ambiguous => {
            mark_claimed_insertion_ambiguous(state);
            false
        }
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
struct TestSecureInputScope {
    requested: bool,
    enabled: bool,
}

#[cfg(feature = "transactional-shortcuts-dev")]
impl TestSecureInputScope {
    fn enable_for_preclaim_seam() -> Self {
        let requested = std::env::var_os("TALKING_QUILL_MACOS_TEST_SECURE_INPUT_PRECLAIM")
            .as_deref()
            == Some(std::ffi::OsStr::new("1"));
        if !requested {
            return Self {
                requested,
                enabled: false,
            };
        }
        // This is the real session API, not a simulated permission flag.
        let enabled = unsafe { ffi::EnableSecureEventInput() == 0 } && secure_input_active();
        Self { requested, enabled }
    }

    fn failed_to_enable(&self) -> bool {
        self.requested && !self.enabled
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
impl Drop for TestSecureInputScope {
    fn drop(&mut self) {
        if self.enabled {
            let _ = unsafe { ffi::DisableSecureEventInput() };
        }
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn pause_after_insertion_claim(deadline: Instant) {
    let (Some(paused), Some(release)) = (
        std::env::var_os("TALKING_QUILL_MACOS_TEST_INSERTION_CLAIMED"),
        std::env::var_os("TALKING_QUILL_MACOS_TEST_INSERTION_RELEASE"),
    ) else {
        return;
    };
    let paused = std::path::PathBuf::from(paused);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::write(&paused, b"AX insertion claimed\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn pause_after_range_set(deadline: Instant) {
    let (Some(paused), Some(release)) = (
        std::env::var_os("TALKING_QUILL_MACOS_TEST_RANGE_SET"),
        std::env::var_os("TALKING_QUILL_MACOS_TEST_RANGE_SET_RELEASE"),
    ) else {
        return;
    };
    let paused = std::path::PathBuf::from(paused);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::write(&paused, b"AX exact range set\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

fn process_insertion_work(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    work: InsertionWork,
) {
    #[cfg(feature = "transactional-shortcuts-dev")]
    let secure_input_scope = TestSecureInputScope::enable_for_preclaim_seam();
    #[cfg(feature = "transactional-shortcuts-dev")]
    if secure_input_scope.failed_to_enable() {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }

    // Cheap real safety preflights precede AX messaging as well as being
    // repeated in the immediately-adjacent claim proof below.
    if !permissions_allow_native_input(permission_snapshot()) {
        fail_pending_insertion(&work.state, PasteFailure::PermissionDenied);
        return;
    }
    if secure_input_active() {
        fail_pending_insertion(&work.state, PasteFailure::SecureInput);
        return;
    }

    let slot = shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(cached) = slot.as_ref() else {
        fail_insertion(&work.state, InsertionFailure::TargetInvalid);
        return;
    };
    if let Some(reason) = insertion_target_failure(resources, shared, &work, cached) {
        fail_insertion(&work.state, reason);
        return;
    }
    let Some(conversion_deadline) = work
        .deadline
        .checked_sub(AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN)
    else {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    };
    let Some(preclaim_clipboard) = clipboard_plain_text(conversion_deadline) else {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    };
    if !clipboard_sample_is_authorized(
        preclaim_clipboard.hash,
        preclaim_clipboard.change_count,
        work.expected_clipboard_sha256,
        current_clipboard_change_count().unwrap_or(isize::MIN),
    ) {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }
    if let Some(reason) = insertion_target_failure(resources, shared, &work, cached) {
        fail_insertion(&work.state, reason);
        return;
    }
    if !clipboard_change_count_is_current(preclaim_clipboard.change_count) {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }
    if work
        .state
        .compare_exchange(
            INSERTION_PENDING,
            INSERTION_CLAIMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pause_after_insertion_claim(work.deadline);

    if insertion_target_failure(resources, shared, &work, cached).is_some()
        || !postclaim_clipboard_revision_is_authorized(
            preclaim_clipboard.change_count,
            current_clipboard_change_count().unwrap_or(isize::MIN),
        )
    {
        mark_claimed_insertion_ambiguous(&work.state);
        return;
    }

    // Restore the exact retained typed AXValue, not a reconstructed scalar.
    // Any timeout/unknown acceptance after claim is terminal ambiguity.
    let range_error = unsafe {
        ffi::AXUIElementSetAttributeValue(
            cached.evidence.focused_control.as_type_ref().cast_mut(),
            resources.selected_text_range.as_type_ref(),
            cached.evidence.selected_text_range_value.as_type_ref(),
        )
    };
    if !settle_claimed_ax_error(&work.state, range_error) {
        return;
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pause_after_range_set(work.deadline);

    if insertion_target_failure(resources, shared, &work, cached).is_some()
        || !postclaim_clipboard_revision_is_authorized(
            preclaim_clipboard.change_count,
            current_clipboard_change_count().unwrap_or(isize::MIN),
        )
        || !clipboard_change_count_is_current(preclaim_clipboard.change_count)
    {
        mark_claimed_insertion_ambiguous(&work.state);
        return;
    }

    // SAFETY: the retained control/range and preclaim-bounded immutable
    // NSString remain live on this worker. Postclaim performs only scalar
    // changeCount and target checks—never a second conversion, hash, or large
    // allocation—immediately before selected-text mutation.
    let text_error = unsafe {
        ffi::AXUIElementSetAttributeValue(
            cached.evidence.focused_control.as_type_ref().cast_mut(),
            resources.selected_text.as_type_ref(),
            preclaim_clipboard.text.as_type_ref(),
        )
    };
    if settle_claimed_ax_error(&work.state, text_error) {
        complete_claimed_insertion(&work.state, true);
    }
}

fn target_cache_worker(
    shared: Arc<CacheShared>,
    validation_pool: Arc<ValidationPool>,
    requests: Receiver<ValidationWork>,
    insertions: Receiver<InsertionWork>,
) {
    let Ok(_worker_pool) = AutoreleasePool::push() else {
        shared.invalidate_notification();
        validation_pool.stop();
        return;
    };
    target_cache_worker_inner(&shared, &validation_pool, &requests, &insertions);
    validation_pool.stop();
}

fn target_cache_worker_inner(
    shared: &Arc<CacheShared>,
    validation_pool: &ValidationPool,
    requests: &Receiver<ValidationWork>,
    insertions: &Receiver<InsertionWork>,
) {
    let Ok(resources) = TargetCaptureResources::new() else {
        shared.invalidate_notification();
        return;
    };
    // SAFETY: this worker owns and services its current Core Foundation loop.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    let Ok(_workspace_observer) = WorkspaceObserver::install(shared) else {
        shared.invalidate_notification();
        return;
    };
    let mut observer: Option<WorkerObserver> = None;
    let mut last_target: Option<TargetEvidence> = None;
    while !shared.stopping.load(Ordering::Acquire) {
        let Ok(_iteration_pool) = AutoreleasePool::push() else {
            shared.invalidate_notification();
            break;
        };
        service_worker_notifications(TARGET_CACHE_REFRESH_INTERVAL.as_secs_f64());
        while let Ok(work) = insertions.try_recv() {
            process_insertion_work(&resources, shared, work);
        }
        while let Ok(request) = requests.try_recv() {
            if !validation_pool.is_pending(request) {
                continue;
            }
            service_worker_notifications(0.001);
            let confirmed = capture_observer_confirmed(&resources, shared, run_loop, &mut observer);
            let observed_epoch = confirmed.as_ref().map_or_else(
                || shared.current_notification_epoch(),
                |value| value.notification_epoch,
            );
            let observed_boundary_epoch = confirmed.as_ref().map_or_else(
                || shared.current_boundary_epoch(),
                |value| value.boundary_epoch,
            );
            let observed_selected_range_epoch = confirmed.as_ref().map_or_else(
                || shared.current_selected_range_epoch(),
                |value| value.selected_range_epoch,
            );
            let pre_boundary_confirmed = confirmed.as_ref().is_some_and(|confirmed| {
                if request.expected_publication_id == 0 {
                    return true;
                }
                shared
                    .slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .is_some_and(|cached| {
                        cached.publication_id == request.expected_publication_id
                            && cached.notification_epoch == request.ticket.start_epoch
                            && same_target(&cached.evidence, &confirmed.evidence)
                    })
            });
            let handle = confirmed.as_ref().and_then(|confirmed| {
                (pre_boundary_confirmed
                    && request.expected_publication_id != 0
                    && request.ticket.start_epoch == confirmed.notification_epoch
                    && request.ticket.start_boundary_epoch == confirmed.boundary_epoch
                    && confirmed.notification_epoch == shared.current_notification_epoch()
                    && confirmed.boundary_epoch == shared.current_boundary_epoch()
                    && confirmed.selected_range_epoch == shared.current_selected_range_epoch())
                .then_some(TargetHandle {
                    publication_id: request.expected_publication_id,
                })
            });
            let _ = validation_pool.publish(
                request,
                ValidationResponse {
                    ticket: request.ticket,
                    handle,
                    observed_epoch,
                    observed_boundary_epoch,
                    observed_selected_range_epoch,
                },
            );
        }
        if let Some(confirmed) =
            capture_observer_confirmed(&resources, shared, run_loop, &mut observer)
        {
            publish_confirmed_target(shared, confirmed, &mut last_target);
        } else if last_target.take().is_some() {
            shared.invalidate_notification();
        }
    }
    drop(observer);
    // Release cached CF/AX evidence on the AX worker, never from the event tap
    // or owner teardown thread.
    *shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

struct TargetEntry {
    generation: ActivationGeneration,
    token: NativeTargetToken,
    target: TargetHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessEpoch([u8; 16]);

/// Fixed generation-to-target ring. The process epoch is sourced from the
/// operating system CSPRNG on owner startup. Without it, registry insertion is
/// disabled and every activation is conservatively targetless.
pub(super) struct TargetRegistry {
    entries: [Option<TargetEntry>; TARGET_REGISTRY_CAPACITY],
    process_epoch: Option<ProcessEpoch>,
}

impl TargetRegistry {
    pub(super) fn new() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
            process_epoch: None,
        }
    }

    pub(super) fn initialize_process_epoch(&mut self) -> bool {
        let mut bytes = [0_u8; 16];
        // SAFETY: a null random source requests kSecRandomDefault and `bytes`
        // is valid writable storage for the exact supplied length.
        let status =
            unsafe { ffi::SecRandomCopyBytes(null_mut(), bytes.len(), bytes.as_mut_ptr().cast()) };
        if status != 0 {
            self.process_epoch = None;
            false
        } else {
            self.process_epoch = Some(ProcessEpoch(bytes));
            true
        }
    }

    pub(super) fn bind_context(
        &mut self,
        generation: ActivationGeneration,
        target: Option<TargetHandle>,
    ) -> ActivationContext {
        let Some(epoch) = self.process_epoch else {
            return ActivationContext::target_unavailable(generation);
        };
        let Some(target) = target else {
            return ActivationContext::target_unavailable(generation);
        };
        self.record(generation, target, epoch)
    }

    pub(super) fn take(&mut self, context: ActivationContext) -> Option<TargetHandle> {
        let token = context.target_token()?;
        let slot = slot(context.activation_generation());
        let entry = self.entries[slot].take()?;
        if entry.generation == context.activation_generation() && entry.token == token {
            Some(entry.target)
        } else {
            self.entries[slot] = Some(entry);
            None
        }
    }

    pub(super) fn remove(&mut self, context: ActivationContext) {
        let slot = slot(context.activation_generation());
        if self.entries[slot].as_ref().is_some_and(|entry| {
            entry.generation == context.activation_generation()
                && Some(entry.token) == context.target_token()
        }) {
            self.entries[slot] = None;
        }
    }

    fn record(
        &mut self,
        generation: ActivationGeneration,
        target: TargetHandle,
        epoch: ProcessEpoch,
    ) -> ActivationContext {
        let token = token_for(epoch, generation);
        self.entries[slot(generation)] = Some(TargetEntry {
            generation,
            token,
            target,
        });
        ActivationContext::target_unavailable(generation).with_target_token(token)
    }

    #[cfg(test)]
    fn with_epoch(epoch: [u8; 16]) -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
            process_epoch: Some(ProcessEpoch(epoch)),
        }
    }
}

#[cfg(test)]
pub(super) const fn activation_reservation_for_test(
    publication_id: i32,
    notification_epoch: u64,
) -> ActivationReservation {
    ActivationReservation {
        notification_epoch,
        boundary_epoch: 1,
        selected_range_epoch: 1,
        publication_id: publication_id as u64,
    }
}

#[cfg(test)]
pub(super) fn validation_request_for_test(request_id: u64, start_epoch: u64) -> ValidationRequest {
    Arc::new(ValidationPool::new())
        .acquire(ValidationTicket {
            request_id,
            start_epoch,
            start_boundary_epoch: 1,
        })
        .expect("test validation slot")
        .0
}

impl Default for TargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Captures the complete focused application/window/control tuple twice and
/// accepts it only when both complete samples are identical. Each sample also
/// requires the control's AXWindow identity to be exactly the sampled focused
/// window; unsupported or missing AXWindow evidence fails conservatively.
fn capture_target(resources: &TargetCaptureResources) -> Option<TargetEvidence> {
    if permission_snapshot().accessibility != PermissionState::Granted {
        return None;
    }
    let first = capture_target_once(resources)?;
    let second = capture_target_once(resources)?;
    same_target(&first, &second).then_some(second)
}

const fn selected_text_range_is_valid(
    value_type: u32,
    extracted: bool,
    range: ffi::CFRange,
) -> bool {
    value_type == ffi::K_AX_VALUE_CFRANGE_TYPE
        && extracted
        && range.location >= 0
        && range.length >= 0
        && range.location.checked_add(range.length).is_some()
}

fn capture_target_once(resources: &TargetCaptureResources) -> Option<TargetEvidence> {
    let application = ax_copy_attribute(
        resources.system.as_type_ref().cast_mut(),
        resources.focused_application.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&application).ok()?;
    let window = ax_copy_attribute(
        application.as_type_ref().cast_mut(),
        resources.focused_window.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&window).ok()?;
    let focused_control = ax_copy_attribute(
        resources.system.as_type_ref().cast_mut(),
        resources.focused_ui_element.as_type_ref(),
    )
    .ok()?;
    set_ax_messaging_timeout(&focused_control).ok()?;
    let control_window = ax_copy_attribute(
        focused_control.as_type_ref().cast_mut(),
        resources.window.as_type_ref(),
    )
    .ok()?;
    // A focused control without a stable insertion range is not strong enough
    // for deferred paste. Unsupported controls therefore remain targetless.
    let selected_text_range_value = ax_copy_attribute(
        focused_control.as_type_ref().cast_mut(),
        resources.selected_text_range.as_type_ref(),
    )
    .ok()?;
    // AXSelectedTextRange is authoritative only when it is the documented
    // kAXValueCFRangeType and can be copied into fixed scalar storage. CFEqual
    // on an arbitrary AX/CF object is not caret-position evidence.
    let value_type = unsafe { ffi::AXValueGetType(selected_text_range_value.as_type_ref()) };
    if value_type != ffi::K_AX_VALUE_CFRANGE_TYPE {
        return None;
    }
    let mut selected_text_range = ffi::CFRange::default();
    let extracted = unsafe {
        ffi::AXValueGetValue(
            selected_text_range_value.as_type_ref(),
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (&raw mut selected_text_range).cast(),
        )
    } != 0;
    if !selected_text_range_is_valid(value_type, extracted, selected_text_range) {
        return None;
    }
    set_ax_messaging_timeout(&control_window).ok()?;

    let process_id = ax_pid(&application)?;
    if ax_pid(&window)? != process_id
        || ax_pid(&focused_control)? != process_id
        || ax_pid(&control_window)? != process_id
        || !cf_equal(&window, &control_window)
    {
        return None;
    }
    Some(TargetEvidence {
        process_id,
        application,
        window,
        focused_control,
        selected_text_range_value,
        selected_text_range,
    })
}

pub(super) fn same_target(left: &TargetEvidence, right: &TargetEvidence) -> bool {
    left.process_id == right.process_id
        && cf_equal(&left.application, &right.application)
        && cf_equal(&left.window, &right.window)
        && cf_equal(&left.focused_control, &right.focused_control)
        && left.selected_text_range == right.selected_text_range
}

#[cfg(any(test, feature = "transactional-shortcuts-dev"))]
fn create_range_value_for_test(range: &ffi::CFRange) -> Result<OwnedCf, PlatformError> {
    OwnedCf::from_created(unsafe {
        ffi::AXValueCreate(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (range as *const ffi::CFRange).cast(),
        )
    })
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn test_target_caret_identity_contract() -> bool {
    let result = (|| {
        let range = ffi::CFRange {
            location: 1,
            length: 0,
        };
        let baseline = TargetEvidence {
            process_id: 42,
            application: create_cf_string(c"app")?,
            window: create_cf_string(c"window")?,
            focused_control: create_cf_string(c"control")?,
            selected_text_range_value: create_range_value_for_test(&range)?,
            selected_text_range: range,
        };
        let same = baseline.retained_clone()?;
        let mut moved = baseline.retained_clone()?;
        moved.selected_text_range.location = 2;
        Ok::<_, PlatformError>(same_target(&baseline, &same) && !same_target(&baseline, &moved))
    })();
    result.unwrap_or(false)
}

fn set_ax_messaging_timeout(element: &OwnedCf) -> Result<(), PlatformError> {
    set_ax_messaging_timeout_ref(element.as_type_ref().cast_mut())
}

fn set_ax_messaging_timeout_ref(element: ffi::AXUIElementRef) -> Result<(), PlatformError> {
    // SAFETY: the retained object is an AXUIElement and the timeout is finite
    // and positive. Bounding AX messaging keeps a nonresponsive target from
    // indefinitely occupying the dedicated worker.
    if unsafe { ffi::AXUIElementSetMessagingTimeout(element, AX_MESSAGING_TIMEOUT_SECONDS) } == 0 {
        Ok(())
    } else {
        Err(PlatformError::NativeFailure)
    }
}

fn ax_copy_attribute(
    element: ffi::AXUIElementRef,
    attribute: ffi::CFStringRef,
) -> Result<OwnedCf, PlatformError> {
    set_ax_messaging_timeout_ref(element)?;
    let mut value: ffi::CFTypeRef = null();
    // SAFETY: both inputs are retained AX/CF objects and `value` is writable.
    let error = unsafe { ffi::AXUIElementCopyAttributeValue(element, attribute, &raw mut value) };
    if error != 0 {
        return Err(PlatformError::NativeFailure);
    }
    OwnedCf::from_created(value)
}

fn ax_pid(element: &OwnedCf) -> Option<i32> {
    set_ax_messaging_timeout(element).ok()?;
    let mut process_id = 0;
    // SAFETY: the retained object is an AXUIElement returned by an AX focused
    // object attribute and `process_id` is writable.
    if unsafe { ffi::AXUIElementGetPid(element.as_type_ref().cast_mut(), &raw mut process_id) } != 0
        || process_id <= 0
    {
        None
    } else {
        Some(process_id)
    }
}

fn cf_equal(left: &OwnedCf, right: &OwnedCf) -> bool {
    // SAFETY: both references remain retained for this comparison.
    unsafe { ffi::CFEqual(left.as_type_ref(), right.as_type_ref()) != 0 }
}

const fn slot(generation: ActivationGeneration) -> usize {
    generation.get() as usize % TARGET_REGISTRY_CAPACITY
}

fn token_for(epoch: ProcessEpoch, generation: ActivationGeneration) -> NativeTargetToken {
    const PREFIX: &[u8] = b"mac-v8:";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 56];
    bytes[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut index = 0;
    while index < epoch.0.len() {
        bytes[PREFIX.len() + index * 2] = HEX[(epoch.0[index] >> 4) as usize];
        bytes[PREFIX.len() + index * 2 + 1] = HEX[(epoch.0[index] & 0x0f) as usize];
        index += 1;
    }
    let separator = PREFIX.len() + 32;
    bytes[separator] = b':';
    let mut nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[separator + 1 + nibble] = HEX[((generation.get() >> shift) & 0xF) as usize];
        nibble += 1;
    }
    let value = std::str::from_utf8(&bytes).expect("token alphabet is ASCII");
    NativeTargetToken::new(value).expect("fixed macOS target token is bounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: u64) -> ActivationGeneration {
        ActivationGeneration::new(value).unwrap()
    }

    const fn handle(value: u64) -> TargetHandle {
        TargetHandle {
            publication_id: value,
        }
    }

    #[test]
    fn selected_text_range_requires_exact_ax_cf_range_type_and_scalar_extraction() {
        let valid = ffi::CFRange {
            location: 4,
            length: 2,
        };
        assert!(selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            valid,
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CGPOINT_TYPE,
            true,
            valid,
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            false,
            valid,
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            ffi::CFRange {
                location: -1,
                length: 0,
            },
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            ffi::CFRange {
                location: 1,
                length: -1,
            },
        ));
        assert!(selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            ffi::CFRange {
                location: isize::MAX,
                length: 0,
            },
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            ffi::CFRange {
                location: isize::MAX,
                length: 1,
            },
        ));
        assert!(!selected_text_range_is_valid(
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            true,
            ffi::CFRange {
                location: isize::MAX - 1,
                length: 2,
            },
        ));
    }

    fn evidence(value: i32) -> TargetEvidence {
        let range = ffi::CFRange {
            location: value as isize,
            length: 0,
        };
        TargetEvidence {
            process_id: value,
            application: create_cf_string(c"app").unwrap(),
            window: create_cf_string(c"window").unwrap(),
            focused_control: create_cf_string(c"control").unwrap(),
            selected_text_range_value: create_range_value_for_test(&range).unwrap(),
            selected_text_range: range,
        }
    }

    fn distinct_evidence(value: i32, app: &CStr, window: &CStr, control: &CStr) -> TargetEvidence {
        let range = ffi::CFRange {
            location: value as isize,
            length: 0,
        };
        TargetEvidence {
            process_id: value,
            application: create_cf_string(app).unwrap(),
            window: create_cf_string(window).unwrap(),
            focused_control: create_cf_string(control).unwrap(),
            selected_text_range_value: create_range_value_for_test(&range).unwrap(),
            selected_text_range: range,
        }
    }

    #[test]
    fn same_generation_tokens_are_distinct_across_process_epochs() {
        let generation = generation(7);
        let first = token_for(ProcessEpoch([0x11; 16]), generation);
        let second = token_for(ProcessEpoch([0x22; 16]), generation);
        assert_ne!(first, second);
        assert_eq!(first.as_str().len(), 56);
        assert!(first.as_str().is_ascii());
        assert!(first.as_str().len() <= NativeTargetToken::MAX_BYTES);
    }

    #[test]
    fn target_slots_are_bounded_and_wrap_only_at_registry_capacity() {
        assert_eq!(slot(generation(1)), 1);
        assert_eq!(
            slot(generation(TARGET_REGISTRY_CAPACITY as u64 + 1)),
            slot(generation(1))
        );
    }

    #[test]
    fn registry_consumes_exact_generation_and_token_once() {
        let mut registry = TargetRegistry::with_epoch([7; 16]);
        let context = registry.bind_context(generation(7), Some(handle(7)));
        let wrong = ActivationContext::target_unavailable(generation(8))
            .with_target_token(context.target_token().unwrap());
        assert!(registry.take(wrong).is_none());
        assert_eq!(registry.take(context), Some(handle(7)));
        assert!(registry.take(context).is_none());
    }

    #[test]
    fn separate_registries_same_generation_use_distinct_tokens() {
        let mut first = TargetRegistry::with_epoch([1; 16]);
        let mut second = TargetRegistry::with_epoch([2; 16]);
        let first = first.bind_context(generation(1), Some(handle(1)));
        let second = second.bind_context(generation(1), Some(handle(1)));
        assert_ne!(first.target_token(), second.target_token());
    }

    #[test]
    fn bounded_ring_evicts_old_generation_to_clipboard_only_failure() {
        let mut registry = TargetRegistry::with_epoch([3; 16]);
        let old = registry.bind_context(generation(1), Some(handle(1)));
        let mut newest = old;
        for value in 2..=TARGET_REGISTRY_CAPACITY as u64 + 1 {
            newest = registry.bind_context(generation(value), Some(handle(value)));
        }
        assert!(registry.take(old).is_none());
        assert_eq!(
            registry.take(newest),
            Some(handle(TARGET_REGISTRY_CAPACITY as u64 + 1))
        );
    }

    #[test]
    fn cache_reservation_is_nondestructive_epoch_index_only_and_fresh() {
        let cache = TargetCache::without_worker();
        let now = Instant::now();
        cache.publish_for_test(evidence(1), now);
        let first = cache.reserve_activation_at(now).unwrap();
        let second = cache.reserve_activation_at(now).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.publication_id, 1);

        cache.publish_for_test(
            evidence(2),
            now - TARGET_CACHE_MAX_AGE - Duration::from_millis(1),
        );
        assert!(cache.reserve_activation_at(now).is_none());

        cache.publish_for_test(evidence(3), now);
        cache.invalidate_boundary();
        assert!(cache.reserve_activation_at(now).is_none());
    }

    #[test]
    fn notification_epoch_change_rejects_old_tuple_and_accepts_only_new_publication() {
        let cache = TargetCache::without_worker();
        let now = Instant::now();
        cache.publish_for_test(evidence(1), now);
        cache.shared.invalidate_notification();
        assert!(cache.reserve_activation_at(now).is_none());
        cache.publish_for_test(evidence(2), now);
        assert_eq!(cache.reserve_activation_at(now).unwrap().publication_id, 2);
    }

    #[test]
    fn candidate_reservation_tracks_target_and_range_but_not_keyboard_boundaries() {
        let cache = TargetCache::without_worker();
        cache.shared.notification_epoch.store(7, Ordering::Release);
        cache.shared.boundary_epoch.store(11, Ordering::Release);
        cache.publish_for_test(evidence(3), Instant::now());
        let reservation = ActivationReservation {
            notification_epoch: 7,
            boundary_epoch: 11,
            selected_range_epoch: 1,
            publication_id: 1,
        };
        assert!(cache.reservation_is_current(&reservation));
        cache.invalidate_boundary();
        assert!(cache.reservation_is_current(&reservation));
        cache.shared.invalidate_selected_range();
        assert!(!cache.reservation_is_current(&reservation));
    }

    #[test]
    fn activation_before_after_proof_rejects_every_epoch_or_worker_race() {
        let reservation = ActivationReservation {
            notification_epoch: 7,
            boundary_epoch: 11,
            selected_range_epoch: 13,
            publication_id: 3,
        };
        assert!(reservation.confirms_after_boundary(7, 12, 7, 12, true));
        assert!(!reservation.confirms_after_boundary(8, 12, 8, 12, true));
        assert!(!reservation.confirms_after_boundary(7, 11, 7, 11, true));
        assert!(!reservation.confirms_after_boundary(7, 12, 8, 12, true));
        assert!(!reservation.confirms_after_boundary(7, 12, 7, 12, false));
    }

    #[test]
    fn unchanged_target_refresh_preserves_scalar_publication_handle() {
        let cache = TargetCache::without_worker();
        let now = Instant::now();
        cache.publish_for_test(evidence(1), now);
        let first = cache.reserve_activation_at(now).unwrap().publication_id;
        let mut last = Some(evidence(1));
        publish_confirmed_target(
            &cache.shared,
            ConfirmedTarget {
                evidence: evidence(1),
                notification_epoch: cache.current_epoch(),
                boundary_epoch: cache.current_boundary_epoch(),
                selected_range_epoch: cache.current_selected_range_epoch(),
            },
            &mut last,
        );
        assert_eq!(
            cache
                .reserve_activation_at(Instant::now())
                .unwrap()
                .publication_id,
            first
        );
    }

    #[test]
    fn publication_never_relabels_evidence_with_a_newer_epoch() {
        let cache = TargetCache::without_worker();
        let validated_epoch = cache.current_epoch();
        let confirmed = ConfirmedTarget {
            evidence: evidence(1),
            notification_epoch: validated_epoch,
            boundary_epoch: cache.shared.current_boundary_epoch(),
            selected_range_epoch: cache.shared.current_selected_range_epoch(),
        };
        cache.shared.invalidate_notification();
        let mut last = None;
        publish_confirmed_target(&cache.shared, confirmed, &mut last);
        assert!(cache.shared.slot.lock().unwrap().is_none());
        assert!(last.is_none());
    }

    #[test]
    fn focus_and_programmatic_range_notifications_invalidate_exact_epochs() {
        let cache = TargetCache::without_worker();
        let now = Instant::now();
        cache.publish_for_test(evidence(1), now);
        let focused_control = cache
            .shared
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .evidence
            .focused_control
            .retained_clone()
            .unwrap();
        let context = TargetObserverContext {
            shared: Arc::clone(&cache.shared),
            focused_control,
        };
        let epoch = cache.current_epoch();
        let range_epoch = cache.current_selected_range_epoch();
        unsafe {
            target_observer_callback(
                null_mut(),
                context.focused_control.as_type_ref().cast_mut(),
                null(),
                (&raw const context).cast_mut().cast(),
            );
        }
        assert_ne!(cache.current_epoch(), epoch);
        assert_ne!(cache.current_selected_range_epoch(), range_epoch);
        assert!(cache.reserve_activation_at(now).is_none());

        let focus_epoch = cache.current_epoch();
        unsafe {
            target_observer_callback(
                null_mut(),
                null_mut(),
                null(),
                (&raw const context).cast_mut().cast(),
            );
        }
        assert_ne!(cache.current_epoch(), focus_epoch);
    }

    #[test]
    fn same_pid_control_switch_and_range_aba_require_a_fresh_observer_publication() {
        assert!(observer_identity_matches(41, 41, true));
        assert!(!observer_identity_matches(41, 41, false));

        let cache = TargetCache::without_worker();
        let now = Instant::now();
        let control_a = distinct_evidence(41, c"app", c"window", c"control-a");
        let old_context = TargetObserverContext {
            shared: Arc::clone(&cache.shared),
            focused_control: control_a.focused_control.retained_clone().unwrap(),
        };
        cache.publish_for_test(control_a, now);
        let old_publication = cache.reserve_activation_at(now).unwrap().publication_id;

        // Reinstall policy for a same-PID control switch poisons broad/range
        // epochs before any capture under control B can publish.
        cache.shared.invalidate_selected_range();
        let control_b = distinct_evidence(41, c"app", c"window", c"control-b");
        let new_context = TargetObserverContext {
            shared: Arc::clone(&cache.shared),
            focused_control: control_b.focused_control.retained_clone().unwrap(),
        };
        cache.publish_for_test(control_b, now);
        let switched_publication = cache.reserve_activation_at(now).unwrap().publication_id;
        assert_ne!(switched_publication, old_publication);

        // A callback already admitted by the retired A observer can only
        // invalidate B; it cannot authorize B under A's old epochs.
        unsafe {
            target_observer_callback(
                null_mut(),
                old_context.focused_control.as_type_ref().cast_mut(),
                null(),
                (&raw const old_context).cast_mut().cast(),
            );
        }
        assert!(cache.reserve_activation_at(now).is_none());

        // Programmatic B range move-and-return is still an ABA: both callbacks
        // advance the independent range epoch, so the switched publication
        // never becomes current again merely because the scalar range matches.
        for _ in 0..2 {
            unsafe {
                target_observer_callback(
                    null_mut(),
                    new_context.focused_control.as_type_ref().cast_mut(),
                    null(),
                    (&raw const new_context).cast_mut().cast(),
                );
            }
        }
        assert!(cache.reserve_activation_at(now).is_none());
        cache.publish_for_test(distinct_evidence(41, c"app", c"window", c"control-b"), now);
        assert_ne!(
            cache.reserve_activation_at(now).unwrap().publication_id,
            switched_publication
        );
    }

    #[test]
    fn selected_range_epoch_invalidates_final_retained_handle_check() {
        let cache = TargetCache::without_worker();
        let handle = handle(9);
        cache.install_current_handle_for_test(handle, 11, 13);
        assert!(cache.handle_is_current(handle, 11, 13, 1));
        cache.shared.invalidate_selected_range();
        assert!(!cache.handle_is_current(handle, 11, 13, 1));
    }

    #[test]
    fn workspace_retirement_waits_for_admitted_callback_arc() {
        let shared = Arc::new(CacheShared::new());
        let object = unsafe {
            ffi::objc_msgSend(
                ffi::objc_getClass(c"NSObject".as_ptr()).cast(),
                ffi::sel_registerName(c"new".as_ptr()),
            )
        };
        assert!(!object.is_null());
        let observer = object as usize;
        workspace_callback_registry()
            .0
            .lock()
            .unwrap()
            .entries
            .insert(
                observer,
                WorkspaceCallbackEntry {
                    shared: Arc::clone(&shared),
                    active: 0,
                    retiring: false,
                },
            );
        let invocation = WorkspaceInvocation::begin(observer as ffi::ObjcId).unwrap();
        let (started_tx, started_rx) = bounded(1);
        let (done_tx, done_rx) = bounded(1);
        let retire = thread::spawn(move || {
            started_tx.send(()).unwrap();
            retire_workspace_callback(observer);
            done_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(10)).is_err());
        drop(invocation);
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        retire.join().unwrap();
        assert_eq!(Arc::strong_count(&shared), 1);
        unsafe {
            let _ = ffi::objc_msgSend(object, ffi::sel_registerName(c"release".as_ptr()));
        }
    }

    #[test]
    fn validation_pool_has_fixed_capacity_and_stopped_pool_fails_closed() {
        let pool = Arc::new(ValidationPool::new());
        let mut requests = Vec::new();
        for request_id in 1..=VALIDATION_QUEUE_CAPACITY as u64 {
            let ticket = ValidationTicket {
                request_id,
                start_epoch: 7,
                start_boundary_epoch: 9,
            };
            let (request, _work) = pool.acquire(ticket).expect("fixed slot available");
            requests.push(request);
        }
        assert!(
            pool.acquire(ValidationTicket {
                request_id: 99,
                start_epoch: 7,
                start_boundary_epoch: 9,
            })
            .is_none()
        );
        drop(requests.pop());
        assert!(
            pool.acquire(ValidationTicket {
                request_id: 100,
                start_epoch: 7,
                start_boundary_epoch: 9,
            })
            .is_some()
        );
        pool.stop();
        assert!(
            pool.acquire(ValidationTicket {
                request_id: 101,
                start_epoch: 7,
                start_boundary_epoch: 9,
            })
            .is_none()
        );
    }

    #[test]
    fn worker_shutdown_rejects_pending_publication_and_new_acquisition() {
        let pool = Arc::new(ValidationPool::new());
        let ticket = ValidationTicket {
            request_id: 102,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (mut request, work) = pool.acquire(ticket).unwrap();
        pool.stop();
        assert!(!pool.publish(
            work,
            ValidationResponse {
                ticket,
                handle: Some(handle(1)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert!(request.try_response().is_none());
        assert!(
            pool.acquire(ValidationTicket {
                request_id: 103,
                start_epoch: 7,
                start_boundary_epoch: 9,
            })
            .is_none()
        );
    }

    #[test]
    fn full_work_queue_recycles_the_reserved_slot_without_waiting() {
        let shared = Arc::new(CacheShared::new());
        let validation_pool = Arc::new(ValidationPool::new());
        let (requests, queued) = bounded(1);
        let (insertions, _insertion_receiver) = bounded(1);
        let cache = TargetCache {
            shared,
            validation_pool: Arc::clone(&validation_pool),
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: None,
            worker_completion: bounded(1).1,
            _test_request_receiver: None,
        };
        let first = cache.request_validation().expect("first work item fits");
        drop(first);
        assert!(cache.request_validation().is_none());
        let _stale = queued.try_recv().unwrap();
        assert!(cache.request_validation().is_some());
    }

    #[test]
    fn stale_aba_work_cannot_publish_into_a_reused_slot() {
        let pool = Arc::new(ValidationPool::new());
        let first_ticket = ValidationTicket {
            request_id: 31,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (first_request, stale_work) = pool.acquire(first_ticket).unwrap();
        let first_slot = first_request.slot_index;
        let first_generation = first_request.slot_generation;
        drop(first_request);

        let second_ticket = ValidationTicket {
            request_id: 32,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (mut second_request, second_work) = pool.acquire(second_ticket).unwrap();
        assert_eq!(second_request.slot_index, first_slot);
        assert_ne!(second_request.slot_generation, first_generation);
        assert!(!pool.publish(
            stale_work,
            ValidationResponse {
                ticket: first_ticket,
                handle: Some(handle(1)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert!(second_request.try_response().is_none());
        assert!(pool.publish(
            second_work,
            ValidationResponse {
                ticket: second_ticket,
                handle: Some(handle(2)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert_eq!(second_request.try_response().unwrap().ticket, second_ticket);
    }

    #[test]
    fn ready_response_cancellation_reuses_slot_without_exposing_stale_value() {
        let pool = Arc::new(ValidationPool::new());
        let first_ticket = ValidationTicket {
            request_id: 33,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (first_request, first_work) = pool.acquire(first_ticket).unwrap();
        assert!(pool.publish(
            first_work,
            ValidationResponse {
                ticket: first_ticket,
                handle: Some(handle(1)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        drop(first_request);

        let second_ticket = ValidationTicket {
            request_id: 34,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (mut second_request, second_work) = pool.acquire(second_ticket).unwrap();
        assert!(second_request.try_response().is_none());
        assert!(pool.publish(
            second_work,
            ValidationResponse {
                ticket: second_ticket,
                handle: Some(handle(2)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert_eq!(second_request.try_response().unwrap().ticket, second_ticket);
    }

    #[test]
    fn simultaneous_activation_and_paste_replies_cannot_cross_consume() {
        let activation_ticket = ValidationTicket {
            request_id: 41,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let paste_ticket = ValidationTicket {
            request_id: 42,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let pool = Arc::new(ValidationPool::new());
        let (mut activation_request, activation_work) = pool.acquire(activation_ticket).unwrap();
        let (mut paste_request, paste_work) = pool.acquire(paste_ticket).unwrap();

        // Worker completion order is intentionally reversed. Each request has
        // a fixed private slot, so polling activation cannot remove paste's
        // response or vice versa.
        assert!(pool.publish(
            paste_work,
            ValidationResponse {
                ticket: paste_ticket,
                handle: Some(handle(2)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert!(activation_request.try_response().is_none());
        assert_eq!(paste_request.try_response().unwrap().ticket, paste_ticket);

        assert!(pool.publish(
            activation_work,
            ValidationResponse {
                ticket: activation_ticket,
                handle: Some(handle(1)),
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
        assert_eq!(
            activation_request.try_response().unwrap().ticket,
            activation_ticket
        );
        assert!(paste_request.try_response().is_none());
    }

    #[test]
    fn cancelled_validation_drops_its_bounded_reply_without_affecting_others() {
        let ticket = ValidationTicket {
            request_id: 43,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let pool = Arc::new(ValidationPool::new());
        let (request, work) = pool.acquire(ticket).unwrap();
        drop(request);
        assert!(!pool.publish(
            work,
            ValidationResponse {
                ticket,
                handle: None,
                observed_epoch: 7,
                observed_boundary_epoch: 9,
                observed_selected_range_epoch: 1,
            }
        ));
    }

    #[test]
    fn validation_request_epoch_rejects_notification_races() {
        let cache = TargetCache::without_worker();
        let ticket = ValidationTicket {
            request_id: 7,
            start_epoch: cache.current_epoch(),
            start_boundary_epoch: cache.current_boundary_epoch(),
        };
        let accepted = ValidationResponse {
            ticket,
            handle: Some(handle(1)),
            observed_epoch: ticket.start_epoch,
            observed_boundary_epoch: ticket.start_boundary_epoch,
            observed_selected_range_epoch: 1,
        };
        assert_eq!(
            accepted
                .into_current_handle(
                    ticket,
                    cache.current_epoch(),
                    cache.current_boundary_epoch(),
                    cache.current_selected_range_epoch(),
                )
                .unwrap()
                .0,
            handle(1)
        );

        let raced = ValidationResponse {
            ticket,
            handle: Some(handle(2)),
            observed_epoch: ticket.start_epoch,
            observed_boundary_epoch: ticket.start_boundary_epoch,
            observed_selected_range_epoch: 1,
        };
        cache.shared.invalidate_notification();
        assert!(
            raced
                .into_current_handle(
                    ticket,
                    cache.current_epoch(),
                    cache.current_boundary_epoch(),
                    cache.current_selected_range_epoch(),
                )
                .is_none()
        );

        let boundary_ticket = ValidationTicket {
            request_id: 8,
            start_epoch: cache.current_epoch(),
            start_boundary_epoch: cache.current_boundary_epoch(),
        };
        let boundary_raced = ValidationResponse {
            ticket: boundary_ticket,
            handle: Some(handle(3)),
            observed_epoch: boundary_ticket.start_epoch,
            observed_boundary_epoch: boundary_ticket.start_boundary_epoch,
            observed_selected_range_epoch: 1,
        };
        cache.invalidate_boundary();
        assert!(
            boundary_raced
                .into_current_handle(
                    boundary_ticket,
                    cache.current_epoch(),
                    cache.current_boundary_epoch(),
                    cache.current_selected_range_epoch(),
                )
                .is_none()
        );
    }

    #[test]
    fn worker_notification_epoch_after_sample_invalidates_immediate_post_proof() {
        let cache = TargetCache::without_worker();
        let validated_epoch = cache.current_epoch();
        assert_eq!(cache.current_epoch(), validated_epoch);
        cache.shared.invalidate_notification();
        assert_ne!(cache.current_epoch(), validated_epoch);
    }

    #[test]
    fn post_claim_pause_exposes_claim_and_preserves_exact_completion() {
        for inserted in [true, false] {
            let state = Arc::new(AtomicU8::new(INSERTION_PENDING));
            let request = InsertionRequest {
                work: None,
                state: Arc::clone(&state),
            };
            let (claimed_tx, claimed_rx) = bounded(1);
            let (release_tx, release_rx) = bounded(1);
            let worker_state = Arc::clone(&state);
            let worker = thread::spawn(move || {
                worker_state
                    .compare_exchange(
                        INSERTION_PENDING,
                        INSERTION_CLAIMED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .unwrap();
                claimed_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                complete_claimed_insertion(&worker_state, inserted);
            });
            claimed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(request.status(), InsertionStatus::Claimed);
            assert!(!request.cancel(), "claim authority is immutable");
            release_tx.send(()).unwrap();
            worker.join().unwrap();
            assert_eq!(
                request.status(),
                if inserted {
                    InsertionStatus::Succeeded
                } else {
                    InsertionStatus::Failed(PasteFailure::OsRejected)
                }
            );
        }
    }

    #[test]
    fn clipboard_hash_and_change_count_jointly_prevent_torn_authorization() {
        let expected = ClipboardTextHash::from_bytes([7; 32]);
        assert!(clipboard_sample_is_authorized(expected, 11, expected, 11));
        assert!(!clipboard_sample_is_authorized(expected, 11, expected, 12));
        assert!(!clipboard_sample_is_authorized(
            ClipboardTextHash::from_bytes([8; 32]),
            11,
            expected,
            11,
        ));
    }

    #[test]
    fn postclaim_clipboard_authority_is_only_the_retained_scalar_revision() {
        assert!(postclaim_clipboard_revision_is_authorized(17, 17));
        assert!(!postclaim_clipboard_revision_is_authorized(17, 18));
        // The postclaim helper accepts no CFString or byte buffer, making a
        // second full conversion/hash structurally unavailable after claim.
        assert_eq!(size_of_val(&17_isize), size_of::<isize>());
    }

    #[test]
    fn both_ax_range_and_text_set_errors_use_acceptance_safe_classification() {
        for _step in ["set-range", "set-selected-text"] {
            assert_eq!(
                classify_ax_set_error(ffi::K_AX_ERROR_SUCCESS),
                AxSetOutcome::Succeeded
            );
            assert_eq!(
                classify_ax_set_error(ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED),
                AxSetOutcome::DefinitiveFailure
            );
            assert_eq!(
                classify_ax_set_error(ffi::K_AX_ERROR_CANNOT_COMPLETE),
                AxSetOutcome::Ambiguous
            );
            assert_eq!(classify_ax_set_error(-29_999), AxSetOutcome::Ambiguous);
        }

        let cannot_complete = AtomicU8::new(INSERTION_CLAIMED);
        assert!(!settle_claimed_ax_error(
            &cannot_complete,
            ffi::K_AX_ERROR_CANNOT_COMPLETE,
        ));
        assert_eq!(
            insertion_status(cannot_complete.load(Ordering::Acquire)),
            InsertionStatus::Ambiguous
        );

        let succeeded = AtomicU8::new(INSERTION_CLAIMED);
        if settle_claimed_ax_error(&succeeded, ffi::K_AX_ERROR_SUCCESS) {
            complete_claimed_insertion(&succeeded, true);
        }
        assert_eq!(
            insertion_status(succeeded.load(Ordering::Acquire)),
            InsertionStatus::Succeeded
        );

        let definitive = AtomicU8::new(INSERTION_CLAIMED);
        assert!(!settle_claimed_ax_error(
            &definitive,
            ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED,
        ));
        assert_eq!(
            insertion_status(definitive.load(Ordering::Acquire)),
            InsertionStatus::Failed(PasteFailure::OsRejected)
        );
    }

    #[test]
    fn real_safety_failures_are_clipboard_only_only_before_claim() {
        assert_eq!(
            insertion_identity_failure(true, false),
            Some(InsertionFailure::Paste(PasteFailure::Unavailable))
        );
        assert_eq!(
            insertion_identity_failure(false, false),
            Some(InsertionFailure::TargetInvalid)
        );
        assert_eq!(insertion_identity_failure(false, true), None);

        let pending = AtomicU8::new(INSERTION_PENDING);
        fail_pending_insertion(&pending, PasteFailure::SecureInput);
        assert_eq!(
            insertion_status(pending.load(Ordering::Acquire)),
            InsertionStatus::Failed(PasteFailure::SecureInput)
        );

        let target_invalid = AtomicU8::new(INSERTION_PENDING);
        fail_insertion(&target_invalid, InsertionFailure::TargetInvalid);
        assert_eq!(
            insertion_status(target_invalid.load(Ordering::Acquire)),
            InsertionStatus::TargetInvalid
        );

        let claimed = AtomicU8::new(INSERTION_CLAIMED);
        fail_pending_insertion(&claimed, PasteFailure::PermissionDenied);
        assert_eq!(
            insertion_status(claimed.load(Ordering::Acquire)),
            InsertionStatus::Claimed,
            "a post-claim safety transition cannot publish clipboard-only",
        );
        mark_claimed_insertion_ambiguous(&claimed);
        assert_eq!(
            insertion_status(claimed.load(Ordering::Acquire)),
            InsertionStatus::Ambiguous
        );
    }

    #[test]
    fn claimed_timeout_is_explicitly_ambiguous_and_budget_is_reserved() {
        let now = Instant::now();
        assert!(!insertion_completion_budget_available(
            now + AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN - Duration::from_nanos(1),
            now,
        ));
        assert!(insertion_completion_budget_available(
            now + AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN,
            now,
        ));
        let state = Arc::new(AtomicU8::new(INSERTION_CLAIMED));
        let (release_tx, release_rx) = bounded(1);
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            release_rx.recv().unwrap();
            complete_claimed_insertion(&worker_state, true);
        });
        // Deterministically expire owner proof while the worker is paused after
        // claim but before its simulated AX completion.
        mark_claimed_insertion_ambiguous(&state);
        assert_eq!(
            insertion_status(state.load(Ordering::Acquire)),
            InsertionStatus::Ambiguous
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            insertion_status(state.load(Ordering::Acquire)),
            InsertionStatus::Ambiguous,
            "terminal ambiguity cannot be rewritten as ordinary success/failure",
        );
    }

    #[test]
    fn cache_read_never_waits_for_worker_slot_lock() {
        let cache = TargetCache::without_worker();
        let _worker_guard = cache.shared.slot.lock().unwrap();
        assert!(cache.reserve_activation_at(Instant::now()).is_none());
    }

    #[test]
    fn complete_tuple_comparison_rejects_every_identity_change() {
        let baseline = distinct_evidence(7, c"app", c"window", c"control");
        assert!(same_target(
            &baseline,
            &distinct_evidence(7, c"app", c"window", c"control")
        ));
        assert!(!same_target(
            &baseline,
            &distinct_evidence(8, c"app", c"window", c"control")
        ));
        assert!(!same_target(
            &baseline,
            &distinct_evidence(7, c"other-app", c"window", c"control")
        ));
        assert!(!same_target(
            &baseline,
            &distinct_evidence(7, c"app", c"other-window", c"control")
        ));
        assert!(!same_target(
            &baseline,
            &distinct_evidence(7, c"app", c"window", c"other-control")
        ));
        let mut moved_caret = distinct_evidence(7, c"app", c"window", c"control");
        moved_caret.selected_text_range.location += 1;
        assert!(!same_target(&baseline, &moved_caret));
    }

    #[test]
    fn unavailable_epoch_or_evidence_is_always_targetless() {
        let mut unavailable = TargetRegistry::new();
        assert!(
            unavailable
                .bind_context(generation(1), Some(handle(1)))
                .target_token()
                .is_none()
        );
        let mut initialized = TargetRegistry::with_epoch([4; 16]);
        assert!(
            initialized
                .bind_context(generation(1), None)
                .target_token()
                .is_none()
        );
    }

    #[test]
    fn focused_identity_attribute_names_match_accessibility_constants() {
        assert_eq!(AX_FOCUSED_APPLICATION.to_bytes(), b"AXFocusedApplication");
        assert_eq!(AX_FOCUSED_WINDOW.to_bytes(), b"AXFocusedWindow");
        assert_eq!(AX_FOCUSED_UI_ELEMENT.to_bytes(), b"AXFocusedUIElement");
        assert_eq!(AX_WINDOW.to_bytes(), b"AXWindow");
        assert_eq!(
            AX_FOCUSED_WINDOW_CHANGED.to_bytes(),
            b"AXFocusedWindowChanged"
        );
        assert_eq!(
            AX_FOCUSED_UI_ELEMENT_CHANGED.to_bytes(),
            b"AXFocusedUIElementChanged"
        );
    }
}
