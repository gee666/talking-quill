//! Native platform boundary.
//!
//! All OS FFI and callback entry points are confined to the target modules.
//! Shared protocol and reducer code contains no unsafe code.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
};

use crossbeam_channel::Sender;
use serde::Serialize;
use thiserror::Error;

use crate::ActivationCaptureGate;
use talking_quill_keyboard_core::{
    ActivationBindings, ActivationContext, KeyboardEvent, SessionCaptureMode,
};

#[cfg(target_os = "macos")]
mod macos;
mod observability;
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

/// Shared liveness bit checked by every callback before reducer processing.
/// A writer failure, queue saturation, or shutdown clears it, making callbacks
/// immediately pass input through to the operating system.
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
        let acquired = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if state & CALLBACK_GATE_CLOSED != 0
                    || state & CALLBACK_GATE_LEASE_MASK == CALLBACK_GATE_LEASE_MASK
                {
                    None
                } else {
                    Some(state + 1)
                }
            });
        acquired.ok().map(|_| CallbackDeliveryLease { gate: self })
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
            _ => None,
        }
    }
}

/// Idempotent, nonblocking terminal-failure signal shared by callbacks, the
/// stdout writer, and the coordinator. Triggering closes the callback gate
/// before attempting a bounded notification, so hooks fail open even if the
/// coordinator has already gone away.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CallbackDeliveryOutcome {
    Delivered,
    Failed(TerminalReason),
}

const fn callback_delivery_outcome(encoded: bool, queued: bool) -> CallbackDeliveryOutcome {
    if !encoded {
        CallbackDeliveryOutcome::Failed(TerminalReason::OutboundEncodingUnavailable)
    } else if !queued {
        CallbackDeliveryOutcome::Failed(TerminalReason::OutboundQueueUnavailable)
    } else {
        CallbackDeliveryOutcome::Delivered
    }
}

/// Attempts one bounded callback notification. The finite, strongly typed event
/// is serialized once by the writer thread; any nonblocking queue failure closes
/// the gate before returning false. The reducer decides whether the current event
/// is an initial fail-open down or a balancing up.
pub(crate) fn deliver_callback_event(
    outbound: &Sender<NativeEvent>,
    terminal: &TerminalSignal,
    event: KeyboardEvent,
) -> bool {
    let Some(_lease) = terminal.gate.try_acquire_delivery() else {
        return false;
    };
    let message = NativeEvent::Keyboard(event);
    let outcome = callback_delivery_outcome(true, outbound.try_send(message).is_ok());
    match outcome {
        CallbackDeliveryOutcome::Delivered => true,
        CallbackDeliveryOutcome::Failed(reason) => {
            terminal.trigger(reason);
            false
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TapRecoveryPolicy {
    consecutive_timeouts: u8,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TapRecoveryEvent {
    Activity,
    TimeoutRecovered,
    TimeoutRecoveryFailed,
    DisabledByUserInput,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TapRecoveryDecision {
    Continue,
    Terminal(TerminalReason),
}

#[cfg(any(target_os = "macos", test))]
impl TapRecoveryPolicy {
    pub(crate) const fn from_consecutive_timeouts(value: u8) -> Self {
        Self {
            consecutive_timeouts: value,
        }
    }

    pub(crate) const fn consecutive_timeouts(self) -> u8 {
        self.consecutive_timeouts
    }

    pub(crate) const fn observe(self, event: TapRecoveryEvent) -> (Self, TapRecoveryDecision) {
        match event {
            TapRecoveryEvent::Activity => (
                Self::from_consecutive_timeouts(0),
                TapRecoveryDecision::Continue,
            ),
            TapRecoveryEvent::TimeoutRecovered if self.consecutive_timeouts == 0 => (
                Self::from_consecutive_timeouts(1),
                TapRecoveryDecision::Continue,
            ),
            TapRecoveryEvent::TimeoutRecovered => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapRepeatedTimeout),
            ),
            TapRecoveryEvent::TimeoutRecoveryFailed => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapTimeoutRecoveryFailed),
            ),
            TapRecoveryEvent::DisabledByUserInput => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapDisabledByUserInput),
            ),
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
mod tests {
    use super::*;
    use talking_quill_keyboard_core::{
        ActivationBinding, ActivationContext, ActivationGeneration, ActivationKey, ProfileId,
        Shortcut, ShortcutModifiers,
    };

    fn test_shortcut() -> Shortcut {
        Shortcut::new(
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            &[ActivationKey::Z],
        )
        .unwrap()
    }

    #[test]
    fn callback_delivery_policy_distinguishes_initial_fail_open_failures() {
        for (encoded, queued, expected) in [
            (
                false,
                false,
                CallbackDeliveryOutcome::Failed(TerminalReason::OutboundEncodingUnavailable),
            ),
            (
                false,
                true,
                CallbackDeliveryOutcome::Failed(TerminalReason::OutboundEncodingUnavailable),
            ),
            (
                true,
                false,
                CallbackDeliveryOutcome::Failed(TerminalReason::OutboundQueueUnavailable),
            ),
            (true, true, CallbackDeliveryOutcome::Delivered),
        ] {
            assert_eq!(callback_delivery_outcome(encoded, queued), expected);
        }
    }

    #[test]
    fn callback_delivery_lease_linearizes_against_gate_close() {
        let gate = CallbackGate::new();
        gate.open();
        let lease = gate.try_acquire_delivery().expect("open gate lease");
        gate.close();
        assert!(!gate.is_open());
        assert!(gate.try_acquire_delivery().is_none());
        drop(lease);
        assert_eq!(gate.state.load(Ordering::Acquire), CALLBACK_GATE_CLOSED);
    }

    #[test]
    fn callback_queue_failure_closes_gate_and_is_terminal() {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, terminal_rx) = crossbeam_channel::bounded(1);
        let terminal = TerminalSignal::new(Arc::clone(&gate), terminal_tx);
        let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(1);
        drop(outbound_rx);

        assert!(!deliver_callback_event(
            &outbound_tx,
            &terminal,
            KeyboardEvent::Activation {
                binding: ActivationBinding::new(ProfileId::GENERAL, test_shortcut()),
                context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
                phase: talking_quill_keyboard_core::EventPhase::Down,
            },
        ));
        assert!(!gate.is_open());
        assert_eq!(
            terminal_rx.try_recv(),
            Ok(TerminalReason::OutboundQueueUnavailable)
        );
    }

    #[test]
    fn callback_up_failure_is_terminal_but_current_captured_up_stays_swallowed() {
        use talking_quill_keyboard_core::{KeyInput, KeyPhase, KeyboardReducer, PhysicalKey};

        let key = PhysicalKey::Letter(ActivationKey::A);
        let mut reducer = KeyboardReducer::default();
        let down = reducer.plan(
            KeyInput {
                key,
                phase: KeyPhase::Down,
                modifiers: talking_quill_keyboard_core::ModifierMask::new(
                    false, true, false, false,
                ),
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            SessionCaptureMode::Off,
        );
        assert!(reducer.apply(down, true));

        let up = reducer.plan(
            KeyInput {
                key,
                phase: KeyPhase::Up,
                modifiers: talking_quill_keyboard_core::ModifierMask::default(),
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            SessionCaptureMode::Off,
        );
        let event = up.event().unwrap();

        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, terminal_rx) = crossbeam_channel::bounded(1);
        let terminal = TerminalSignal::new(Arc::clone(&gate), terminal_tx);
        let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(1);
        drop(outbound_rx);
        let delivered = deliver_callback_event(&outbound_tx, &terminal, event);

        assert!(!delivered);
        assert!(!gate.is_open());
        assert_eq!(
            terminal_rx.try_recv(),
            Ok(TerminalReason::OutboundQueueUnavailable)
        );
        assert!(reducer.apply(up, delivered));
    }

    #[test]
    fn tap_recovery_policy_covers_timeout_and_user_disable_paths() {
        let initial = TapRecoveryPolicy::default();
        let (after_first, decision) = initial.observe(TapRecoveryEvent::TimeoutRecovered);
        assert_eq!(decision, TapRecoveryDecision::Continue);
        assert_eq!(after_first.consecutive_timeouts(), 1);

        assert_eq!(
            initial.observe(TapRecoveryEvent::TimeoutRecoveryFailed).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapTimeoutRecoveryFailed)
        );
        assert_eq!(
            after_first.observe(TapRecoveryEvent::TimeoutRecovered).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapRepeatedTimeout)
        );
        assert_eq!(
            initial.observe(TapRecoveryEvent::DisabledByUserInput).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapDisabledByUserInput)
        );

        let (reset, decision) = after_first.observe(TapRecoveryEvent::Activity);
        assert_eq!(decision, TapRecoveryDecision::Continue);
        assert_eq!(reset, TapRecoveryPolicy::default());
        assert_eq!(
            reset.observe(TapRecoveryEvent::TimeoutRecovered).1,
            TapRecoveryDecision::Continue
        );
    }

    #[test]
    fn new_callback_terminal_reasons_round_trip_through_signal() {
        for reason in [
            TerminalReason::OutboundEncodingUnavailable,
            TerminalReason::EventTapTimeoutRecoveryFailed,
            TerminalReason::EventTapRepeatedTimeout,
            TerminalReason::EventTapDisabledByUserInput,
            TerminalReason::ActivationConfigurationUnavailable,
            TerminalReason::OwnerThreadUnresponsive,
            TerminalReason::AudioDeviceMonitorUnavailable,
            TerminalReason::InputInjectionUnavailable,
        ] {
            let gate = Arc::new(CallbackGate::new());
            let (sender, _receiver) = crossbeam_channel::bounded(1);
            let terminal = TerminalSignal::new(gate, sender);
            terminal.trigger(reason);
            assert_eq!(terminal.reason(), Some(reason));
        }
    }

    #[test]
    fn native_input_permission_policy_fails_closed_on_denied_or_unknown_states() {
        let granted = Permissions {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            event_post: PermissionState::Granted,
        };
        assert!(permissions_allow_native_input(granted));
        assert!(permissions_allow_native_input(Permissions {
            accessibility: PermissionState::NotApplicable,
            input_monitoring: PermissionState::NotApplicable,
            event_post: PermissionState::NotApplicable,
        }));
        for denied in [PermissionState::Denied, PermissionState::Unknown] {
            assert!(!permissions_allow_native_input(Permissions {
                accessibility: denied,
                ..granted
            }));
            assert!(!permissions_allow_native_input(Permissions {
                input_monitoring: denied,
                ..granted
            }));
            assert!(!permissions_allow_native_input(Permissions {
                event_post: denied,
                ..granted
            }));
        }
    }
}
