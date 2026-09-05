use super::connection::wire_bindings;
use super::connection::{
    EXPECTED_FAKE_OWNER_CRASH_EXIT, FIRST_FAKE_OWNER_TAG, OWNER_INSTANCE,
    REPLACEMENT_FAKE_OWNER_TAG, TestTcpTransport, material,
};
use super::process::address_from_env;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use talking_quill_owner_protocol::schema::*;
use talking_quill_owner_protocol::{
    Bytes32, OrderedTransport, OwnerSessionCodec, ReceiveResult, StreamOrderedTransport,
    TransportError, TransportProgress, U64String,
};

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

pub(super) fn run_crashing_fake_owner_process() {
    let _ = accept_fake_capture(FIRST_FAKE_OWNER_TAG, 7, true);
    panic!("crashing fake owner returned instead of losing its process");
}

pub(super) fn run_replacement_fake_owner_process() {
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

pub(super) fn run_deadline_fake_owner_process() {
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
