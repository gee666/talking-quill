use std::{
    collections::VecDeque,
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use talking_quill_helper::gateway::{
    ActivationCaptureGate, CallbackGate, GatewayBackend, HookStatus, TerminalSignal,
};
use talking_quill_helper::owner::client::{
    CaptureRevocation, ConnectError, ConnectedOwner, MaintenanceClock, OwnerCaptureClient,
    OwnerClientError, OwnerClock, OwnerConnector, OwnerEventDisposition, OwnerMaintenanceClient,
    OwnerShutdownControl,
};
use talking_quill_helper::owner::platform_client::{OwnerGatewayBackend, OwnerWorkerSpawner};
use talking_quill_helper::protocol::Outbound;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationKey, ProfileId as CoreProfileId, Shortcut,
};
use talking_quill_owner_protocol::fake_transport::fake_ordered_transport_pair;
use talking_quill_owner_protocol::schema::*;
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, GatewayMessage, OrderedTransport, OwnerSessionCodec,
    ReceiveResult, StreamOrderedTransport, TransportError, TransportProgress, U64String,
};

fn material() -> FakeAuthenticatedMaterial {
    FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([1; 32]),
        Bytes32::new([2; 32]),
        Purpose::Capture,
        [3; 32],
        [4; 32],
    )
}

fn health() -> HealthResult {
    HealthResult {
        owner_instance_id: Bytes32::new([2; 32]),
        reported_state: OwnerReportedState::LeaseDisabled,
        process_state: ProcessState::Healthy,
        rollback_latched: false,
        native_state_unknown: false,
        maintenance_sealed: false,
        keyboard_build_eligible: true,
        paste_ready: true,
        permissions_eligible: true,
        hook_healthy: true,
    }
}

fn success_for(request: &Request) -> Response {
    let success = match request {
        Request::LeaseAcquire(_) => SuccessResult::LeaseAcquire(LeaseAcquireResult {
            capture_lease_id: Bytes32::new([9; 32]),
            capture_lease_epoch: U64String::try_from(7).unwrap(),
            state: AcquireState::Disabled,
        }),
        Request::HealthGet(_) => SuccessResult::Health(health()),
        Request::PermissionsGet(_) => SuccessResult::Permissions(PermissionsResult {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            event_post: PermissionState::Granted,
        }),
        Request::SessionReconcileOff(_) => SuccessResult::SessionMode(SessionModeResult {
            mode: SessionMode::Off,
        }),
        Request::SessionSetMode(value) => {
            SuccessResult::SessionMode(SessionModeResult { mode: value.mode })
        }
        Request::CaptureReplaceConfiguration(value) => {
            SuccessResult::Configuration(ConfigurationResult {
                revision: value.revision,
            })
        }
        Request::CaptureSetEnabled(value) => SuccessResult::Enabled(EnabledResult {
            enabled: value.enabled,
        }),
        Request::LeaseRenew(_) => SuccessResult::Renew(RenewResult { renewed: true }),
        Request::LeaseRelease(_) | Request::OwnerExitWhenNeutral(_) => {
            SuccessResult::Release(ReleaseResult {
                disposition: LeaseDisposition::Neutral,
            })
        }
        Request::RuntimeRollback(_) => SuccessResult::Rollback(RollbackResult {
            latched: true,
            disposition: LeaseDisposition::Neutral,
        }),
        Request::MaintenanceAcquire(_) => {
            SuccessResult::MaintenanceAcquire(MaintenanceAcquireResult {
                maintenance_capability_id: Bytes32::new([8; 32]),
                maintenance_capability_epoch: U64String::try_from(9).unwrap(),
                state: MaintenanceAcquireState::Sealed,
            })
        }
        Request::MaintenancePrepare(_) => {
            SuccessResult::MaintenancePrepare(MaintenancePrepareResult {
                ready_to_exit: true,
                owner_handoff: Bytes32::new([19; 32]),
            })
        }
        Request::MaintenanceRenew(_) => SuccessResult::Renew(RenewResult { renewed: true }),
        _ => panic!("unexpected request in fake owner"),
    };
    Response::Success(success)
}

fn run_owner(
    mut endpoint: Box<dyn OrderedTransport>,
    mut codec: OwnerSessionCodec,
    seen: Arc<Mutex<Vec<Request>>>,
    stop_after: Option<Method>,
) {
    loop {
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => thread::yield_now(),
            ReceiveResult::PeerClosed => return,
            ReceiveResult::Frame(frame) => {
                let request = codec.receive_request(&frame).unwrap();
                seen.lock().unwrap().push(request.request().clone());
                if stop_after == Some(request.request().method()) {
                    endpoint.abort();
                    return;
                }
                let response = success_for(request.request());
                let frame = codec.encode_response(&request, &response).unwrap();
                let receipt = endpoint.try_send(frame).unwrap();
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    thread::yield_now();
                }
            }
        }
    }
}

fn run_owner_stopping_after_response(
    mut endpoint: Box<dyn OrderedTransport>,
    mut codec: OwnerSessionCodec,
    seen: Arc<Mutex<Vec<Request>>>,
    stop_after: Method,
    occurrence: usize,
) {
    let mut observed = 0;
    loop {
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => thread::yield_now(),
            ReceiveResult::PeerClosed => return,
            ReceiveResult::Frame(frame) => {
                let request = codec.receive_request(&frame).unwrap();
                seen.lock().unwrap().push(request.request().clone());
                if request.request().method() == stop_after {
                    observed += 1;
                }
                let response = success_for(request.request());
                let frame = codec.encode_response(&request, &response).unwrap();
                let receipt = endpoint.try_send(frame).unwrap();
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    thread::yield_now();
                }
                if observed == occurrence {
                    endpoint.abort();
                    return;
                }
            }
        }
    }
}

fn run_owner_rejecting_first_session_mode(
    mut endpoint: Box<dyn OrderedTransport>,
    mut codec: OwnerSessionCodec,
    seen: Arc<Mutex<Vec<Request>>>,
    rejection: ErrorCode,
) {
    let mut rejected = false;
    loop {
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => thread::yield_now(),
            ReceiveResult::PeerClosed => return,
            ReceiveResult::Frame(frame) => {
                let request = codec.receive_request(&frame).unwrap();
                seen.lock().unwrap().push(request.request().clone());
                let response = if request.request().method() == Method::SessionSetMode && !rejected
                {
                    rejected = true;
                    Response::Error(ErrorBody::new(rejection))
                } else {
                    success_for(request.request())
                };
                let frame = codec.encode_response(&request, &response).unwrap();
                let receipt = endpoint.try_send(frame).unwrap();
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    thread::yield_now();
                }
            }
        }
    }
}

struct OneShot(Option<ConnectedOwner>);
impl OwnerConnector for OneShot {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.0.take().ok_or(ConnectError::Unavailable)
    }
}

#[derive(Debug)]
struct TestShutdownControl {
    observed: crossbeam_channel::Sender<()>,
    release: crossbeam_channel::Sender<()>,
    result: CaptureRevocation,
}

impl OwnerShutdownControl for TestShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        let _ = self.observed.try_send(());
        let _ = self.release.try_send(());
        self.result
    }
}

struct ControlledOneShot {
    owner: Option<ConnectedOwner>,
    control: Arc<dyn OwnerShutdownControl>,
}

impl OwnerConnector for ControlledOneShot {
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        Arc::clone(&self.control)
    }

    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.owner.take().ok_or(ConnectError::Unavailable)
    }
}

struct FakeEndpointShutdownControl {
    control: talking_quill_owner_protocol::fake_transport::FakeTransportControl,
    revoked: Arc<std::sync::atomic::AtomicBool>,
}

impl OwnerShutdownControl for FakeEndpointShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        self.revoked.store(true, Ordering::Release);
        self.control.abort();
        CaptureRevocation::Confirmed
    }
}

#[derive(Debug)]
struct BlockingServiceTransport {
    inner: talking_quill_owner_protocol::fake_transport::FakeOrderedEndpoint,
    block_receive: Arc<std::sync::atomic::AtomicBool>,
    block_send: Arc<std::sync::atomic::AtomicBool>,
    entered: crossbeam_channel::Sender<()>,
    release: crossbeam_channel::Receiver<()>,
}

impl OrderedTransport for BlockingServiceTransport {
    fn is_test_only(&self) -> bool {
        true
    }

    fn try_send(
        &mut self,
        frame: Vec<u8>,
    ) -> Result<talking_quill_owner_protocol::FlushReceipt, TransportError> {
        if self.block_send.load(Ordering::Acquire) {
            let _ = self.entered.try_send(());
            let _ = self.release.recv();
        }
        self.inner.try_send(frame)
    }

    fn flush(
        &mut self,
        receipt: talking_quill_owner_protocol::FlushReceipt,
    ) -> Result<TransportProgress, TransportError> {
        self.inner.flush(receipt)
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        if self.block_receive.load(Ordering::Acquire) {
            let _ = self.entered.try_send(());
            let _ = self.release.recv();
        }
        OrderedTransport::try_receive(&mut self.inner)
    }

    fn close(&mut self) -> Result<TransportProgress, TransportError> {
        OrderedTransport::close(&mut self.inner)
    }

    fn abort(&mut self) {
        self.inner.abort();
    }
}

struct DelayedShutdownControl {
    release_connect: crossbeam_channel::Sender<()>,
    finished: crossbeam_channel::Sender<()>,
    delay: Duration,
}

impl OwnerShutdownControl for DelayedShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        thread::sleep(self.delay);
        let _ = self.release_connect.try_send(());
        let _ = self.finished.try_send(());
        CaptureRevocation::Confirmed
    }
}

struct BlockingReconnect {
    attempts: usize,
    entered: crossbeam_channel::Sender<()>,
    release: crossbeam_channel::Receiver<()>,
    control: Arc<dyn OwnerShutdownControl>,
}

impl OwnerConnector for BlockingReconnect {
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        Arc::clone(&self.control)
    }

    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.attempts += 1;
        if self.attempts == 1 {
            return Err(ConnectError::Unavailable);
        }
        let _ = self.entered.try_send(());
        let _ = self.release.recv();
        Err(ConnectError::Unavailable)
    }
}

#[test]
fn shutdown_joins_delayed_revocation_and_reconnect_workers_before_returning() {
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
    let (finished_tx, finished_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(DelayedShutdownControl {
        release_connect: release_tx,
        finished: finished_tx,
        delay: Duration::from_millis(650),
    });
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(BlockingReconnect {
            attempts: 0,
            entered: entered_tx,
            release: release_rx,
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    let shutdown = backend.shutdown();
    assert!(started.elapsed() >= Duration::from_millis(600));
    assert!(started.elapsed() < Duration::from_secs(2));
    finished_rx.recv_timeout(Duration::from_millis(10)).unwrap();
    assert!(shutdown.terminal_reason.is_none());
    drop(backend);
}

#[test]
fn shutdown_is_bounded_while_reconnect_is_blocked() {
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
    let (observed_tx, observed_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(TestShutdownControl {
        observed: observed_tx,
        release: release_tx,
        result: CaptureRevocation::Confirmed,
    });
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let admission_gate = Arc::new(talking_quill_helper::gateway::CallbackGate::new());
    admission_gate.open();
    let mut backend = OwnerGatewayBackend::connect_with_admission_gate(
        Box::new(BlockingReconnect {
            attempts: 0,
            entered: entered_tx,
            release: release_rx,
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
        Arc::clone(&admission_gate),
    )
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    let shutdown_worker = thread::spawn(move || backend.shutdown());
    thread::sleep(Duration::from_millis(25));
    assert!(!admission_gate.is_open());
    let shutdown = shutdown_worker.join().unwrap();
    observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(shutdown.terminal_reason, None);
    assert!(!shutdown.observability_quiescent);
}

#[test]
fn drop_revokes_and_joins_after_cancelling_blocked_connect() {
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded(1);
    let (observed_tx, observed_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(TestShutdownControl {
        observed: observed_tx,
        release: release_tx,
        result: CaptureRevocation::Confirmed,
    });
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let backend = OwnerGatewayBackend::connect_with(
        Box::new(BlockingReconnect {
            attempts: 0,
            entered: entered_tx,
            release: release_rx,
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    drop(backend);
    observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
}

fn backend_with_blockable_service(
    block_receive: Arc<std::sync::atomic::AtomicBool>,
    block_send: Arc<std::sync::atomic::AtomicBool>,
    entered: crossbeam_channel::Sender<()>,
    release: crossbeam_channel::Receiver<()>,
    control: Arc<dyn OwnerShutdownControl>,
) -> (OwnerGatewayBackend, thread::JoinHandle<()>) {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let worker = thread::spawn(move || {
        run_owner(
            Box::new(owner),
            owner_codec,
            Arc::new(Mutex::new(Vec::new())),
            None,
        )
    });
    let protocol = talking_quill_owner_protocol::client::OwnerProtocolClient::new(
        BlockingServiceTransport {
            inner: gateway,
            block_receive,
            block_send,
            entered,
            release,
        },
        gateway_codec,
    )
    .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let backend = OwnerGatewayBackend::connect_with(
        Box::new(ControlledOneShot {
            owner: Some(ConnectedOwner {
                client: protocol,
                build_id: "blocked-service-build".into(),
            }),
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    (backend, worker)
}

#[test]
fn blocked_poll_released_by_revocation_cannot_publish_owner_event() {
    let receive = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let send = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded(1);
    let (observed_tx, observed_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(TestShutdownControl {
        observed: observed_tx,
        release: release_tx,
        result: CaptureRevocation::Confirmed,
    });
    let (publish_tx, publish_rx) = crossbeam_channel::bounded(0);
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner_endpoint) = fake_ordered_transport_pair();
    let owner = thread::spawn(move || {
        loop {
            match owner_endpoint.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    let reconciled = request.request().method() == Method::SessionReconcileOff;
                    let response = success_for(request.request());
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let receipt = owner_endpoint.try_send(frame).unwrap();
                    while owner_endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                    if reconciled {
                        let _ = publish_rx.recv();
                        let event = Event::AudioDevicesChanged(AudioDevicesChangedEvent {
                            capture_lease_epoch: U64String::try_from(7).unwrap(),
                        });
                        let frame = owner_codec.encode_event(&event).unwrap();
                        let receipt = owner_endpoint.try_send(frame).unwrap();
                        while owner_endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                            thread::yield_now();
                        }
                    }
                }
            }
        }
    });
    let protocol = talking_quill_owner_protocol::client::OwnerProtocolClient::new(
        BlockingServiceTransport {
            inner: gateway,
            block_receive: Arc::clone(&receive),
            block_send: send,
            entered: entered_tx,
            release: release_rx,
        },
        gateway_codec,
    )
    .unwrap();
    let (outbound, events) = crossbeam_channel::bounded::<Outbound>(4);
    let admission_gate = Arc::new(talking_quill_helper::gateway::CallbackGate::new());
    admission_gate.open();
    let mut backend = OwnerGatewayBackend::connect_with_admission_gate(
        Box::new(ControlledOneShot {
            owner: Some(ConnectedOwner {
                client: protocol,
                build_id: "post-shutdown-event-build".into(),
            }),
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
        Arc::clone(&admission_gate),
    )
    .unwrap();
    receive.store(true, Ordering::Release);
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    publish_tx.send(()).unwrap();
    let shutdown = thread::spawn(move || backend.shutdown());
    let result = shutdown.join().unwrap();
    observed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(result.terminal_reason.is_none());
    assert!(!admission_gate.is_open());
    thread::sleep(Duration::from_millis(50));
    assert!(events.try_recv().is_err());
    owner.join().unwrap();
}

#[test]
fn shutdown_revokes_transport_while_event_polling_is_blocked() {
    let receive = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let send = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded(1);
    let (observed_tx, observed_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(TestShutdownControl {
        observed: observed_tx,
        release: release_tx,
        result: CaptureRevocation::Confirmed,
    });
    let (mut backend, owner) =
        backend_with_blockable_service(Arc::clone(&receive), send, entered_tx, release_rx, control);
    receive.store(true, Ordering::Release);
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    assert!(backend.shutdown().terminal_reason.is_none());
    observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(backend);
    owner.join().unwrap();
}

#[test]
fn shutdown_revokes_transport_while_lease_renew_send_is_blocked() {
    let receive = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let send = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered_tx, entered_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded(1);
    let (observed_tx, observed_rx) = crossbeam_channel::bounded(1);
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(TestShutdownControl {
        observed: observed_tx,
        release: release_tx,
        result: CaptureRevocation::Confirmed,
    });
    let (mut backend, owner) =
        backend_with_blockable_service(receive, Arc::clone(&send), entered_tx, release_rx, control);
    send.store(true, Ordering::Release);
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let started = Instant::now();
    assert!(backend.shutdown().terminal_reason.is_none());
    observed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(backend);
    owner.join().unwrap();
}

fn wire_bindings() -> Bindings {
    Bindings::new(vec![Binding::new(
        ProfileId::new("general".into()).unwrap(),
        BindingShortcut::new(Modifiers::new(false, true, false, false), vec![Letter::X]).unwrap(),
    )])
    .unwrap()
}

#[test]
fn fake_owner_enforces_disabled_first_and_contiguous_capability_sequence() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || run_owner(Box::new(owner), owner_codec, worker_seen, None));
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "local-build".into(),
    }));
    let mut client = OwnerCaptureClient::connect(&mut connector, |_event: GatewayMessage| {
        OwnerEventDisposition::Continue
    })
    .unwrap();
    client.configure(wire_bindings(), true).unwrap();
    client.set_session_mode(SessionMode::Recording).unwrap();
    client
        .renew_until(
            Instant::now() + Duration::from_secs(2),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(client.release().unwrap(), LeaseDisposition::Neutral);
    drop(client);
    worker.join().unwrap();

    let requests = seen.lock().unwrap();
    let methods: Vec<_> = requests.iter().map(Request::method).collect();
    assert_eq!(
        methods,
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::CaptureReplaceConfiguration,
            Method::CaptureSetEnabled,
            Method::SessionSetMode,
            Method::LeaseRenew,
            Method::CaptureSetEnabled,
            Method::LeaseRelease,
        ]
    );
    let sequences: Vec<u64> = requests.iter().filter_map(command_sequence).collect();
    assert_eq!(sequences, (1..=7).collect::<Vec<_>>());
}

fn command_sequence(request: &Request) -> Option<u64> {
    Some(match request {
        Request::SessionReconcileOff(v)
        | Request::LeaseRenew(v)
        | Request::LeaseRelease(v)
        | Request::RuntimeRollback(v) => v.command_sequence.get(),
        Request::CaptureReplaceConfiguration(v) => v.command_sequence.get(),
        Request::CaptureSetEnabled(v) => v.command_sequence.get(),
        Request::SessionSetMode(v) => v.command_sequence.get(),
        _ => return None,
    })
}

#[test]
fn uncertain_mutation_is_not_retried() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        run_owner(
            Box::new(owner),
            owner_codec,
            worker_seen,
            Some(Method::CaptureReplaceConfiguration),
        )
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "local-build".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    assert!(client.configure(wire_bindings(), true).is_err());
    worker.join().unwrap();
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|r| r.method() == Method::CaptureReplaceConfiguration)
            .count(),
        1
    );
}

struct ExpiringClock {
    origin: Instant,
    expired: Arc<std::sync::atomic::AtomicBool>,
}

impl OwnerClock for ExpiringClock {
    fn now(&self) -> Instant {
        let elapsed = if self.expired.load(Ordering::Acquire) {
            Duration::from_secs(1)
        } else {
            Duration::ZERO
        };
        self.origin + elapsed
    }

    fn sleep(&self, _duration: Duration) {
        thread::yield_now();
    }
}

#[test]
fn fake_clock_expiry_between_configuration_steps_never_sends_enable() {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let expired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_expired = Arc::clone(&expired);
    let worker = thread::spawn(move || {
        let mut replacements = 0;
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    worker_seen.lock().unwrap().push(request.request().clone());
                    if request.request().method() == Method::CaptureReplaceConfiguration {
                        replacements += 1;
                    }
                    let response = success_for(request.request());
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let receipt = owner.try_send(frame).unwrap();
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                    if replacements == 2
                        && request.request().method() == Method::CaptureReplaceConfiguration
                    {
                        worker_expired.store(true, Ordering::Release);
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "fake-clock-config".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    client.configure(wire_bindings(), true).unwrap();
    let origin = Instant::now();
    client.replace_clock(Box::new(ExpiringClock {
        origin,
        expired: Arc::clone(&expired),
    }));
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    assert!(matches!(
        client.configure_until(
            wire_bindings(),
            true,
            origin + Duration::from_millis(100),
            &cancelled,
        ),
        Err(OwnerClientError::Uncertain)
    ));
    assert!(!client.enabled());
    drop(client);
    worker.join().unwrap();
    let methods = seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    let final_replace = methods
        .iter()
        .rposition(|method| *method == Method::CaptureReplaceConfiguration)
        .unwrap();
    assert_eq!(methods[final_replace - 1], Method::CaptureSetEnabled);
    assert!(matches!(
        seen.lock().unwrap()[final_replace - 1],
        Request::CaptureSetEnabled(ref value) if !value.enabled
    ));
    assert!(!methods[final_replace + 1..].contains(&Method::CaptureSetEnabled));
}

struct GatedClock {
    origin: Instant,
    elapsed_ms: Arc<AtomicU64>,
    advances: Arc<std::sync::atomic::AtomicBool>,
}

impl OwnerClock for GatedClock {
    fn now(&self) -> Instant {
        self.origin + Duration::from_millis(self.elapsed_ms.load(Ordering::Acquire))
    }

    fn sleep(&self, duration: Duration) {
        if self.advances.load(Ordering::Acquire) {
            self.elapsed_ms
                .fetch_add(duration.as_millis() as u64, Ordering::AcqRel);
        }
        thread::yield_now();
    }
}

#[test]
fn planned_retirement_wait_uses_the_callers_absolute_deadline() {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let advances = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_advances = Arc::clone(&advances);
    let worker = thread::spawn(move || {
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    worker_seen.lock().unwrap().push(request.request().clone());
                    let response = if request.request().method() == Method::OwnerExitWhenNeutral {
                        Response::Success(SuccessResult::Release(ReleaseResult {
                            disposition: LeaseDisposition::Draining,
                        }))
                    } else {
                        success_for(request.request())
                    };
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let Ok(receipt) = owner.try_send(frame) else {
                        return;
                    };
                    while owner.flush(receipt).unwrap_or(TransportProgress::Complete)
                        == TransportProgress::Pending
                    {
                        thread::yield_now();
                    }
                    if request.request().method() == Method::OwnerExitWhenNeutral {
                        worker_advances.store(true, Ordering::Release);
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "planned-retirement-clock".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    let elapsed = Arc::new(AtomicU64::new(0));
    let origin = Instant::now();
    client.replace_clock(Box::new(GatedClock {
        origin,
        elapsed_ms: Arc::clone(&elapsed),
        advances,
    }));
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    assert!(matches!(
        client.release_and_exit_when_neutral_until(origin + Duration::from_millis(25), &cancelled,),
        Err(OwnerClientError::Uncertain)
    ));
    assert!(elapsed.load(Ordering::Acquire) <= 25);
    drop(client);
    worker.join().unwrap();
    assert_eq!(
        seen.lock().unwrap().last().unwrap().method(),
        Method::OwnerExitWhenNeutral
    );
}

#[test]
fn actor_deadline_mid_configuration_revokes_without_a_later_enable() {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let gateway_control = gateway.control();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let owner_disabled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let worker_disabled = Arc::clone(&owner_disabled);
    let worker = thread::spawn(move || {
        let mut replacements = 0;
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    worker_seen.lock().unwrap().push(request.request().clone());
                    if let Request::CaptureSetEnabled(value) = request.request() {
                        worker_disabled.store(!value.enabled, Ordering::Release);
                    }
                    if request.request().method() == Method::CaptureReplaceConfiguration {
                        replacements += 1;
                        if replacements == 2 {
                            thread::sleep(Duration::from_millis(80));
                        }
                    }
                    let response = success_for(request.request());
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let Ok(receipt) = owner.try_send(frame) else {
                        return;
                    };
                    while owner.flush(receipt).unwrap_or(TransportProgress::Complete)
                        == TransportProgress::Pending
                    {
                        thread::yield_now();
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let control: Arc<dyn OwnerShutdownControl> = Arc::new(FakeEndpointShutdownControl {
        control: gateway_control,
        revoked: Arc::clone(&revoked),
    });
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(8);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(ControlledOneShot {
            owner: Some(ConnectedOwner {
                client: protocol,
                build_id: "actor-deadline-config".into(),
            }),
            control,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    backend.configure_activation(true, bindings).unwrap();
    assert!(!owner_disabled.load(Ordering::Acquire));
    assert!(
        backend
            .configure_activation_until(true, bindings, Instant::now() + Duration::from_millis(25),)
            .is_err()
    );
    worker.join().unwrap();
    assert!(revoked.load(Ordering::Acquire));
    assert!(owner_disabled.load(Ordering::Acquire));
    let methods = seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    let final_replace = methods
        .iter()
        .rposition(|method| *method == Method::CaptureReplaceConfiguration)
        .unwrap();
    assert!(!methods[final_replace + 1..].contains(&Method::CaptureSetEnabled));
    assert!(!backend.keyboard_owner().authenticated);
    let _ = backend.shutdown();
}

#[derive(Debug)]
struct TestLocalTransport(StreamOrderedTransport<TcpStream>);
impl OrderedTransport for TestLocalTransport {
    fn is_test_only(&self) -> bool {
        true
    }
    fn try_send(
        &mut self,
        frame: Vec<u8>,
    ) -> Result<talking_quill_owner_protocol::FlushReceipt, TransportError> {
        self.0.try_send(frame)
    }
    fn flush(
        &mut self,
        receipt: talking_quill_owner_protocol::FlushReceipt,
    ) -> Result<TransportProgress, TransportError> {
        self.0.flush(receipt)
    }
    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        self.0.try_receive()
    }
    fn close(&mut self) -> Result<TransportProgress, TransportError> {
        self.0.close()
    }
    fn abort(&mut self) {
        self.0.abort();
    }
}

#[test]
fn real_loopback_endpoint_runs_authenticated_protocol_codec_end_to_end() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let endpoint = TestLocalTransport(StreamOrderedTransport::new(stream).unwrap());
        run_owner(Box::new(endpoint), owner_codec, worker_seen, None);
    });
    let stream = TcpStream::connect(address).unwrap();
    stream.set_nonblocking(true).unwrap();
    let endpoint = TestLocalTransport(StreamOrderedTransport::new(stream).unwrap());
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(endpoint, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "local-loopback".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    client.configure(wire_bindings(), false).unwrap();
    assert_eq!(client.release().unwrap(), LeaseDisposition::Neutral);
    drop(client);
    worker.join().unwrap();
    assert_eq!(seen.lock().unwrap()[0].method(), Method::LeaseAcquire);
}

struct MaintenanceOneShot(Option<ConnectedOwner>);
impl OwnerConnector for MaintenanceOneShot {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        Err(ConnectError::Incompatible)
    }
    fn connect_maintenance(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.0.take().ok_or(ConnectError::Unavailable)
    }
}

#[test]
fn maintenance_uses_a_disjoint_connection_capability_and_sequence() {
    let material = FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([11; 32]),
        Bytes32::new([12; 32]),
        Purpose::Maintenance,
        [13; 32],
        [14; 32],
    );
    let (gateway_codec, owner_codec) = material.codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || run_owner(Box::new(owner), owner_codec, worker_seen, None));
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = MaintenanceOneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "previous-local-build".into(),
    }));
    let transaction = Bytes32::new([15; 32]);
    let acquire = MaintenanceAcquireParams::Update {
        transaction_id: transaction,
        source_build_digest: Bytes32::new([16; 32]),
        target_build_digest: Bytes32::new([17; 32]),
        target_owner_sha256: Bytes32::new([18; 32]),
    };
    let mut client = OwnerMaintenanceClient::acquire(&mut connector, acquire).unwrap();
    client.renew().unwrap();
    client
        .prepare(transaction, MaintenanceOperation::Update)
        .unwrap();
    drop(client);
    worker.join().unwrap();
    let requests = seen.lock().unwrap();
    assert_eq!(
        requests.iter().map(Request::method).collect::<Vec<_>>(),
        [
            Method::MaintenanceAcquire,
            Method::MaintenanceRenew,
            Method::MaintenancePrepare,
        ]
    );
    let sequences: Vec<_> = requests
        .iter()
        .filter_map(|request| match request {
            Request::MaintenanceRenew(v) => Some(v.command_sequence.get()),
            Request::MaintenancePrepare(v) => Some(v.command_sequence.get()),
            _ => None,
        })
        .collect();
    assert_eq!(sequences, [1, 2]);
}

fn run_expiring_slow_owner(
    mut endpoint: Box<dyn OrderedTransport>,
    mut codec: OwnerSessionCodec,
    seen: Arc<Mutex<Vec<Request>>>,
    slow: Arc<std::sync::atomic::AtomicBool>,
) {
    let heartbeat = Duration::from_secs(5);
    let mut deadline: Option<Instant> = None;
    loop {
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => {
                if deadline.is_some_and(|value| Instant::now() >= value) {
                    endpoint.abort();
                    return;
                }
                thread::yield_now();
            }
            ReceiveResult::PeerClosed => return,
            ReceiveResult::Frame(frame) => {
                let request = codec.receive_request(&frame).unwrap();
                if deadline.is_some_and(|value| Instant::now() >= value) {
                    endpoint.abort();
                    return;
                }
                seen.lock().unwrap().push(request.request().clone());
                if slow.load(Ordering::Acquire)
                    && matches!(
                        request.request(),
                        Request::CaptureReplaceConfiguration(_) | Request::CaptureSetEnabled(_)
                    )
                {
                    thread::sleep(Duration::from_millis(1_800));
                }
                if matches!(
                    request.request(),
                    Request::LeaseAcquire(_) | Request::LeaseRenew(_)
                ) {
                    deadline = Some(Instant::now() + heartbeat);
                }
                let response = success_for(request.request());
                let frame = codec.encode_response(&request, &response).unwrap();
                let Ok(receipt) = endpoint.try_send(frame) else {
                    return;
                };
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    thread::yield_now();
                }
            }
        }
    }
}

#[test]
fn service_event_budget_returns_control_during_a_continuous_flood() {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let (flood_ready_tx, flood_ready_rx) = crossbeam_channel::bounded(1);
    let (release_tx, release_rx) = crossbeam_channel::bounded(1);
    let worker = thread::spawn(move || {
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    let reconciled = request.request().method() == Method::SessionReconcileOff;
                    let response = success_for(request.request());
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let receipt = owner.try_send(frame).unwrap();
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                    if reconciled {
                        for _ in 0..1_000 {
                            let frame = owner_codec
                                .encode_event(&Event::HealthChanged(health()))
                                .unwrap();
                            loop {
                                match owner.try_send(frame.clone()) {
                                    Ok(receipt) => {
                                        while owner.flush(receipt).unwrap()
                                            == TransportProgress::Pending
                                        {
                                            thread::yield_now();
                                        }
                                        break;
                                    }
                                    Err(TransportError::QueueFull) => thread::yield_now(),
                                    Err(_) => return,
                                }
                            }
                        }
                        let _ = flood_ready_tx.send(());
                        let _ = release_rx.recv();
                        return;
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "service-event-flood".into(),
    }));
    let mut client = OwnerCaptureClient::connect(&mut connector, |_| {
        thread::sleep(Duration::from_millis(1));
        OwnerEventDisposition::Continue
    })
    .unwrap();
    flood_ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    client
        .service_until(
            Instant::now(),
            Instant::now() + Duration::from_secs(2),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    drop(client);
    release_tx.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn continuous_unsolicited_events_cannot_extend_a_call_past_its_deadline() {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let worker = thread::spawn(move || {
        let request = loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => break owner_codec.receive_request(&frame).unwrap(),
            }
        };
        let started = Instant::now();
        let event = Event::HealthChanged(health());
        while started.elapsed() < Duration::from_millis(2_300) {
            let frame = owner_codec.encode_event(&event).unwrap();
            match owner.try_send(frame) {
                Ok(receipt) => {
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                }
                Err(TransportError::QueueFull) => thread::yield_now(),
                Err(_) => return,
            }
        }
        let frame = owner_codec
            .encode_response(&request, &success_for(request.request()))
            .unwrap();
        if let Ok(receipt) = owner.try_send(frame) {
            while owner.flush(receipt).unwrap_or(TransportProgress::Complete)
                == TransportProgress::Pending
            {
                thread::yield_now();
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "event-flood-deadline".into(),
    }));
    let started = Instant::now();
    let result = OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue);
    assert!(matches!(result, Err(OwnerClientError::Uncertain)));
    assert!(started.elapsed() < Duration::from_millis(2_250));
    worker.join().unwrap();
}

#[test]
fn slow_reconfiguration_and_concurrent_health_keep_renewals_inside_owner_expiry() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let slow = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let owner_worker = thread::spawn({
        let seen = Arc::clone(&seen);
        let slow = Arc::clone(&slow);
        move || run_expiring_slow_owner(Box::new(owner), owner_codec, seen, slow)
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(8);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "slow-owner-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    backend.configure_activation(true, bindings).unwrap();
    slow.store(true, Ordering::Release);
    thread::sleep(Duration::from_millis(1_100));

    thread::scope(|scope| {
        let health = scope.spawn(|| backend.hook_status());
        backend.configure_activation(true, bindings).unwrap();
        assert_eq!(
            health.join().unwrap(),
            talking_quill_helper::gateway::HookStatus::InstalledUnobserved
        );
    });
    assert!(backend.keyboard_owner().authenticated);

    let methods = seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    let slow_start = methods
        .iter()
        .rposition(|method| *method == Method::CaptureReplaceConfiguration)
        .unwrap()
        .saturating_sub(4);
    let slow_tail = &methods[slow_start..];
    assert!(
        slow_tail
            .windows(2)
            .filter(|window| {
                matches!(
                    window[1],
                    Method::CaptureSetEnabled | Method::CaptureReplaceConfiguration
                )
            })
            .all(|window| window[0] == Method::LeaseRenew),
        "slow capability operations were not fenced by renewals: {slow_tail:?}"
    );
    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    owner_worker.join().unwrap();
}

#[test]
fn background_service_renews_without_reusing_a_command_sequence() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || run_owner(Box::new(owner), owner_codec, worker_seen, None));
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "local-build".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    client
        .service_until(
            Instant::now() + Duration::from_secs(2),
            Instant::now() + Duration::from_secs(2),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(client.release().unwrap(), LeaseDisposition::Neutral);
    drop(client);
    worker.join().unwrap();
    let sequences: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(command_sequence)
        .collect();
    assert_eq!(sequences, [1, 2, 3]);
}

struct QueueConnector(VecDeque<ConnectedOwner>);
impl OwnerConnector for QueueConnector {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.0.pop_front().ok_or(ConnectError::Unavailable)
    }
}

#[test]
fn gateway_worker_reconnects_with_backoff_and_replays_only_full_disabled_first_state() {
    let mut connections = VecDeque::new();
    let mut workers = Vec::new();
    let first_seen = Arc::new(Mutex::new(Vec::new()));
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    for (index, seen) in [Arc::clone(&first_seen), Arc::clone(&second_seen)]
        .into_iter()
        .enumerate()
    {
        let material = FakeAuthenticatedMaterial::new_with_owner_instance(
            Bytes32::new([20 + index as u8; 32]),
            Bytes32::new([2; 32]),
            Purpose::Capture,
            [30 + index as u8; 32],
            [40 + index as u8; 32],
        );
        let (gateway_codec, owner_codec) = material.codecs().unwrap();
        let (gateway, owner) = fake_ordered_transport_pair();
        workers.push(thread::spawn(move || {
            if index == 0 {
                run_owner_stopping_after_response(
                    Box::new(owner),
                    owner_codec,
                    seen,
                    Method::HealthGet,
                    2,
                );
            } else {
                run_owner(Box::new(owner), owner_codec, seen, None);
            }
        }));
        let client =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        connections.push_back(ConnectedOwner {
            client,
            build_id: "same-owner-build".into(),
        });
    }
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(32);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(QueueConnector(connections)),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    backend.configure_activation(true, bindings).unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    while second_seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method() != Method::CaptureSetEnabled)
        || !backend.keyboard_owner().authenticated
    {
        assert!(
            Instant::now() < deadline,
            "reconciliation did not reach replacement owner"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(backend.keyboard_owner().authenticated);
    let second = second_seen.lock().unwrap();
    assert_eq!(
        second
            .iter()
            .map(Request::method)
            .take(7)
            .collect::<Vec<_>>(),
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::CaptureReplaceConfiguration,
            Method::CaptureSetEnabled,
            Method::SessionSetMode,
        ]
    );
    drop(second);
    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn expired_retained_reconciliation_is_not_retransmitted_to_a_third_connection() {
    let mut connections = VecDeque::new();
    let mut workers = Vec::new();
    let first_seen = Arc::new(Mutex::new(Vec::new()));
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    let third_seen = Arc::new(Mutex::new(Vec::new()));

    for (index, seen) in [
        Arc::clone(&first_seen),
        Arc::clone(&second_seen),
        Arc::clone(&third_seen),
    ]
    .into_iter()
    .enumerate()
    {
        let material = FakeAuthenticatedMaterial::new_with_owner_instance(
            Bytes32::new([100 + index as u8; 32]),
            Bytes32::new([2; 32]),
            Purpose::Capture,
            [110 + index as u8; 32],
            [120 + index as u8; 32],
        );
        let (gateway_codec, mut owner_codec) = material.codecs().unwrap();
        let (gateway, mut owner) = fake_ordered_transport_pair();
        let worker_seen = Arc::clone(&seen);
        workers.push(thread::spawn(move || {
            let mut health_requests = 0;
            loop {
                match owner.try_receive() {
                    ReceiveResult::Empty => thread::yield_now(),
                    ReceiveResult::PeerClosed => return,
                    ReceiveResult::Frame(frame) => {
                        let request = owner_codec.receive_request(&frame).unwrap();
                        worker_seen.lock().unwrap().push(request.request().clone());
                        if request.request().method() == Method::HealthGet {
                            health_requests += 1;
                        }
                        if index == 1
                            && request.request().method() == Method::CaptureReplaceConfiguration
                        {
                            thread::sleep(Duration::from_millis(700));
                        }
                        let response = success_for(request.request());
                        let frame = owner_codec.encode_response(&request, &response).unwrap();
                        let Ok(receipt) = owner.try_send(frame) else {
                            return;
                        };
                        while owner.flush(receipt).unwrap_or(TransportProgress::Complete)
                            == TransportProgress::Pending
                        {
                            thread::yield_now();
                        }
                        if index == 0 && health_requests == 2 {
                            owner.abort();
                            return;
                        }
                    }
                }
            }
        }));
        let client =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        connections.push_back(ConnectedOwner {
            client,
            build_id: "retained-deadline-build".into(),
        });
    }

    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(16);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(QueueConnector(connections)),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    let reconcile_deadline = Instant::now() + Duration::from_millis(500);
    backend
        .configure_activation_until(true, bindings, reconcile_deadline)
        .unwrap();

    let expiry_wait = Instant::now() + Duration::from_secs(2);
    while second_seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method() != Method::CaptureReplaceConfiguration)
    {
        assert!(
            Instant::now() < expiry_wait,
            "retained reconciliation did not reach the replacement"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!backend.keyboard_owner().authenticated);
    assert!(!backend.keyboard_capture_available());

    // Queue a fresh command while the retained-state reconciliation is still
    // blocked. The actor must invalidate the old expired budget before this
    // command can install a third connection.
    assert_eq!(backend.hook_status(), HookStatus::InstalledUnobserved);
    let second_methods = second_seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    let second_replace = second_methods
        .iter()
        .rposition(|method| *method == Method::CaptureReplaceConfiguration)
        .unwrap();
    assert!(!second_methods[second_replace + 1..].contains(&Method::CaptureSetEnabled));
    assert!(backend.keyboard_owner().authenticated);
    let third_methods = third_seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    assert_eq!(
        third_methods,
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::HealthGet,
        ],
        "expired desired configuration was retransmitted: {third_methods:?}"
    );
    assert!(!third_methods.contains(&Method::CaptureSetEnabled));

    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn expired_background_renewal_clears_enabled_replay_before_third_connection() {
    let mut connections = VecDeque::new();
    let mut workers = Vec::new();
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    let third_seen = Arc::new(Mutex::new(Vec::new()));

    for index in 0..3 {
        let material = FakeAuthenticatedMaterial::new_with_owner_instance(
            Bytes32::new([130 + index as u8; 32]),
            Bytes32::new([2; 32]),
            Purpose::Capture,
            [140 + index as u8; 32],
            [150 + index as u8; 32],
        );
        let (gateway_codec, mut owner_codec) = material.codecs().unwrap();
        let (gateway, mut owner) = fake_ordered_transport_pair();
        let seen = if index == 1 {
            Arc::clone(&second_seen)
        } else if index == 2 {
            Arc::clone(&third_seen)
        } else {
            Arc::new(Mutex::new(Vec::new()))
        };
        workers.push(thread::spawn(move || {
            let mut health_requests = 0;
            loop {
                match owner.try_receive() {
                    ReceiveResult::Empty => thread::yield_now(),
                    ReceiveResult::PeerClosed => return,
                    ReceiveResult::Frame(frame) => {
                        let request = owner_codec.receive_request(&frame).unwrap();
                        seen.lock().unwrap().push(request.request().clone());
                        if request.request().method() == Method::HealthGet {
                            health_requests += 1;
                        }
                        if index == 1 && request.request().method() == Method::LeaseRenew {
                            thread::sleep(Duration::from_millis(2_300));
                        }
                        let response = success_for(request.request());
                        let frame = owner_codec.encode_response(&request, &response).unwrap();
                        let Ok(receipt) = owner.try_send(frame) else {
                            return;
                        };
                        while owner.flush(receipt).unwrap_or(TransportProgress::Complete)
                            == TransportProgress::Pending
                        {
                            thread::yield_now();
                        }
                        if index == 0 && health_requests == 2 {
                            owner.abort();
                            return;
                        }
                    }
                }
            }
        }));
        let client =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        connections.push_back(ConnectedOwner {
            client,
            build_id: "renewal-deadline-build".into(),
        });
    }

    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(16);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(QueueConnector(connections)),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    backend.configure_activation(true, bindings).unwrap();

    let renewal_wait = Instant::now() + Duration::from_secs(5);
    while second_seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method() != Method::LeaseRenew)
    {
        assert!(
            Instant::now() < renewal_wait,
            "background renewal was not sent"
        );
        thread::sleep(Duration::from_millis(10));
    }
    while !backend.keyboard_owner().authenticated {
        assert!(
            Instant::now() < renewal_wait,
            "replacement owner was never published"
        );
        thread::sleep(Duration::from_millis(10));
    }
    while backend.keyboard_owner().authenticated {
        assert!(
            Instant::now() < renewal_wait,
            "expired renewal did not revoke capture"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!backend.keyboard_capture_available());
    let second_methods = second_seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    assert!(second_methods.contains(&Method::CaptureReplaceConfiguration));
    assert!(second_methods.contains(&Method::CaptureSetEnabled));

    assert_eq!(backend.hook_status(), HookStatus::InstalledUnobserved);
    assert!(backend.keyboard_owner().authenticated);
    let third_methods = third_seen
        .lock()
        .unwrap()
        .iter()
        .map(Request::method)
        .collect::<Vec<_>>();
    assert_eq!(
        third_methods,
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::HealthGet,
        ],
        "expired enabled state replayed after renewal failure: {third_methods:?}"
    );
    assert!(!third_methods.contains(&Method::CaptureSetEnabled));

    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn personal_gateway_retries_with_backoff_after_authenticated_owner_is_lost() {
    let material = FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([60; 32]),
        Bytes32::new([2; 32]),
        Purpose::Capture,
        [61; 32],
        [62; 32],
    );
    let (gateway_codec, owner_codec) = material.codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker = thread::spawn(move || {
        run_owner_stopping_after_response(
            Box::new(owner),
            owner_codec,
            seen,
            Method::SessionReconcileOff,
            1,
        )
    });
    let client =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(32);
    let admission = Arc::new(CallbackGate::new());
    admission.open();
    let (terminal_sender, terminal_events) = crossbeam_channel::bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&admission), terminal_sender));
    let attempts = Arc::new(AtomicU64::new(0));
    struct CountedOneShot {
        connection: Option<ConnectedOwner>,
        attempts: Arc<AtomicU64>,
    }
    impl OwnerConnector for CountedOneShot {
        fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
            self.attempts.fetch_add(1, Ordering::AcqRel);
            self.connection.take().ok_or(ConnectError::Unavailable)
        }
    }
    let mut backend = OwnerGatewayBackend::connect_with_terminal(
        Box::new(CountedOneShot {
            connection: Some(ConnectedOwner {
                client,
                build_id: "one-shot-owner-build".into(),
            }),
            attempts: Arc::clone(&attempts),
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
        admission,
        Arc::clone(&terminal),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    while attempts.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(attempts.load(Ordering::Acquire) >= 2);
    assert!(terminal_events.try_recv().is_err());
    assert!(!terminal.is_triggered());
    assert!(!backend.keyboard_owner().authenticated);
    let shutdown = backend.shutdown();
    assert_eq!(shutdown.terminal_reason, None);
    assert!(!shutdown.observability_quiescent);
    worker.join().unwrap();
}

#[test]
fn runtime_rollback_is_propagated_as_a_one_way_owner_mutation() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || run_owner(Box::new(owner), owner_codec, worker_seen, None));
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "local-build".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    assert_eq!(
        client.runtime_rollback().unwrap(),
        LeaseDisposition::Neutral
    );
    drop(client);
    worker.join().unwrap();
    let requests = seen.lock().unwrap();
    assert_eq!(
        requests.iter().map(Request::method).collect::<Vec<_>>(),
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::RuntimeRollback,
        ]
    );
    assert_eq!(
        requests
            .iter()
            .filter_map(command_sequence)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

struct ControlledClock {
    origin: Instant,
    elapsed_ms: Arc<AtomicU64>,
}

impl MaintenanceClock for ControlledClock {
    fn now(&self) -> Instant {
        self.origin + Duration::from_millis(self.elapsed_ms.load(Ordering::Acquire))
    }

    fn sleep(&self, duration: Duration) {
        self.elapsed_ms
            .fetch_add(duration.as_millis() as u64, Ordering::AcqRel);
        thread::yield_now();
    }
}

#[test]
fn maintenance_renews_past_five_second_expiry_window_before_prepare() {
    let material = FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([51; 32]),
        Bytes32::new([52; 32]),
        Purpose::Maintenance,
        [53; 32],
        [54; 32],
    );
    let (gateway_codec, mut owner_codec) = material.codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        let mut renewals = 0;
        let mut pending_prepare = None;
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    worker_seen.lock().unwrap().push(request.request().clone());
                    if request.request().method() == Method::MaintenancePrepare {
                        pending_prepare = Some(request);
                        continue;
                    }
                    let response = if request.request().method() == Method::MaintenanceAcquire {
                        Response::Success(SuccessResult::MaintenanceAcquire(
                            MaintenanceAcquireResult {
                                maintenance_capability_id: Bytes32::new([55; 32]),
                                maintenance_capability_epoch: U64String::try_from(3).unwrap(),
                                state: MaintenanceAcquireState::Draining,
                            },
                        ))
                    } else {
                        renewals += 1;
                        success_for(request.request())
                    };
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let Ok(receipt) = owner.try_send(frame) else {
                        return;
                    };
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                    if renewals >= 6
                        && let Some(prepare) = pending_prepare.take()
                    {
                        let response = success_for(prepare.request());
                        let frame = owner_codec.encode_response(&prepare, &response).unwrap();
                        let Ok(receipt) = owner.try_send(frame) else {
                            return;
                        };
                        while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                            thread::yield_now();
                        }
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = MaintenanceOneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "previous-build".into(),
    }));
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let transaction = Bytes32::new([56; 32]);
    let mut client = OwnerMaintenanceClient::acquire_with_clock(
        &mut connector,
        MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([57; 32]),
        },
        Box::new(ControlledClock {
            origin: Instant::now(),
            elapsed_ms: Arc::clone(&elapsed_ms),
        }),
    )
    .unwrap();
    if let Err(error) = client.prepare(transaction, MaintenanceOperation::Uninstall) {
        drop(client);
        worker.join().unwrap();
        panic!(
            "prepare failed: {error:?}; requests: {:?}",
            seen.lock().unwrap()
        );
    }
    assert!(elapsed_ms.load(Ordering::Acquire) >= 6_000);
    drop(client);
    worker.join().unwrap();

    let requests = seen.lock().unwrap();
    let sequences = requests
        .iter()
        .filter_map(|request| match request {
            Request::MaintenanceRenew(value) => Some(value.command_sequence.get()),
            Request::MaintenancePrepare(value) => Some(value.command_sequence.get()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(sequences.len() >= 6);
    assert_eq!(
        sequences,
        (1..=u64::try_from(sequences.len()).unwrap()).collect::<Vec<_>>()
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method() == Method::MaintenancePrepare)
            .count(),
        1
    );
}

struct UnavailableConnector;
impl OwnerConnector for UnavailableConnector {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        Err(ConnectError::Unavailable)
    }
}

struct CountedFailureConnector {
    attempts: Arc<AtomicU64>,
    collision_after: Option<u64>,
}

impl OwnerConnector for CountedFailureConnector {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        let attempt = self.attempts.fetch_add(1, Ordering::AcqRel) + 1;
        if self.collision_after == Some(attempt) {
            Err(ConnectError::SingletonCollision)
        } else {
            Err(ConnectError::Unavailable)
        }
    }
}

#[test]
fn initial_singleton_collision_stays_failed_closed_and_retries_without_gateway_churn() {
    let attempts = Arc::new(AtomicU64::new(0));
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(CountedFailureConnector {
            attempts: Arc::clone(&attempts),
            collision_after: Some(1),
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while attempts.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(attempts.load(Ordering::Acquire) >= 2);
    assert!(!backend.keyboard_capture_available());
    let _ = backend.shutdown();
}

#[test]
fn singleton_collision_during_transient_retries_keeps_waiting_without_terminalizing() {
    let attempts = Arc::new(AtomicU64::new(0));
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let admission = Arc::new(CallbackGate::new());
    admission.open();
    let (terminal_sender, terminal_events) = crossbeam_channel::bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&admission), terminal_sender));
    let mut backend = OwnerGatewayBackend::connect_with_terminal(
        Box::new(CountedFailureConnector {
            attempts: Arc::clone(&attempts),
            collision_after: Some(2),
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
        admission,
        terminal,
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(1);
    while attempts.load(Ordering::Acquire) < 3 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(attempts.load(Ordering::Acquire) >= 3);
    assert!(terminal_events.try_recv().is_err());
    assert!(!backend.keyboard_capture_available());
    let _ = backend.shutdown();
}

#[test]
fn ordinary_unavailable_owner_failure_keeps_retrying() {
    let attempts = Arc::new(AtomicU64::new(0));
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(CountedFailureConnector {
            attempts: Arc::clone(&attempts),
            collision_after: None,
        }),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(1);
    while attempts.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(attempts.load(Ordering::Acquire) >= 2);
    let _ = backend.shutdown();
}

#[test]
fn unavailable_owner_accepts_safe_disabled_reconciliation_without_gateway_restart() {
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(UnavailableConnector),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();

    assert!(
        backend
            .configure_activation(false, ActivationBindings::default())
            .is_ok()
    );
    assert!(
        backend
            .set_session_capture(talking_quill_keyboard_core::SessionCaptureMode::Off)
            .is_ok()
    );
    assert!(
        backend
            .configure_activation(true, ActivationBindings::default())
            .is_err()
    );
    assert!(
        backend
            .set_session_capture(talking_quill_keyboard_core::SessionCaptureMode::Recording)
            .is_err()
    );
    let _ = backend.shutdown();
}

#[test]
fn shutdown_without_ever_acquiring_is_quiescent_and_conservatively_draining() {
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(UnavailableConnector),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let shutdown = backend.shutdown();
    assert!(shutdown.terminal_reason.is_none());
    assert!(shutdown.observability_quiescent);
    assert_eq!(
        backend.shutdown_owner_disposition(),
        talking_quill_helper::gateway::ShutdownOwnerDisposition::Draining
    );
}

struct FailingSpawner;
impl OwnerWorkerSpawner for FailingSpawner {
    fn spawn(
        &self,
        _worker: Box<dyn FnOnce() + Send + 'static>,
    ) -> std::io::Result<thread::JoinHandle<()>> {
        Err(std::io::Error::other("injected spawn failure"))
    }
}

#[test]
fn owner_actor_spawn_failure_fails_before_acquiring_a_lease() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || run_owner(Box::new(owner), owner_codec, worker_seen, None));
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let result = OwnerGatewayBackend::connect_with_spawner(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "local-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
        &FailingSpawner,
    );
    assert!(matches!(
        result,
        Err(talking_quill_helper::gateway::PlatformError::OwnerUnavailable)
    ));
    worker.join().unwrap();
    assert!(seen.lock().unwrap().is_empty());
}

fn run_owner_with_health(
    mut endpoint: Box<dyn OrderedTransport>,
    mut codec: OwnerSessionCodec,
    seen: Arc<Mutex<Vec<Request>>>,
    health_value: HealthResult,
    stop_after: Option<Method>,
) {
    loop {
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => thread::yield_now(),
            ReceiveResult::PeerClosed => return,
            ReceiveResult::Frame(frame) => {
                let request = codec.receive_request(&frame).unwrap();
                seen.lock().unwrap().push(request.request().clone());
                if stop_after == Some(request.request().method()) {
                    endpoint.abort();
                    return;
                }
                let response = if matches!(request.request(), Request::HealthGet(_)) {
                    Response::Success(SuccessResult::Health(health_value.clone()))
                } else {
                    success_for(request.request())
                };
                let frame = codec.encode_response(&request, &response).unwrap();
                let receipt = endpoint.try_send(frame).unwrap();
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    thread::yield_now();
                }
            }
        }
    }
}

fn backend_with_health(value: HealthResult) -> (OwnerGatewayBackend, thread::JoinHandle<()>) {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let worker = thread::spawn(move || {
        run_owner_with_health(
            Box::new(owner),
            owner_codec,
            Arc::new(Mutex::new(Vec::new())),
            value,
            None,
        )
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(8);
    let backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "state-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    (backend, worker)
}

#[test]
fn authoritative_maintenance_and_degraded_health_clear_hook_availability() {
    for (reported_state, process_state, expected) in [
        (
            OwnerReportedState::MaintenanceDraining,
            ProcessState::Healthy,
            talking_quill_helper::gateway::KeyboardOwnerState::Maintenance,
        ),
        (
            OwnerReportedState::DegradedDraining,
            ProcessState::Degraded,
            talking_quill_helper::gateway::KeyboardOwnerState::Degraded,
        ),
    ] {
        let mut value = health();
        value.reported_state = reported_state;
        value.process_state = process_state;
        value.hook_healthy = true;
        let (mut backend, worker) = backend_with_health(value);
        assert_eq!(backend.keyboard_owner().state, expected);
        assert_eq!(
            backend.hook_status(),
            talking_quill_helper::gateway::HookStatus::Unavailable
        );
        assert!(!backend.keyboard_capture_available());
        assert!(backend.shutdown().terminal_reason.is_none());
        drop(backend);
        worker.join().unwrap();
    }
}

#[test]
fn disconnect_clears_authoritative_hook_and_uncertain_held_lease_suppresses_shutdown() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        run_owner(
            Box::new(owner),
            owner_codec,
            worker_seen,
            Some(Method::LeaseRenew),
        )
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(8);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "disconnect-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while backend.keyboard_owner().authenticated {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        backend.hook_status(),
        talking_quill_helper::gateway::HookStatus::Unavailable
    );
    assert!(!backend.keyboard_capture_available());
    let shutdown = backend.shutdown();
    assert!(shutdown.terminal_reason.is_none());
    assert!(!shutdown.observability_quiescent);
    drop(backend);
    worker.join().unwrap();
}

#[test]
fn uncertain_config_is_not_replayed_and_reconnect_accepts_fresh_desired_state() {
    let mut connections = VecDeque::new();
    let mut workers = Vec::new();
    let first_seen = Arc::new(Mutex::new(Vec::new()));
    let second_seen = Arc::new(Mutex::new(Vec::new()));
    for (index, seen) in [Arc::clone(&first_seen), Arc::clone(&second_seen)]
        .into_iter()
        .enumerate()
    {
        let material = FakeAuthenticatedMaterial::new_with_owner_instance(
            Bytes32::new([70 + index as u8; 32]),
            Bytes32::new([2; 32]),
            Purpose::Capture,
            [80 + index as u8; 32],
            [90 + index as u8; 32],
        );
        let (gateway_codec, owner_codec) = material.codecs().unwrap();
        let (gateway, owner) = fake_ordered_transport_pair();
        workers.push(thread::spawn(move || {
            run_owner(
                Box::new(owner),
                owner_codec,
                seen,
                (index == 0).then_some(Method::CaptureReplaceConfiguration),
            )
        }));
        let client =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        connections.push_back(ConnectedOwner {
            client,
            build_id: "same-build".into(),
        });
    }
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(16);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(QueueConnector(connections)),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let bindings = ActivationBindings::new(&[ActivationBinding::new(
        CoreProfileId::GENERAL,
        Shortcut::legacy_alt_letter(ActivationKey::X, false),
    )])
    .unwrap();
    assert!(backend.configure_activation(true, bindings).is_err());

    assert_eq!(backend.hook_status(), HookStatus::InstalledUnobserved);
    assert_eq!(
        first_seen
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method() == Method::CaptureReplaceConfiguration)
            .count(),
        1
    );
    assert_eq!(
        second_seen
            .lock()
            .unwrap()
            .iter()
            .map(Request::method)
            .collect::<Vec<_>>(),
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::HealthGet,
        ],
        "the uncertain configuration must not become desired state"
    );

    backend.configure_activation(true, bindings).unwrap();
    backend
        .set_session_capture(talking_quill_keyboard_core::SessionCaptureMode::Recording)
        .unwrap();
    let second = second_seen.lock().unwrap();
    assert_eq!(
        second.iter().map(Request::method).collect::<Vec<_>>(),
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
            Method::HealthGet,
            Method::CaptureReplaceConfiguration,
            Method::CaptureSetEnabled,
            Method::HealthGet,
            Method::SessionSetMode,
        ]
    );
    assert_eq!(
        second
            .iter()
            .filter_map(command_sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    drop(second);
    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn successful_operation_clears_the_exact_preceding_failure_attribution() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        run_owner_rejecting_first_session_mode(
            Box::new(owner),
            owner_codec,
            worker_seen,
            ErrorCode::Busy,
        )
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let mut connector = OneShot(Some(ConnectedOwner {
        client: protocol,
        build_id: "diagnostic-clear-build".into(),
    }));
    let mut client =
        OwnerCaptureClient::connect(&mut connector, |_| OwnerEventDisposition::Continue).unwrap();
    assert!(matches!(
        client.set_session_mode(SessionMode::Recording),
        Err(OwnerClientError::Rejected(ErrorCode::Busy))
    ));
    let failure = client.last_failure().unwrap();
    assert_eq!(failure.operation, "session.set_mode");
    assert_eq!(failure.category, "rejected");
    client.refresh_health().unwrap();
    assert_eq!(
        client.last_failure(),
        None,
        "success retained stale failure"
    );
    client.set_session_mode(SessionMode::Recording).unwrap();
    assert_eq!(client.last_failure(), None);
    drop(client);
    worker.join().unwrap();
}

#[test]
fn transient_authenticated_session_rejections_refresh_and_reuse_the_same_owner_connection() {
    for (code, expected_error) in [
        (
            ErrorCode::Busy,
            talking_quill_helper::gateway::PlatformError::OwnerBusy,
        ),
        (
            ErrorCode::Unavailable,
            talking_quill_helper::gateway::PlatformError::OwnerUnavailable,
        ),
    ] {
        let (gateway_codec, owner_codec) = material().codecs().unwrap();
        let (gateway, owner) = fake_ordered_transport_pair();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let worker_seen = Arc::clone(&seen);
        let worker = thread::spawn(move || {
            run_owner_rejecting_first_session_mode(Box::new(owner), owner_codec, worker_seen, code)
        });
        let protocol =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(8);
        let mut backend = OwnerGatewayBackend::connect_with(
            Box::new(OneShot(Some(ConnectedOwner {
                client: protocol,
                build_id: "semantic-rejection-build".into(),
            }))),
            outbound,
            ActivationCaptureGate::open_for_test_harness(),
        )
        .unwrap();

        let before = backend.keyboard_owner();
        assert!(before.authenticated);
        assert_eq!(
            backend.set_session_capture(talking_quill_keyboard_core::SessionCaptureMode::Recording),
            Err(expected_error),
            "{code:?}"
        );
        let after_rejection = backend.keyboard_owner();
        assert!(after_rejection.authenticated, "{code:?}");
        assert_eq!(after_rejection.instance_id, before.instance_id, "{code:?}");
        assert_eq!(after_rejection.lease_epoch, before.lease_epoch, "{code:?}");

        backend
            .set_session_capture(talking_quill_keyboard_core::SessionCaptureMode::Recording)
            .unwrap();
        let requests = seen.lock().unwrap();
        assert_eq!(
            requests.iter().map(Request::method).collect::<Vec<_>>(),
            [
                Method::LeaseAcquire,
                Method::HealthGet,
                Method::PermissionsGet,
                Method::SessionReconcileOff,
                Method::SessionSetMode,
                Method::HealthGet,
                Method::SessionSetMode,
            ],
            "{code:?}"
        );
        assert_eq!(
            requests
                .iter()
                .filter_map(command_sequence)
                .collect::<Vec<_>>(),
            [1, 2, 3],
            "{code:?}"
        );
        drop(requests);
        assert!(backend.shutdown().terminal_reason.is_none(), "{code:?}");
        drop(backend);
        worker.join().unwrap();
    }
}

fn backend_rejected_at_acquire(code: ErrorCode) -> (OwnerGatewayBackend, thread::JoinHandle<()>) {
    let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let worker = thread::spawn(move || {
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    assert_eq!(request.request().method(), Method::LeaseAcquire);
                    let response = Response::Error(ErrorBody::new(code));
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let receipt = owner.try_send(frame).unwrap();
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "rejected-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    (backend, worker)
}

#[test]
fn every_semantic_acquire_rejection_remains_never_acquired_at_shutdown() {
    for code in [
        ErrorCode::Busy,
        ErrorCode::Draining,
        ErrorCode::Incompatible,
        ErrorCode::Rollback,
        ErrorCode::SecurityFault,
        ErrorCode::InvalidState,
        ErrorCode::NativeFailure,
        ErrorCode::Indeterminate,
        ErrorCode::Unavailable,
    ] {
        let (mut backend, worker) = backend_rejected_at_acquire(code);
        let shutdown = backend.shutdown();
        assert!(shutdown.terminal_reason.is_none(), "{code:?}");
        assert!(shutdown.observability_quiescent, "{code:?}");
        assert_eq!(
            backend.shutdown_owner_disposition(),
            talking_quill_helper::gateway::ShutdownOwnerDisposition::Draining,
            "{code:?}"
        );
        drop(backend);
        worker.join().unwrap();
    }
}

#[test]
fn post_grant_reconciliation_disconnect_reports_nonquiescent_draining_shutdown() {
    let (gateway_codec, owner_codec) = material().codecs().unwrap();
    let (gateway, owner) = fake_ordered_transport_pair();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker_seen = Arc::clone(&seen);
    let worker = thread::spawn(move || {
        run_owner(
            Box::new(owner),
            owner_codec,
            worker_seen,
            Some(Method::SessionReconcileOff),
        )
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "post-grant-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let shutdown = backend.shutdown();
    assert!(shutdown.terminal_reason.is_none());
    assert!(!shutdown.observability_quiescent);
    drop(backend);
    worker.join().unwrap();
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .map(Request::method)
            .collect::<Vec<_>>(),
        [
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff
        ]
    );
}

#[test]
fn owner_v1_front_app_metadata_maps_exactly_without_opaque_token_fabrication() {
    let metadata_material =
        material().with_features(talking_quill_owner_protocol::FRONT_APP_METADATA_V1);
    let (gateway_codec, mut owner_codec) = metadata_material.codecs().unwrap();
    let (gateway, mut owner) = fake_ordered_transport_pair();
    let worker = thread::spawn(move || {
        loop {
            match owner.try_receive() {
                ReceiveResult::Empty => thread::yield_now(),
                ReceiveResult::PeerClosed => return,
                ReceiveResult::Frame(frame) => {
                    let request = owner_codec.receive_request(&frame).unwrap();
                    let response = if request.request().method() == Method::FrontAppMetadataGet {
                        Response::Success(SuccessResult::FrontAppMetadata(FrontAppMetadataResult {
                            available: true,
                            process_name: Some("editor.exe".into()),
                            window_title: Some("Document".into()),
                            window_bounds: Some(FrontAppWindowBounds {
                                x: -10,
                                y: 20,
                                width: 800,
                                height: 600,
                            }),
                        }))
                    } else {
                        success_for(request.request())
                    };
                    let frame = owner_codec.encode_response(&request, &response).unwrap();
                    let receipt = owner.try_send(frame).unwrap();
                    while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                        thread::yield_now();
                    }
                }
            }
        }
    });
    let protocol =
        talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
            .unwrap();
    let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
    let mut backend = OwnerGatewayBackend::connect_with(
        Box::new(OneShot(Some(ConnectedOwner {
            client: protocol,
            build_id: "front-metadata-build".into(),
        }))),
        outbound,
        ActivationCaptureGate::open_for_test_harness(),
    )
    .unwrap();
    let front = backend.front_app().unwrap();
    assert_eq!(front.process_name, "editor.exe");
    assert_eq!(front.window_title, "Document");
    assert_eq!(front.window_bounds.unwrap().x, -10);
    assert!(backend.shutdown().terminal_reason.is_none());
    drop(backend);
    worker.join().unwrap();
}

#[test]
fn owner_v1_front_app_token_is_never_fabricated_into_v10_metadata() {
    for available in [false, true] {
        let (gateway_codec, mut owner_codec) = material().codecs().unwrap();
        let (gateway, mut owner) = fake_ordered_transport_pair();
        let worker = thread::spawn(move || {
            loop {
                match owner.try_receive() {
                    ReceiveResult::Empty => thread::yield_now(),
                    ReceiveResult::PeerClosed => return,
                    ReceiveResult::Frame(frame) => {
                        let request = owner_codec.receive_request(&frame).unwrap();
                        let response = if request.request().method() == Method::FrontAppGet {
                            Response::Success(SuccessResult::FrontApp(FrontAppResult {
                                available,
                                application_token: available
                                    .then(|| WireToken::new("opaque-target".into()).unwrap()),
                            }))
                        } else {
                            success_for(request.request())
                        };
                        let frame = owner_codec.encode_response(&request, &response).unwrap();
                        let receipt = owner.try_send(frame).unwrap();
                        while owner.flush(receipt).unwrap() == TransportProgress::Pending {
                            thread::yield_now();
                        }
                    }
                }
            }
        });
        let protocol =
            talking_quill_owner_protocol::client::OwnerProtocolClient::new(gateway, gateway_codec)
                .unwrap();
        let (outbound, _events) = crossbeam_channel::bounded::<Outbound>(4);
        let mut backend = OwnerGatewayBackend::connect_with(
            Box::new(OneShot(Some(ConnectedOwner {
                client: protocol,
                build_id: "front-build".into(),
            }))),
            outbound,
            ActivationCaptureGate::open_for_test_harness(),
        )
        .unwrap();
        let expected = talking_quill_helper::gateway::PlatformError::OwnerIncompatible;
        assert_eq!(backend.front_app().unwrap_err(), expected);
        assert!(backend.shutdown().terminal_reason.is_none());
        drop(backend);
        worker.join().unwrap();
    }
}
