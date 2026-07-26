//! Native platform boundary.
//!
//! All OS FFI and callback entry points are confined to the target modules.
//! Shared protocol and reducer code contains no unsafe code.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use crossbeam_channel::Sender;
use serde::Serialize;
use thiserror::Error;

use crate::{
    keyboard::{ActivationBindings, EventPhase, HelperEvent},
    protocol::Outbound,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::NativePlatform;
#[cfg(not(any(windows, target_os = "macos")))]
pub use unsupported::NativePlatform;
#[cfg(windows)]
pub use windows::NativePlatform;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    Ready,
    PermissionRequired,
    Unavailable,
    Stopped,
}

const ACTIVATION_ENABLED_MASK: u64 = 1_u64 << 63;

pub(crate) const fn activation_config_value(enabled: bool, bindings: ActivationBindings) -> u64 {
    bindings.bits() | if enabled { ACTIVATION_ENABLED_MASK } else { 0 }
}

pub(crate) const fn activation_config_from_value(value: u64) -> (bool, ActivationBindings) {
    (
        value & ACTIVATION_ENABLED_MASK != 0,
        ActivationBindings::from_bits(value),
    )
}

pub(crate) const fn hook_status_to_u8(status: HookStatus) -> u8 {
    match status {
        HookStatus::Ready => 0,
        HookStatus::PermissionRequired => 1,
        HookStatus::Unavailable => 2,
        HookStatus::Stopped => 3,
    }
}

pub(crate) const fn hook_status_from_u8(value: u8) -> HookStatus {
    match value {
        0 => HookStatus::Ready,
        1 => HookStatus::PermissionRequired,
        3 => HookStatus::Stopped,
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

// Two fields at this escaped-content limit plus the worst valid request ID and
// response envelope remain comfortably below the 16 KiB frame limit.
pub(crate) const MAX_FRONT_APP_FIELD_ESCAPED_BYTES: usize = 7 * 1024;

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
        let character_bytes = json_escape_upper_bound(character);
        if escaped_bytes + character_bytes > MAX_FRONT_APP_FIELD_ESCAPED_BYTES {
            break;
        }
        escaped_bytes += character_bytes;
        end = index + character.len_utf8();
    }
    value.truncate(end);
    value
}

const fn json_escape_upper_bound(character: char) -> usize {
    if character <= '\u{001f}' {
        6
    } else if character == '"' || character == '\\' {
        2
    } else {
        character.len_utf8()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteFailure {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    OsRejected,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasteResult {
    pub submitted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PasteFailure>,
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

/// Shared liveness bit checked by every callback before reducer processing.
/// A writer failure, queue saturation, or shutdown clears it, making callbacks
/// immediately pass input through to the operating system.
#[derive(Debug)]
pub struct CallbackGate {
    accepting: AtomicBool,
}

impl CallbackGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            accepting: AtomicBool::new(false),
        }
    }

    pub fn open(&self) {
        self.accepting.store(true, Ordering::Release);
    }

    pub fn close(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.accepting.load(Ordering::Acquire)
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
    outbound: &Sender<Outbound>,
    terminal: &TerminalSignal,
    event: HelperEvent,
) -> bool {
    let message = Outbound::Event(event);
    let outcome = callback_delivery_outcome(true, outbound.try_send(message).is_ok());
    match outcome {
        CallbackDeliveryOutcome::Delivered => true,
        CallbackDeliveryOutcome::Failed(reason) => {
            terminal.trigger(reason);
            false
        }
    }
}

/// Activation acceptance and session-key capture are one native transaction: capture becomes
/// visible before the activation notification is queued, and failed queueing rolls it back.
pub(crate) fn deliver_callback_event_with_session_arm(
    outbound: &Sender<Outbound>,
    terminal: &TerminalSignal,
    session_capture: &AtomicBool,
    event: HelperEvent,
) -> bool {
    let arms_session = matches!(
        event,
        HelperEvent::Activation {
            phase: EventPhase::Down,
            ..
        }
    );
    if arms_session {
        session_capture.store(true, Ordering::Release);
    }
    let delivered = deliver_callback_event(outbound, terminal, event);
    if arms_session && !delivered {
        session_capture.store(false, Ordering::Release);
    }
    delivered
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

#[cfg(any(target_os = "macos", test))]
pub(crate) const fn secure_input_paste_result(active: bool) -> Option<PasteResult> {
    if active {
        Some(PasteResult {
            submitted: false,
            reason: Some(PasteFailure::SecureInput),
        })
    } else {
        None
    }
}

pub trait Platform: Sized {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError>;
    fn hook_status(&self) -> HookStatus;
    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError>;
    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError>;
    fn inject_paste(&self) -> PasteResult;
    fn front_app(&self) -> Result<FrontApp, PlatformError>;
    fn permissions(&self) -> Permissions;
    fn shutdown(&mut self) -> Option<TerminalReason>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::ActivationKey;

    fn escaped_content_bytes(value: &str) -> usize {
        serde_json::to_string(value).unwrap().len() - 2
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
    fn accepted_activation_arms_session_capture_and_failed_delivery_rolls_back() {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_sender, _terminal_receiver) = crossbeam_channel::bounded(1);
        let terminal = TerminalSignal::new(gate, terminal_sender);
        let capture = AtomicBool::new(false);
        let activation = HelperEvent::Activation {
            key: ActivationKey::Z,
            phase: EventPhase::Down,
            shift: false,
        };
        let (sender, receiver) = crossbeam_channel::bounded(1);

        assert!(deliver_callback_event_with_session_arm(
            &sender, &terminal, &capture, activation,
        ));
        assert!(capture.load(Ordering::Acquire));
        assert!(matches!(receiver.recv().unwrap(), Outbound::Event(_)));

        capture.store(false, Ordering::Release);
        sender.try_send(Outbound::Event(activation)).unwrap();
        let failure_gate = Arc::new(CallbackGate::new());
        failure_gate.open();
        let (failure_sender, _failure_receiver) = crossbeam_channel::bounded(1);
        let failure_terminal = TerminalSignal::new(failure_gate, failure_sender);
        assert!(!deliver_callback_event_with_session_arm(
            &sender,
            &failure_terminal,
            &capture,
            activation,
        ));
        assert!(!capture.load(Ordering::Acquire));
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
            HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: crate::keyboard::EventPhase::Down,
                shift: false,
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
        use crate::keyboard::{KeyInput, KeyPhase, KeyboardReducer, PhysicalKey};

        let key = PhysicalKey::Letter(ActivationKey::A);
        let mut reducer = KeyboardReducer::default();
        let down = reducer.plan(
            KeyInput {
                key,
                phase: KeyPhase::Down,
                alt: true,
                shift: false,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            false,
        );
        assert!(reducer.apply(down, true));

        let up = reducer.plan(
            KeyInput {
                key,
                phase: KeyPhase::Up,
                alt: false,
                shift: false,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            false,
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

    #[test]
    fn secure_input_maps_to_stable_paste_failure() {
        assert_eq!(secure_input_paste_result(false), None);
        assert_eq!(
            secure_input_paste_result(true),
            Some(PasteResult {
                submitted: false,
                reason: Some(PasteFailure::SecureInput),
            })
        );
    }

    #[test]
    fn native_front_app_fields_are_utf8_safe_and_escape_bounded() {
        let control = bound_json_string("\u{0001}".repeat(10_000));
        assert!(escaped_content_bytes(&control) <= MAX_FRONT_APP_FIELD_ESCAPED_BYTES);
        assert!(
            escaped_content_bytes(&(control.clone() + "\u{0001}"))
                > MAX_FRONT_APP_FIELD_ESCAPED_BYTES
        );

        let unicode = bound_json_string("🦀".repeat(10_000));
        assert!(unicode.is_char_boundary(unicode.len()));
        assert!(escaped_content_bytes(&unicode) <= MAX_FRONT_APP_FIELD_ESCAPED_BYTES);
        assert!(
            escaped_content_bytes(&(unicode.clone() + "🦀")) > MAX_FRONT_APP_FIELD_ESCAPED_BYTES
        );

        let escaped = bound_json_string("\"\\".repeat(10_000));
        assert!(escaped_content_bytes(&escaped) <= MAX_FRONT_APP_FIELD_ESCAPED_BYTES);
    }
}
