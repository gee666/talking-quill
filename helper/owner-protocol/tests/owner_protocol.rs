use std::io::Cursor;
use std::num::NonZeroU64;
use std::process::Command;

use talking_quill_owner_protocol::envelope::{
    CorrelationError, MAX_OUTSTANDING_REQUESTS, SequenceError,
};
use talking_quill_owner_protocol::framing::{FramingError, MAX_BODY_LENGTH};
use talking_quill_owner_protocol::release_policy::{OwnerMode, PolicySignature, ReleasePolicy};
use talking_quill_owner_protocol::scalar::ScalarError;
use talking_quill_owner_protocol::schema::{
    Architecture, AuthorityCeiling, Binding, BindingShortcut, Bindings, CaptureCommandParams,
    Challenge, Empty, ErrorBody, ErrorCode, Event, HandshakeMessage, Hello, Letter, Method,
    Modifiers, ObservabilityResult, Platform, ProfileId, ProtocolHeader, Purpose,
    RegisteredInputCounters, RegisteredObservationEvent, ReplaceConfigurationParams, Request,
    Response, SchemaError, SelectedProtocol, SessionMode, SessionModeResult, SessionSetModeParams,
    SuccessResult, parse_event_json, parse_handshake_json, parse_predecessor_terminal_json,
    parse_request_json, parse_response_json,
};
use talking_quill_owner_protocol::{
    AuthenticatedEnvelope, AuthenticationError, Bytes32, CapabilityKind,
    CapabilitySequenceValidator, CorrelationTracker, Direction, EnvelopeKind, FeatureBits,
    FrameKey, HandshakeTrustVerifier, P256PublicKey, PredecessorRouteValidator, SequenceValidator,
    TranscriptInput, U64String, decode_outer_frame, encode_outer_frame, read_outer_frame,
};

const ZERO_BYTES32: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn independent_node_fixture_validation_is_an_automated_gate() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("repository root");
    let status = Command::new("node")
        .arg(repository.join(
            "tests/fixtures/compatibility/keyboard-owner-v1/validate-owner-protocol-vectors.mjs",
        ))
        .current_dir(repository)
        .status()
        .expect("Node is required by the repository validation toolchain");
    assert!(status.success());
}

#[test]
fn front_app_metadata_extension_is_strict_and_token_only_predecessor_is_readable() {
    let metadata = br#"{"ok":true,"result":{"available":true,"processName":"editor","windowTitle":"Document","windowBounds":{"x":-1,"y":2,"width":800,"height":600}}}"#;
    assert!(parse_response_json(Method::FrontAppGet, metadata).is_err());
    assert!(matches!(
        parse_response_json(Method::FrontAppMetadataGet, metadata).unwrap(),
        Response::Success(SuccessResult::FrontAppMetadata(_))
    ));
    let predecessor = br#"{"ok":true,"result":{"available":true,"applicationToken":"opaque"}}"#;
    assert!(parse_response_json(Method::FrontAppGet, predecessor).is_ok());
    let unknown = br#"{"ok":true,"result":{"available":true,"processName":"editor","windowTitle":"Document","windowBounds":null,"pid":7}}"#;
    assert!(parse_response_json(Method::FrontAppMetadataGet, unknown).is_err());
    let missing_required_null =
        br#"{"ok":true,"result":{"available":true,"processName":"editor","windowTitle":"Document"}}"#;
    assert!(parse_response_json(Method::FrontAppMetadataGet, missing_required_null).is_err());
}

#[test]
fn every_paste_refusal_reason_round_trips_without_collapsing() {
    for reason in [
        "permission_denied",
        "conflicting_modifiers",
        "secure_input",
        "target_unavailable",
        "clipboard_changed",
        "native_unavailable",
        "native_rejected",
    ] {
        let response = format!(
            "{{\"ok\":true,\"result\":{{\"state\":\"clipboard_only\",\"reason\":\"{reason}\"}}}}"
        );
        assert!(matches!(
            parse_response_json(Method::PasteInject, response.as_bytes()).unwrap(),
            Response::Success(SuccessResult::Paste(_))
        ));
    }
    let unknown = br#"{"ok":true,"result":{"state":"clipboard_only","reason":"unavailable"}}"#;
    assert!(parse_response_json(Method::PasteInject, unknown).is_err());
}

#[test]
fn outer_framing_enforces_every_boundary() {
    for length in [1, MAX_BODY_LENGTH] {
        let body = vec![0x5a; length];
        let encoded = encode_outer_frame(&body).expect("valid boundary");
        assert_eq!(decode_outer_frame(&encoded).expect("decode"), body);
        assert_eq!(
            read_outer_frame(&mut Cursor::new(encoded))
                .expect("read")
                .expect("frame"),
            body
        );
    }
    assert!(matches!(
        encode_outer_frame(&[]),
        Err(FramingError::InvalidLength)
    ));
    assert!(matches!(
        encode_outer_frame(&vec![0; MAX_BODY_LENGTH + 1]),
        Err(FramingError::InvalidLength)
    ));
    assert!(matches!(
        decode_outer_frame(&[0, 0, 0, 0]),
        Err(FramingError::InvalidLength)
    ));
    assert!(matches!(
        decode_outer_frame(&[0, 0, 0, 2, 1]),
        Err(FramingError::Truncated)
    ));
    assert!(matches!(
        decode_outer_frame(&[0, 0, 0, 1, 1, 2]),
        Err(FramingError::TrailingBytes)
    ));
    assert!(matches!(
        read_outer_frame(&mut Cursor::new([0, 0, 0])),
        Err(FramingError::Truncated)
    ));
    assert!(
        read_outer_frame(&mut Cursor::new([]))
            .expect("clean EOF")
            .is_none()
    );
}

#[test]
fn typed_authenticated_finish_is_the_only_zero_correlation_response() {
    let authenticated = talking_quill_owner_protocol::Authenticated::new(
        talking_quill_owner_protocol::schema::SelectedProtocol {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            feature_bits: FeatureBits::new(1),
        },
        Purpose::Capture,
        talking_quill_owner_protocol::schema::AuthorityCeiling::Capture,
        Bytes32::new([0; 32]),
    )
    .expect("authenticated payload");
    let session = Bytes32::new([1; 32]);
    let key = FrameKey::from_secret(Direction::OwnerToGateway, [2; 32]);
    let finish =
        AuthenticatedEnvelope::authenticated_finish(session, &authenticated).expect("finish");
    assert_eq!(finish.kind(), EnvelopeKind::Response);
    assert_eq!(finish.transport_sequence(), 1);
    assert_eq!(finish.correlation_sequence(), 0);
    finish.encode_body(&key).expect("finish body");
}

#[test]
fn protocol_selection_is_exact_v1_and_ignores_only_optional_unknown_bits() {
    let client = ProtocolHeader {
        major: 1,
        minor: 2,
        compatibility_epoch: 1,
        supported_feature_bits: FeatureBits::new(0b101),
        required_feature_bits: FeatureBits::new(1),
    };
    let owner = ProtocolHeader {
        major: 1,
        minor: 1,
        compatibility_epoch: 1,
        supported_feature_bits: FeatureBits::new(0b011),
        required_feature_bits: FeatureBits::new(1),
    };
    let selected = client.negotiate(owner).expect("v1 negotiation");
    assert_eq!(selected.minor, 1);
    assert_eq!(selected.feature_bits, FeatureBits::new(1));
    assert!(ProtocolHeader { major: 2, ..client }.validate().is_err());
    assert!(
        ProtocolHeader {
            compatibility_epoch: 2,
            ..client
        }
        .validate()
        .is_err()
    );
    assert!(
        ProtocolHeader {
            supported_feature_bits: FeatureBits::new(3),
            required_feature_bits: FeatureBits::new(3),
            ..client
        }
        .validate()
        .is_err()
    );
}

#[test]
fn transport_and_capability_sequences_are_exact_and_never_wrap() {
    let mut sequence = SequenceValidator::new();
    assert_eq!(sequence.next(), Some(1));
    assert_eq!(sequence.accept(2), Err(SequenceError::Skipped));
    assert_eq!(sequence.next(), Some(1));
    sequence.accept(1).expect("first");
    assert_eq!(sequence.accept(1), Err(SequenceError::Duplicate));
    assert_eq!(sequence.next(), Some(2));
    assert_eq!(sequence.accept(0), Err(SequenceError::Wrapped));
    assert_eq!(sequence.next(), Some(2));
    sequence.accept(2).expect("second");

    let capability_id = Bytes32::new([8; 32]);
    let command = |id, command_sequence| {
        Request::LeaseRenew(CaptureCommandParams {
            capture_lease_id: id,
            capture_lease_epoch: U64String::try_from(7).expect("epoch"),
            command_sequence: U64String::try_from(command_sequence).expect("sequence"),
        })
    };
    let mut capability =
        CapabilitySequenceValidator::new(CapabilityKind::Capture, capability_id, 7)
            .expect("valid capability");
    assert!(
        CapabilitySequenceValidator::new(CapabilityKind::Capture, Bytes32::new([0; 32]), 7,)
            .is_err()
    );
    assert!(CapabilitySequenceValidator::new(CapabilityKind::Capture, capability_id, 0).is_err());
    capability
        .accept(&command(capability_id, 1))
        .expect("first command");
    assert!(capability.accept(&command(capability_id, 1)).is_err());
    assert!(
        capability
            .accept(&command(Bytes32::new([9; 32]), 2))
            .is_err()
    );
    capability
        .accept(&command(capability_id, 2))
        .expect("wrong capability did not consume sequence");
}

#[test]
fn correlations_are_bounded_unique_consumed_once_and_request_specific() {
    let health = Request::HealthGet(Empty {});
    let permissions = Request::PermissionsGet(Empty {});
    let semantic_error = Response::Error(ErrorBody::new(ErrorCode::Unavailable));
    let mut tracker = CorrelationTracker::new();
    for sequence in 1..=MAX_OUTSTANDING_REQUESTS as u64 {
        tracker
            .register_request(sequence, &health)
            .expect("within capacity");
    }
    assert_eq!(tracker.len(), MAX_OUTSTANDING_REQUESTS);
    assert_eq!(
        tracker.register_request(9, &health),
        Err(CorrelationError::Capacity)
    );
    assert_eq!(
        tracker.accept_response(4, &semantic_error),
        Ok(Method::HealthGet)
    );
    assert_eq!(
        tracker.accept_response(4, &semantic_error),
        Err(CorrelationError::Unknown)
    );
    tracker
        .register_request(9, &permissions)
        .expect("freed slot");
    assert_eq!(
        tracker.register_request(9, &health),
        Err(CorrelationError::Duplicate)
    );
    assert_eq!(
        tracker.accept_response(9, &semantic_error),
        Ok(Method::PermissionsGet)
    );
    assert_eq!(
        tracker.accept_response(0, &semantic_error),
        Err(CorrelationError::Zero)
    );

    let session_request = Request::SessionSetMode(SessionSetModeParams {
        capture_lease_id: Bytes32::new([3; 32]),
        capture_lease_epoch: U64String::try_from(1).expect("epoch"),
        command_sequence: U64String::try_from(1).expect("sequence"),
        mode: SessionMode::Recording,
    });
    tracker
        .register_request(10, &session_request)
        .expect("session correlation");
    let wrong = Response::Success(SuccessResult::SessionMode(SessionModeResult {
        mode: SessionMode::Off,
    }));
    assert_eq!(
        tracker.accept_response(10, &wrong),
        Err(CorrelationError::Mismatch)
    );
    let matching = Response::Success(SuccessResult::SessionMode(SessionModeResult {
        mode: SessionMode::Recording,
    }));
    assert_eq!(
        tracker.accept_response(10, &matching),
        Ok(Method::SessionSetMode)
    );
}

#[test]
fn strict_payload_schemas_reject_duplicates_unknowns_and_wrong_scalar_forms() {
    assert_eq!(
        parse_request_json(br#"{"method":"health.get","params":{}}"#)
            .expect("valid")
            .method(),
        Method::HealthGet
    );
    for invalid in [
        br#"{"method":"health.get","method":"health.get","params":{}}"#.as_slice(),
        br#"{"method":"health.get","params":{"extra":true}}"#,
        br#"{"method":"unknown","params":{}}"#,
        br#"[]"#,
        br#"{"method":"lease.renew","params":{"captureLeaseId":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","captureLeaseEpoch":"1","commandSequence":1}}"#,
    ] {
        assert!(parse_request_json(invalid).is_err());
    }

    let proof = ZERO_BYTES32;
    let parsed = parse_handshake_json(
        format!(r#"{{"type":"authenticate","clientProof":"{proof}"}}"#).as_bytes(),
    );
    assert!(
        matches!(parsed, Ok(HandshakeMessage::Authenticate(_))),
        "{parsed:?}"
    );
    assert!(
        parse_handshake_json(
            format!(r#"{{"type":"authenticate","clientProof":"{proof}","extra":0}}"#).as_bytes()
        )
        .is_err()
    );
    assert!(
        parse_handshake_json(
            format!(r#"{{"type":"authenticate","type":"authenticate","clientProof":"{proof}"}}"#)
                .as_bytes()
        )
        .is_err()
    );

    let fixed_error = format!(
        r#"{{"ok":false,"error":{{"code":"busy","message":"{}"}}}}"#,
        ErrorCode::Busy.message()
    );
    assert_eq!(
        parse_response_json(Method::HealthGet, fixed_error.as_bytes()),
        Ok(talking_quill_owner_protocol::schema::Response::Error(
            ErrorBody::new(ErrorCode::Busy)
        ))
    );
    assert!(
        parse_response_json(
            Method::HealthGet,
            br#"{"ok":false,"error":{"code":"busy","message":"peer 123 busy"}}"#
        )
        .is_err()
    );
    assert!(
        parse_response_json(
            Method::HealthGet,
            br#"{"ok":true,"result":{},"error":{"code":"busy","message":"owner busy"}}"#
        )
        .is_err()
    );
    assert!(
        parse_response_json(
            Method::HealthGet,
            br#"{"ok":true,"result":{},"error":null}"#
        )
        .is_err()
    );
    assert!(
        parse_response_json(
            Method::HealthGet,
            br#"{"ok":false,"result":null,"error":{"code":"busy","message":"owner busy"}}"#
        )
        .is_err()
    );
    assert!(parse_request_json(
        format!(
            r#"{{"method":"paste.inject","params":{{"captureLeaseId":"{ZERO_BYTES32}","captureLeaseEpoch":"1","commandSequence":"1","operationId":"{ZERO_BYTES32}","ownerInstanceId":"{ZERO_BYTES32}","activationGeneration":"1","fallbackTextSha256":"{ZERO_BYTES32}"}}}}"#
        )
        .as_bytes()
    )
    .is_err());
    assert!(
        parse_response_json(
            Method::FrontAppGet,
            br#"{"ok":true,"result":{"available":false}}"#
        )
        .is_err()
    );
    assert!(parse_event_json(
        format!(
            r#"{{"event":"activation","params":{{"captureLeaseEpoch":"1","ownerInstanceId":"{ZERO_BYTES32}","profileId":"general","shortcut":{{"modifiers":{{"ctrl":true,"alt":false,"shift":false,"meta":false}},"keys":["A"]}},"activationGeneration":"1","targetToken":null,"phase":"up"}}}}"#
        )
        .as_bytes()
    )
    .is_err());
}

#[test]
fn handshake_nullable_keys_are_required_even_when_null() {
    let protocol = ProtocolHeader {
        major: 1,
        minor: 0,
        compatibility_epoch: 1,
        supported_feature_bits: FeatureBits::new(1),
        required_feature_bits: FeatureBits::new(1),
    };
    let policy = ReleasePolicy {
        platform: Platform::Macos,
        architecture: Architecture::X64,
        owner_mode: OwnerMode::SafeDisabled,
        release_build_digest: Bytes32::new([1; 32]),
        gateway_sha256: Bytes32::new([2; 32]),
        owner_sha256: Bytes32::new([3; 32]),
        gateway_signer_policy_digest: Bytes32::new([4; 32]),
        owner_signer_policy_digest: Bytes32::new([5; 32]),
        gateway_protocol: protocol,
        owner_protocol: protocol,
        predecessor: None,
    };
    let hello = Hello::new(
        Purpose::Capture,
        protocol,
        Bytes32::new([6; 32]),
        Platform::Macos,
        Architecture::X64,
        policy.release_build_digest,
        policy.gateway_sha256,
        Bytes32::new([7; 32]),
        policy.gateway_signer_policy_digest,
        Bytes32::new([8; 32]),
        policy.encode().expect("policy"),
        PolicySignature::from_der(vec![0x30, 0]).expect("synthetic DER"),
        Bytes32::new([9; 32]),
        None,
    )
    .expect("hello");
    let valid = hello.to_json().expect("hello JSON");
    assert!(matches!(
        parse_handshake_json(&valid),
        Ok(HandshakeMessage::Hello(_))
    ));
    let mut missing: serde_json::Value = serde_json::from_slice(&valid).expect("value");
    missing
        .as_object_mut()
        .expect("object")
        .remove("clientEphemeralPublicKey");
    assert!(parse_handshake_json(&serde_json::to_vec(&missing).expect("JSON")).is_err());
}

#[test]
fn transcript_construction_requires_the_platform_trust_gate() {
    struct Allow;
    impl HandshakeTrustVerifier for Allow {
        fn verify(
            &self,
            _hello: &Hello,
            _challenge: &Challenge,
            _client_policy: &ReleasePolicy,
            _owner_policy: &ReleasePolicy,
        ) -> Result<(), AuthenticationError> {
            Ok(())
        }
    }
    struct Reject;
    impl HandshakeTrustVerifier for Reject {
        fn verify(
            &self,
            _hello: &Hello,
            _challenge: &Challenge,
            _client_policy: &ReleasePolicy,
            _owner_policy: &ReleasePolicy,
        ) -> Result<(), AuthenticationError> {
            Err(AuthenticationError::Trust)
        }
    }

    let protocol = ProtocolHeader {
        major: 1,
        minor: 0,
        compatibility_epoch: 1,
        supported_feature_bits: FeatureBits::new(1),
        required_feature_bits: FeatureBits::new(1),
    };
    let policy = ReleasePolicy {
        platform: Platform::Macos,
        architecture: Architecture::X64,
        owner_mode: OwnerMode::SafeDisabled,
        release_build_digest: Bytes32::new([1; 32]),
        gateway_sha256: Bytes32::new([2; 32]),
        owner_sha256: Bytes32::new([3; 32]),
        gateway_signer_policy_digest: Bytes32::new([4; 32]),
        owner_signer_policy_digest: Bytes32::new([5; 32]),
        gateway_protocol: protocol,
        owner_protocol: protocol,
        predecessor: None,
    };
    let hello = Hello::new(
        Purpose::Capture,
        protocol,
        Bytes32::new([6; 32]),
        Platform::Macos,
        Architecture::X64,
        policy.release_build_digest,
        policy.gateway_sha256,
        Bytes32::new([7; 32]),
        policy.gateway_signer_policy_digest,
        Bytes32::new([8; 32]),
        policy.encode().expect("policy"),
        PolicySignature::from_der(vec![0x30, 0]).expect("signature"),
        Bytes32::new([9; 32]),
        None,
    )
    .expect("hello");
    let challenge = Challenge::new(
        protocol,
        SelectedProtocol {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            feature_bits: FeatureBits::new(1),
        },
        Purpose::Capture,
        AuthorityCeiling::Capture,
        Bytes32::new([10; 32]),
        Bytes32::new([11; 32]),
        Bytes32::new([12; 32]),
        Platform::Macos,
        Architecture::X64,
        policy.release_build_digest,
        policy.owner_sha256,
        Bytes32::new([7; 32]),
        policy.owner_signer_policy_digest,
        Bytes32::new([8; 32]),
        policy.encode().expect("policy"),
        PolicySignature::from_der(vec![0x30, 0]).expect("signature"),
        Bytes32::new([9; 32]),
        None,
    )
    .expect("challenge");
    assert!(TranscriptInput::from_verified_handshake(&hello, &challenge, &Reject).is_err());
    assert!(TranscriptInput::from_verified_handshake(&hello, &challenge, &Allow).is_ok());
}

#[test]
fn binding_grammar_and_snapshot_bounds_are_enforced() {
    let all_letters = vec![
        Letter::A,
        Letter::B,
        Letter::C,
        Letter::D,
        Letter::E,
        Letter::F,
        Letter::G,
        Letter::H,
        Letter::I,
        Letter::J,
        Letter::K,
        Letter::L,
        Letter::M,
        Letter::N,
        Letter::O,
        Letter::P,
        Letter::Q,
        Letter::R,
        Letter::S,
        Letter::T,
        Letter::U,
        Letter::V,
        Letter::W,
        Letter::X,
        Letter::Y,
        Letter::Z,
    ];
    let shortcut: BindingShortcut = serde_json::from_value(serde_json::json!({
        "modifiers": {"ctrl": true, "alt": false, "shift": false, "meta": false},
        "keys": all_letters.iter().map(|letter| format!("{letter:?}")).collect::<Vec<_>>()
    }))
    .expect("maximum shortcut");
    assert_eq!(shortcut.keys().len(), 26);
    for keys in [
        serde_json::json!([]),
        serde_json::json!(["A", "A"]),
        serde_json::json!(["a"]),
        serde_json::json!(["A"]),
    ] {
        assert!(
            serde_json::from_value::<BindingShortcut>(serde_json::json!({
                "modifiers": if keys == serde_json::json!(["A"]) {
                    serde_json::json!({"ctrl": false, "alt": false, "shift": false, "meta": false})
                } else {
                    serde_json::json!({"ctrl": true, "alt": false, "shift": false, "meta": false})
                },
                "keys": keys
            }))
            .is_err()
        );
    }

    assert!(ProfileId::new("arbitrary-profile".into()).is_err());
    let reserved = Binding::new(
        ProfileId::new("prompt".into()).expect("profile"),
        BindingShortcut::new(Modifiers::new(false, true, false, false), vec![Letter::X])
            .expect("reserved shortcut"),
    );
    assert!(Bindings::new(vec![reserved]).is_err());

    let bindings = all_letters[..13]
        .iter()
        .enumerate()
        .map(|(index, letter)| {
            Binding::new(
                ProfileId::new(format!("00000000-0000-1000-8000-{index:012x}")).expect("profile"),
                BindingShortcut::new(Modifiers::new(true, false, false, false), vec![*letter])
                    .expect("shortcut"),
            )
        })
        .collect();
    assert_eq!(
        Bindings::new(bindings)
            .expect("13 bindings")
            .as_slice()
            .len(),
        13
    );

    let maximum_bindings = (0..13)
        .map(|index| {
            let mut keys = all_letters.clone();
            keys.rotate_left(index);
            Binding::new(
                ProfileId::new(format!("10000000-0000-1000-8000-{index:012x}")).expect("profile"),
                BindingShortcut::new(Modifiers::new(true, true, true, true), keys)
                    .expect("maximum shortcut"),
            )
        })
        .collect();
    let maximum_request = Request::CaptureReplaceConfiguration(ReplaceConfigurationParams {
        capture_lease_id: Bytes32::new([1; 32]),
        capture_lease_epoch: U64String::try_from(u64::MAX).expect("epoch"),
        command_sequence: U64String::try_from(u64::MAX).expect("sequence"),
        revision: U64String::try_from(u64::MAX).expect("revision"),
        bindings: Bindings::new(maximum_bindings).expect("maximum bindings"),
    });
    let maximum_json = maximum_request.to_json().expect("maximum request JSON");
    assert!(maximum_json.len() <= 16_292);
    assert_eq!(parse_request_json(&maximum_json), Ok(maximum_request));

    let duplicate_shortcut =
        BindingShortcut::new(Modifiers::new(true, false, false, false), vec![Letter::A])
            .expect("shortcut");
    assert!(
        Bindings::new(vec![
            Binding::new(
                ProfileId::new("general".into()).expect("profile"),
                duplicate_shortcut.clone(),
            ),
            Binding::new(
                ProfileId::new("prompt".into()).expect("profile"),
                duplicate_shortcut,
            ),
        ])
        .is_err()
    );
}

#[test]
fn request_round_trip_keeps_command_sequence_as_a_string() {
    let request = Request::LeaseRenew(CaptureCommandParams {
        capture_lease_id: Bytes32::new([3; 32]),
        capture_lease_epoch: U64String::try_from(7).expect("nonzero"),
        command_sequence: U64String::try_from(u64::MAX).expect("nonzero"),
    });
    let json = request.to_json().expect("serialize");
    let text = std::str::from_utf8(&json).expect("UTF-8");
    assert!(text.contains(r#""commandSequence":"18446744073709551615""#));
    assert_eq!(parse_request_json(&json), Ok(request));
}

#[test]
fn terminal_event_schema_rejects_nonterminal_neutral_and_unknown_fields() {
    let valid = format!(
        r#"{{"event":"lease.neutral","captureLeaseId":"{ZERO_BYTES32}","captureLeaseEpoch":"1","terminalSequence":"1","disposition":"neutral"}}"#
    );
    assert!(parse_predecessor_terminal_json(valid.as_bytes()).is_ok());
    assert!(
        parse_predecessor_terminal_json(valid.replace("neutral", "draining").as_bytes()).is_err()
    );
    assert!(
        parse_predecessor_terminal_json(valid.replace("}", ",\"extra\":true}").as_bytes()).is_err()
    );
}

#[test]
fn registered_observation_event_is_opaque_strict_and_round_trips() {
    let event = Event::RegisteredObservation(RegisteredObservationEvent {
        capture_lease_epoch: U64String::new(NonZeroU64::new(7).unwrap()),
        generation: U64String::new(NonZeroU64::new(9).unwrap()),
    });
    let encoded = event.to_json().unwrap();
    assert_eq!(parse_event_json(&encoded).unwrap(), event);
    let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    value["params"]["key"] = serde_json::json!("X");
    assert_eq!(
        parse_event_json(&serde_json::to_vec(&value).unwrap()),
        Err(SchemaError::Json)
    );
}

#[test]
fn registered_input_observability_is_negotiated_optional_and_strict() {
    let predecessor = serde_json::to_value(ObservabilityResult::default()).unwrap();
    assert!(predecessor.get("registeredInput").is_none());

    let current = ObservabilityResult {
        registered_input: Some(RegisteredInputCounters::default()),
        ..ObservabilityResult::default()
    };
    let value = serde_json::to_value(&current).unwrap();
    assert!(value.get("registeredInput").is_some());
    let mut invalid = value;
    invalid["registeredInput"]["rawKey"] = serde_json::json!("X");
    assert!(serde_json::from_value::<ObservabilityResult>(invalid).is_err());
}

#[test]
fn oversized_aggregate_snapshot_is_rejected_on_both_send_and_receive() {
    fn maximize(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(counter) if counter == "0" => {
                *counter = "9007199254740991".into();
            }
            serde_json::Value::Array(values) => values.iter_mut().for_each(maximize),
            serde_json::Value::Object(fields) => fields.values_mut().for_each(maximize),
            _ => {}
        }
    }

    let mut value = serde_json::to_value(ObservabilityResult::default()).expect("aggregate JSON");
    maximize(&mut value);
    let aggregate: ObservabilityResult = serde_json::from_value(value).expect("legal counters");
    let response = Response::Success(SuccessResult::Observability(Box::new(aggregate)));
    assert_eq!(response.to_json(), Err(SchemaError::Bounds));
}

#[test]
fn deterministic_property_and_fuzz_corpus_is_robust() {
    let mut random = XorShift64(0x6d5a_56da_1357_9bdf);
    let lengths = [1, 2, 3, 31, 32, 255, 1024, 4096, MAX_BODY_LENGTH];
    for length in lengths {
        let body = random.bytes(length);
        let encoded = encode_outer_frame(&body).expect("bounded frame");
        assert_eq!(decode_outer_frame(&encoded).expect("decode"), body);
    }

    for case in 0..512_u64 {
        let bytes: [u8; 32] = random.bytes(32).try_into().expect("32 bytes");
        let value = Bytes32::new(bytes);
        let encoded = serde_json::to_string(&value).expect("serialize");
        assert_eq!(encoded.len(), 45);
        assert!(!encoded.contains('='));
        assert_eq!(
            serde_json::from_str::<Bytes32>(&encoded).expect("deserialize"),
            value
        );

        let session: [u8; 32] = random.bytes(32).try_into().expect("session");
        let key_bytes: [u8; 32] = random.bytes(32).try_into().expect("key");
        let key = FrameKey::from_secret(Direction::GatewayToOwner, key_bytes);
        let request = Request::HealthGet(Empty {});
        let envelope = AuthenticatedEnvelope::request(Bytes32::new(session), 1, &request)
            .expect("bounded envelope");
        envelope.encode_body(&key).expect("encode");

        let fuzz_length = (random.next() as usize) % 4097;
        let fuzz = random.bytes(fuzz_length);
        let _ = parse_request_json(&fuzz);
        let _ = parse_response_json(Method::HealthGet, &fuzz);
        let _ = parse_event_json(&fuzz);
        let _ = parse_predecessor_terminal_json(&fuzz);
        let _ = parse_handshake_json(&fuzz);
        let _ = decode_outer_frame(&fuzz);
        let _ = case;
    }

    let valid = serde_json::to_string(&Bytes32::new([7; 32])).expect("serialize");
    for index in 0..43 {
        let mut encoded = valid.clone().into_bytes();
        encoded[index + 1] = b'=';
        assert!(serde_json::from_slice::<Bytes32>(&encoded).is_err());
    }

    let maximum = random.bytes(20_000);
    let _ = parse_request_json(&maximum);
    let _ = decode_outer_frame(&maximum);
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn scalar_errors_never_include_rejected_values() {
    let error = serde_json::from_str::<Bytes32>(r#""not-a-secret""#).expect_err("invalid");
    assert!(!error.to_string().contains("not-a-secret"));
    assert_eq!(
        format!("{:?}", Bytes32::new([0x41; 32])),
        "Bytes32([REDACTED])"
    );
    assert_eq!(ScalarError::Bytes32.to_string(), "invalid 32-byte value");
    let shortcut = BindingShortcut::new(
        Modifiers::new(true, false, false, false),
        vec![Letter::A, Letter::B],
    )
    .expect("shortcut");
    assert_eq!(format!("{shortcut:?}"), "BindingShortcut([REDACTED])");
    assert!(P256PublicKey::from_sec1_bytes([0; 65]).is_err());
    let mut wrong_prefix = [0; 65];
    wrong_prefix[0] = 3;
    assert!(P256PublicKey::from_sec1_bytes(wrong_prefix).is_err());
}

#[test]
fn empty_params_are_exact_objects() {
    let request = Request::HealthGet(Empty {});
    assert_eq!(
        std::str::from_utf8(&request.to_json().expect("JSON")).expect("UTF-8"),
        r#"{"method":"health.get","params":{}}"#
    );
    assert_eq!(
        parse_request_json(&request.to_json().expect("JSON")),
        Ok(request)
    );
}

#[test]
fn predecessor_terminal_route_is_exact_contiguous_and_final() {
    let event = |name: &str, sequence: u64, tail: &str| {
        parse_predecessor_terminal_json(
            format!(
                r#"{{"event":"{name}","captureLeaseId":"{ZERO_BYTES32}","captureLeaseEpoch":"7","terminalSequence":"{sequence}",{tail}}}"#
            )
            .as_bytes(),
        )
        .expect("terminal event")
    };
    let mut route = PredecessorRouteValidator::new(Bytes32::new([0; 32]), 7);
    assert!(
        route
            .accept(&event("lease.draining", 1, r#""ownership":"candidate""#))
            .is_err()
    );
    route
        .accept(&event("lease.revoked", 1, r#""reason":"release""#))
        .expect("revoked first");
    assert!(
        route
            .accept(&event("lease.draining", 3, r#""ownership":"candidate""#))
            .is_err()
    );
    route
        .accept(&event("lease.draining", 2, r#""ownership":"candidate""#))
        .expect("draining");
    route
        .accept(&event("lease.neutral", 3, r#""disposition":"neutral""#))
        .expect("final neutral");
    assert!(route.is_final());
    assert!(
        route
            .accept(&event("lease.draining", 4, r#""ownership":"candidate""#))
            .is_err()
    );
}

#[test]
fn malformed_event_corpus_is_rejected() {
    for invalid in [
        br#"{"event":"session_key","params":{"captureLeaseEpoch":"1","key":"space","phase":"down"}}"#.as_slice(),
        br#"{"event":"activation","params":{}}"#,
        br#"{"event":"health_changed","params":{"ownerInstanceId":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}}"#,
        br#"{"event":"unknown","params":{}}"#,
    ] {
        assert!(matches!(parse_event_json(invalid), Err(SchemaError::Json | SchemaError::UnknownMessage)));
    }
}
