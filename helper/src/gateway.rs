//! Structural gateway backend and bounded protocol-facing types.
//!
//! The gateway owns framed Electron protocol v10 and an owner-protocol v1
//! client. It has no dependency on the keyboard-owner package and cannot
//! install native input facilities.

use std::sync::Arc;

use crossbeam_channel::Sender;
use serde::Serialize;
use talking_quill_keyboard_core::{ActivationBindings, ActivationContext, SessionCaptureMode};

use crate::protocol::Outbound;

mod gates;
mod observability;
mod platform_types;

// Preserve the crate-visible lease type path alongside the public facade.
#[allow(unused_imports)]
pub(crate) use gates::CallbackDeliveryLease;
pub use gates::{ActivationCaptureGate, CallbackGate, TerminalReason, TerminalSignal};
pub use observability::{
    CancellationReasonCounters, EffectOutcomeCounters, NativePasteCounters,
    RegisteredInputCounters, RuntimeOwnerObservabilitySnapshot, TransactionCounters,
    TransactionObservabilitySnapshot,
};
pub use platform_types::{
    ClipboardTextHash, FrontApp, HookStatus, PasteFailure, PasteResult, PermissionState,
    Permissions, PlatformError, WindowBounds,
};

pub const GATEWAY_POLICY_MARKER: &str =
    "TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS";
pub(crate) const MAX_OBSERVABILITY_COUNTER: u64 = 9_007_199_254_740_991;

/// Acceptance-only projection of authenticated Windows named-pipe kernel facts.
/// Stable identifiers, SID digests, credential bindings, and artifact digests
/// are deliberately not represented by this type.
#[cfg(feature = "windows-installed-acceptance")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceEndpointPeerFacts {
    pub process_id: u32,
    pub creation_marker: String,
    pub integrity_rid: u32,
    pub session_id: u32,
    pub user_sid_hash: String,
}

#[cfg(feature = "windows-installed-acceptance")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceEndpointObservability {
    pub endpoint_version: u8,
    pub peer_authenticated: bool,
    pub release_build_digest: String,
    pub manifest_sha256: String,
    pub gateway: AcceptanceEndpointPeerFacts,
    pub owner: AcceptanceEndpointPeerFacts,
}

#[cfg(feature = "windows-installed-acceptance")]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptancePauseLeaseRenewalResult {
    pub pause_duration_ms: u16,
    pub before_timestamp_ms: u64,
    pub after_timestamp_ms: u64,
    pub before: OwnerObservabilitySnapshot,
    pub after: OwnerObservabilitySnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaintenanceRequest {
    pub operation: MaintenanceOperation,
    pub transaction_id: String,
    pub source_build_id: String,
    pub target_build_id: Option<String>,
    pub target_owner_sha256: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardOwnerState {
    SafeDisabled,
    Idle,
    LeasedDisabled,
    LeasedEnabled,
    Draining,
    Maintenance,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardOwnerSnapshot {
    pub model: &'static str,
    pub protocol_version: u16,
    pub state: KeyboardOwnerState,
    pub instance_id: String,
    pub build_id: String,
    pub lease_epoch: Option<u64>,
    pub authenticated: bool,
    #[serde(skip)]
    pub keyboard_build_eligible: bool,
    #[serde(skip)]
    pub permissions_eligible: bool,
    #[serde(skip)]
    pub hook_healthy: bool,
    #[serde(skip)]
    pub rollback_latched: bool,
}

impl KeyboardOwnerSnapshot {
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            model: "out_of_process",
            protocol_version: 1,
            state: KeyboardOwnerState::Unavailable,
            instance_id: String::new(),
            build_id: String::new(),
            lease_epoch: None,
            authenticated: false,
            keyboard_build_eligible: false,
            permissions_eligible: false,
            hook_healthy: false,
            rollback_latched: false,
        }
    }

    #[must_use]
    pub const fn capture_available(&self) -> bool {
        self.authenticated
            && self.lease_epoch.is_some()
            && self.keyboard_build_eligible
            && self.permissions_eligible
            && self.hook_healthy
            && !self.rollback_latched
            && matches!(
                self.state,
                KeyboardOwnerState::LeasedDisabled | KeyboardOwnerState::LeasedEnabled
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownOwnerDisposition {
    Neutral,
    Draining,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformShutdown {
    pub terminal_reason: Option<TerminalReason>,
    pub observability_quiescent: bool,
}

impl PlatformShutdown {
    pub const fn quiescent(terminal_reason: Option<TerminalReason>) -> Self {
        Self {
            terminal_reason,
            observability_quiescent: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerAuthFailureCounters {
    pub cross_user: u64,
    pub wrong_session: u64,
    pub code_identity: u64,
    pub mac: u64,
    pub protocol: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerObservabilitySnapshot {
    pub starts: u64,
    pub clean_exits: u64,
    pub abnormal_exits: u64,
    pub singleton_collisions: u64,
    pub auth_attempts: u64,
    pub auth_failures: OwnerAuthFailureCounters,
    pub lease_acquired: u64,
    pub lease_renewed: u64,
    pub lease_expired: u64,
    pub lease_disconnected: u64,
    pub lease_released_neutral: u64,
    pub lease_released_draining: u64,
    pub drain_duration_ms_total: u64,
    pub drain_duration_ms_max: u64,
    pub maintenance_postponed: u64,
    pub handoff_succeeded: u64,
    pub handoff_failed: u64,
    pub degraded: u64,
    pub hook_recoveries: u64,
}

pub trait GatewayBackend: Sized {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError>;
    fn hook_status(&self) -> HookStatus;
    fn keyboard_owner(&self) -> KeyboardOwnerSnapshot {
        KeyboardOwnerSnapshot::unavailable()
    }
    fn keyboard_capture_available(&self) -> bool {
        false
    }
    fn protocol_initialized(&self) {}
    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError>;
    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError>;
    fn inject_paste(&self) -> PasteResult;
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
    fn prepare_maintenance(&self, _request: MaintenanceRequest) -> Result<[u8; 32], PlatformError> {
        Err(PlatformError::OwnerUnavailable)
    }
    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        TransactionObservabilitySnapshot::default()
    }
    fn owner_observability(&self) -> OwnerObservabilitySnapshot {
        OwnerObservabilitySnapshot::default()
    }
    fn runtime_owner_observability(&self) -> RuntimeOwnerObservabilitySnapshot {
        RuntimeOwnerObservabilitySnapshot {
            native: self.transaction_observability(),
            owner: self.owner_observability(),
        }
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(&self) -> Option<AcceptanceEndpointObservability> {
        None
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_pause_lease_renewal(
        &self,
    ) -> Result<AcceptancePauseLeaseRenewalResult, PlatformError> {
        Err(PlatformError::OwnerUnavailable)
    }
    fn shutdown(&mut self) -> PlatformShutdown;
    fn shutdown_owner_disposition(&self) -> ShutdownOwnerDisposition {
        ShutdownOwnerDisposition::Neutral
    }
}
