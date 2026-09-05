use super::{client::OwnerEventDisposition, conversions::core_binding};
use crate::{gateway::CallbackGate, protocol::Outbound};
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicU64, Ordering};
use talking_quill_keyboard_core::{
    ActivationContext, ActivationGeneration, EventPhase, KeyboardEvent, NativeTargetToken,
    SessionKey,
};
use talking_quill_owner_protocol::{GatewayMessage, schema as wire};

#[derive(Debug, Default)]
pub(super) struct GatewayEventCounters {
    pub(super) received: AtomicU64,
    pub(super) v10_accepted: AtomicU64,
}

pub(super) fn map_event(
    message: GatewayMessage,
    outbound: &Sender<Outbound>,
    admission_gate: &CallbackGate,
    counters: &GatewayEventCounters,
) -> OwnerEventDisposition {
    match message {
        GatewayMessage::Event(wire::Event::Activation(event)) => {
            saturating_increment(&counters.received);
            let Some(binding) = core_binding(&event) else {
                return OwnerEventDisposition::Terminal;
            };
            let Some(generation) = ActivationGeneration::new(event.activation_generation.get())
            else {
                return OwnerEventDisposition::Terminal;
            };
            let mut context = ActivationContext::target_unavailable(generation);
            if let Some(token) = event.target_token.as_ref() {
                let Ok(token) = NativeTargetToken::new(token.as_str()) else {
                    return OwnerEventDisposition::Terminal;
                };
                context = context.with_target_token(token);
            }
            let value = if let Some(held_ms) = event.held_ms {
                KeyboardEvent::ActivationComplete {
                    binding,
                    context,
                    held_ms: held_ms.get(),
                }
            } else {
                KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: map_phase(event.phase),
                }
            };
            if !publish_owner_event(admission_gate, outbound, Outbound::Event(value)) {
                return OwnerEventDisposition::Terminal;
            }
            saturating_increment(&counters.v10_accepted);
        }
        GatewayMessage::Event(wire::Event::RegisteredObservation(event)) => {
            saturating_increment(&counters.received);
            if !publish_owner_event(
                admission_gate,
                outbound,
                Outbound::RegisteredObservation(event.generation.get()),
            ) {
                return OwnerEventDisposition::Terminal;
            }
            saturating_increment(&counters.v10_accepted);
        }
        GatewayMessage::Event(wire::Event::SessionKey(event)) => {
            let key = match event.key {
                wire::SessionKey::Escape => SessionKey::Escape,
                wire::SessionKey::Enter => SessionKey::Enter,
            };
            if !publish_owner_event(
                admission_gate,
                outbound,
                Outbound::Event(KeyboardEvent::SessionKey {
                    key,
                    phase: map_phase(event.phase),
                }),
            ) {
                return OwnerEventDisposition::Terminal;
            }
        }
        GatewayMessage::Event(wire::Event::AudioDevicesChanged(_)) => {
            if !publish_owner_event(admission_gate, outbound, Outbound::InputDevicesChanged) {
                return OwnerEventDisposition::Terminal;
            }
        }
        GatewayMessage::Event(wire::Event::HealthChanged(_)) => {}
        GatewayMessage::Event(wire::Event::TerminalDegraded(_)) => {
            return OwnerEventDisposition::Terminal;
        }
        GatewayMessage::PredecessorTerminal(wire::PredecessorTerminalEvent::LeaseUnavailable {
            ..
        }) => return OwnerEventDisposition::Terminal,
        GatewayMessage::PredecessorTerminal(_) => {}
        GatewayMessage::Event(wire::Event::PasteCommitted(_)) | GatewayMessage::Response { .. } => {
            return OwnerEventDisposition::Terminal;
        }
    }
    OwnerEventDisposition::Continue
}

fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1).min(9_007_199_254_740_991))
    });
}

fn publish_owner_event(
    admission_gate: &CallbackGate,
    outbound: &Sender<Outbound>,
    event: Outbound,
) -> bool {
    let Some(_delivery) = admission_gate.try_acquire_delivery() else {
        return false;
    };
    outbound.try_send(event).is_ok()
}

fn map_phase(value: wire::Phase) -> EventPhase {
    match value {
        wire::Phase::Down => EventPhase::Down,
        wire::Phase::Up => EventPhase::Up,
    }
}
