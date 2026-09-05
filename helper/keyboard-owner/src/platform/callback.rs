//! Callback admission and terminal failure delivery.
use super::*;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

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
    if outbound.try_send(message).is_ok() {
        true
    } else {
        terminal.trigger(TerminalReason::OutboundQueueUnavailable);
        false
    }
}

#[cfg(test)]
mod tests;
