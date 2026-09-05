//! Native platform boundary.
//!
//! All OS FFI and callback entry points are confined to the target modules.
//! Shared protocol and reducer code contains no unsafe code.

use std::{fmt, sync::Arc};

use crossbeam_channel::Sender;
use serde::Serialize;
use thiserror::Error;

use crate::ActivationCaptureGate;
use talking_quill_keyboard_core::{
    ActivationBindings, ActivationContext, KeyboardEvent, SessionCaptureMode,
};

mod callback;
#[cfg(target_os = "macos")]
mod macos;
mod observability;
#[cfg(any(target_os = "macos", test))]
mod tap_recovery;
pub(crate) use callback::deliver_callback_event;
pub use callback::{CallbackGate, TerminalReason, TerminalSignal};
#[cfg(target_os = "macos")]
pub(crate) use tap_recovery::{TapRecoveryDecision, TapRecoveryEvent, TapRecoveryPolicy};
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::NativePlatform;
#[cfg(all(target_os = "macos", feature = "transactional-shortcuts-dev"))]
#[doc(hidden)]
pub use macos::{
    MacosPhysicalSeamSnapshot, macos_physical_seam_snapshot, macos_test_modifier_barrier_contract,
    macos_test_permission_disable_recovery_contract, macos_test_target_caret_identity_contract,
    post_macos_test_permission_loss, post_macos_test_physical_key,
    post_macos_test_physical_mouse_down, reset_macos_physical_seam, set_macos_test_permission_loss,
    set_macos_test_secure_input,
};
pub use observability::{
    CancellationReasonCounters, EffectOutcomeCounters, NativePasteCounters,
    RegisteredInputCounters, TransactionCounters, TransactionObservabilitySnapshot,
};
pub(crate) use observability::{ModifierNeutralWait, TransactionObservability};
#[cfg(not(any(windows, target_os = "macos")))]
pub use unsupported::NativePlatform;
#[cfg(windows)]
pub use windows::NativePlatform;

#[doc(hidden)]
pub fn write_native_test_report_from_env() {
    #[cfg(all(target_os = "macos", feature = "transactional-shortcuts-dev"))]
    macos::write_macos_test_report_from_env();
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NativeEvent {
    Keyboard(KeyboardEvent),
    /// Privacy-safe proof that one configured chord matched and its exact
    /// trigger was released in the same passive observation generation.
    RegisteredObservation {
        generation: u64,
    },
    AudioInputDevicesChanged,
}

impl fmt::Debug for NativeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Keyboard(_) => "NativeEvent::Keyboard(<redacted>)",
            Self::RegisteredObservation { .. } => "NativeEvent::RegisteredObservation(<redacted>)",
            Self::AudioInputDevicesChanged => "NativeEvent::AudioInputDevicesChanged",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    /// Hook registration and its owner-thread pump are live; no configured
    /// physical shortcut boundary has yet been observed.
    InstalledUnobserved,
    /// At least one configured shortcut candidate reached the native callback.
    PhysicalObserved,
    PermissionRequired,
    Unavailable,
    Stopped,
}

pub(crate) const fn hook_status_to_u8(status: HookStatus) -> u8 {
    match status {
        HookStatus::InstalledUnobserved => 0,
        HookStatus::PermissionRequired => 1,
        HookStatus::Unavailable => 2,
        HookStatus::Stopped => 3,
        HookStatus::PhysicalObserved => 4,
    }
}

pub(crate) const fn hook_status_from_u8(value: u8) -> HookStatus {
    match value {
        0 => HookStatus::InstalledUnobserved,
        1 => HookStatus::PermissionRequired,
        3 => HookStatus::Stopped,
        4 => HookStatus::PhysicalObserved,
        _ => HookStatus::Unavailable,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub accessibility: PermissionState,
    pub input_monitoring: PermissionState,
    pub event_post: PermissionState,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) const fn permissions_allow_native_input(permissions: Permissions) -> bool {
    permission_satisfied(permissions.accessibility)
        && permission_satisfied(permissions.input_monitoring)
        && permission_satisfied(permissions.event_post)
}

#[cfg(any(target_os = "macos", test))]
const fn permission_satisfied(permission: PermissionState) -> bool {
    matches!(
        permission,
        PermissionState::Granted | PermissionState::NotApplicable
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontApp {
    pub process_name: String,
    pub window_title: String,
    pub window_bounds: Option<WindowBounds>,
}

/// Native strings are bounded before they can enter a later protocol mapper.
#[cfg(target_os = "macos")]
pub(crate) const MAX_FRONT_APP_FIELD_ESCAPED_BYTES: usize = 7 * 1024;
/// Native paste never processes more UTF-8 than the application's transcript
/// and smart-output insertion contract permits.
pub(crate) const MAX_INSERTION_UTF8_BYTES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteFailure {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    OsRejected,
    Unavailable,
    /// Injection authority was claimed but native completion was not proved
    /// before the bounded deadline. Callers must not retry.
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteResult {
    pub submitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PasteFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardTextHash([u8; 32]);

impl ClipboardTextHash {
    #[cfg(any(windows, target_os = "macos", test))]
    pub(crate) const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("native keyboard hook is unavailable")]
    HookUnavailable,
    #[error("native operation was denied by operating-system permissions")]
    PermissionDenied,
    #[error("native operation failed")]
    NativeFailure,
    #[error("native hook thread stopped")]
    ThreadStopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformShutdown {
    pub terminal_reason: Option<TerminalReason>,
    pub observability_quiescent: bool,
    /// The bounded native shutdown deadline expired with physical ownership
    /// still unresolved. The hook is stopped and no balancing input was
    /// synthesized, so process/singleton retirement is safe but incomplete.
    pub terminal_incomplete: bool,
}

impl PlatformShutdown {
    pub const fn quiescent(terminal_reason: Option<TerminalReason>) -> Self {
        Self {
            terminal_reason,
            observability_quiescent: true,
            terminal_incomplete: false,
        }
    }
}

pub trait Platform: Sized {
    fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError>;
    fn hook_status(&self) -> HookStatus;
    fn protocol_initialized(&self) {}
    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError>;
    fn close_activation_admission(
        &self,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        self.configure_activation(false, bindings)
    }
    fn cancel_activation_candidate(&self) -> Result<(), PlatformError> {
        Ok(())
    }
    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError>;
    fn inject_paste(&self) -> PasteResult;
    /// Protocol integration boundary for target-aware paste. Platform-specific adapters must
    /// override this only when they can revalidate the supplied activation target.
    fn inject_paste_for_activation(&self, _context: ActivationContext) -> PasteResult {
        PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        }
    }
    fn inject_paste_for_activation_with_clipboard_hash(
        &self,
        context: ActivationContext,
        _expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        self.inject_paste_for_activation(context)
    }
    fn front_app(&self) -> Result<FrontApp, PlatformError>;
    fn permissions(&self) -> Permissions;
    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        TransactionObservabilitySnapshot::default()
    }
    fn record_adapter_dequeued(&self) {}
    /// Conservative aggregate native authority. `true` means the platform may
    /// still own a hidden edge, replay/cleanup suffix, or submitted operation.
    /// It contains no key, target, journal, or clipboard data.
    fn native_work_pending(&self) -> bool {
        false
    }
    /// Closes native admission and completes one bounded platform shutdown.
    /// A `None` result permits the final protocol response; a terminal reason
    /// suppresses success and lets process exit provide best-effort teardown.
    fn shutdown(&mut self) -> PlatformShutdown;
}

#[cfg(test)]
mod tests;
