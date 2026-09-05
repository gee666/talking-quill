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
mod owner_commands;
mod paste_commands;
mod platform_impl;
#[cfg(test)]
mod platform_tests;
mod submission;
mod target;
#[cfg(feature = "transactional-shortcuts-dev")]
mod test_seam;
use owner_commands::*;
use paste_commands::*;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use test_seam::*;
#[cfg(feature = "transactional-shortcuts-dev")]
pub use test_seam::{
    MacosPhysicalSeamSnapshot, macos_physical_seam_snapshot, macos_test_modifier_barrier_contract,
    macos_test_permission_disable_recovery_contract, macos_test_target_caret_identity_contract,
    post_macos_test_permission_loss, post_macos_test_physical_key,
    post_macos_test_physical_mouse_down, reset_macos_physical_seam, set_macos_test_permission_loss,
    set_macos_test_secure_input,
};

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

pub(super) fn secure_input_active() -> bool {
    // SAFETY: this public HIToolbox status query has no arguments or ownership.
    unsafe { ffi::IsSecureEventInputEnabled() != 0 }
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
    // SAFETY: startup publishes an explicit retain for each wake resource.
    // SharedState releases those retains only on drop, so this borrow keeps
    // both objects alive even after owner teardown invalidates its resources.
    // Core Foundation permits source signalling and wake-up across threads.
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

fn permission_poll_requires_suspension(status: HookStatus, permissions: Permissions) -> bool {
    status != HookStatus::PermissionRequired && !permissions_allow_native_input(permissions)
}
