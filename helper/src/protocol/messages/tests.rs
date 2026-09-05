use super::*;
use crate::{framing::MAX_FRAME_BYTES, gateway::FrontApp};
use talking_quill_keyboard_core::{ActivationBinding, ActivationContext, EventPhase, SessionKey};
use talking_quill_keyboard_core::{ActivationGeneration, ActivationKey, ProfileId, Shortcut};

fn string_response(length: usize) -> Outbound {
    Outbound::Response(RpcResponse::success(RequestId::for_test(1), "a".repeat(length)).unwrap())
}

#[test]
fn outbound_encoding_accepts_exact_max_and_rejects_max_plus_one() {
    let overhead = encode_outbound(&string_response(0)).unwrap().len();
    let exact = encode_outbound(&string_response(MAX_FRAME_BYTES - overhead)).unwrap();
    assert_eq!(exact.len(), MAX_FRAME_BYTES);
    assert!(matches!(
        encode_outbound(&string_response(MAX_FRAME_BYTES - overhead + 1)),
        Err(OutboundEncodingError::FrameTooLarge(size)) if size == MAX_FRAME_BYTES + 1
    ));
}

#[test]
fn worst_case_front_app_escaping_stays_inside_one_frame() {
    let front_app = FrontApp {
        process_name: "\u{0001}".repeat(10_000),
        window_title: "\u{0001}".repeat(10_000),
        window_bounds: None,
    }
    .bounded();
    let outbound = Outbound::Response(
        RpcResponse::success(RequestId::String("\u{0001}".repeat(64)), front_app).unwrap(),
    );
    let payload = encode_outbound(&outbound).unwrap();
    assert!(payload.len() <= MAX_FRAME_BYTES);
}

#[test]
fn input_device_change_notification_contains_no_endpoint_identifier() {
    let payload = encode_outbound(&Outbound::InputDevicesChanged).unwrap();
    let notification: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(
        notification,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "audio.input_devices_changed",
            "params": {},
        })
    );
}

#[test]
fn every_keyboard_notification_is_frame_bounded() {
    for event in [
        KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::GENERAL,
                Shortcut::legacy_alt_letter(ActivationKey::Z, false),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
            phase: EventPhase::Down,
        },
        KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::PROMPT,
                Shortcut::legacy_alt_letter(ActivationKey::Z, true),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
            phase: EventPhase::Up,
        },
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        },
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Up,
        },
    ] {
        assert!(encode_outbound(&Outbound::Event(event)).is_ok());
    }
}
