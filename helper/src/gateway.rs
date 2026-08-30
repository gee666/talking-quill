//! Structural gateway backend and bounded protocol-facing types.
//!
//! The gateway owns framed Electron protocol v10 and an owner-protocol v1
//! client. It has no dependency on the keyboard-owner package and cannot
//! install native input facilities.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use serde::Serialize;
use talking_quill_keyboard_core::{ActivationBindings, ActivationContext, SessionCaptureMode};
use thiserror::Error;

use crate::protocol::Outbound;

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
pub struct ActivationCaptureGate {
    open: bool,
    runtime_rollback: bool,
    development_disabled: bool,
}

impl ActivationCaptureGate {
    #[must_use]
    pub const fn closed() -> Self {
        Self {
            open: false,
            runtime_rollback: false,
            development_disabled: true,
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub const fn open_for_test_harness() -> Self {
        Self {
            open: true,
            runtime_rollback: false,
            development_disabled: false,
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub const fn closed_for_test_harness(
        runtime_rollback: bool,
        development_disabled: bool,
    ) -> Self {
        Self {
            open: false,
            runtime_rollback,
            development_disabled,
        }
    }

    #[must_use]
    pub(crate) fn for_process() -> Self {
        let _ = std::hint::black_box(GATEWAY_POLICY_MARKER);
        let runtime_rollback = std::env::var_os("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
            .is_some_and(|value| value == "1");
        Self {
            // This gate authorizes only forwarding to an authenticated detached
            // owner. The gateway still has no suppression/injection code.
            open: !runtime_rollback,
            runtime_rollback,
            development_disabled: false,
        }
    }

    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    #[must_use]
    pub const fn runtime_rollback_active(self) -> bool {
        self.runtime_rollback
    }

    #[must_use]
    pub const fn development_disabled(self) -> bool {
        self.development_disabled
    }

    #[must_use]
    pub const fn filter_enabled(self, requested: bool) -> bool {
        self.open && requested
    }

    #[must_use]
    pub const fn filter_session_mode(self, requested: SessionCaptureMode) -> SessionCaptureMode {
        if self.open {
            requested
        } else {
            SessionCaptureMode::Off
        }
    }
}

impl Default for ActivationCaptureGate {
    fn default() -> Self {
        Self::closed()
    }
}

#[derive(Debug)]
pub struct CallbackGate {
    state: AtomicUsize,
}

const CALLBACK_GATE_CLOSED: usize = 1 << (usize::BITS - 1);
const CALLBACK_GATE_LEASE_MASK: usize = !CALLBACK_GATE_CLOSED;

pub(crate) struct CallbackDeliveryLease<'a> {
    gate: &'a CallbackGate,
}

impl Drop for CallbackDeliveryLease<'_> {
    fn drop(&mut self) {
        self.gate.state.fetch_sub(1, Ordering::Release);
    }
}

impl CallbackGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(CALLBACK_GATE_CLOSED),
        }
    }

    pub fn open(&self) {
        self.state.store(0, Ordering::Release);
    }

    pub fn close(&self) {
        self.state.fetch_or(CALLBACK_GATE_CLOSED, Ordering::AcqRel);
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) & CALLBACK_GATE_CLOSED == 0
    }

    pub(crate) fn try_acquire_delivery(&self) -> Option<CallbackDeliveryLease<'_>> {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if state & CALLBACK_GATE_CLOSED != 0
                    || state & CALLBACK_GATE_LEASE_MASK == CALLBACK_GATE_LEASE_MASK
                {
                    None
                } else {
                    Some(state + 1)
                }
            })
            .ok()
            .map(|_| CallbackDeliveryLease { gate: self })
    }

    pub(crate) fn wait_for_delivery_quiescence(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.state.load(Ordering::Acquire) == CALLBACK_GATE_CLOSED {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::yield_now();
        }
    }
}

impl Default for CallbackGate {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TerminalReason {
    StdoutDisconnected,
    OutboundQueueUnavailable,
    CallbackPanicked,
    ReducerPoisoned,
    HookStopped,
    OutboundEncodingUnavailable,
    EventTapTimeoutRecoveryFailed,
    EventTapRepeatedTimeout,
    EventTapDisabledByUserInput,
    ActivationConfigurationUnavailable,
    OwnerThreadUnresponsive,
    AudioDeviceMonitorUnavailable,
    InputInjectionUnavailable,
    OwnerSingletonCollision,
}

impl TerminalReason {
    const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::StdoutDisconnected),
            1 => Some(Self::OutboundQueueUnavailable),
            2 => Some(Self::CallbackPanicked),
            3 => Some(Self::ReducerPoisoned),
            4 => Some(Self::HookStopped),
            5 => Some(Self::OutboundEncodingUnavailable),
            6 => Some(Self::EventTapTimeoutRecoveryFailed),
            7 => Some(Self::EventTapRepeatedTimeout),
            8 => Some(Self::EventTapDisabledByUserInput),
            9 => Some(Self::ActivationConfigurationUnavailable),
            10 => Some(Self::OwnerThreadUnresponsive),
            11 => Some(Self::AudioDeviceMonitorUnavailable),
            12 => Some(Self::InputInjectionUnavailable),
            13 => Some(Self::OwnerSingletonCollision),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct TerminalSignal {
    gate: Arc<CallbackGate>,
    sender: Sender<TerminalReason>,
    reason: AtomicU8,
}

impl TerminalSignal {
    #[must_use]
    pub const fn new(gate: Arc<CallbackGate>, sender: Sender<TerminalReason>) -> Self {
        Self {
            gate,
            sender,
            reason: AtomicU8::new(u8::MAX),
        }
    }

    pub fn trigger(&self, reason: TerminalReason) {
        self.gate.close();
        if self
            .reason
            .compare_exchange(u8::MAX, reason as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self.sender.try_send(reason);
        }
    }

    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.reason.load(Ordering::Acquire) != u8::MAX
    }

    #[must_use]
    pub fn reason(&self) -> Option<TerminalReason> {
        TerminalReason::from_u8(self.reason.load(Ordering::Acquire))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    InstalledUnobserved,
    PhysicalObserved,
    PermissionRequired,
    Unavailable,
    Stopped,
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

impl Permissions {
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            accessibility: PermissionState::Unknown,
            input_monitoring: PermissionState::Unknown,
            event_post: PermissionState::Unknown,
        }
    }
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

const MAX_FRONT_APP_FIELD_ESCAPED_BYTES: usize = 7 * 1024;

impl FrontApp {
    pub(crate) fn bounded(self) -> Self {
        Self {
            process_name: bound_json_string(self.process_name),
            window_title: bound_json_string(self.window_title),
            window_bounds: self.window_bounds,
        }
    }
}

fn bound_json_string(mut value: String) -> String {
    let mut escaped_bytes = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let character_bytes = if character <= '\u{001f}' {
            6
        } else if character == '"' || character == '\\' {
            2
        } else {
            character.len_utf8()
        };
        if escaped_bytes + character_bytes > MAX_FRONT_APP_FIELD_ESCAPED_BYTES {
            break;
        }
        escaped_bytes += character_bytes;
        end = index + character.len_utf8();
    }
    value.truncate(end);
    value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteFailure {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    OsRejected,
    Unavailable,
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
    pub(crate) fn from_lower_hex(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = decode_lower_hex(pair[0])?;
            let low = decode_lower_hex(pair[1])?;
            bytes[index] = (high << 4) | low;
        }
        Some(Self(bytes))
    }

    #[must_use]
    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

const fn decode_lower_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PlatformError {
    #[error("keyboard owner is unavailable")]
    OwnerUnavailable,
    #[error("keyboard owner authentication failed")]
    OwnerAuthentication,
    #[error("keyboard owner is incompatible")]
    OwnerIncompatible,
    #[error("keyboard owner is busy")]
    OwnerBusy,
    #[error("keyboard owner singleton collision")]
    OwnerSingletonCollision,
    #[error("keyboard owner is draining")]
    OwnerDraining,
    #[error("keyboard owner rollback is latched")]
    OwnerRollback,
    #[error("keyboard owner reported a security fault")]
    OwnerSecurityFault,
    #[error("owner operation result is indeterminate")]
    Indeterminate,
    #[error("native operation failed")]
    NativeFailure,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionObservabilitySnapshot {
    pub transactions: TransactionCounters,
    pub replay: EffectOutcomeCounters,
    pub dummy: EffectOutcomeCounters,
    pub registered_input: RegisteredInputCounters,
    pub native_paste: NativePasteCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredInputCounters {
    pub hook_installed: u64,
    pub pump_alive: u64,
    pub hc_action_callbacks: u64,
    pub physical_callbacks: u64,
    pub physical_callbacks_filtered: u64,
    pub registered_candidate_callbacks: u64,
    pub registered_match_callbacks: u64,
    pub registered_release_callbacks: u64,
    pub callback_channel_accepted: u64,
    pub callback_channel_rejected: u64,
    pub adapter_dequeued: u64,
    pub owner_admitted: u64,
    pub owner_flushed: u64,
    pub owner_rejected: u64,
    pub gateway_received: u64,
    pub v10_notification_accepted: u64,
    pub electron_received: u64,
    pub observation_accepted: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOwnerObservabilitySnapshot {
    pub native: TransactionObservabilitySnapshot,
    pub owner: OwnerObservabilitySnapshot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePasteCounters {
    pub target_validation_fallbacks: u64,
    pub modifier_wait_duration_ms_total: u64,
    pub modifier_wait_duration_ms_max: u64,
    pub modifier_timeouts: u64,
    pub shutdown_ownership_deadlines: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionCounters {
    pub started: u64,
    pub committed: u64,
    pub replayed: u64,
    pub cancelled: u64,
    pub journal_high_water: u64,
    pub cancellation_reasons: CancellationReasonCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutcomeCounters {
    pub attempted: u64,
    pub succeeded: u64,
    pub partial: u64,
    pub failed: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancellationReasonCounters {
    pub invalid_continuation: u64,
    pub modifier_changed: u64,
    pub alt_gr: u64,
    pub journal_overflow: u64,
    pub configuration_replaced: u64,
    pub revision_mismatch: u64,
    pub gate_closed: u64,
    pub shutdown: u64,
    pub helper_disconnected: u64,
    pub secure_desktop: u64,
    pub timeout: u64,
    pub activation_delivery_failed: u64,
    pub neutralization_failed: u64,
    pub replay_failed: u64,
    pub effect_protocol_violation: u64,
    pub physical_state_mismatch: u64,
    pub target_changed: u64,
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
