use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded};

use super::{
    CallbackGate, FrontApp, HookStatus, PasteResult, Permissions, Platform, PlatformError,
    TerminalReason, TerminalSignal, hook_status_from_u8, hook_status_to_u8,
    permissions_allow_native_input,
};
use crate::{keyboard::ActivationBindings, protocol::Outbound};

mod accessibility;
mod cf;
mod event_tap;
mod ffi;
mod paste;

const INJECTED_MARKER: i64 = 0x4D45_4348_4F50_5354;
const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnerJoinOutcome {
    Joined,
    Panicked,
    TimedOut,
}

pub(super) fn join_owner_bounded(thread: JoinHandle<()>, timeout: Duration) -> OwnerJoinOutcome {
    let (joined_tx, joined_rx) = bounded(1);
    let spawned = thread::Builder::new()
        .name("talking-quill-helper-macos-reaper".into())
        .spawn(move || {
            let outcome = if thread.join().is_ok() {
                OwnerJoinOutcome::Joined
            } else {
                OwnerJoinOutcome::Panicked
            };
            let _ = joined_tx.try_send(outcome);
        });
    if spawned.is_err() {
        return OwnerJoinOutcome::TimedOut;
    }
    joined_rx
        .recv_timeout(timeout)
        .unwrap_or(OwnerJoinOutcome::TimedOut)
}

pub(super) struct OwnerCompletion(pub(super) Sender<()>);

impl Drop for OwnerCompletion {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
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
    pub(super) session_capture: bool,
}

impl OwnerMutation {
    fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture: false,
        }
    }

    fn set_session_capture(active: bool) -> Self {
        Self {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture: active,
        }
    }

    fn suspend_native_input() -> Self {
        Self {
            kind: OwnerMutationKind::SuspendNativeInput,
            activation: ActivationConfig::default(),
            session_capture: false,
        }
    }
}

pub(super) struct OwnerCommand {
    pub(super) mutation: OwnerMutation,
    pub(super) state: Arc<AtomicU8>,
    pub(super) acknowledgement: Sender<Result<(), PlatformError>>,
}

#[derive(Default)]
struct OwnerEndpoint {
    run_loop: usize,
    command_source: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerSignalOutcome {
    Signalled,
    Unavailable,
    Poisoned,
}

fn signal_owner_endpoint(state: &SharedState) -> OwnerSignalOutcome {
    let endpoint = match state.owner_endpoint.lock() {
        Ok(endpoint) => endpoint,
        Err(_) => return OwnerSignalOutcome::Poisoned,
    };
    let source = endpoint.command_source as ffi::CFRunLoopSourceRef;
    let run_loop = endpoint.run_loop as ffi::CFRunLoopRef;
    if source.is_null() || run_loop.is_null() {
        return OwnerSignalOutcome::Unavailable;
    }
    // SAFETY: the endpoint lock prevents owner teardown from invalidating or
    // releasing either object until signaling and wake-up return.
    unsafe {
        ffi::CFRunLoopSourceSignal(source);
        ffi::CFRunLoopWakeUp(run_loop);
    }
    OwnerSignalOutcome::Signalled
}

fn signal_owner_bounded(state: Arc<SharedState>, timeout: Duration) -> Option<OwnerSignalOutcome> {
    let (signalled_tx, signalled_rx) = bounded(1);
    let spawned = thread::Builder::new()
        .name("talking-quill-helper-macos-wake".into())
        .spawn(move || {
            let _ = signalled_tx.try_send(signal_owner_endpoint(&state));
        });
    if spawned.is_err() {
        return None;
    }
    signalled_rx.recv_timeout(timeout).ok()
}

struct SharedState {
    owner_endpoint: Mutex<OwnerEndpoint>,
    session_capture: AtomicBool,
    hook_status: AtomicU8,
    event_tap: AtomicPtr<c_void>,
    tap_recovery: AtomicU8,
    quiescing: AtomicBool,
    stopping: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            owner_endpoint: Mutex::new(OwnerEndpoint::default()),
            session_capture: AtomicBool::new(false),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            event_tap: AtomicPtr::new(null_mut()),
            tap_recovery: AtomicU8::new(0),
            quiescing: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        }
    }
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    owner_commands: Sender<OwnerCommand>,
    owner_completion: Receiver<()>,
    thread: Option<JoinHandle<()>>,
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
        self.owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .map_err(|_| PlatformError::NativeFailure)?;

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

    fn signal_owner(&self) -> bool {
        self.signal_owner_with_timeout(OWNER_COMMAND_TIMEOUT)
    }

    fn signal_owner_with_timeout(&self, timeout: Duration) -> bool {
        match signal_owner_bounded(Arc::clone(&self.state), timeout) {
            Some(OwnerSignalOutcome::Signalled) => true,
            Some(OwnerSignalOutcome::Poisoned) => {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                false
            }
            Some(OwnerSignalOutcome::Unavailable) | None => false,
        }
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

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(SharedState::new());
        let (owner_commands, owner_command_receiver) = bounded(8);
        let (thread, owner_completion) = event_tap::start_hook(
            Arc::clone(&state),
            owner_command_receiver,
            outbound,
            Arc::clone(&gate),
            Arc::clone(&terminal),
        )?;
        Ok(Self {
            state,
            gate,
            terminal,
            owner_commands,
            owner_completion,
            thread: Some(thread),
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
        if self.hook_status() != HookStatus::Ready {
            return if enabled {
                Err(PlatformError::HookUnavailable)
            } else {
                Ok(())
            };
        }
        if enabled && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            let _ = self.suspend_native_input();
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        if !active && !self.gate.is_open() {
            self.state.quiescing.store(true, Ordering::Release);
        }
        if self.hook_status() != HookStatus::Ready {
            if !active {
                self.state.session_capture.store(false, Ordering::Release);
                return Ok(());
            }
            return Err(PlatformError::HookUnavailable);
        }
        if active && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            let _ = self.suspend_native_input();
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(active),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn inject_paste(&self) -> PasteResult {
        paste::inject_paste()
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        accessibility::front_app()
    }

    fn permissions(&self) -> Permissions {
        let permissions = accessibility::permission_snapshot();
        if !permissions_allow_native_input(permissions) {
            // The owner acknowledgement is the fail-open linearization point:
            // activation, session capture, and reducer capture are all cleared
            // before this permission snapshot is returned.
            let _ = self.suspend_native_input();
        }
        permissions
    }

    fn shutdown(&mut self) -> Option<TerminalReason> {
        self.gate.close();
        event_tap::request_stop(&self.state);
        let Some(thread) = self.thread.take() else {
            return self.terminal.reason();
        };
        let completed = owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT) || {
            event_tap::request_stop(&self.state);
            owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)
        };
        if completed {
            match join_owner_bounded(thread, OWNER_COMPLETION_TIMEOUT) {
                OwnerJoinOutcome::Joined => self
                    .state
                    .hook_status
                    .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release),
                OwnerJoinOutcome::Panicked => {
                    self.mark_owner_failure(TerminalReason::HookStopped);
                }
                OwnerJoinOutcome::TimedOut => {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                }
            }
        } else {
            drop(thread);
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
        }
        self.terminal.reason()
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
    fn owner_completion_wait_is_bounded_and_accepts_cleanup() {
        let (completed_tx, completed_rx) = bounded(1);
        drop(OwnerCompletion(completed_tx));
        assert!(owner_completed(&completed_rx, Duration::from_millis(1)));

        let (_pending_tx, pending_rx) = bounded(1);
        assert!(!owner_completed(&pending_rx, Duration::from_millis(1)));
    }

    #[test]
    fn stop_request_does_not_wait_for_the_owner_endpoint_lock() {
        let state = Arc::new(SharedState::new());
        let endpoint = state.owner_endpoint.lock().unwrap();
        let caller_state = Arc::clone(&state);
        let (returned_tx, returned_rx) = bounded(1);
        let caller = std::thread::spawn(move || {
            event_tap::request_stop(&caller_state);
            let _ = returned_tx.send(());
        });

        assert!(returned_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(state.quiescing.load(Ordering::Acquire));
        assert!(state.stopping.load(Ordering::Acquire));
        assert!(!state.session_capture.load(Ordering::Acquire));

        drop(endpoint);
        caller.join().unwrap();
    }

    #[test]
    fn owner_signal_wait_is_bounded_when_the_endpoint_lock_is_busy() {
        let state = Arc::new(SharedState::new());
        let endpoint = state.owner_endpoint.lock().unwrap();
        assert_eq!(
            signal_owner_bounded(Arc::clone(&state), Duration::from_millis(10)),
            None
        );
        drop(endpoint);
    }

    #[test]
    fn owner_join_wait_is_bounded_after_cleanup_completion() {
        let (completion_tx, completion_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let owner = std::thread::spawn(move || {
            drop(OwnerCompletion(completion_tx));
            let _ = release_rx.recv();
        });
        assert!(owner_completed(&completion_rx, Duration::from_millis(100)));
        assert_eq!(
            join_owner_bounded(owner, Duration::from_millis(10)),
            OwnerJoinOutcome::TimedOut
        );
        let _ = release_tx.send(());
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
}
