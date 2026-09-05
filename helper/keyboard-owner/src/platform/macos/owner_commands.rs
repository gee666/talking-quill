//! Startup completion and owner mutation handoffs.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(in crate::platform) enum StartupState {
    Pending,
    Running,
    Cancelled,
}

pub(in crate::platform) fn claim_startup(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            StartupState::Pending as u8,
            StartupState::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(in crate::platform) fn cancel_startup(state: &AtomicU8) -> StartupState {
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

pub(in crate::platform) fn owner_completed(receiver: &Receiver<()>, timeout: Duration) -> bool {
    receiver.recv_timeout(timeout).is_ok()
}

pub(super) fn owner_is_already_quiescent(receiver: &Receiver<()>, thread: &JoinHandle<()>) -> bool {
    receiver.try_recv().is_ok() || thread.is_finished()
}

pub(in crate::platform) struct OwnerCompletion(pub(in crate::platform) Sender<()>);

impl Drop for OwnerCompletion {
    fn drop(&mut self) {
        #[cfg(feature = "transactional-shortcuts-dev")]
        TEST_NATIVE_RESOURCE_TEARDOWN_COMPLETE.store(true, Ordering::Release);
        let _ = self.0.try_send(());
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(in crate::platform) fn mark_test_semantic_drain_complete() {
    TEST_SEMANTIC_DRAIN_COMPLETE.store(true, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform) struct ActivationConfig {
    pub(in crate::platform) enabled: bool,
    pub(in crate::platform) bindings: ActivationBindings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(in crate::platform) enum OwnerCommandState {
    Pending,
    Applying,
    Applied,
    Cancelled,
}

pub(in crate::platform) fn owner_command_state(state: &AtomicU8) -> OwnerCommandState {
    match state.load(Ordering::Acquire) {
        1 => OwnerCommandState::Applying,
        2 => OwnerCommandState::Applied,
        3 => OwnerCommandState::Cancelled,
        _ => OwnerCommandState::Pending,
    }
}

pub(in crate::platform) fn claim_owner_command(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            OwnerCommandState::Pending as u8,
            OwnerCommandState::Applying as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(in crate::platform) fn cancel_owner_command(state: &AtomicU8) -> OwnerCommandState {
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
pub(in crate::platform) enum OwnerMutationKind {
    Configure,
    SetSessionCapture,
    SuspendNativeInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform) struct OwnerMutation {
    pub(in crate::platform) kind: OwnerMutationKind,
    pub(in crate::platform) activation: ActivationConfig,
    pub(in crate::platform) session_capture_mode: SessionCaptureMode,
}

impl OwnerMutation {
    pub(super) fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    pub(super) fn set_session_capture(mode: SessionCaptureMode) -> Self {
        Self {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture_mode: mode,
        }
    }

    pub(super) fn suspend_native_input() -> Self {
        Self {
            kind: OwnerMutationKind::SuspendNativeInput,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }
}

pub(in crate::platform) struct OwnerCommand {
    pub(in crate::platform) mutation: OwnerMutation,
    pub(in crate::platform) state: Arc<AtomicU8>,
    pub(in crate::platform) acknowledgement: Sender<Result<(), PlatformError>>,
}
