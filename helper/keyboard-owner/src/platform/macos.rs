use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded};
#[cfg(feature = "transactional-shortcuts-dev")]
use std::sync::atomic::AtomicU64;

use super::{
    CallbackGate, ClipboardTextHash, FrontApp, HookStatus, ModifierNeutralWait, PasteFailure,
    PasteResult, Permissions, Platform, PlatformError, PlatformShutdown, TerminalReason,
    TerminalSignal, TransactionObservability, TransactionObservabilitySnapshot,
    hook_status_from_u8, hook_status_to_u8, permissions_allow_native_input,
};
use crate::{ActivationCaptureGate, platform::NativeEvent};
use talking_quill_keyboard_core::{ActivationBindings, ActivationContext, SessionCaptureMode};

mod accessibility;
mod cf;
mod event_tap;
mod ffi;
mod injection;
mod target;

const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_millis(1_500);
const OWNER_TERMINAL_COMPLETION_MARGIN: Duration = Duration::from_millis(500);
/// One absolute native paste budget, leaving at least one second for the
/// coordinator/writer before Electron's three-second request timeout.
const PASTE_COMMAND_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Reserve bounded time for tap observation, owner acknowledgement, and the
/// framed RPC writer. No first paste event may be posted after this cutoff.
const PASTE_FINAL_ACK_MARGIN: Duration = Duration::from_millis(500);

#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_SEMANTIC_DRAIN_COMPLETE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_REPLAY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_MOUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_MARKER_ACKS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_OPERATION_GENERATIONS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_OPERATION_OBSERVATIONS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_FORCE_PERMISSION_LOSS: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_TAP_DISABLE_CONFIRMED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_TAP_DRAIN_REENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_KEY_SEEN: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];
#[cfg(feature = "transactional-shortcuts-dev")]
static TEST_KEY_HELD: [AtomicBool; 128] = [const { AtomicBool::new(false) }; 128];

#[cfg(feature = "transactional-shortcuts-dev")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
enum MacosTestOperationClass {
    Replay = 0,
    Cleanup = 1,
    GapBarrier = 2,
    PasteBarrier = 3,
}

#[cfg(feature = "transactional-shortcuts-dev")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacosPhysicalSeamSnapshot {
    pub replay_sequence: u64,
    pub mouse_sequence: u64,
    pub marker_acknowledgements: u64,
    pub operation_generations: [u64; 4],
    pub operation_observations: [u64; 4],
    pub tap_disable_confirmed: bool,
    pub tap_drain_reenabled: bool,
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn reset_macos_physical_seam() {
    TEST_EVENT_SEQUENCE.store(1, Ordering::Release);
    TEST_REPLAY_SEQUENCE.store(0, Ordering::Release);
    TEST_MOUSE_SEQUENCE.store(0, Ordering::Release);
    TEST_MARKER_ACKS.store(0, Ordering::Release);
    for value in TEST_OPERATION_GENERATIONS
        .iter()
        .chain(TEST_OPERATION_OBSERVATIONS.iter())
    {
        value.store(0, Ordering::Release);
    }
    TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE.store(false, Ordering::Release);
    TEST_SEMANTIC_DRAIN_COMPLETE.store(false, Ordering::Release);
    TEST_FORCE_PERMISSION_LOSS.store(false, Ordering::Release);
    TEST_TAP_DISABLE_CONFIRMED.store(false, Ordering::Release);
    TEST_TAP_DRAIN_REENABLED.store(false, Ordering::Release);
    for value in TEST_KEY_SEEN.iter().chain(TEST_KEY_HELD.iter()) {
        value.store(false, Ordering::Release);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_physical_seam_snapshot() -> MacosPhysicalSeamSnapshot {
    MacosPhysicalSeamSnapshot {
        replay_sequence: TEST_REPLAY_SEQUENCE.load(Ordering::Acquire),
        mouse_sequence: TEST_MOUSE_SEQUENCE.load(Ordering::Acquire),
        marker_acknowledgements: TEST_MARKER_ACKS.load(Ordering::Acquire),
        operation_generations: std::array::from_fn(|index| {
            TEST_OPERATION_GENERATIONS[index].load(Ordering::Acquire)
        }),
        operation_observations: std::array::from_fn(|index| {
            TEST_OPERATION_OBSERVATIONS[index].load(Ordering::Acquire)
        }),
        tap_disable_confirmed: TEST_TAP_DISABLE_CONFIRMED.load(Ordering::Acquire),
        tap_drain_reenabled: TEST_TAP_DRAIN_REENABLED.load(Ordering::Acquire),
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn write_macos_test_report_from_env() {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ProcessTestReport {
        native_resource_teardown_complete: bool,
        semantic_drain_complete: bool,
        #[serde(flatten)]
        seam: MacosPhysicalSeamSnapshot,
    }

    let Some(path) = std::env::var_os("TALKING_QUILL_MACOS_TEST_REPORT") else {
        return;
    };
    let report = serde_json::to_vec(&ProcessTestReport {
        native_resource_teardown_complete: TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE
            .load(Ordering::Acquire),
        semantic_drain_complete: TEST_SEMANTIC_DRAIN_COMPLETE.load(Ordering::Acquire),
        seam: macos_physical_seam_snapshot(),
    })
    .expect("fixed macOS test report serializes");
    if let Err(error) = std::fs::write(path, report) {
        eprintln!("talking-quill-helper: cannot write macOS test report: {error}");
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_physical_key(key_code: u16, key_down: bool, flags: u64) -> bool {
    injection::post_test_physical_key(key_code, key_down, flags)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_physical_mouse_down() -> bool {
    injection::post_test_physical_mouse_down()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn post_macos_test_permission_loss() -> bool {
    injection::post_test_permission_loss()
}

pub(super) fn secure_input_active() -> bool {
    // SAFETY: this public HIToolbox status query has no arguments or ownership.
    unsafe { ffi::IsSecureEventInputEnabled() != 0 }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn set_macos_test_secure_input(active: bool) -> bool {
    // This seam calls the real Carbon session API. It is not a simulated flag;
    // callers must always disable it in cleanup.
    let status = unsafe {
        if active {
            ffi::EnableSecureEventInput()
        } else {
            ffi::DisableSecureEventInput()
        }
    };
    status == 0 && (unsafe { ffi::IsSecureEventInputEnabled() != 0 }) == active
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_modifier_barrier_contract() -> bool {
    event_tap::test_modifier_barrier_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_permission_disable_recovery_contract() -> bool {
    event_tap::test_permission_disable_recovery_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn set_macos_test_permission_loss(active: bool) {
    TEST_FORCE_PERMISSION_LOSS.store(active, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn macos_test_permission_loss_active() -> bool {
    TEST_FORCE_PERMISSION_LOSS.load(Ordering::Acquire)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn observe_macos_test_physical_key(key_code: u16, held: bool) {
    if let (Some(seen), Some(state)) = (
        TEST_KEY_SEEN.get(usize::from(key_code)),
        TEST_KEY_HELD.get(usize::from(key_code)),
    ) {
        state.store(held, Ordering::Release);
        seen.store(true, Ordering::Release);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn macos_test_physical_key_state(key_code: u16) -> Option<bool> {
    let seen = TEST_KEY_SEEN.get(usize::from(key_code))?;
    seen.load(Ordering::Acquire)
        .then(|| TEST_KEY_HELD[usize::from(key_code)].load(Ordering::Acquire))
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn record_macos_test_tap_disable(confirmed: bool, reenabled: bool) {
    TEST_TAP_DISABLE_CONFIRMED.store(confirmed, Ordering::Release);
    TEST_TAP_DRAIN_REENABLED.store(reenabled, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub fn macos_test_target_caret_identity_contract() -> bool {
    target::test_target_caret_identity_contract()
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn record_test_replay_submission() {
    let sequence = TEST_EVENT_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    TEST_REPLAY_SEQUENCE.store(sequence, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn record_test_mouse_repost() {
    let sequence = TEST_EVENT_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    TEST_MOUSE_SEQUENCE.store(sequence, Ordering::Release);
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn record_test_marker_acknowledgement(
    class: MacosTestOperationClass,
    token: injection::OperationToken,
) {
    let index = class as usize;
    let generation = token.generation();
    let previous = TEST_OPERATION_GENERATIONS[index].swap(generation, Ordering::AcqRel);
    if previous != generation {
        TEST_OPERATION_OBSERVATIONS[index].store(0, Ordering::Release);
    }
    TEST_OPERATION_OBSERVATIONS[index].fetch_add(1, Ordering::AcqRel);
    TEST_MARKER_ACKS.fetch_add(1, Ordering::AcqRel);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum StartupState {
    Pending,
    Running,
    Cancelled,
}

pub(super) fn claim_startup(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            StartupState::Pending as u8,
            StartupState::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(super) fn cancel_startup(state: &AtomicU8) -> StartupState {
    match state.compare_exchange(
        StartupState::Pending as u8,
        StartupState::Cancelled as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => StartupState::Cancelled,
        Err(value) if value == StartupState::Running as u8 => StartupState::Running,
        Err(_) => StartupState::Cancelled,
    }
}

pub(super) fn owner_completed(receiver: &Receiver<()>, timeout: Duration) -> bool {
    receiver.recv_timeout(timeout).is_ok()
}

fn owner_is_already_quiescent(receiver: &Receiver<()>, thread: &JoinHandle<()>) -> bool {
    receiver.try_recv().is_ok() || thread.is_finished()
}

pub(super) struct OwnerCompletion(pub(super) Sender<()>);

impl Drop for OwnerCompletion {
    fn drop(&mut self) {
        #[cfg(feature = "transactional-shortcuts-dev")]
        TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE.store(true, Ordering::Release);
        let _ = self.0.try_send(());
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn mark_test_semantic_drain_complete() {
    TEST_SEMANTIC_DRAIN_COMPLETE.store(true, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ActivationConfig {
    pub(super) enabled: bool,
    pub(super) bindings: ActivationBindings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum OwnerCommandState {
    Pending,
    Applying,
    Applied,
    Cancelled,
}

pub(super) fn owner_command_state(state: &AtomicU8) -> OwnerCommandState {
    match state.load(Ordering::Acquire) {
        1 => OwnerCommandState::Applying,
        2 => OwnerCommandState::Applied,
        3 => OwnerCommandState::Cancelled,
        _ => OwnerCommandState::Pending,
    }
}

pub(super) fn claim_owner_command(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            OwnerCommandState::Pending as u8,
            OwnerCommandState::Applying as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(super) fn cancel_owner_command(state: &AtomicU8) -> OwnerCommandState {
    match state.compare_exchange(
        OwnerCommandState::Pending as u8,
        OwnerCommandState::Cancelled as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => OwnerCommandState::Cancelled,
        Err(_) => owner_command_state(state),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnerMutationKind {
    Configure,
    SetSessionCapture,
    SuspendNativeInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OwnerMutation {
    pub(super) kind: OwnerMutationKind,
    pub(super) activation: ActivationConfig,
    pub(super) session_capture_mode: SessionCaptureMode,
}

impl OwnerMutation {
    fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    fn set_session_capture(mode: SessionCaptureMode) -> Self {
        Self {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture_mode: mode,
        }
    }

    fn suspend_native_input() -> Self {
        Self {
            kind: OwnerMutationKind::SuspendNativeInput,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }
}

pub(super) struct OwnerCommand {
    pub(super) mutation: OwnerMutation,
    pub(super) state: Arc<AtomicU8>,
    pub(super) acknowledgement: Sender<Result<(), PlatformError>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum PasteCommandState {
    Pending,
    Waiting,
    Injecting,
    Committed,
    Applied,
    Cancelled,
}

pub(super) struct PasteResultSlot {
    value: AtomicU8,
}

impl PasteResultSlot {
    const PENDING: u8 = 0;
    const SUCCESS: u8 = 1;
    const PERMISSION_DENIED: u8 = 2;
    const CONFLICTING_MODIFIERS: u8 = 3;
    const SECURE_INPUT: u8 = 4;
    const OS_REJECTED: u8 = 5;
    const UNAVAILABLE: u8 = 6;
    const INDETERMINATE: u8 = 7;

    pub(super) const fn new() -> Self {
        Self {
            value: AtomicU8::new(Self::PENDING),
        }
    }

    const fn encode(result: PasteResult) -> u8 {
        if result.submitted {
            Self::SUCCESS
        } else {
            match result.reason {
                Some(PasteFailure::PermissionDenied) => Self::PERMISSION_DENIED,
                Some(PasteFailure::ConflictingModifiers) => Self::CONFLICTING_MODIFIERS,
                Some(PasteFailure::SecureInput) => Self::SECURE_INPUT,
                Some(PasteFailure::OsRejected) => Self::OS_REJECTED,
                Some(PasteFailure::Unavailable) | None => Self::UNAVAILABLE,
                Some(PasteFailure::Indeterminate) => Self::INDETERMINATE,
            }
        }
    }

    const fn decode(value: u8) -> Option<PasteResult> {
        match value {
            Self::SUCCESS => Some(PasteResult {
                submitted: true,
                reason: None,
            }),
            Self::PERMISSION_DENIED => Some(failed_paste(PasteFailure::PermissionDenied)),
            Self::CONFLICTING_MODIFIERS => Some(failed_paste(PasteFailure::ConflictingModifiers)),
            Self::SECURE_INPUT => Some(failed_paste(PasteFailure::SecureInput)),
            Self::OS_REJECTED => Some(failed_paste(PasteFailure::OsRejected)),
            Self::UNAVAILABLE => Some(failed_paste(PasteFailure::Unavailable)),
            Self::INDETERMINATE => Some(failed_paste(PasteFailure::Indeterminate)),
            _ => None,
        }
    }

    pub(super) fn publish(&self, result: PasteResult) -> PasteResult {
        let proposed = Self::encode(result);
        let value = self
            .value
            .compare_exchange(Self::PENDING, proposed, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|current| current, |_| proposed);
        Self::decode(value).expect("published paste result encoding")
    }

    pub(super) fn result(&self) -> Option<PasteResult> {
        Self::decode(self.value.load(Ordering::Acquire))
    }
}

pub(super) struct PasteCommand {
    pub(super) context: ActivationContext,
    pub(super) expected_clipboard_sha256: ClipboardTextHash,
    pub(super) state: Arc<AtomicU8>,
    pub(super) result: Arc<PasteResultSlot>,
    pub(super) acknowledgement: Sender<()>,
    pub(super) deadline: Instant,
}

pub(in crate::platform::macos) struct PendingPaste {
    pub(in crate::platform::macos) state: Arc<AtomicU8>,
    pub(in crate::platform::macos) result: Arc<PasteResultSlot>,
    pub(in crate::platform::macos) acknowledgement: Sender<()>,
    pub(in crate::platform::macos) evidence: target::TargetHandle,
    pub(in crate::platform::macos) expected_clipboard_sha256: ClipboardTextHash,
    pub(in crate::platform::macos) validation_request: Option<target::ValidationRequest>,
    pub(in crate::platform::macos) validated_target_epoch: Option<u64>,
    pub(in crate::platform::macos) validated_target_boundary_epoch: Option<u64>,
    pub(in crate::platform::macos) validated_selected_range_epoch: Option<u64>,
    pub(in crate::platform::macos) insertion_request: Option<target::InsertionRequest>,
    pub(in crate::platform::macos) neutral_modifier_epoch: Option<u64>,
    pub(in crate::platform::macos) neutral_barrier_state: u8,
    pub(in crate::platform::macos) neutral_barrier_token: Option<injection::OperationToken>,
    pub(in crate::platform::macos) deadline: Instant,
    pub(in crate::platform::macos) injection_cutoff: Instant,
    pub(in crate::platform::macos) modifier_wait: ModifierNeutralWait,
}

pub(super) fn paste_command_state(state: &AtomicU8) -> PasteCommandState {
    match state.load(Ordering::Acquire) {
        1 => PasteCommandState::Waiting,
        2 => PasteCommandState::Injecting,
        3 => PasteCommandState::Committed,
        4 => PasteCommandState::Applied,
        5 => PasteCommandState::Cancelled,
        _ => PasteCommandState::Pending,
    }
}

pub(super) fn cancel_paste_command(state: &AtomicU8) -> PasteCommandState {
    loop {
        let current = paste_command_state(state);
        if !matches!(
            current,
            PasteCommandState::Pending | PasteCommandState::Waiting
        ) {
            return current;
        }
        if state
            .compare_exchange(
                current as u8,
                PasteCommandState::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return PasteCommandState::Cancelled;
        }
    }
}

pub(super) fn paste_before_deadline(deadline: Instant, now: Instant) -> bool {
    now < deadline
}

pub(super) fn paste_injection_cutoff(deadline: Instant) -> Instant {
    deadline - PASTE_FINAL_ACK_MARGIN
}

fn await_paste_result(
    result: &PasteResultSlot,
    acknowledgement: &Receiver<()>,
    deadline: Instant,
) -> Option<PasteResult> {
    if let Some(result) = result.result() {
        return Some(result);
    }
    let _ = acknowledgement.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    result.result()
}

pub(super) const fn failed_paste(reason: PasteFailure) -> PasteResult {
    PasteResult {
        submitted: false,
        reason: Some(reason),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerSignalOutcome {
    Signalled,
    Unavailable,
}

fn signal_owner_endpoint(state: &SharedState) -> OwnerSignalOutcome {
    let source = state.owner_command_source.load(Ordering::Acquire);
    let run_loop = state.owner_run_loop.load(Ordering::Acquire);
    if source.is_null() || run_loop.is_null() {
        return OwnerSignalOutcome::Unavailable;
    }
    // SAFETY: the source and run loop are permanent startup resources. They
    // are published before Ready and cleared only after the authoritative
    // pending-native-work predicate permits owner-loop exit. Core Foundation
    // documents source signalling and wake-up as thread-safe operations.
    unsafe {
        ffi::CFRunLoopSourceSignal(source);
        ffi::CFRunLoopWakeUp(run_loop);
    }
    OwnerSignalOutcome::Signalled
}

struct SharedState {
    owner_run_loop: AtomicPtr<c_void>,
    owner_command_source: AtomicPtr<c_void>,
    owner_wake_timer: AtomicPtr<c_void>,
    pending_native_work: AtomicBool,
    session_capture_mode: AtomicU8,
    hook_status: AtomicU8,
    event_tap: AtomicPtr<c_void>,
    maintenance_timer: AtomicPtr<c_void>,
    tap_recovery: AtomicU8,
    recovery_pending: AtomicBool,
    recovery_deferred_mode: AtomicBool,
    quiescing: AtomicBool,
    stopping: AtomicBool,
    shutdown_deadline: Mutex<Option<Instant>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            owner_run_loop: AtomicPtr::new(null_mut()),
            owner_command_source: AtomicPtr::new(null_mut()),
            owner_wake_timer: AtomicPtr::new(null_mut()),
            pending_native_work: AtomicBool::new(false),
            session_capture_mode: AtomicU8::new(SessionCaptureMode::Off.as_u8()),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            event_tap: AtomicPtr::new(null_mut()),
            maintenance_timer: AtomicPtr::new(null_mut()),
            tap_recovery: AtomicU8::new(0),
            recovery_pending: AtomicBool::new(false),
            recovery_deferred_mode: AtomicBool::new(false),
            quiescing: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            shutdown_deadline: Mutex::new(None),
        }
    }
}

impl Drop for SharedState {
    fn drop(&mut self) {
        for value in [
            self.owner_wake_timer.load(Ordering::Acquire),
            self.owner_command_source.load(Ordering::Acquire),
            self.owner_run_loop.load(Ordering::Acquire),
        ] {
            if !value.is_null() {
                // SAFETY: owner startup published one explicit retain for each
                // permanent wake resource. Owner teardown invalidates but does
                // not release these shared retains; SharedState drops once.
                unsafe { ffi::CFRelease(value.cast_const()) };
            }
        }
    }
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
    owner_commands: Sender<OwnerCommand>,
    paste_commands: Sender<PasteCommand>,
    owner_completion: Receiver<()>,
    thread: Option<JoinHandle<()>>,
    shutdown_observability_quiescent: bool,
    capture_gate: ActivationCaptureGate,
}

impl NativePlatform {
    fn submit_owner_mutation(
        &self,
        mutation: OwnerMutation,
        terminal_reason: TerminalReason,
    ) -> Result<(), PlatformError> {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return Err(PlatformError::ThreadStopped);
        }

        let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        if self
            .owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .is_err()
        {
            self.mark_owner_failure(terminal_reason);
            return Err(PlatformError::NativeFailure);
        }

        if !self.signal_owner() {
            let state = cancel_owner_command(&command_state);
            if state == OwnerCommandState::Cancelled {
                self.mark_owner_failure(terminal_reason);
                return Err(PlatformError::NativeFailure);
            }
        }

        if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
            return result;
        }

        match cancel_owner_command(&command_state) {
            OwnerCommandState::Applied => Ok(()),
            OwnerCommandState::Applying => {
                if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
                    return result;
                }
                if owner_command_state(&command_state) == OwnerCommandState::Applied {
                    Ok(())
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    Err(PlatformError::NativeFailure)
                }
            }
            OwnerCommandState::Pending | OwnerCommandState::Cancelled => {
                self.mark_owner_failure(terminal_reason);
                Err(PlatformError::NativeFailure)
            }
        }
    }

    fn submit_paste(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        let deadline = Instant::now() + PASTE_COMMAND_TIMEOUT;
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return failed_paste(PasteFailure::Unavailable);
        }
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, response) = bounded(1);
        if self
            .paste_commands
            .send_timeout(
                PasteCommand {
                    context,
                    expected_clipboard_sha256,
                    state: Arc::clone(&state),
                    result: Arc::clone(&result),
                    acknowledgement,
                    deadline,
                },
                deadline.saturating_duration_since(Instant::now()),
            )
            .is_err()
        {
            let _ = cancel_paste_command(&state);
            return result.publish(failed_paste(PasteFailure::Unavailable));
        }
        if !self.signal_owner_with_timeout(deadline.saturating_duration_since(Instant::now())) {
            match cancel_paste_command(&state) {
                PasteCommandState::Pending
                | PasteCommandState::Waiting
                | PasteCommandState::Cancelled => {
                    return result.publish(failed_paste(PasteFailure::Unavailable));
                }
                PasteCommandState::Injecting
                | PasteCommandState::Committed
                | PasteCommandState::Applied => {}
            }
        }
        if let Some(observed) = await_paste_result(&result, &response, deadline) {
            return observed;
        }
        match cancel_paste_command(&state) {
            PasteCommandState::Pending
            | PasteCommandState::Waiting
            | PasteCommandState::Cancelled => {
                result.publish(failed_paste(PasteFailure::Unavailable))
            }
            PasteCommandState::Injecting
            | PasteCommandState::Committed
            | PasteCommandState::Applied => {
                // Native acceptance may have occurred. Do not poison the fixed
                // result slot with an ordinary failure which a later AX success
                // could contradict; close admission and let the server suppress
                // this RPC response as an explicit terminal platform failure.
                self.mark_owner_failure(TerminalReason::InputInjectionUnavailable);
                failed_paste(PasteFailure::Indeterminate)
            }
        }
    }

    fn signal_owner(&self) -> bool {
        self.signal_owner_with_timeout(OWNER_COMMAND_TIMEOUT)
    }

    fn signal_owner_with_timeout(&self, _timeout: Duration) -> bool {
        signal_owner_endpoint(&self.state) == OwnerSignalOutcome::Signalled
    }

    fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }

    fn suspend_native_input(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::suspend_native_input(),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }
}

fn permission_poll_requires_suspension(status: HookStatus, permissions: Permissions) -> bool {
    status != HookStatus::PermissionRequired && !permissions_allow_native_input(permissions)
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(SharedState::new());
        let observability = Arc::new(TransactionObservability::new());
        let (owner_commands, owner_command_receiver) = bounded(8);
        let (paste_commands, paste_command_receiver) = bounded(1);
        let (thread, owner_completion) = event_tap::start_hook(
            Arc::clone(&state),
            owner_command_receiver,
            paste_command_receiver,
            outbound,
            Arc::clone(&gate),
            Arc::clone(&terminal),
            Arc::clone(&observability),
            capture_gate.is_open(),
        )?;
        Ok(Self {
            state,
            gate,
            terminal,
            observability,
            owner_commands,
            paste_commands,
            owner_completion,
            thread: Some(thread),
            shutdown_observability_quiescent: true,
            capture_gate,
        })
    }

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let enabled = self.capture_gate.filter_enabled(enabled);
        if !matches!(
            self.hook_status(),
            HookStatus::InstalledUnobserved | HookStatus::PhysicalObserved
        ) {
            return if enabled {
                Err(PlatformError::HookUnavailable)
            } else {
                Ok(())
            };
        }
        if enabled && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                return Err(PlatformError::NativeFailure);
            }
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError> {
        let mode = self.capture_gate.filter_session_mode(mode);
        if mode == SessionCaptureMode::Off {
            // Fail open at the caller's commit boundary. The owner command acknowledges the
            // mutation without clearing reducer ownership needed to balance an already-held key.
            self.state
                .session_capture_mode
                .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        }
        if mode == SessionCaptureMode::Off && !self.gate.is_open() {
            self.state.quiescing.store(true, Ordering::Release);
        }
        if !matches!(
            self.hook_status(),
            HookStatus::InstalledUnobserved | HookStatus::PhysicalObserved
        ) {
            if mode == SessionCaptureMode::Off {
                return Ok(());
            }
            return Err(PlatformError::HookUnavailable);
        }
        if mode != SessionCaptureMode::Off
            && !permissions_allow_native_input(accessibility::permission_snapshot())
        {
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                return Err(PlatformError::NativeFailure);
            }
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(mode),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn inject_paste(&self) -> PasteResult {
        // Protocol v8 requires an activation context. Never downgrade to an
        // untargeted paste when a caller uses the legacy trait boundary.
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation(&self, _context: ActivationContext) -> PasteResult {
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation_with_clipboard_hash(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        self.submit_paste(context, expected_clipboard_sha256)
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        accessibility::front_app()
    }

    fn permissions(&self) -> Permissions {
        let permissions = accessibility::permission_snapshot();
        // PermissionRequired means startup never installed a tap and therefore
        // owns no native edge. Polling must remain a side-effect-free retry
        // path; attempting owner suspension here would incorrectly terminalize
        // the still-live helper.
        if permission_poll_requires_suspension(self.hook_status(), permissions) {
            // The owner acknowledgement is the fail-open linearization point.
            // Failure is itself terminal; never return as though suspension
            // succeeded while native ownership may remain.
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            }
        }
        permissions
    }

    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        self.observability.snapshot()
    }

    fn native_work_pending(&self) -> bool {
        self.state.pending_native_work.load(Ordering::Acquire)
    }

    fn shutdown(&mut self) -> PlatformShutdown {
        self.gate.close();
        self.state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return PlatformShutdown {
                terminal_reason: self.terminal.reason(),
                observability_quiescent: self.shutdown_observability_quiescent,
                terminal_incomplete: self
                    .observability
                    .snapshot()
                    .native_paste
                    .shutdown_ownership_deadlines
                    != 0,
            };
        };

        // PermissionRequired/no-tap startup owns no native edge and normally
        // finishes before shutdown is requested. Treat that completed owner as
        // clean even though no run-loop wake endpoint was ever published.
        let already_completed = owner_is_already_quiescent(&self.owner_completion, &thread);
        let deadline = Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        match self.state.shutdown_deadline.lock() {
            Ok(mut installed) => {
                let _ = installed.get_or_insert(deadline);
            }
            Err(poisoned) => {
                let _ = poisoned.into_inner().get_or_insert(deadline);
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            }
        }
        if !already_completed {
            let _ = event_tap::request_stop(&self.state);
        }
        let completion_deadline = deadline + OWNER_TERMINAL_COMPLETION_MARGIN;
        let completed = already_completed
            || owner_completed(
                &self.owner_completion,
                completion_deadline.saturating_duration_since(Instant::now()),
            )
            || thread.is_finished();
        while completed && !thread.is_finished() && Instant::now() < completion_deadline {
            std::thread::yield_now();
        }
        if completed && thread.is_finished() {
            if thread.join().is_ok() {
                self.state
                    .hook_status
                    .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
            } else {
                self.mark_owner_failure(TerminalReason::HookStopped);
            }
        } else {
            // A missing endpoint is clean only when the already-quiescent owner
            // actually completed. Live owners still get exactly one deadline.
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.shutdown_observability_quiescent = false;
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            drop(thread);
        }
        PlatformShutdown {
            terminal_reason: self.terminal.reason(),
            observability_quiescent: self.shutdown_observability_quiescent,
            terminal_incomplete: self
                .observability
                .snapshot()
                .native_paste
                .shutdown_ownership_deadlines
                != 0,
        }
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_handoff_is_cancel_or_run_exclusive() {
        let cancelled = AtomicU8::new(StartupState::Pending as u8);
        assert_eq!(cancel_startup(&cancelled), StartupState::Cancelled);
        assert!(!claim_startup(&cancelled));

        let running = AtomicU8::new(StartupState::Pending as u8);
        assert!(claim_startup(&running));
        assert_eq!(cancel_startup(&running), StartupState::Running);
    }

    #[test]
    fn permission_required_poll_never_requests_owner_suspension() {
        let denied = Permissions {
            accessibility: crate::platform::PermissionState::Denied,
            input_monitoring: crate::platform::PermissionState::Denied,
            event_post: crate::platform::PermissionState::Denied,
        };
        assert!(!permission_poll_requires_suspension(
            HookStatus::PermissionRequired,
            denied,
        ));
        assert!(permission_poll_requires_suspension(
            HookStatus::InstalledUnobserved,
            denied,
        ));
    }

    #[test]
    fn permission_required_no_tap_completion_is_cleanly_detected() {
        let (completed_tx, completed_rx) = bounded(1);
        let thread = std::thread::spawn(move || {
            let _ = completed_tx.try_send(());
        });
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        assert!(owner_is_already_quiescent(&completed_rx, &thread));
        assert!(thread.join().is_ok());
    }

    #[test]
    fn owner_completion_wait_is_bounded_and_accepts_cleanup() {
        let (completed_tx, completed_rx) = bounded(1);
        drop(OwnerCompletion(completed_tx));
        assert!(owner_completed(&completed_rx, Duration::from_millis(1)));

        let (_pending_tx, pending_rx) = bounded(1);
        assert!(!owner_completed(&pending_rx, Duration::from_millis(1)));
    }

    #[test]
    fn stop_request_is_nonblocking_when_startup_resources_are_unpublished() {
        let state = Arc::new(SharedState::new());
        event_tap::request_stop(&state);
        assert!(state.quiescing.load(Ordering::Acquire));
        assert!(state.stopping.load(Ordering::Acquire));
        assert_eq!(
            SessionCaptureMode::from_u8(state.session_capture_mode.load(Ordering::Acquire)),
            SessionCaptureMode::Off,
        );
        assert_eq!(
            signal_owner_endpoint(&state),
            OwnerSignalOutcome::Unavailable
        );
    }

    #[test]
    fn owner_signal_has_no_queue_or_lock_full_state() {
        let state = SharedState::new();
        for _ in 0..32 {
            assert_eq!(
                signal_owner_endpoint(&state),
                OwnerSignalOutcome::Unavailable
            );
        }
        assert!(!state.stopping.load(Ordering::Acquire));
    }

    #[test]
    fn full_owner_queue_does_not_prevent_nonblocking_stop_fallback() {
        let state = Arc::new(SharedState::new());
        let (commands, receiver) = bounded(1);
        let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, _response) = bounded(1);
        commands
            .try_send(OwnerCommand {
                mutation: OwnerMutation::suspend_native_input(),
                state: Arc::clone(&command_state),
                acknowledgement,
            })
            .unwrap();
        let (acknowledgement, _response) = bounded(1);
        assert!(
            commands
                .try_send(OwnerCommand {
                    mutation: OwnerMutation::suspend_native_input(),
                    state: Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8)),
                    acknowledgement,
                })
                .is_err()
        );
        event_tap::request_stop(&state);
        assert!(state.stopping.load(Ordering::Acquire));
        assert_eq!(
            signal_owner_endpoint(&state),
            OwnerSignalOutcome::Unavailable
        );
        drop(receiver);
    }

    #[test]
    fn suspension_enqueue_failure_is_terminal_and_closes_gate() {
        let state = Arc::new(SharedState::new());
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (owner_commands, owner_receiver) = bounded(1);
        drop(owner_receiver);
        let (paste_commands, _paste_receiver) = bounded(1);
        let (_completion_tx, owner_completion) = bounded(1);
        let thread = std::thread::spawn(|| {});
        let mut platform = NativePlatform {
            state,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
            observability: Arc::new(TransactionObservability::new()),
            owner_commands,
            paste_commands,
            owner_completion,
            thread: Some(thread),
            shutdown_observability_quiescent: true,
            capture_gate: ActivationCaptureGate::open_for_test_harness(),
        };

        assert!(platform.suspend_native_input().is_err());
        assert!(!gate.is_open());
        assert_eq!(
            terminal.reason(),
            Some(TerminalReason::OwnerThreadUnresponsive)
        );
        if let Some(thread) = platform.thread.take() {
            thread.join().unwrap();
        }
    }

    #[test]
    fn owner_command_handoff_is_cancel_or_apply_exclusive() {
        let cancelled = AtomicU8::new(OwnerCommandState::Pending as u8);
        assert_eq!(
            cancel_owner_command(&cancelled),
            OwnerCommandState::Cancelled
        );
        assert!(!claim_owner_command(&cancelled));

        let applying = AtomicU8::new(OwnerCommandState::Pending as u8);
        assert!(claim_owner_command(&applying));
        assert_eq!(cancel_owner_command(&applying), OwnerCommandState::Applying);
    }

    #[test]
    fn one_absolute_paste_deadline_covers_every_delayed_stage_without_reset() {
        let start = Instant::now();
        let deadline = start + PASTE_COMMAND_TIMEOUT;
        for cumulative_delay in [100, 450, 900, 1_400, 1_999] {
            assert!(paste_before_deadline(
                deadline,
                start + Duration::from_millis(cumulative_delay)
            ));
        }
        assert!(!paste_before_deadline(
            deadline,
            start + PASTE_COMMAND_TIMEOUT
        ));
        assert!(!paste_before_deadline(
            deadline,
            start + PASTE_COMMAND_TIMEOUT + Duration::from_millis(1)
        ));
    }

    #[test]
    fn final_paste_cutoff_reserves_acknowledgement_and_writer_margin() {
        let start = Instant::now();
        let deadline = start + PASTE_COMMAND_TIMEOUT;
        let cutoff = paste_injection_cutoff(deadline);
        assert_eq!(deadline.duration_since(cutoff), PASTE_FINAL_ACK_MARGIN);
        assert!(paste_before_deadline(
            cutoff,
            cutoff - Duration::from_nanos(1)
        ));
        assert!(!paste_before_deadline(cutoff, cutoff));
        assert!(paste_before_deadline(deadline, cutoff));
    }

    #[test]
    fn paste_result_slot_linearizes_only_authoritative_preclaim_or_completion_results() {
        let success_first = PasteResultSlot::new();
        assert!(
            success_first
                .publish(PasteResult {
                    submitted: true,
                    reason: None,
                })
                .submitted
        );
        assert!(
            success_first
                .publish(failed_paste(PasteFailure::OsRejected))
                .submitted
        );

        let rejected_before_claim = PasteResultSlot::new();
        assert_eq!(
            rejected_before_claim.publish(failed_paste(PasteFailure::OsRejected)),
            failed_paste(PasteFailure::OsRejected)
        );
        assert_eq!(
            rejected_before_claim.publish(PasteResult {
                submitted: true,
                reason: None,
            }),
            failed_paste(PasteFailure::OsRejected)
        );
    }

    #[test]
    fn committed_paste_cannot_be_cancelled_while_matching_up_drains() {
        let committed = AtomicU8::new(PasteCommandState::Committed as u8);
        assert_eq!(
            cancel_paste_command(&committed),
            PasteCommandState::Committed
        );
        assert_eq!(
            paste_command_state(&committed),
            PasteCommandState::Committed
        );
    }

    #[test]
    fn paste_command_handoff_cancels_only_before_owner_injection() {
        let pending = AtomicU8::new(PasteCommandState::Pending as u8);
        assert_eq!(cancel_paste_command(&pending), PasteCommandState::Cancelled);
        assert_eq!(paste_command_state(&pending), PasteCommandState::Cancelled);

        let waiting = AtomicU8::new(PasteCommandState::Waiting as u8);
        assert_eq!(cancel_paste_command(&waiting), PasteCommandState::Cancelled);

        for state in [
            PasteCommandState::Injecting,
            PasteCommandState::Committed,
            PasteCommandState::Applied,
        ] {
            let value = AtomicU8::new(state as u8);
            assert_eq!(cancel_paste_command(&value), state);
        }
    }
}
