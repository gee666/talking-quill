use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use talking_quill_keyboard_core::SessionCaptureMode;

use super::GATEWAY_POLICY_MARKER;

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
