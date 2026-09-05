//! Owner mutation messages and their atomic claim/cancellation protocol.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnerMutationKind {
    Configure,
    CloseAdmission,
    CancelCandidate,
    SetSessionCapture,
    #[cfg(test)]
    InjectTestUnmatchedDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OwnerMutation {
    pub(super) kind: OwnerMutationKind,
    pub(super) activation: ActivationConfig,
    pub(super) session_capture_mode: SessionCaptureMode,
}

impl OwnerMutation {
    pub(super) fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    pub(super) fn close_admission() -> Self {
        Self {
            kind: OwnerMutationKind::CloseAdmission,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    pub(super) fn cancel_candidate() -> Self {
        Self {
            kind: OwnerMutationKind::CancelCandidate,
            activation: ActivationConfig::default(),
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

    #[cfg(test)]
    pub(super) fn inject_test_unmatched_down() -> Self {
        Self {
            kind: OwnerMutationKind::InjectTestUnmatchedDown,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }
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

pub(super) struct OwnerCommand {
    pub(super) mutation: OwnerMutation,
    pub(super) state: Arc<AtomicU8>,
    pub(super) acknowledgement: Sender<Result<(), PlatformError>>,
}
