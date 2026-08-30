#![cfg_attr(feature = "local-unsigned-owner", allow(dead_code, unused_imports))]

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use proptest::prelude::*;
use talking_quill_keyboard_core::{
    EventPhase, KeyboardEvent, OWNER_ADMITTED_EFFECT_CAPACITY, SessionKey,
};
use talking_quill_keyboard_owner::executor::{
    ExecutorCommand, ExecutorResult, OwnerExecutor, RecordingFakeExecutor,
};
use talking_quill_keyboard_owner::state::{
    AdmissionState, CandidateOwnership, ConnectionId, NativeActionFailure, NativeOwnership,
    NativeReadiness, OwnerActivationGeneration, OwnerInstanceId, PasteAuthorization,
    PasteOperationId, PasteOwnership, ProcessState, ReportedState, RequiredActionKind,
};
use talking_quill_keyboard_owner::{OwnerProtocolServer, ServerError, ServerPump};
use talking_quill_owner_protocol::client::{ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::fake_transport::{
    FakeTransportControl, ReceiveResult, fake_ordered_transport_pair,
    fake_ordered_transport_pair_with_capacity,
};
use talking_quill_owner_protocol::schema::{
    AcquireState, CaptureCommandParams, Empty, ErrorCode, MaintenanceAcquireParams,
    MaintenanceAcquireState, MaintenanceCommandParams,
    MaintenanceOperation as WireMaintenanceOperation, MaintenancePrepareParams, PasteInjectParams,
    PasteRefusalReason, PasteResult, Purpose, Request, Response, SessionMode, SessionSetModeParams,
    SuccessResult,
};
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, FlushReceipt, GatewayMessage, OrderedTransport,
    TransportError, TransportProgress, U64String, encode_outer_frame,
};

#[derive(Debug)]
struct PendingOnceTransport {
    inner: talking_quill_owner_protocol::fake_transport::FakeOrderedEndpoint,
    pending_next_flush: Arc<AtomicBool>,
    fail_next_flush: Arc<AtomicBool>,
    pending_next_close: Arc<AtomicBool>,
}

impl OrderedTransport for PendingOnceTransport {
    fn is_test_only(&self) -> bool {
        true
    }

    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        self.inner.try_send(frame)
    }

    fn flush(&mut self, receipt: FlushReceipt) -> Result<TransportProgress, TransportError> {
        if self.pending_next_flush.swap(false, Ordering::SeqCst) {
            Ok(TransportProgress::Pending)
        } else if self.fail_next_flush.swap(false, Ordering::SeqCst) {
            Err(TransportError::PeerClosed)
        } else {
            OrderedTransport::flush(&mut self.inner, receipt)
        }
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        OrderedTransport::try_receive(&mut self.inner)
    }

    fn close(&mut self) -> Result<TransportProgress, TransportError> {
        if self.pending_next_close.swap(false, Ordering::SeqCst) {
            Ok(TransportProgress::Pending)
        } else {
            OrderedTransport::close(&mut self.inner)
        }
    }

    fn abort(&mut self) {
        self.inner.abort();
    }
}

fn owner(value: u8) -> OwnerInstanceId {
    OwnerInstanceId::new([value; 32]).expect("nonzero owner")
}

fn connection(value: u64) -> ConnectionId {
    ConnectionId::new(value).expect("nonzero connection")
}

fn wire_u64(value: u64) -> U64String {
    U64String::new(NonZeroU64::new(value).expect("nonzero wire integer"))
}

fn material(value: u8, purpose: Purpose) -> FakeAuthenticatedMaterial {
    FakeAuthenticatedMaterial::new(
        Bytes32::new([value; 32]),
        purpose,
        [value.wrapping_add(1); 32],
        [value.wrapping_add(2); 32],
    )
}

fn attach<'a, E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'a, E>,
    material: &'a FakeAuthenticatedMaterial,
    id: ConnectionId,
) -> OwnerProtocolClient<'a> {
    attach_with_control(server, material, id).0
}

fn attach_with_control<'a, E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'a, E>,
    material: &'a FakeAuthenticatedMaterial,
    id: ConnectionId,
) -> (
    OwnerProtocolClient<'a>,
    talking_quill_owner_protocol::fake_transport::FakeTransportControl,
) {
    let (client_codec, server_codec) = material.codecs().expect("fake codecs");
    let (client_endpoint, server_endpoint) = fake_ordered_transport_pair();
    let control = client_endpoint.control();
    server
        .attach_connection(id, server_endpoint, server_codec)
        .expect("attach fake connection");
    (
        OwnerProtocolClient::new(client_endpoint, client_codec).unwrap(),
        control,
    )
}

fn attach_pending_once<'a, E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'a, E>,
    material: &'a FakeAuthenticatedMaterial,
    id: ConnectionId,
) -> (
    OwnerProtocolClient<'a>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
) {
    let (client_codec, server_codec) = material.codecs().expect("fake codecs");
    let (client_endpoint, server_endpoint) = fake_ordered_transport_pair();
    let pending_next_flush = Arc::new(AtomicBool::new(false));
    let fail_next_flush = Arc::new(AtomicBool::new(false));
    let pending_next_close = Arc::new(AtomicBool::new(false));
    server
        .attach_connection(
            id,
            PendingOnceTransport {
                inner: server_endpoint,
                pending_next_flush: Arc::clone(&pending_next_flush),
                fail_next_flush: Arc::clone(&fail_next_flush),
                pending_next_close: Arc::clone(&pending_next_close),
            },
            server_codec,
        )
        .expect("attach pending transport");
    (
        OwnerProtocolClient::new(client_endpoint, client_codec).unwrap(),
        pending_next_flush,
        fail_next_flush,
        pending_next_close,
    )
}

fn request_response<E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'_, E>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
    request: &Request,
) -> Response {
    let expected = client.send_request(request).expect("send request");
    assert_eq!(server.pump(id).expect("server pump"), ServerPump::Processed);
    loop {
        match client.poll().expect("client poll") {
            ClientPoll::Empty => panic!("response was not enqueued"),
            ClientPoll::PeerClosed => panic!("connection closed before response"),
            ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response,
            }) => {
                assert_eq!(correlation_sequence, expected);
                return response;
            }
            ClientPoll::Message(_) => continue,
        }
    }
}

#[derive(Clone, Copy)]
struct CaptureWire {
    id: Bytes32,
    epoch: U64String,
    next_sequence: u64,
}

impl CaptureWire {
    fn params(&mut self) -> CaptureCommandParams {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        CaptureCommandParams {
            capture_lease_id: self.id,
            capture_lease_epoch: self.epoch,
            command_sequence: wire_u64(sequence),
        }
    }
}

fn acquire_capture<E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'_, E>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
) -> CaptureWire {
    let response = request_response(server, client, id, &Request::LeaseAcquire(Empty {}));
    let Response::Success(SuccessResult::LeaseAcquire(acquired)) = response else {
        panic!("capture lease was not acquired")
    };
    assert_eq!(acquired.state, AcquireState::Disabled);
    CaptureWire {
        id: acquired.capture_lease_id,
        epoch: acquired.capture_lease_epoch,
        next_sequence: 1,
    }
}

#[derive(Clone, Copy)]
struct MaintenanceWire {
    id: Bytes32,
    epoch: U64String,
    next_sequence: u64,
}

impl MaintenanceWire {
    fn params(&mut self) -> MaintenanceCommandParams {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        MaintenanceCommandParams {
            maintenance_capability_id: self.id,
            maintenance_capability_epoch: self.epoch,
            command_sequence: wire_u64(sequence),
        }
    }
}

fn enabled_readiness() -> NativeReadiness {
    NativeReadiness {
        keyboard_build_eligible: true,
        paste_ready: true,
        permissions_eligible: true,
        hook_healthy: true,
    }
}

fn enable_empty_configuration<E: OwnerExecutor>(
    server: &mut OwnerProtocolServer<'_, E>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
    capture: &mut CaptureWire,
) {
    let off = capture.params();
    assert!(matches!(
        request_response(server, client, id, &Request::SessionReconcileOff(off)),
        Response::Success(SuccessResult::SessionMode(_))
    ));
    let params = talking_quill_owner_protocol::schema::ReplaceConfigurationParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        revision: wire_u64(1),
        bindings: talking_quill_owner_protocol::schema::Bindings::new(Vec::new())
            .expect("empty full snapshot"),
    };
    capture.next_sequence += 1;
    assert!(matches!(
        request_response(
            server,
            client,
            id,
            &Request::CaptureReplaceConfiguration(params)
        ),
        Response::Success(SuccessResult::Configuration(_))
    ));
    let params = talking_quill_owner_protocol::schema::SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: true,
    };
    capture.next_sequence += 1;
    assert!(matches!(
        request_response(server, client, id, &Request::CaptureSetEnabled(params)),
        Response::Success(SuccessResult::Enabled(_))
    ));
}

#[test]
fn fake_transport_acquires_an_initially_disabled_capture_lease() {
    let material = material(1, Purpose::Capture);
    let mut server = OwnerProtocolServer::start_fake(owner(1), RecordingFakeExecutor::default())
        .expect("start fake server");
    let id = connection(1);
    let mut client = attach(&mut server, &material, id);

    let lease = acquire_capture(&mut server, &mut client, id);
    assert_eq!(lease.epoch.get(), 1);
    assert_eq!(
        server.state().reported_state(),
        ReportedState::LeaseDisabled
    );
    assert_eq!(server.state().admission(), AdmissionState::Closed);
}

#[test]
fn response_backpressure_is_retried_without_revoking_the_lease() {
    let material = material(41, Purpose::Capture);
    let mut server = OwnerProtocolServer::start(owner(41), RecordingFakeExecutor::default())
        .expect("start server");
    let id = connection(41);
    let (mut client, pending, _, _) = attach_pending_once(&mut server, &material, id);
    pending.store(true, Ordering::SeqCst);
    let expected = client
        .send_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();

    assert_eq!(server.pump(id).unwrap(), ServerPump::Backpressured);
    assert_eq!(
        server.state().reported_state(),
        ReportedState::LeaseDisabled
    );
    assert_eq!(client.poll().unwrap(), ClientPoll::Empty);
    assert_eq!(server.pump(id).unwrap(), ServerPump::Empty);
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response {
            correlation_sequence,
            response: Response::Success(SuccessResult::LeaseAcquire(_)),
        }) if correlation_sequence == expected
    ));
}

#[test]
fn pending_flush_failure_revokes_the_capability_and_keeps_owner_alive() {
    let material = material(43, Purpose::Capture);
    let mut server = OwnerProtocolServer::start(owner(43), RecordingFakeExecutor::default())
        .expect("start server");
    let id = connection(43);
    let (mut client, pending, fail, _) = attach_pending_once(&mut server, &material, id);
    pending.store(true, Ordering::SeqCst);
    client
        .send_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Backpressured);
    fail.store(true, Ordering::SeqCst);

    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
    assert!(!server.exit_requested());
}

#[test]
fn pending_terminal_write_failure_is_retired_during_teardown() {
    let material = material(44, Purpose::Capture);
    let mut server = OwnerProtocolServer::start(owner(44), RecordingFakeExecutor::default())
        .expect("start server");
    let id = connection(44);
    let (mut client, pending, _, _) = attach_pending_once(&mut server, &material, id);
    let lease = acquire_capture(&mut server, &mut client, id);
    pending.store(true, Ordering::SeqCst);
    client
        .send_request(&Request::LeaseRelease(CaptureCommandParams {
            capture_lease_id: lease.id,
            capture_lease_epoch: lease.epoch,
            command_sequence: wire_u64(1),
        }))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Backpressured);
    client.abort();

    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
    assert!(!server.exit_requested());
}

#[test]
fn production_safe_default_refuses_enable_without_opening_admission() {
    let material = material(2, Purpose::Capture);
    let mut server = OwnerProtocolServer::start_fake(owner(2), RecordingFakeExecutor::default())
        .expect("start fake server");
    let id = connection(2);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);

    let off = capture.params();
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionReconcileOff(off),
    );
    let params = talking_quill_owner_protocol::schema::ReplaceConfigurationParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        revision: wire_u64(1),
        bindings: talking_quill_owner_protocol::schema::Bindings::new(Vec::new()).unwrap(),
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureReplaceConfiguration(params),
    );
    let params = talking_quill_owner_protocol::schema::SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: true,
    };
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureSetEnabled(params),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::Unavailable
    ));
    assert!(
        !server
            .executor()
            .actions()
            .contains(&RequiredActionKind::OpenFreshAdmission)
    );
    assert_eq!(server.state().admission(), AdmissionState::Closed);
}

#[test]
fn semantic_error_consumes_command_sequence_but_keeps_connection() {
    let material = material(3, Purpose::Capture);
    let mut server =
        OwnerProtocolServer::start_fake(owner(3), RecordingFakeExecutor::default()).unwrap();
    let id = connection(3);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let enable = talking_quill_owner_protocol::schema::SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(1),
        enabled: true,
    };
    let rejected = request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureSetEnabled(enable),
    );
    assert!(matches!(rejected, Response::Error(_)));

    let renew = CaptureCommandParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(2),
    };
    assert!(matches!(
        request_response(&mut server, &mut client, id, &Request::LeaseRenew(renew)),
        Response::Success(SuccessResult::Renew(_))
    ));
}

#[test]
fn skipped_capability_sequence_fatally_closes_and_revokes() {
    let material = material(4, Purpose::Capture);
    let mut server =
        OwnerProtocolServer::start_fake(owner(4), RecordingFakeExecutor::default()).unwrap();
    let id = connection(4);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let skipped = CaptureCommandParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(2),
    };
    client
        .send_request(&Request::LeaseRenew(skipped))
        .expect("send skipped command");
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert!(!matches!(
        server.state().reported_state(),
        ReportedState::LeaseDisabled | ReportedState::LeaseEnabled
    ));
    assert!(matches!(client.poll(), Ok(ClientPoll::PeerClosed)));
}

#[test]
fn authenticated_mac_fault_fatally_revokes_without_dispatch() {
    let material = material(5, Purpose::Capture);
    let mut server =
        OwnerProtocolServer::start_fake(owner(5), RecordingFakeExecutor::default()).unwrap();
    let id = connection(5);
    let (mut gateway_codec, owner_codec) = material.codecs().unwrap();
    let (mut client_endpoint, server_endpoint) = fake_ordered_transport_pair();
    server
        .attach_connection(id, server_endpoint, owner_codec)
        .unwrap();
    let mut frame = gateway_codec
        .encode_request(&Request::LeaseAcquire(Empty {}))
        .unwrap()
        .into_frame();
    let last = frame.len() - 1;
    frame[last] ^= 1;
    let receipt = client_endpoint.try_send(frame).unwrap();
    client_endpoint.confirm_flushed(receipt).unwrap();

    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
    assert!(server.executor().actions().is_empty());
}

#[test]
fn malformed_outer_framing_reaches_receiver_and_fatally_revokes() {
    let material = material(18, Purpose::Capture);
    let mut server =
        OwnerProtocolServer::start_fake(owner(18), RecordingFakeExecutor::default()).unwrap();
    let id = connection(18);
    let (mut gateway_codec, owner_codec) = material.codecs().unwrap();
    let (mut client_endpoint, server_endpoint) = fake_ordered_transport_pair();
    server
        .attach_connection(id, server_endpoint, owner_codec)
        .unwrap();
    let acquire = gateway_codec
        .encode_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    let receipt = client_endpoint.try_send(acquire.into_frame()).unwrap();
    client_endpoint.confirm_flushed(receipt).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    let ReceiveResult::Frame(response) = client_endpoint.try_receive() else {
        panic!("lease response")
    };
    gateway_codec.receive_owner_frame(&response).unwrap();

    let receipt = client_endpoint
        .try_send_receiver_fault(vec![0, 0, 0, 8, 1])
        .expect("bounded malformed frame reaches receiver");
    client_endpoint.confirm_flushed(receipt).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
}

#[test]
fn direct_event_writer_failure_aborts_and_revokes_current_route() {
    let material = material(34, Purpose::Capture);
    let mut server =
        OwnerProtocolServer::start_fake(owner(34), RecordingFakeExecutor::default()).unwrap();
    let id = connection(34);
    let mut client = attach(&mut server, &material, id);
    acquire_capture(&mut server, &mut client, id);
    client.abort();
    assert!(matches!(
        server.publish_health_changed(id),
        Err(ServerError::EventDelivery)
    ));
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
}

#[test]
fn dropped_fake_peers_are_reaped_without_exhausting_connection_capacity() {
    let materials = (20_u8..29)
        .map(|value| material(value, Purpose::Observe))
        .collect::<Vec<_>>();
    let mut server =
        OwnerProtocolServer::start_fake(owner(19), RecordingFakeExecutor::default()).unwrap();
    for (index, material) in materials.iter().enumerate() {
        let id = connection(index as u64 + 20);
        let client = attach(&mut server, material, id);
        drop(client);
        assert_eq!(server.pump(id).unwrap(), ServerPump::PeerClosed);
    }
}

#[test]
fn maintenance_terminal_events_use_only_the_original_capture_connection() {
    let capture_material = material(6, Purpose::Capture);
    let maintenance_material = material(7, Purpose::Maintenance);
    let mut server =
        OwnerProtocolServer::start_fake(owner(6), RecordingFakeExecutor::default()).unwrap();
    let capture_id = connection(6);
    let maintenance_id = connection(7);
    let mut capture_client = attach(&mut server, &capture_material, capture_id);
    let capture = acquire_capture(&mut server, &mut capture_client, capture_id);
    let mut maintenance_client = attach(&mut server, &maintenance_material, maintenance_id);
    let transaction = Bytes32::new([81; 32]);
    let acquire = Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
        transaction_id: transaction,
        source_build_digest: Bytes32::new([82; 32]),
    });
    let request_sequence = maintenance_client.send_request(&acquire).unwrap();
    assert_eq!(server.pump(maintenance_id).unwrap(), ServerPump::Processed);

    let first = capture_client.poll().unwrap();
    let second = capture_client.poll().unwrap();
    assert!(matches!(
        first,
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseRevoked {
                capture_lease_id,
                ..
            }
        )) if capture_lease_id == capture.id
    ));
    assert!(matches!(
        second,
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseNeutral { .. }
        ))
    ));
    assert_eq!(capture_client.poll().unwrap(), ClientPoll::Empty);

    let maintenance_response = maintenance_client.poll().unwrap();
    let ClientPoll::Message(GatewayMessage::Response {
        correlation_sequence,
        response: Response::Success(SuccessResult::MaintenanceAcquire(acquired)),
    }) = maintenance_response
    else {
        panic!("maintenance response missing")
    };
    assert_eq!(correlation_sequence, request_sequence);
    assert_eq!(acquired.state, MaintenanceAcquireState::Sealed);
    assert_eq!(maintenance_client.poll().unwrap(), ClientPoll::Empty);
}

#[test]
fn final_maintenance_response_is_flushed_before_exit() {
    let material = material(8, Purpose::Maintenance);
    let mut server =
        OwnerProtocolServer::start_fake(owner(8), RecordingFakeExecutor::default()).unwrap();
    let id = connection(8);
    let mut client = attach(&mut server, &material, id);
    let transaction = Bytes32::new([91; 32]);
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([92; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = response else {
        panic!("maintenance acquire failed")
    };
    let mut maintenance = MaintenanceWire {
        id: acquired.maintenance_capability_id,
        epoch: acquired.maintenance_capability_epoch,
        next_sequence: 1,
    };
    let command = maintenance.params();
    let prepare = Request::MaintenancePrepare(MaintenancePrepareParams {
        maintenance_capability_id: command.maintenance_capability_id,
        maintenance_capability_epoch: command.maintenance_capability_epoch,
        command_sequence: command.command_sequence,
        transaction_id: transaction,
        operation: WireMaintenanceOperation::Uninstall,
    });
    let sequence = client.send_request(&prepare).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert!(server.exit_requested());
    assert_eq!(server.state().process(), ProcessState::Exiting);
    assert!(server.executor().actions().ends_with(&[
        RequiredActionKind::StopNativeAdapter,
        RequiredActionKind::ExitOwner
    ]));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response {
            correlation_sequence,
            response: Response::Success(SuccessResult::MaintenancePrepare(_)),
        }) if correlation_sequence == sequence
    ));
}

#[test]
fn maintenance_exit_waits_for_backpressured_final_response_flush() {
    let material = material(42, Purpose::Maintenance);
    let mut server = OwnerProtocolServer::start(owner(42), RecordingFakeExecutor::default())
        .expect("start server");
    let id = connection(42);
    let (mut client, pending, _, _) = attach_pending_once(&mut server, &material, id);
    let transaction = Bytes32::new([121; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([122; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    pending.store(true, Ordering::SeqCst);
    let sequence = client
        .send_request(&Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(1),
            transaction_id: transaction,
            operation: WireMaintenanceOperation::Uninstall,
        }))
        .unwrap();

    assert_eq!(server.pump(id).unwrap(), ServerPump::Backpressured);
    assert!(!server.exit_requested());
    assert_eq!(client.poll().unwrap(), ClientPoll::Empty);
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert!(server.exit_requested());
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response {
            correlation_sequence,
            response: Response::Success(SuccessResult::MaintenancePrepare(_)),
        }) if correlation_sequence == sequence
    ));
}

#[test]
fn maintenance_exit_retries_asynchronous_graceful_close() {
    let material = material(45, Purpose::Maintenance);
    let mut server = OwnerProtocolServer::start(owner(45), RecordingFakeExecutor::default())
        .expect("start server");
    let id = connection(45);
    let (mut client, _, _, pending_close) = attach_pending_once(&mut server, &material, id);
    let transaction = Bytes32::new([131; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([132; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    pending_close.store(true, Ordering::SeqCst);
    client
        .send_request(&Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(1),
            transaction_id: transaction,
            operation: WireMaintenanceOperation::Uninstall,
        }))
        .unwrap();

    assert_eq!(server.pump(id).unwrap(), ServerPump::Backpressured);
    assert!(!server.exit_requested());
    assert!(matches!(client.poll().unwrap(), ClientPoll::Message(_)));
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert!(server.exit_requested());
}

struct AbortOnStopExecutor {
    inner: RecordingFakeExecutor,
    abort: Option<FakeTransportControl>,
    barrier: Option<Arc<Barrier>>,
    fail_exit: bool,
}

impl AbortOnStopExecutor {
    fn new() -> Self {
        Self {
            inner: RecordingFakeExecutor::default(),
            abort: None,
            barrier: None,
            fail_exit: false,
        }
    }
}

impl OwnerExecutor for AbortOnStopExecutor {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }

    fn readiness(&self) -> NativeReadiness {
        self.inner.readiness()
    }

    fn allocate_capability_id(
        &mut self,
    ) -> Option<talking_quill_keyboard_owner::state::CapabilityId> {
        self.inner.allocate_capability_id()
    }

    fn execute(&mut self, command: ExecutorCommand) -> ExecutorResult {
        if command.action().kind() == RequiredActionKind::ExitOwner && self.fail_exit {
            return ExecutorResult::Failed(NativeActionFailure::FailedNotApplied);
        }
        if command.action().kind() == RequiredActionKind::StopNativeAdapter {
            if let Some(barrier) = &self.barrier {
                barrier.wait();
                barrier.wait();
            }
            if let Some(abort) = &self.abort {
                abort.abort();
            }
        }
        self.inner.execute(command)
    }

    fn permissions(&self) -> talking_quill_owner_protocol::schema::PermissionsResult {
        self.inner.permissions()
    }

    fn front_app(&self) -> talking_quill_owner_protocol::schema::FrontAppResult {
        self.inner.front_app()
    }

    fn observability(&self) -> talking_quill_owner_protocol::schema::ObservabilityResult {
        self.inner.observability()
    }
}

#[test]
fn final_flush_authorizes_exit_even_if_outer_executor_reports_failure() {
    let material = material(35, Purpose::Maintenance);
    let mut executor = AbortOnStopExecutor::new();
    executor.fail_exit = true;
    let mut server = OwnerProtocolServer::start_fake(owner(35), executor).unwrap();
    let id = connection(35);
    let mut client = attach(&mut server, &material, id);
    let transaction = Bytes32::new([135; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([136; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(1),
            transaction_id: transaction,
            operation: WireMaintenanceOperation::Uninstall,
        }),
    );
    assert!(matches!(
        response,
        Response::Success(SuccessResult::MaintenancePrepare(_))
    ));
    assert!(server.exit_requested());
    assert_eq!(server.state().process(), ProcessState::Exiting);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn final_response_enqueue_race_never_confirms_exit() {
    let material = material(9, Purpose::Maintenance);
    let mut server = OwnerProtocolServer::start_fake(owner(9), AbortOnStopExecutor::new()).unwrap();
    let id = connection(9);
    let (mut client, control) = attach_with_control(&mut server, &material, id);
    let transaction = Bytes32::new([101; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([102; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    server.executor_mut().abort = Some(control);
    let prepare = Request::MaintenancePrepare(MaintenancePrepareParams {
        maintenance_capability_id: acquired.maintenance_capability_id,
        maintenance_capability_epoch: acquired.maintenance_capability_epoch,
        command_sequence: wire_u64(1),
        transaction_id: transaction,
        operation: WireMaintenanceOperation::Uninstall,
    });
    client.send_request(&prepare).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert!(!server.exit_requested());
    assert_ne!(server.state().process(), ProcessState::Exiting);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn stop_race_with_peer_abort_is_ordered_and_never_exits() {
    let material = material(10, Purpose::Maintenance);
    let mut server =
        OwnerProtocolServer::start_fake(owner(10), AbortOnStopExecutor::new()).unwrap();
    let id = connection(10);
    let (mut client, control) = attach_with_control(&mut server, &material, id);
    let transaction = Bytes32::new([111; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([112; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    let barrier = Arc::new(Barrier::new(2));
    server.executor_mut().barrier = Some(Arc::clone(&barrier));
    let race = thread::spawn(move || {
        barrier.wait();
        control.abort();
        barrier.wait();
    });
    let prepare = Request::MaintenancePrepare(MaintenancePrepareParams {
        maintenance_capability_id: acquired.maintenance_capability_id,
        maintenance_capability_epoch: acquired.maintenance_capability_epoch,
        command_sequence: wire_u64(1),
        transaction_id: transaction,
        operation: WireMaintenanceOperation::Uninstall,
    });
    client.send_request(&prepare).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    race.join().unwrap();
    assert!(!server.exit_requested());
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn separate_paste_admission_works_while_keyboard_is_safe_disabled() {
    let readiness = NativeReadiness {
        keyboard_build_eligible: false,
        paste_ready: true,
        permissions_eligible: false,
        hook_healthy: false,
    };
    let material = material(11, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(readiness);
    let mut server = OwnerProtocolServer::start_fake(owner(11), executor).unwrap();
    let id = connection(11);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let operation = Bytes32::new([121; 32]);
    let paste = PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(1),
        operation_id: operation,
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([122; 32]),
    };
    assert!(matches!(
        request_response(&mut server, &mut client, id, &Request::PasteInject(paste)),
        Response::Success(SuccessResult::Paste(PasteResult::Waiting { operation_id }))
            if operation_id == operation
    ));
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(
        server
            .executor()
            .actions()
            .contains(&RequiredActionKind::AdmitPaste)
    );

    let authorization = PasteAuthorization::new(
        PasteOperationId::new(*operation.as_bytes()).unwrap(),
        server.state().owner_instance(),
        match server.state().controller() {
            talking_quill_keyboard_owner::state::ControllerState::CaptureLeaseDisabled {
                authority,
                ..
            } => authority.epoch(),
            _ => panic!("disabled capture lease expected"),
        },
        OwnerActivationGeneration::new(1).unwrap(),
    );
    server.confirm_paste_claimed(authorization).unwrap();
    server
        .publish_paste_committed(authorization, false)
        .unwrap();
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Event(
            talking_quill_owner_protocol::schema::Event::PasteCommitted(_)
        ))
    ));
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(matches!(
        server.publish_paste_committed(authorization, false),
        Err(ServerError::EventScope)
    ));
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn paste_failure_results_distinguish_not_applied_from_indeterminate() {
    for (case, failure) in [
        (32_u8, NativeActionFailure::FailedNotApplied),
        (33_u8, NativeActionFailure::Indeterminate),
    ] {
        let readiness = NativeReadiness {
            keyboard_build_eligible: false,
            paste_ready: true,
            permissions_eligible: false,
            hook_healthy: false,
        };
        let material = material(case, Purpose::Capture);
        let executor = RecordingFakeExecutor::default().with_readiness(readiness);
        let mut server = OwnerProtocolServer::start_fake(owner(case), executor).unwrap();
        let id = connection(u64::from(case));
        let mut client = attach(&mut server, &material, id);
        let capture = acquire_capture(&mut server, &mut client, id);
        let operation = Bytes32::new([case.wrapping_add(50); 32]);
        let authorization = PasteAuthorization::new(
            PasteOperationId::new(*operation.as_bytes()).unwrap(),
            server.state().owner_instance(),
            match server.state().controller() {
                talking_quill_keyboard_owner::state::ControllerState::CaptureLeaseDisabled {
                    authority,
                    ..
                } => authority.epoch(),
                _ => panic!("disabled capture lease expected"),
            },
            OwnerActivationGeneration::new(1).unwrap(),
        );
        server
            .executor_mut()
            .script(ExecutorResult::Failed(failure));
        let owner_instance_id = Bytes32::new(*server.state().owner_instance().as_bytes());
        let response = request_response(
            &mut server,
            &mut client,
            id,
            &Request::PasteInject(PasteInjectParams {
                capture_lease_id: capture.id,
                capture_lease_epoch: capture.epoch,
                command_sequence: wire_u64(1),
                operation_id: operation,
                owner_instance_id,
                activation_generation: wire_u64(1),
                target_token: None,
                fallback_text_sha256: Bytes32::new([case.wrapping_add(51); 32]),
            }),
        );
        match failure {
            NativeActionFailure::FailedNotApplied => {
                assert!(matches!(
                    response,
                    Response::Success(SuccessResult::Paste(PasteResult::ClipboardOnly {
                        reason: PasteRefusalReason::NativeRejected,
                    }))
                ));
                assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
                assert!(matches!(
                    server.confirm_paste_completed(authorization),
                    Err(ServerError::EventScope)
                ));
            }
            NativeActionFailure::Indeterminate => {
                assert!(matches!(
                    response,
                    Response::Success(SuccessResult::Paste(PasteResult::Indeterminate {
                        operation_id,
                    })) if operation_id == operation
                ));
                assert_eq!(
                    server.state().ownership().paste(),
                    PasteOwnership::Indeterminate
                );
                server.confirm_paste_completed(authorization).unwrap();
                assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
            }
        }
    }
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn indeterminate_paste_survives_a_later_close_failure() {
    let material = material(37, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(37), executor).unwrap();
    let id = connection(37);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    let operation = Bytes32::new([137; 32]);
    let authorization = PasteAuthorization::new(
        PasteOperationId::new(*operation.as_bytes()).unwrap(),
        server.state().owner_instance(),
        match server.state().controller() {
            talking_quill_keyboard_owner::state::ControllerState::CaptureLeaseEnabled {
                authority,
                ..
            } => authority.epoch(),
            _ => panic!("enabled capture lease expected"),
        },
        OwnerActivationGeneration::new(1).unwrap(),
    );
    server
        .executor_mut()
        .script(ExecutorResult::Failed(NativeActionFailure::Indeterminate));
    server.executor_mut().script(ExecutorResult::Failed(
        NativeActionFailure::FailedNotApplied,
    ));
    server.executor_mut().script(ExecutorResult::Applied);
    let owner_instance_id = Bytes32::new(*server.state().owner_instance().as_bytes());
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::PasteInject(PasteInjectParams {
            capture_lease_id: capture.id,
            capture_lease_epoch: capture.epoch,
            command_sequence: wire_u64(capture.next_sequence),
            operation_id: operation,
            owner_instance_id,
            activation_generation: wire_u64(1),
            target_token: None,
            fallback_text_sha256: Bytes32::new([138; 32]),
        }),
    );
    assert!(matches!(
        response,
        Response::Success(SuccessResult::Paste(PasteResult::Indeterminate {
            operation_id,
        })) if operation_id == operation
    ));
    assert_eq!(
        server.state().ownership().paste(),
        PasteOwnership::Indeterminate
    );
    server.confirm_paste_completed(authorization).unwrap();
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn indeterminate_paste_survives_exhausted_and_deferred_close() {
    let material = material(39, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(39), executor).unwrap();
    let id = connection(39);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    let operation = Bytes32::new([139; 32]);
    let authorization = PasteAuthorization::new(
        PasteOperationId::new(*operation.as_bytes()).unwrap(),
        server.state().owner_instance(),
        match server.state().controller() {
            talking_quill_keyboard_owner::state::ControllerState::CaptureLeaseEnabled {
                authority,
                ..
            } => authority.epoch(),
            _ => panic!("enabled capture lease expected"),
        },
        OwnerActivationGeneration::new(1).unwrap(),
    );
    server
        .executor_mut()
        .script(ExecutorResult::Failed(NativeActionFailure::Indeterminate));
    for _ in 0..=8 {
        server.executor_mut().script(ExecutorResult::Failed(
            NativeActionFailure::FailedNotApplied,
        ));
    }
    let owner_instance_id = Bytes32::new(*server.state().owner_instance().as_bytes());
    client
        .send_request(&Request::PasteInject(PasteInjectParams {
            capture_lease_id: capture.id,
            capture_lease_epoch: capture.epoch,
            command_sequence: wire_u64(capture.next_sequence),
            operation_id: operation,
            owner_instance_id,
            activation_generation: wire_u64(1),
            target_token: None,
            fallback_text_sha256: Bytes32::new([140; 32]),
        }))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert!(server.has_deferred_close());
    assert_eq!(
        server.state().ownership().paste(),
        PasteOwnership::Indeterminate
    );
    assert!(server.service_deferred_close().unwrap());
    server.confirm_paste_completed(authorization).unwrap();
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn disconnect_cancels_candidate_then_reaches_neutral_without_a_new_lease() {
    let material = material(12, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(12), executor).unwrap();
    let id = connection(12);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    server
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    server.executor_mut().script(ExecutorResult::Applied);
    server
        .executor_mut()
        .script(ExecutorResult::CandidateCancelled(NativeOwnership::NEUTRAL));
    client.abort();
    assert_eq!(server.pump(id).unwrap(), ServerPump::PeerClosed);
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
    assert!(
        server
            .executor()
            .actions()
            .contains(&RequiredActionKind::CancelCandidate)
    );
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn committed_activation_disconnect_drains_without_replay_or_new_lease() {
    let material = material(13, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(13), executor).unwrap();
    let id = connection(13);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    server
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    client.abort();
    assert_eq!(server.pump(id).unwrap(), ServerPump::PeerClosed);
    assert_eq!(
        server.state().reported_state(),
        ReportedState::OrphanDraining
    );
    assert!(
        !server
            .executor()
            .actions()
            .contains(&RequiredActionKind::CancelCandidate)
    );
    server
        .observe_native_ownership(NativeOwnership::NEUTRAL)
        .unwrap();
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn revocation_waits_for_retry_after_failed_or_indeterminate_close() {
    for (case, failure) in [
        (30_u8, NativeActionFailure::FailedNotApplied),
        (31_u8, NativeActionFailure::Indeterminate),
    ] {
        let material = material(case, Purpose::Capture);
        let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
        let mut server = OwnerProtocolServer::start_fake(owner(case), executor).unwrap();
        let id = connection(u64::from(case));
        let mut client = attach(&mut server, &material, id);
        let mut capture = acquire_capture(&mut server, &mut client, id);
        enable_empty_configuration(&mut server, &mut client, id, &mut capture);
        server
            .executor_mut()
            .script(ExecutorResult::Failed(failure));
        server.executor_mut().script(ExecutorResult::Applied);

        let request_sequence = client
            .send_request(&Request::LeaseRelease(capture.params()))
            .unwrap();
        assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
        assert_eq!(server.state().admission(), AdmissionState::Closed);
        assert_eq!(
            server
                .executor()
                .actions()
                .iter()
                .filter(|kind| **kind == RequiredActionKind::CloseFreshAdmission)
                .count(),
            2
        );
        assert!(matches!(
            client.poll().unwrap(),
            ClientPoll::Message(GatewayMessage::PredecessorTerminal(
                talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseRevoked { .. }
            ))
        ));
        // The final terminal status is sequenced after revocation and before
        // the correlated native-failure response.
        assert!(matches!(
            client.poll().unwrap(),
            ClientPoll::Message(GatewayMessage::PredecessorTerminal(_))
        ));
        assert!(matches!(
            client.poll().unwrap(),
            ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response: Response::Error(ref error),
            }) if correlation_sequence == request_sequence
                && error.code() == ErrorCode::NativeFailure
        ));
    }
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn repeated_close_failure_is_bounded_without_stack_recursion() {
    let material = material(36, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(36), executor).unwrap();
    let id = connection(36);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    for _ in 0..=8 {
        server.executor_mut().script(ExecutorResult::Failed(
            NativeActionFailure::FailedNotApplied,
        ));
    }
    client
        .send_request(&Request::LeaseRelease(capture.params()))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert_eq!(
        server
            .executor()
            .actions()
            .iter()
            .filter(|kind| **kind == RequiredActionKind::CloseFreshAdmission)
            .count(),
        8
    );
    assert!(server.has_deferred_close());
    assert!(matches!(client.poll(), Ok(ClientPoll::PeerClosed)));
    assert!(server.service_deferred_close().unwrap());
    assert!(!server.has_deferred_close());
    assert_eq!(server.state().admission(), AdmissionState::Closed);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn draining_release_responds_once_and_finalizes_on_the_original_route() {
    let material = material(17, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(17), executor).unwrap();
    let id = connection(17);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    server
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();

    let sequence = client
        .send_request(&Request::LeaseRelease(capture.params()))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseRevoked { .. }
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseDraining { .. }
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response {
            correlation_sequence,
            response: Response::Success(SuccessResult::Release(
                talking_quill_owner_protocol::schema::ReleaseResult {
                    disposition: talking_quill_owner_protocol::schema::LeaseDisposition::Draining,
                }
            )),
        }) if correlation_sequence == sequence
    ));
    assert_eq!(
        server.state().reported_state(),
        ReportedState::OrphanDraining
    );

    server
        .observe_native_ownership(NativeOwnership::NEUTRAL)
        .unwrap();
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseNeutral { .. }
        ))
    ));
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn disable_clears_session_event_route_before_reenable() {
    let material = material(38, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(38), executor).unwrap();
    let id = connection(38);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);

    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );
    server
        .admit_keyboard_event(KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        })
        .unwrap();
    let disable = talking_quill_owner_protocol::schema::SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: false,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureSetEnabled(disable),
    );
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionReconcileOff(capture.params()),
    );
    let enable = talking_quill_owner_protocol::schema::SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: true,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureSetEnabled(enable),
    );
    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );
    server
        .admit_keyboard_event(KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        })
        .unwrap();
    server.flush_admitted_events().unwrap();
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Event(
            talking_quill_owner_protocol::schema::Event::SessionKey(_)
        ))
    ));
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[test]
fn eight_events_are_admitted_and_the_ninth_fails_closed_before_admission() {
    let material = material(14, Purpose::Capture);
    let executor = RecordingFakeExecutor::default().with_readiness(enabled_readiness());
    let mut server = OwnerProtocolServer::start_fake(owner(14), executor).unwrap();
    let id = connection(14);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    enable_empty_configuration(&mut server, &mut client, id, &mut capture);
    for _ in 0..OWNER_ADMITTED_EFFECT_CAPACITY {
        server.admit_audio_devices_changed().unwrap();
    }
    assert_eq!(server.queued_event_count(), OWNER_ADMITTED_EFFECT_CAPACITY);
    assert!(matches!(
        server.admit_audio_devices_changed(),
        Err(ServerError::EventCapacity)
    ));
    assert_eq!(server.queued_event_count(), 0);
    assert_eq!(server.state().ownership().admitted_effects(), 0);
    assert_eq!(server.state().process(), ProcessState::Degraded);
    let mut delivered = 0;
    while let ClientPoll::Message(GatewayMessage::Event(_)) = client.poll().unwrap() {
        delivered += 1;
    }
    assert_eq!(delivered, OWNER_ADMITTED_EFFECT_CAPACITY);
}

#[test]
fn wrong_maintenance_transaction_is_semantic_and_does_not_stop_native() {
    let material = material(15, Purpose::Maintenance);
    let mut server =
        OwnerProtocolServer::start_fake(owner(15), RecordingFakeExecutor::default()).unwrap();
    let id = connection(15);
    let mut client = attach(&mut server, &material, id);
    let transaction = Bytes32::new([131; 32]);
    let acquired = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([132; 32]),
        }),
    );
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = acquired else {
        panic!("maintenance acquire")
    };
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(1),
            transaction_id: Bytes32::new([133; 32]),
            operation: WireMaintenanceOperation::Uninstall,
        }),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::InvalidState
    ));
    assert!(
        !server
            .executor()
            .actions()
            .contains(&RequiredActionKind::StopNativeAdapter)
    );
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(2),
            transaction_id: transaction,
            operation: WireMaintenanceOperation::Uninstall,
        }),
    );
    assert!(matches!(
        response,
        Response::Success(SuccessResult::MaintenancePrepare(_))
    ));
    assert!(server.exit_requested());
}

proptest! {
    #[test]
    fn fake_transport_preserves_frame_order(values in prop::collection::vec(any::<u8>(), 1..=32)) {
        let capacity = NonZeroUsize::new(values.len()).unwrap();
        let (mut sender, mut receiver) = fake_ordered_transport_pair_with_capacity(capacity).unwrap();
        for value in &values {
            let frame = encode_outer_frame(&[*value]).unwrap();
            let receipt = sender.try_send(frame).unwrap();
            sender.confirm_flushed(receipt).unwrap();
        }
        sender.close();
        let mut observed = Vec::new();
        loop {
            match receiver.try_receive() {
                ReceiveResult::Frame(frame) => observed.push(frame[4]),
                ReceiveResult::PeerClosed => break,
                ReceiveResult::Empty => prop_assert!(false, "EOF must follow queued frames"),
            }
        }
        prop_assert_eq!(observed, values);
    }

    #[test]
    fn every_fresh_skipped_command_sequence_is_fatal(sequence in 2_u64..=u64::MAX) {
        let material = material(16, Purpose::Capture);
        let mut server = OwnerProtocolServer::start_fake(owner(16), RecordingFakeExecutor::default()).unwrap();
        let id = connection(16);
        let mut client = attach(&mut server, &material, id);
        let capture = acquire_capture(&mut server, &mut client, id);
        let request = Request::LeaseRenew(CaptureCommandParams {
            capture_lease_id: capture.id,
            capture_lease_epoch: capture.epoch,
            command_sequence: wire_u64(sequence),
        });
        client.send_request(&request).unwrap();
        prop_assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
        prop_assert!(!matches!(server.state().reported_state(), ReportedState::LeaseDisabled | ReportedState::LeaseEnabled));
    }
}
