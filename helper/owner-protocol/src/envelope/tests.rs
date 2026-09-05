//! Envelope integrity and receive-state regression tests.
use super::{
    AuthenticatedEnvelope, CorrelationTracker, Direction, EnvelopeError, EnvelopeKind,
    EnvelopeReceiver, FrameKey, PredecessorRouteValidator, SequenceError, SequenceValidator,
    frame_mac,
};
use crate::scalar::{Bytes32, FeatureBits};
use crate::schema::{
    Authenticated, AuthorityCeiling, Empty, ErrorBody, ErrorCode, Method, Purpose, Request,
    Response, SelectedProtocol, parse_predecessor_terminal_json,
};

#[test]
fn sequence_accepts_maximum_once_and_never_wraps() {
    let mut sequence = SequenceValidator::from_high_water(Some(u64::MAX - 1));
    sequence.accept(u64::MAX).expect("maximum accepted once");
    assert_eq!(sequence.next(), None);
    assert_eq!(sequence.accept(u64::MAX), Err(SequenceError::Wrapped));
    assert_eq!(sequence.accept(1), Err(SequenceError::Wrapped));
}

#[test]
fn authenticated_receiver_rejects_every_byte_mutation() {
    let session = Bytes32::new([3; 32]);
    let key = FrameKey::from_secret(Direction::GatewayToOwner, [4; 32]);
    let request = Request::HealthGet(Empty {});
    let body = AuthenticatedEnvelope::request(session, 1, &request)
        .expect("envelope")
        .encode_body(&key)
        .expect("body");
    for index in 0..body.len() {
        let mut changed = body.clone();
        changed[index] ^= 1;
        let mut receiver =
            EnvelopeReceiver::new(Direction::GatewayToOwner, session, &key, Purpose::Capture)
                .expect("receiver");
        assert!(receiver.accept_request(&changed).is_err(), "byte {index}");
    }
}

#[test]
fn valid_mac_structural_and_schema_faults_are_rejected() {
    let session = Bytes32::new([5; 32]);
    let key = FrameKey::from_secret(Direction::GatewayToOwner, [6; 32]);
    let body = AuthenticatedEnvelope::request(session, 1, &Request::HealthGet(Empty {}))
        .expect("envelope")
        .encode_body(&key)
        .expect("body");
    let resign = |mut changed: Vec<u8>| {
        let mac_offset = changed.len() - 32;
        let mac = frame_mac(
            key.as_bytes(),
            u32::try_from(changed.len()).expect("length"),
            &changed[..mac_offset],
        );
        changed[mac_offset..].copy_from_slice(&mac);
        changed
    };
    let mut corpus = Vec::new();
    let mut flags = body.clone();
    flags[7] = 1;
    corpus.push(resign(flags));
    let mut zero_sequence = body.clone();
    zero_sequence[40..48].fill(0);
    corpus.push(resign(zero_sequence));
    let mut wrong_kind = body.clone();
    wrong_kind[5] = EnvelopeKind::Event as u8;
    corpus.push(resign(wrong_kind));
    let mut request_correlation = body.clone();
    request_correlation[48..56].copy_from_slice(&1_u64.to_be_bytes());
    corpus.push(resign(request_correlation));
    let mut invalid_json = body.clone();
    let payload_length =
        u32::from_be_bytes(invalid_json[56..60].try_into().expect("length")) as usize;
    invalid_json[60..60 + payload_length].fill(b' ');
    corpus.push(resign(invalid_json));

    for malformed in corpus {
        let mut receiver =
            EnvelopeReceiver::new(Direction::GatewayToOwner, session, &key, Purpose::Capture)
                .expect("receiver");
        assert!(receiver.accept_request(&malformed).is_err());
    }
}

#[test]
fn skipped_transport_sequence_does_not_mutate_correlation_or_terminal_route() {
    let session = Bytes32::new([7; 32]);
    let owner_key = FrameKey::from_secret(Direction::OwnerToGateway, [8; 32]);
    let authenticated = Authenticated::new(
        SelectedProtocol {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            feature_bits: FeatureBits::new(1),
        },
        Purpose::Capture,
        AuthorityCeiling::Capture,
        Bytes32::new([9; 32]),
    )
    .expect("authenticated");
    let finish = AuthenticatedEnvelope::authenticated_finish(session, &authenticated)
        .expect("finish")
        .encode_body(&owner_key)
        .expect("finish body");
    let mut receiver = EnvelopeReceiver::new(
        Direction::OwnerToGateway,
        session,
        &owner_key,
        Purpose::Capture,
    )
    .expect("receiver");
    receiver
        .accept_authenticated_finish(&finish)
        .expect("finish accepted");
    let request = Request::HealthGet(Empty {});
    let response = Response::Error(ErrorBody::new(ErrorCode::Unavailable));
    let skipped = AuthenticatedEnvelope::response_for_request(session, 3, 1, &request, &response)
        .expect("response")
        .encode_body(&owner_key)
        .expect("body");
    let current = AuthenticatedEnvelope::response_for_request(session, 2, 1, &request, &response)
        .expect("response")
        .encode_body(&owner_key)
        .expect("body");
    let mut correlations = CorrelationTracker::new();
    correlations.register_request(1, &request).expect("request");
    assert!(matches!(
        receiver.accept_response(&skipped, &mut correlations),
        Err(EnvelopeError::Sequence(SequenceError::Skipped))
    ));
    assert_eq!(correlations.expected_method(1), Ok(Method::HealthGet));
    receiver
        .accept_response(&current, &mut correlations)
        .expect("current response");

    let lease_id = Bytes32::new([0; 32]);
    let terminal = parse_predecessor_terminal_json(
        br#"{"event":"lease.revoked","captureLeaseId":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","captureLeaseEpoch":"1","terminalSequence":"1","reason":"release"}"#,
    )
    .expect("terminal");
    let skipped_terminal = AuthenticatedEnvelope::predecessor_terminal_event(session, 2, &terminal)
        .expect("terminal envelope")
        .encode_body(&owner_key)
        .expect("terminal body");
    let current_terminal = AuthenticatedEnvelope::predecessor_terminal_event(session, 1, &terminal)
        .expect("terminal envelope")
        .encode_body(&owner_key)
        .expect("terminal body");
    let mut terminal_receiver = EnvelopeReceiver::new(
        Direction::OwnerToGateway,
        session,
        &owner_key,
        Purpose::Capture,
    )
    .expect("receiver");
    let mut route = PredecessorRouteValidator::new(lease_id, 1);
    assert!(matches!(
        terminal_receiver.accept_predecessor_terminal_event(&skipped_terminal, &mut route),
        Err(EnvelopeError::Sequence(SequenceError::Skipped))
    ));
    terminal_receiver
        .accept_predecessor_terminal_event(&current_terminal, &mut route)
        .expect("route remained unchanged");
}
