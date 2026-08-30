use std::num::{NonZeroU64, NonZeroUsize};

use talking_quill_owner_protocol::fake_transport::{
    FakeTransportError, ReceiveResult, fake_ordered_transport_pair,
    fake_ordered_transport_pair_with_capacity,
};
use talking_quill_owner_protocol::schema::{
    AcquireState, AudioDevicesChangedEvent, Empty, Event, HealthResult, LeaseAcquireResult,
    LeaseDisposition, OwnerReportedState, Phase, PredecessorTerminalEvent, ProcessState, Purpose,
    Request, Response, RevocationReason, SessionKey, SessionKeyEvent, SuccessResult,
    TerminalDegradedEvent, TerminalDegradedReason,
};
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, GatewayMessage, U64String, encode_outer_frame,
};

fn wire(value: u64) -> U64String {
    U64String::new(NonZeroU64::new(value).unwrap())
}

#[test]
fn front_app_metadata_method_requires_exact_negotiated_feature() {
    let material = FakeAuthenticatedMaterial::new(
        Bytes32::new([41; 32]),
        Purpose::Capture,
        [42; 32],
        [43; 32],
    );
    let (mut old_gateway, _old_owner) = material.codecs().unwrap();
    assert!(matches!(
        old_gateway.encode_request(&Request::FrontAppMetadataGet(Empty {})),
        Err(talking_quill_owner_protocol::session::SessionCodecError::FeatureNotNegotiated)
    ));

    let negotiated = material.with_features(talking_quill_owner_protocol::FRONT_APP_METADATA_V1);
    let (mut gateway, mut owner) = negotiated.codecs().unwrap();
    let frame = gateway
        .encode_request(&Request::FrontAppMetadataGet(Empty {}))
        .unwrap()
        .into_frame();
    assert_eq!(
        owner.receive_request(&frame).unwrap().request(),
        &Request::FrontAppMetadataGet(Empty {})
    );
}

#[test]
fn nonzero_lease_success_round_trips_through_the_exact_response_union() {
    let response = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: Bytes32::new([7; 32]),
        capture_lease_epoch: wire(9),
        state: AcquireState::Disabled,
    }));
    let bytes = response.to_json().unwrap();
    assert_eq!(
        talking_quill_owner_protocol::schema::parse_response_json(
            talking_quill_owner_protocol::schema::Method::LeaseAcquire,
            &bytes,
        ),
        Ok(response)
    );
    let zero = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: Bytes32::new([0; 32]),
        capture_lease_epoch: wire(1),
        state: AcquireState::Disabled,
    }));
    assert!(zero.to_json().is_err());
}

#[test]
fn mixed_receiver_preserves_response_event_and_terminal_order() {
    let material =
        FakeAuthenticatedMaterial::new(Bytes32::new([1; 32]), Purpose::Capture, [2; 32], [3; 32]);
    let (mut gateway, mut owner) = material.codecs().unwrap();
    let encoded = gateway
        .encode_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    let request = owner.receive_request(&encoded.into_frame()).unwrap();
    let lease_id = Bytes32::new([4; 32]);
    let response = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: lease_id,
        capture_lease_epoch: wire(1),
        state: AcquireState::Disabled,
    }));
    let event = Event::TerminalDegraded(TerminalDegradedEvent {
        reason: TerminalDegradedReason::CallbackDelivery,
    });
    let revoked = PredecessorTerminalEvent::LeaseRevoked {
        capture_lease_id: lease_id,
        capture_lease_epoch: wire(1),
        terminal_sequence: wire(1),
        reason: RevocationReason::Maintenance,
    };
    let neutral = PredecessorTerminalEvent::LeaseNeutral {
        capture_lease_id: lease_id,
        capture_lease_epoch: wire(1),
        terminal_sequence: wire(2),
        disposition: LeaseDisposition::Neutral,
    };

    let frames = [
        owner.encode_response(&request, &response).unwrap(),
        owner.encode_event(&event).unwrap(),
        owner.encode_predecessor_terminal(&revoked).unwrap(),
        owner.encode_predecessor_terminal(&neutral).unwrap(),
    ];
    assert!(matches!(
        gateway.receive_owner_frame(&frames[0]),
        Ok(GatewayMessage::Response { response: actual, .. }) if actual == response
    ));
    assert_eq!(
        gateway.receive_owner_frame(&frames[1]).unwrap(),
        GatewayMessage::Event(event)
    );
    assert_eq!(
        gateway.receive_owner_frame(&frames[2]).unwrap(),
        GatewayMessage::PredecessorTerminal(revoked)
    );
    assert_eq!(
        gateway.receive_owner_frame(&frames[3]).unwrap(),
        GatewayMessage::PredecessorTerminal(neutral)
    );
}

#[test]
fn health_owner_instance_cannot_replace_the_authenticated_identity() {
    let authenticated_owner = Bytes32::new([41; 32]);
    let material = FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([40; 32]),
        authenticated_owner,
        Purpose::Capture,
        [42; 32],
        [43; 32],
    );
    let (mut gateway, mut owner) = material.codecs().unwrap();
    let forged = Event::HealthChanged(HealthResult {
        owner_instance_id: Bytes32::new([44; 32]),
        reported_state: OwnerReportedState::IdleNeutral,
        process_state: ProcessState::Healthy,
        rollback_latched: false,
        native_state_unknown: false,
        maintenance_sealed: false,
        keyboard_build_eligible: false,
        paste_ready: false,
        permissions_eligible: false,
        hook_healthy: false,
    });
    let frame = owner.encode_event(&forged).unwrap();
    assert!(gateway.receive_owner_frame(&frame).is_err());
}

#[test]
fn capture_events_require_a_current_lease_and_stop_after_revocation() {
    let material = FakeAuthenticatedMaterial::new(
        Bytes32::new([31; 32]),
        Purpose::Capture,
        [32; 32],
        [33; 32],
    );
    let (mut gateway, mut owner) = material.codecs().unwrap();
    let premature = Event::AudioDevicesChanged(AudioDevicesChangedEvent {
        capture_lease_epoch: wire(1),
    });
    let frame = owner.encode_event(&premature).unwrap();
    assert!(gateway.receive_owner_frame(&frame).is_err());

    let material = FakeAuthenticatedMaterial::new(
        Bytes32::new([34; 32]),
        Purpose::Capture,
        [35; 32],
        [36; 32],
    );
    let (mut gateway, mut owner) = material.codecs().unwrap();
    let request = gateway
        .encode_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    let request = owner.receive_request(&request.into_frame()).unwrap();
    let lease_id = Bytes32::new([37; 32]);
    let response = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: lease_id,
        capture_lease_epoch: wire(1),
        state: AcquireState::Disabled,
    }));
    let frame = owner.encode_response(&request, &response).unwrap();
    gateway.receive_owner_frame(&frame).unwrap();
    let revoked = PredecessorTerminalEvent::LeaseRevoked {
        capture_lease_id: lease_id,
        capture_lease_epoch: wire(1),
        terminal_sequence: wire(1),
        reason: RevocationReason::Release,
    };
    let frame = owner.encode_predecessor_terminal(&revoked).unwrap();
    gateway.receive_owner_frame(&frame).unwrap();
    let after_revocation = Event::TerminalDegraded(TerminalDegradedEvent {
        reason: TerminalDegradedReason::Protocol,
    });
    let frame = owner.encode_event(&after_revocation).unwrap();
    assert!(gateway.receive_owner_frame(&frame).is_err());
}

#[test]
fn capture_events_are_rejected_on_maintenance_sessions_before_sequence_advances() {
    let material = FakeAuthenticatedMaterial::new(
        Bytes32::new([21; 32]),
        Purpose::Maintenance,
        [22; 32],
        [23; 32],
    );
    let (mut gateway, mut owner) = material.codecs().unwrap();
    let forbidden = Event::SessionKey(SessionKeyEvent {
        capture_lease_epoch: wire(1),
        key: SessionKey::Escape,
        phase: Phase::Down,
    });
    let frame = owner.encode_event(&forbidden).unwrap();
    assert!(gateway.receive_owner_frame(&frame).is_err());

    // The rejected frame did not advance the receiver. A valid health-class
    // event encoded by a fresh owner codec at that same sequence is accepted.
    let (_, mut retry_owner) = material.codecs().unwrap();
    let allowed = Event::TerminalDegraded(TerminalDegradedEvent {
        reason: TerminalDegradedReason::Protocol,
    });
    let frame = retry_owner.encode_event(&allowed).unwrap();
    assert_eq!(
        gateway.receive_owner_frame(&frame).unwrap(),
        GatewayMessage::Event(allowed)
    );
}

#[test]
fn enqueue_is_not_flush_confirmation_and_receipts_are_connection_bound() {
    let (mut first_writer, mut first_reader) = fake_ordered_transport_pair();
    let (mut second_writer, _) = fake_ordered_transport_pair();
    let receipt = first_writer
        .try_send(encode_outer_frame(b"one").unwrap())
        .unwrap();
    assert!(!first_writer.is_flushed(receipt));
    assert_eq!(
        second_writer.confirm_flushed(receipt),
        Err(FakeTransportError::WrongReceipt)
    );
    first_writer.confirm_flushed(receipt).unwrap();
    assert!(first_writer.is_flushed(receipt));
    assert!(matches!(
        first_reader.try_receive(),
        ReceiveResult::Frame(_)
    ));
}

#[test]
fn bounded_queue_rejects_before_mutation_and_abort_discards_queued_frames() {
    let (mut writer, mut reader) =
        fake_ordered_transport_pair_with_capacity(NonZeroUsize::new(1).unwrap()).unwrap();
    let first = encode_outer_frame(b"first").unwrap();
    let second = encode_outer_frame(b"second").unwrap();
    writer.try_send(first).unwrap();
    assert_eq!(writer.queued_outbound(), 1);
    assert_eq!(writer.try_send(second), Err(FakeTransportError::QueueFull));
    assert_eq!(writer.queued_outbound(), 1);
    writer.abort();
    assert_eq!(reader.try_receive(), ReceiveResult::PeerClosed);
    assert_eq!(reader.try_receive(), ReceiveResult::Empty);
}
