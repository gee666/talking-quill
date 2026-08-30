#![cfg(debug_assertions)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

use talking_quill_helper::gateway::ActivationCaptureGate;
use talking_quill_helper::owner::client::{
    ConnectError, ConnectedOwner, OwnerCaptureClient, OwnerClientError, OwnerConnector,
    OwnerEventDisposition, OwnerMaintenanceClient,
};
use talking_quill_helper::owner::platform_client::OwnerGatewayBackend;
use talking_quill_keyboard_owner::{
    ActivationCaptureGate as OwnerCaptureGate, AdapterEvent, AdapterEventDisposition,
    AdapterEventId, BrokerEvent, CapabilityIdSource, NativeAdapter, NativeAdapterExecutor,
    NativeEffect, NativeEffectKind, NativeEffectResult, NoRuntimeSignals, OwnerProtocolServer,
    OwnerRuntime, ProcessSingleton,
};
use talking_quill_owner_protocol::client::{ClientError, ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::*;
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, FlushReceipt, GatewayMessage, OrderedTransport,
    OwnerSessionCodec, ReceiveResult, StreamOrderedTransport, TransportError, TransportProgress,
    U64String,
};

const ROLE_ENV: &str = "TALKING_QUILL_COMMON_E2E_ROLE";
const ADDRESS_ENV: &str = "TALKING_QUILL_COMMON_E2E_ADDRESS";
const OWNER_INSTANCE: [u8; 32] = [42; 32];
const FIRST_FAKE_OWNER_TAG: u8 = 51;
const REPLACEMENT_FAKE_OWNER_TAG: u8 = 52;
const EXPECTED_FAKE_OWNER_CRASH_EXIT: i32 = 86;

#[derive(Debug)]
struct TestTcpTransport(StreamOrderedTransport<TcpStream>);

impl OrderedTransport for TestTcpTransport {
    fn is_test_only(&self) -> bool {
        true
    }

    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        self.0.try_send(frame)
    }

    fn flush(&mut self, receipt: FlushReceipt) -> Result<TransportProgress, TransportError> {
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

fn material(purpose: Purpose, owner_tag: u8) -> FakeAuthenticatedMaterial {
    let purpose_tag = match purpose {
        Purpose::Capture => 1,
        Purpose::Maintenance => 2,
        Purpose::Observe => 3,
    };
    FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([owner_tag.wrapping_add(purpose_tag); 32]),
        Bytes32::new([owner_tag; 32]),
        purpose,
        [owner_tag.wrapping_add(10 + purpose_tag); 32],
        [owner_tag.wrapping_add(20 + purpose_tag); 32],
    )
}

fn free_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn connect_retry(address: SocketAddr) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(address) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("could not connect to cross-process fixture: {error}"),
        }
    }
}

fn release_authoritative_neutral(address: SocketAddr) {
    let mut control = connect_retry(address);
    control.write_all(&[4]).unwrap();
}

struct FixtureChild {
    child: Option<Child>,
    label: String,
}

impl FixtureChild {
    fn new(child: Child, label: impl Into<String>) -> Self {
        Self {
            child: Some(child),
            label: label.into(),
        }
    }

    fn wait_status(&mut self) -> (std::process::ExitStatus, String) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let status = self
                .child
                .as_mut()
                .expect("fixture child present")
                .try_wait()
                .unwrap();
            if let Some(status) = status {
                let mut stderr = String::new();
                if let Some(pipe) = self
                    .child
                    .as_mut()
                    .expect("fixture child present")
                    .stderr
                    .as_mut()
                {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                self.child.take();
                return (status, stderr);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.kill_and_reap();
        panic!("{} did not exit", self.label);
    }

    fn kill_and_reap(&mut self) -> Option<std::process::ExitStatus> {
        let mut child = self.child.take()?;
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        let _ = child.kill();
        child.wait().ok()
    }

    fn wait_success(mut self) {
        let (status, stderr) = self.wait_status();
        assert!(
            status.success(),
            "{} failed with {status}: {stderr}",
            self.label
        );
    }

    fn wait_exit_code(mut self, expected: i32) {
        let (status, stderr) = self.wait_status();
        assert_eq!(
            status.code(),
            Some(expected),
            "unexpected {} status: {status}; {stderr}",
            self.label
        );
    }

    fn terminate(mut self) {
        let status = self.kill_and_reap().expect("fixture child present");
        assert!(
            !status.success(),
            "terminated {} unexpectedly succeeded",
            self.label
        );
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.kill_and_reap();
    }
}

fn spawn_role(role: &str, address: SocketAddr) -> FixtureChild {
    FixtureChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common_cross_process_foundation", "--nocapture"])
            .env(ROLE_ENV, role)
            .env(ADDRESS_ENV, address.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        role,
    )
}

fn spawn_gateway_role(
    role: &str,
    gateway_address: SocketAddr,
    owner_address: SocketAddr,
) -> FixtureChild {
    FixtureChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common_cross_process_foundation", "--nocapture"])
            .env(ROLE_ENV, role)
            .env(ADDRESS_ENV, gateway_address.to_string())
            .env(
                "TALKING_QUILL_COMMON_E2E_OWNER_ADDRESS",
                owner_address.to_string(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        role,
    )
}

fn address_from_env() -> SocketAddr {
    std::env::var(ADDRESS_ENV).unwrap().parse().unwrap()
}

/// Test-only connector using `FakeAuthenticatedMaterial`; it does not exercise
/// Windows/macOS production peer, code-identity, or credential authentication.
struct TcpConnector {
    address: SocketAddr,
    owner_tag: u8,
    rotate_capture_identity: bool,
    capture_attempt: u8,
}

impl TcpConnector {
    fn fixed(address: SocketAddr, owner_tag: u8) -> Self {
        Self {
            address,
            owner_tag,
            rotate_capture_identity: false,
            capture_attempt: 0,
        }
    }

    fn rotating_fake_owners(address: SocketAddr) -> Self {
        Self {
            address,
            owner_tag: FIRST_FAKE_OWNER_TAG,
            rotate_capture_identity: true,
            capture_attempt: 0,
        }
    }

    fn connected(&self, purpose: Purpose, owner_tag: u8) -> Result<ConnectedOwner, ConnectError> {
        const CONNECT_BUDGET: Duration = Duration::from_millis(25);
        let mut stream = TcpStream::connect_timeout(&self.address, CONNECT_BUDGET)
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .set_write_timeout(Some(CONNECT_BUDGET))
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .write_all(&[match purpose {
                Purpose::Capture => 1,
                Purpose::Maintenance => 2,
                Purpose::Observe => 3,
            }])
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .set_nonblocking(true)
            .map_err(|_| ConnectError::Unavailable)?;
        let (gateway, _) = material(purpose, owner_tag)
            .codecs()
            .map_err(|_| ConnectError::Authentication)?;
        Ok(ConnectedOwner {
            client: OwnerProtocolClient::new(
                TestTcpTransport(
                    StreamOrderedTransport::new(stream).map_err(|_| ConnectError::Unavailable)?,
                ),
                gateway,
            )
            .map_err(|_| ConnectError::Authentication)?,
            build_id: "common-e2e-build".into(),
        })
    }
}

impl OwnerConnector for TcpConnector {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        let owner_tag = if self.rotate_capture_identity {
            self.owner_tag
                .checked_add(self.capture_attempt)
                .ok_or(ConnectError::Unavailable)?
        } else {
            self.owner_tag
        };
        let connected = self.connected(Purpose::Capture, owner_tag);
        if connected.is_ok() && self.rotate_capture_identity {
            self.capture_attempt = self
                .capture_attempt
                .checked_add(1)
                .ok_or(ConnectError::Unavailable)?;
        }
        connected
    }

    fn connect_maintenance(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.connected(Purpose::Maintenance, self.owner_tag)
    }
}

fn fake_owner_response(request: &Request, owner_tag: u8, lease_epoch: u64) -> Response {
    let result = match request {
        Request::LeaseAcquire(_) => SuccessResult::LeaseAcquire(LeaseAcquireResult {
            capture_lease_id: Bytes32::new([owner_tag.wrapping_add(40); 32]),
            capture_lease_epoch: U64String::try_from(lease_epoch).unwrap(),
            state: AcquireState::Disabled,
        }),
        Request::HealthGet(_) => SuccessResult::Health(HealthResult {
            owner_instance_id: Bytes32::new([owner_tag; 32]),
            reported_state: OwnerReportedState::LeaseDisabled,
            process_state: ProcessState::Healthy,
            rollback_latched: false,
            native_state_unknown: false,
            maintenance_sealed: false,
            keyboard_build_eligible: true,
            paste_ready: true,
            permissions_eligible: true,
            hook_healthy: true,
        }),
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
        Request::LeaseRelease(_) => SuccessResult::Release(ReleaseResult {
            disposition: LeaseDisposition::Draining,
        }),
        Request::OwnerExitWhenNeutral(_) => SuccessResult::Release(ReleaseResult {
            disposition: LeaseDisposition::Neutral,
        }),
        _ => return Response::Error(ErrorBody::new(ErrorCode::InvalidState)),
    };
    Response::Success(result)
}

fn serve_fake_capture(
    stream: TcpStream,
    mut codec: OwnerSessionCodec,
    owner_tag: u8,
    lease_epoch: u64,
    crash_process_on_enable: bool,
) -> Vec<Request> {
    stream.set_nonblocking(true).unwrap();
    let mut endpoint = TestTcpTransport(StreamOrderedTransport::new(stream).unwrap());
    let mut methods = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() >= deadline {
            return methods;
        }
        match endpoint.try_receive().unwrap() {
            ReceiveResult::Empty => std::thread::yield_now(),
            ReceiveResult::PeerClosed => return methods,
            ReceiveResult::Frame(frame) => {
                let received = codec.receive_request(&frame).unwrap();
                let method = received.request().method();
                methods.push(received.request().clone());
                if crash_process_on_enable
                    && matches!(received.request(), Request::CaptureSetEnabled(value) if value.enabled)
                {
                    // This is intentional whole-process loss. The parent starts
                    // a distinct replacement fixture with a new owner identity.
                    std::process::exit(EXPECTED_FAKE_OWNER_CRASH_EXIT);
                }
                let response = fake_owner_response(received.request(), owner_tag, lease_epoch);
                let frame = codec.encode_response(&received, &response).unwrap();
                let receipt = endpoint.try_send(frame).unwrap();
                while endpoint.flush(receipt).unwrap() == TransportProgress::Pending {
                    std::thread::yield_now();
                }
                if matches!(method, Method::LeaseRelease | Method::OwnerExitWhenNeutral) {
                    return methods;
                }
            }
        }
    }
}

fn accept_fake_capture(owner_tag: u8, lease_epoch: u64, crash_process: bool) -> Vec<Request> {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut purpose = [0_u8; 1];
    stream.read_exact(&mut purpose).unwrap();
    assert_eq!(purpose, [1]);
    let (_, codec) = material(Purpose::Capture, owner_tag).codecs().unwrap();
    serve_fake_capture(stream, codec, owner_tag, lease_epoch, crash_process)
}

fn run_crashing_fake_owner_process() {
    let _ = accept_fake_capture(FIRST_FAKE_OWNER_TAG, 7, true);
    panic!("crashing fake owner returned instead of losing its process");
}

fn run_replacement_fake_owner_process() {
    let requests = accept_fake_capture(REPLACEMENT_FAKE_OWNER_TAG, 8, false);
    let methods = requests.iter().map(Request::method).collect::<Vec<_>>();
    assert_eq!(
        &methods[..4],
        &[
            Method::LeaseAcquire,
            Method::HealthGet,
            Method::PermissionsGet,
            Method::SessionReconcileOff,
        ]
    );
    let capture_commands = requests
        .iter()
        .filter(|request| {
            matches!(
                request,
                Request::SessionReconcileOff(_)
                    | Request::CaptureReplaceConfiguration(_)
                    | Request::CaptureSetEnabled(_)
                    | Request::SessionSetMode(_)
                    | Request::LeaseRelease(_)
                    | Request::OwnerExitWhenNeutral(_)
            )
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        capture_commands[0],
        Request::SessionReconcileOff(params) if params.command_sequence.get() == 1
    ));
    assert!(matches!(
        capture_commands[1],
        Request::CaptureReplaceConfiguration(params)
            if params.command_sequence.get() == 2
                && params.revision.get() == 1
                && params.bindings == wire_bindings()
    ));
    assert!(matches!(
        capture_commands[2],
        Request::CaptureSetEnabled(params)
            if params.command_sequence.get() == 3 && params.enabled
    ));
    assert!(matches!(
        capture_commands[3],
        Request::SessionSetMode(params)
            if params.command_sequence.get() == 4 && params.mode == SessionMode::Recording
    ));
    assert!(matches!(
        capture_commands[4],
        Request::OwnerExitWhenNeutral(params) if params.command_sequence.get() == 5
    ));
}

fn run_deadline_fake_owner_process() {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    let mut purpose = [0_u8; 1];
    stream.read_exact(&mut purpose).unwrap();
    assert_eq!(purpose, [1]);
    stream.set_nonblocking(true).unwrap();
    let (_, mut codec) = material(Purpose::Capture, OWNER_INSTANCE[0])
        .codecs()
        .unwrap();
    let mut endpoint = TestTcpTransport(StreamOrderedTransport::new(stream).unwrap());
    let mut requests = Vec::new();
    let mut replacements = 0;
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        match endpoint.try_receive() {
            Ok(ReceiveResult::Empty) => std::thread::yield_now(),
            Ok(ReceiveResult::PeerClosed) | Err(TransportError::PeerClosed) => break,
            Err(_) if replacements == 2 => break,
            Err(error) => panic!("deadline fixture transport failed: {error}"),
            Ok(ReceiveResult::Frame(frame)) => {
                let received = codec.receive_request(&frame).unwrap();
                if received.request().method() == Method::CaptureReplaceConfiguration {
                    replacements += 1;
                    if replacements == 2 {
                        std::thread::sleep(Duration::from_millis(8_200));
                    }
                }
                requests.push(received.request().clone());
                let response = fake_owner_response(received.request(), OWNER_INSTANCE[0], 7);
                let frame = codec.encode_response(&received, &response).unwrap();
                let Ok(receipt) = endpoint.try_send(frame) else {
                    break;
                };
                while endpoint
                    .flush(receipt)
                    .unwrap_or(TransportProgress::Complete)
                    == TransportProgress::Pending
                {
                    std::thread::yield_now();
                }
            }
        }
    }
    let second_replace = requests
        .iter()
        .rposition(|request| request.method() == Method::CaptureReplaceConfiguration)
        .expect("second replacement observed");
    assert_eq!(replacements, 2);
    assert!(matches!(
        requests[second_replace - 1],
        Request::CaptureSetEnabled(ref value) if !value.enabled
    ));
    assert!(
        !requests[second_replace + 1..]
            .iter()
            .any(|request| matches!(request, Request::CaptureSetEnabled(value) if value.enabled))
    );
}

fn run_gateway_with_fake_auth_process(rotating_fake_owner_identity: bool) {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let input = stream.try_clone().unwrap();
    let output = stream;
    talking_quill_helper::run_framed_stream_with_factory_and_gate(
        input,
        output,
        ActivationCaptureGate::open_for_test_harness(),
        |outbound, _, _, capture_gate| {
            let owner_address = std::env::var("TALKING_QUILL_COMMON_E2E_OWNER_ADDRESS")
                .unwrap()
                .parse()
                .unwrap();
            let connector = if rotating_fake_owner_identity {
                TcpConnector::rotating_fake_owners(owner_address)
            } else {
                TcpConnector::fixed(owner_address, OWNER_INSTANCE[0])
            };
            OwnerGatewayBackend::connect_with(Box::new(connector), outbound, capture_gate)
        },
    )
    .unwrap();
}

fn write_v10(stream: &mut TcpStream, id: u64, method: &str, params: serde_json::Value) {
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

fn read_v10(stream: &mut TcpStream, expected_id: u64) -> serde_json::Value {
    loop {
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length).unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if value.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

fn wait_gateway_owner_ready(
    stream: &mut TcpStream,
    mut response: serde_json::Value,
    mut next_id: u64,
) -> (serde_json::Value, u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if response["result"]["keyboardOwner"]["authenticated"] == true
            && response["result"]["hookStatus"] == "installed_unobserved"
        {
            return (response, next_id);
        }
        assert!(
            Instant::now() < deadline,
            "gateway owner did not become protocol-v10 ready: {response}"
        );
        write_v10(stream, next_id, "ping", serde_json::json!({}));
        response = read_v10(stream, next_id);
        next_id += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn gateway_fake_owner_forwarding_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let crashing_fake_owner = spawn_role("crashing-fake-owner", owner_address);
    let gateway = spawn_gateway_role("gateway-fake-auth", gateway_address, owner_address);
    let mut electron = connect_retry(gateway_address);
    electron
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    write_v10(
        &mut electron,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": 10}),
    );
    let initialized = read_v10(&mut electron, 1);
    let (initialized, mut id) = wait_gateway_owner_ready(&mut electron, initialized, 2);
    let first_owner_instance = initialized["result"]["keyboardOwner"]["instanceId"].clone();
    let first_lease_epoch = initialized["result"]["keyboardOwner"]["leaseEpoch"].clone();
    assert_eq!(initialized["result"]["protocolVersion"], 10);
    assert_eq!(initialized["result"]["hookStatus"], "installed_unobserved");
    assert_eq!(
        initialized["result"]["keyboardOwner"]["model"],
        "out_of_process"
    );
    assert_eq!(initialized["result"]["keyboardOwner"]["protocolVersion"], 1);
    assert_eq!(
        initialized["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );
    assert_eq!(
        initialized["result"]["keyboardOwner"]["authenticated"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardCapture"]["activationAvailable"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardCapture"]["sessionKeyCaptureAvailable"],
        true
    );
    // Establish an accepted session mode, then force an uncertain configuration
    // mutation. The uncertain configuration must not become reconnect intent.
    write_v10(
        &mut electron,
        id,
        "session.set_capture",
        serde_json::json!({"mode":"recording"}),
    );
    let _ = read_v10(&mut electron, id);
    id += 1;
    write_v10(
        &mut electron,
        id,
        "activation.configure",
        serde_json::json!({
            "enabled": true,
            "bindings": [{"profileId":"general","shortcut":{
                "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
                "keys":["X"]
            }}]
        }),
    );
    let crashed_configuration = read_v10(&mut electron, id);
    id += 1;
    assert!(matches!(
        crashed_configuration["error"]["code"].as_i64(),
        Some(-32_003) | Some(-32_011)
    ));
    crashing_fake_owner.wait_exit_code(EXPECTED_FAKE_OWNER_CRASH_EXIT);

    // The owner process is now gone and no replacement endpoint exists. A
    // fresh mutation must fail rather than being reported as applied.
    let unavailable_started = Instant::now();
    write_v10(
        &mut electron,
        id,
        "activation.configure",
        serde_json::json!({
            "enabled": true,
            "bindings": [{"profileId":"general","shortcut":{
                "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
                "keys":["X"]
            }}]
        }),
    );
    let unavailable_configuration = read_v10(&mut electron, id);
    assert!(
        unavailable_started.elapsed() < Duration::from_secs(2),
        "post-crash mutation exceeded the gateway RPC timeout"
    );
    assert!(matches!(
        unavailable_configuration["error"]["code"].as_i64(),
        Some(-32_003) | Some(-32_011)
    ));
    id += 1;
    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let unavailable = read_v10(&mut electron, id);
    id += 1;
    assert_eq!(unavailable["result"]["hookStatus"], "unavailable");
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["state"],
        "unavailable"
    );
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["authenticated"],
        false
    );

    let replacement_fake_owner = spawn_role("replacement-fake-owner", owner_address);
    let (replacement_ready, next_id) = wait_gateway_owner_ready(&mut electron, unavailable, id);
    id = next_id;
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );
    assert_ne!(
        replacement_ready["result"]["keyboardOwner"]["instanceId"],
        first_owner_instance
    );
    assert_ne!(
        replacement_ready["result"]["keyboardOwner"]["leaseEpoch"],
        first_lease_epoch
    );
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["leaseEpoch"],
        8
    );
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["buildId"],
        "common-e2e-build"
    );
    // The replacement starts disabled with no replay of the uncertain
    // configuration. Fresh accepted operations establish the new desired
    // configuration and session mode on that replacement connection.
    write_v10(
        &mut electron,
        id,
        "activation.configure",
        serde_json::json!({
            "enabled": true,
            "bindings": [{"profileId":"general","shortcut":{
                "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
                "keys":["X"]
            }}]
        }),
    );
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(
        &mut electron,
        id,
        "session.set_capture",
        serde_json::json!({"mode":"recording"}),
    );
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let shutdown = read_v10(&mut electron, id);
    assert_eq!(shutdown["result"]["ownerDisposition"], "neutral");
    drop(electron);
    gateway.wait_success();
    replacement_fake_owner.wait_success();
}

#[derive(Debug)]
struct FakeNativeAdapter {
    events: VecDeque<AdapterEvent>,
    next_event_id: u64,
    neutral_pending: bool,
    neutral_release: Arc<AtomicBool>,
}

impl FakeNativeAdapter {
    fn new(neutral_release: Arc<AtomicBool>) -> Self {
        Self {
            events: VecDeque::new(),
            next_event_id: 0,
            neutral_pending: false,
            neutral_release,
        }
    }

    fn emit_ownership(
        &mut self,
        candidate: talking_quill_keyboard_owner::state::CandidateOwnership,
        replay_cleanup_edges: u16,
    ) {
        self.next_event_id += 1;
        self.events.push_back(AdapterEvent::new(
            AdapterEventId::new(self.next_event_id).unwrap(),
            BrokerEvent::OwnershipChanged(
                talking_quill_keyboard_owner::state::NativeOwnershipObservation {
                    candidate,
                    activation_drain_keys: 0,
                    session_drain_keys: 0,
                    replay_cleanup_edges,
                    paste: talking_quill_keyboard_owner::state::PasteOwnership::None,
                    conservative_native_work: false,
                    admitted_effects: 0,
                },
            ),
        ));
    }
}

impl NativeAdapter for FakeNativeAdapter {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }
    fn readiness(&self) -> talking_quill_keyboard_owner::state::NativeReadiness {
        talking_quill_keyboard_owner::state::NativeReadiness {
            keyboard_build_eligible: true,
            paste_ready: true,
            permissions_eligible: true,
            hook_healthy: true,
        }
    }
    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult {
        use talking_quill_keyboard_owner::state::{
            CandidateOwnership, NativeOwnership, PasteOwnership,
        };

        match effect.kind() {
            NativeEffectKind::OpenFreshAdmission => {
                self.emit_ownership(CandidateOwnership::Active, 0);
                NativeEffectResult::Applied
            }
            NativeEffectKind::CloseFreshAdmission
            | NativeEffectKind::EmergencyCloseFreshAdmission => {
                NativeEffectResult::AdmissionClosed {
                    through_event: AdapterEventId::new(self.next_event_id),
                }
            }
            NativeEffectKind::CancelCandidate => {
                self.neutral_pending = true;
                NativeEffectResult::CandidateCancelled(
                    NativeOwnership::new(
                        CandidateOwnership::None,
                        0,
                        0,
                        1,
                        PasteOwnership::None,
                        0,
                    )
                    .unwrap(),
                )
            }
            _ => NativeEffectResult::Applied,
        }
    }
    fn try_next_event(&mut self) -> Option<AdapterEvent> {
        if self.neutral_pending && self.neutral_release.load(Ordering::Acquire) {
            self.neutral_pending = false;
            self.emit_ownership(
                talking_quill_keyboard_owner::state::CandidateOwnership::None,
                0,
            );
        }
        self.events.pop_front()
    }
    fn acknowledge_event(
        &mut self,
        _: talking_quill_keyboard_owner::AdapterEventId,
        _: AdapterEventDisposition,
    ) {
    }
    fn permissions(&self) -> PermissionsResult {
        PermissionsResult {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            event_post: PermissionState::Granted,
        }
    }
    fn front_app(&self) -> FrontAppResult {
        FrontAppResult {
            available: false,
            application_token: None,
        }
    }
    fn observability(&self) -> ObservabilityResult {
        ObservabilityResult::default()
    }
}

#[derive(Debug, Default)]
struct Capabilities(AtomicU8);
impl CapabilityIdSource for Capabilities {
    fn next_capability_id(&mut self) -> Option<talking_quill_keyboard_owner::state::CapabilityId> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).checked_add(1)?;
        let mut bytes = [0_u8; 32];
        bytes[0] = value;
        talking_quill_keyboard_owner::state::CapabilityId::new(bytes)
    }
}

/// Test-only connection source: it bypasses OS peer/code identity and supplies
/// a fake authenticated codec over loopback. It exercises the real owner
/// runtime/state machine, not production endpoint authentication.
#[derive(Debug)]
struct FakeAuthenticatedTcpConnectionSource {
    listener: TcpListener,
    neutral_release: Arc<AtomicBool>,
}
impl talking_quill_keyboard_owner::AuthenticatedConnectionSource
    for FakeAuthenticatedTcpConnectionSource
{
    fn poll_authenticated(
        &mut self,
    ) -> Result<
        Option<talking_quill_keyboard_owner::AuthenticatedConnection>,
        talking_quill_keyboard_owner::ConnectionSourceError,
    > {
        match self.listener.accept() {
            Ok((mut stream, _)) => {
                let mut discriminator = [0_u8; 1];
                stream
                    .read_exact(&mut discriminator)
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                let purpose = match discriminator[0] {
                    1 => Purpose::Capture,
                    2 => Purpose::Maintenance,
                    3 => Purpose::Observe,
                    4 => {
                        self.neutral_release.store(true, Ordering::Release);
                        return Ok(None);
                    }
                    _ => return Err(talking_quill_keyboard_owner::ConnectionSourceError),
                };
                stream
                    .set_nonblocking(true)
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                let (_, codec) = material(purpose, OWNER_INSTANCE[0])
                    .codecs()
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                Ok(Some(
                    talking_quill_keyboard_owner::AuthenticatedConnection::new(
                        Box::new(TestTcpTransport(
                            StreamOrderedTransport::new(stream)
                                .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?,
                        )),
                        codec,
                    ),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(_) => Err(talking_quill_keyboard_owner::ConnectionSourceError),
        }
    }
    fn shutdown_endpoint(
        &mut self,
    ) -> Result<(), talking_quill_keyboard_owner::ConnectionSourceError> {
        Ok(())
    }
}

fn run_owner_runtime_with_fake_auth_process() {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    listener.set_nonblocking(true).unwrap();
    let neutral_release = Arc::new(AtomicBool::new(false));
    let executor = NativeAdapterExecutor::new_for_test(
        FakeNativeAdapter::new(Arc::clone(&neutral_release)),
        Capabilities::default(),
        OwnerCaptureGate::open_for_test_harness(),
    );
    let server = OwnerProtocolServer::start(
        talking_quill_keyboard_owner::state::OwnerInstanceId::new(OWNER_INSTANCE).unwrap(),
        executor,
    )
    .unwrap();
    let mut runtime = OwnerRuntime::from_parts(
        server,
        FakeAuthenticatedTcpConnectionSource {
            listener,
            neutral_release,
        },
        NoRuntimeSignals,
        ProcessSingleton::default(),
    )
    .unwrap();
    runtime.run().unwrap();
}

fn fake_authentication_cannot_cross_production_transport_brand() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client_stream = TcpStream::connect(address).unwrap();
    let (_peer_stream, _) = listener.accept().unwrap();
    client_stream.set_nonblocking(true).unwrap();
    let (fake_gateway_codec, _) = material(Purpose::Capture, OWNER_INSTANCE[0])
        .codecs()
        .unwrap();

    // `StreamOrderedTransport` retains the production/default brand because it
    // does not override `OrderedTransport::is_test_only`. Fake session material
    // is rejected before any frame can cross that boundary.
    let result = OwnerProtocolClient::new(
        StreamOrderedTransport::new(client_stream).unwrap(),
        fake_gateway_codec,
    );
    assert!(matches!(
        result,
        Err(ClientError::TransportAuthenticationBoundary)
    ));
}

fn gateway_deadline_cross_process_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let owner = spawn_role("deadline-fake-owner", owner_address);
    let gateway = spawn_gateway_role(
        "gateway-owner-runtime-fake-auth",
        gateway_address,
        owner_address,
    );
    let mut electron = connect_retry(gateway_address);
    electron
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();

    write_v10(
        &mut electron,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": 10}),
    );
    let initialized = read_v10(&mut electron, 1);
    let (_, mut id) = wait_gateway_owner_ready(&mut electron, initialized, 2);
    let configuration = serde_json::json!({
        "enabled": true,
        "bindings": [{"profileId":"general","shortcut":{
            "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
            "keys":["X"]
        }}]
    });
    write_v10(
        &mut electron,
        id,
        "activation.configure",
        configuration.clone(),
    );
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(&mut electron, id, "activation.configure", configuration);
    let expired = read_v10(&mut electron, id);
    assert!(expired["error"].is_object(), "{expired}");
    id += 1;
    owner.wait_success();

    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let unavailable = read_v10(&mut electron, id);
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["authenticated"],
        false
    );
    id += 1;
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let _ = read_v10(&mut electron, id);
    drop(electron);
    gateway.wait_success();
}

fn gateway_owner_runtime_composition_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let owner = spawn_role("owner-runtime-fake-auth", owner_address);
    let gateway = spawn_gateway_role(
        "gateway-owner-runtime-fake-auth",
        gateway_address,
        owner_address,
    );
    let mut electron = connect_retry(gateway_address);
    electron
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    write_v10(
        &mut electron,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": 10}),
    );
    let initialized = read_v10(&mut electron, 1);
    let (initialized, mut id) = wait_gateway_owner_ready(&mut electron, initialized, 2);
    assert_eq!(initialized["result"]["protocolVersion"], 10);
    assert_eq!(initialized["result"]["hookStatus"], "installed_unobserved");
    assert_eq!(
        initialized["result"]["keyboardOwner"]["authenticated"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );

    write_v10(
        &mut electron,
        id,
        "activation.configure",
        serde_json::json!({
            "enabled": true,
            "bindings": [{"profileId":"general","shortcut":{
                "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
                "keys":["X"]
            }}]
        }),
    );
    let configured = read_v10(&mut electron, id);
    assert!(configured.get("result").is_some(), "{configured}");
    id += 1;

    // This following request serializes behind owner configuration and the
    // native-adapter active-candidate observation.
    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let enabled = read_v10(&mut electron, id);
    assert_eq!(
        enabled["result"]["keyboardOwner"]["state"],
        "leased_enabled"
    );
    id += 1;
    release_authoritative_neutral(owner_address);
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let shutdown = read_v10(&mut electron, id);
    assert_eq!(shutdown["result"]["ownerDisposition"], "neutral");

    // Planned quit closes the stable endpoint after the correlated release and
    // exits only after authoritative neutrality.
    drop(electron);
    gateway.wait_success();
    owner.wait_success();
}

fn wire_bindings() -> Bindings {
    Bindings::new(vec![Binding::new(
        ProfileId::new("general".into()).unwrap(),
        BindingShortcut::new(Modifiers::new(false, true, false, false), vec![Letter::X]).unwrap(),
    )])
    .unwrap()
}

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

fn owner_runtime_fake_gateway_suite() {
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

#[test]
fn common_cross_process_foundation() {
    match std::env::var(ROLE_ENV).ok().as_deref() {
        Some("crashing-fake-owner") => run_crashing_fake_owner_process(),
        Some("replacement-fake-owner") => run_replacement_fake_owner_process(),
        Some("deadline-fake-owner") => run_deadline_fake_owner_process(),
        Some("gateway-fake-auth") => run_gateway_with_fake_auth_process(true),
        Some("gateway-owner-runtime-fake-auth") => run_gateway_with_fake_auth_process(false),
        Some("owner-runtime-fake-auth") => run_owner_runtime_with_fake_auth_process(),
        None => {
            fake_authentication_cannot_cross_production_transport_brand();
            gateway_fake_owner_forwarding_suite();
            gateway_deadline_cross_process_suite();
            gateway_owner_runtime_composition_suite();
            owner_runtime_fake_gateway_suite();
        }
        Some(role) => panic!("unknown cross-process role: {role}"),
    }
}
