use std::sync::Arc;

use crossbeam_channel::Sender;

use super::{
    CallbackGate, FrontApp, HookStatus, PasteFailure, PasteResult, PermissionState, Permissions,
    Platform, PlatformError, TerminalReason, TerminalSignal,
};
use crate::{keyboard::ActivationBindings, protocol::Outbound};

/// Build-only backend so protocol and reducer tests remain portable. Release
/// artifacts are produced only for Windows and macOS.
pub struct NativePlatform {
    gate: Arc<CallbackGate>,
}

impl Platform for NativePlatform {
    fn start(
        _outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        _terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        Ok(Self { gate })
    }

    fn hook_status(&self) -> HookStatus {
        HookStatus::Unavailable
    }

    fn configure_activation(
        &self,
        _enabled: bool,
        _bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    fn set_session_capture(&self, _active: bool) -> Result<(), PlatformError> {
        Ok(())
    }

    fn inject_paste(&self) -> PasteResult {
        PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        }
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        Err(PlatformError::NativeFailure)
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            accessibility: PermissionState::NotApplicable,
            input_monitoring: PermissionState::NotApplicable,
            event_post: PermissionState::NotApplicable,
        }
    }

    fn shutdown(&mut self) -> Option<TerminalReason> {
        self.gate.close();
        None
    }
}
