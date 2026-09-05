use super::connection::{OWNER_INSTANCE, TcpConnector, wire_bindings};
use super::process::{free_address, release_authoritative_neutral, spawn_role};
use std::time::{Duration, Instant};
use talking_quill_helper::owner::client::{
    ConnectError, ConnectedOwner, OwnerCaptureClient, OwnerClientError, OwnerConnector,
    OwnerEventDisposition, OwnerMaintenanceClient,
};
use talking_quill_owner_protocol::client::{ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::*;
use talking_quill_owner_protocol::{Bytes32, GatewayMessage, U64String};

fn connect_capture_when_ready(connector: &mut TcpConnector) -> OwnerCaptureClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match OwnerCaptureClient::connect(connector, |_| OwnerEventDisposition::Continue) {
            Ok(client) => return client,
            Err(OwnerClientError::Connect(ConnectError::Unavailable))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("owner capture endpoint did not become ready: {error}"),
        }
    }
}

fn connect_maintenance_when_ready(connector: &mut TcpConnector) -> ConnectedOwner {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match connector.connect_maintenance() {
            Ok(connected) => return connected,
            Err(ConnectError::Unavailable) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("owner maintenance endpoint did not become ready: {error}"),
        }
    }
}

fn wait_owner_response(client: &mut OwnerProtocolClient<'static>, request: Request) -> Response {
    let correlation = client.send_request(&request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client.poll().unwrap() {
            ClientPoll::Empty if Instant::now() < deadline => std::thread::yield_now(),
            ClientPoll::Empty => panic!("owner response timed out"),
            ClientPoll::PeerClosed => panic!("owner closed before response"),
            ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response,
            }) if correlation_sequence == correlation => return response,
            ClientPoll::Message(_) => panic!("unexpected owner message"),
        }
    }
}

fn assert_maintenance_command_replay_is_rejected(
    connector: &mut TcpConnector,
    acquire: MaintenanceAcquireParams,
) {
    let ConnectedOwner { mut client, .. } = connect_maintenance_when_ready(connector);
    let capability = match wait_owner_response(&mut client, Request::MaintenanceAcquire(acquire)) {
        Response::Success(SuccessResult::MaintenanceAcquire(capability)) => capability,
        response => panic!("maintenance acquisition failed: {response:?}"),
    };
    let renew = MaintenanceCommandParams {
        maintenance_capability_id: capability.maintenance_capability_id,
        maintenance_capability_epoch: capability.maintenance_capability_epoch,
        command_sequence: U64String::try_from(1).unwrap(),
    };
    assert!(matches!(
        wait_owner_response(&mut client, Request::MaintenanceRenew(renew.clone())),
        Response::Success(SuccessResult::Renew(RenewResult { renewed: true }))
    ));

    let _duplicate_correlation = client
        .send_request(&Request::MaintenanceRenew(renew))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client.poll().unwrap() {
            ClientPoll::PeerClosed => break,
            ClientPoll::Empty if Instant::now() < deadline => std::thread::yield_now(),
            ClientPoll::Empty => panic!("duplicate maintenance sequence was not rejected"),
            ClientPoll::Message(message) => {
                panic!("duplicate maintenance sequence produced a message: {message:?}")
            }
        }
    }
}

pub(super) fn owner_runtime_fake_gateway_suite() {
    let replay_address = free_address();
    let replay_owner = spawn_role("owner-runtime-fake-auth", replay_address);
    let mut replay_connector = TcpConnector::fixed(replay_address, OWNER_INSTANCE[0]);
    assert_maintenance_command_replay_is_rejected(
        &mut replay_connector,
        MaintenanceAcquireParams::Update {
            transaction_id: Bytes32::new([70; 32]),
            source_build_digest: Bytes32::new([71; 32]),
            target_build_digest: Bytes32::new([72; 32]),
            target_owner_sha256: Bytes32::new([73; 32]),
        },
    );
    // A maintenance protocol fault intentionally leaves this isolated fixture
    // sealed rather than restoring capture. Termination is test cleanup only.
    replay_owner.terminate();

    let address = free_address();
    let owner = spawn_role("owner-runtime-fake-auth", address);
    let mut connector = TcpConnector::fixed(address, OWNER_INSTANCE[0]);
    let mut first = connect_capture_when_ready(&mut connector);
    first.configure(wire_bindings(), true).unwrap();
    // A following owner request is an observable serialization barrier: the
    // runtime pumps the queued active-candidate adapter observation after the
    // enable response and before it can process this health request.
    assert_eq!(
        first.refresh_health().unwrap().reported_state,
        OwnerReportedState::LeaseEnabled
    );
    drop(first);

    let drain_observation_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue) {
            Err(OwnerClientError::AcquireRejected(ErrorCode::Draining)) => break,
            Err(OwnerClientError::AcquireRejected(ErrorCode::Busy))
                if Instant::now() < drain_observation_deadline =>
            {
                std::thread::yield_now();
            }
            Ok(_) => panic!("replacement lease opened before real owner drain was observable"),
            Err(error) => panic!("real owner did not expose draining admission state: {error}"),
        }
    }

    release_authoritative_neutral(address);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut replacement = loop {
        match OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue) {
            Ok(client) => break client,
            Err(OwnerClientError::AcquireRejected(ErrorCode::Draining))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("replacement capture did not wait through real drain: {error}"),
        }
    };
    assert!(
        !replacement.enabled(),
        "replacement lease must begin disabled-first after real drain"
    );
    replacement.configure(wire_bindings(), false).unwrap();
    assert_eq!(replacement.release().unwrap(), LeaseDisposition::Neutral);
    drop(replacement);

    let transaction = Bytes32::new([77; 32]);
    let acquire = MaintenanceAcquireParams::Update {
        transaction_id: transaction,
        source_build_digest: Bytes32::new([78; 32]),
        target_build_digest: Bytes32::new([79; 32]),
        target_owner_sha256: Bytes32::new([80; 32]),
    };
    let mut maintenance = OwnerMaintenanceClient::acquire(&mut connector, acquire).unwrap();
    assert!(matches!(
        maintenance.prepare(Bytes32::new([76; 32]), MaintenanceOperation::Update),
        Err(OwnerClientError::Rejected(ErrorCode::InvalidState))
    ));
    let handoff = maintenance
        .prepare(transaction, MaintenanceOperation::Update)
        .unwrap();
    assert_ne!(handoff.as_bytes(), &[0; 32]);
    drop(maintenance);
    owner.wait_success();

    // This is restart/fresh-authority coverage only: the old runtime flushes
    // maintenance.prepare and exits before a restarted owner binds. No common
    // API consumes or binds the predecessor handoff token.
    let restart_address = free_address();
    let restarted_owner = spawn_role("owner-runtime-fake-auth", restart_address);
    let mut restarted_connector = TcpConnector::fixed(restart_address, OWNER_INSTANCE[0]);
    let mut restarted_capture = connect_capture_when_ready(&mut restarted_connector);
    assert!(!restarted_capture.enabled());
    restarted_capture.configure(wire_bindings(), false).unwrap();
    assert_eq!(
        restarted_capture.release().unwrap(),
        LeaseDisposition::Neutral
    );
    drop(restarted_capture);

    let restarted_transaction = Bytes32::new([81; 32]);
    let mut restarted_maintenance = OwnerMaintenanceClient::acquire(
        &mut restarted_connector,
        MaintenanceAcquireParams::Update {
            transaction_id: restarted_transaction,
            source_build_digest: Bytes32::new([82; 32]),
            target_build_digest: Bytes32::new([83; 32]),
            target_owner_sha256: Bytes32::new([84; 32]),
        },
    )
    .unwrap();
    let restarted_handoff = restarted_maintenance
        .prepare(restarted_transaction, MaintenanceOperation::Update)
        .unwrap();
    assert_ne!(restarted_handoff.as_bytes(), &[0; 32]);
    assert_ne!(
        restarted_handoff, handoff,
        "a restarted owner must allocate fresh handoff authority"
    );
    drop(restarted_maintenance);
    restarted_owner.wait_success();
}
