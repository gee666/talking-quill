use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use talking_quill_helper::owner::client::{ConnectError, OwnerConnector};
use talking_quill_helper::owner::handshake::authenticate_gateway;
use talking_quill_helper::owner::platform_client::{
    LauncherProvidedOwnerConnector, ProductionOwnerConnector,
};
use talking_quill_owner_protocol::auth::{
    AuthenticationKeys, HandshakeTrustVerifier, KeyAgreementMaterial, Transcript, TranscriptInput,
};
use talking_quill_owner_protocol::client::{ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::envelope::AuthenticatedEnvelope;
use talking_quill_owner_protocol::framing::{encode_outer_frame, read_outer_frame};
use talking_quill_owner_protocol::release_policy::{OwnerMode, PolicySignature, ReleasePolicy};
use talking_quill_owner_protocol::scalar::{Bytes32, FeatureBits, U64String};
use talking_quill_owner_protocol::schema::{
    AcquireState, Architecture, Authenticated, AuthorityCeiling, Challenge, Empty,
    HandshakeMessage, Hello, LeaseAcquireResult, Platform, ProtocolHeader, Purpose, Request,
    Response, SuccessResult, parse_handshake_json,
};
use talking_quill_owner_protocol::session::{GatewayMessage, OwnerSessionCodec};

struct ExactTestTrust;

impl HandshakeTrustVerifier for ExactTestTrust {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let expected = policy();
        if hello.platform != Platform::Macos
            || challenge.platform != Platform::Macos
            || hello.installation_identity_digest != Bytes32::new([15; 32])
            || challenge.installation_identity_digest != Bytes32::new([15; 32])
            || hello.platform_credential_binding_digest != Bytes32::new([17; 32])
            || challenge.platform_credential_binding_digest != Bytes32::new([17; 32])
            || client_policy != &expected
            || owner_policy != &expected
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Policy);
        }
        Ok(())
    }
}

fn header() -> ProtocolHeader {
    ProtocolHeader {
        major: talking_quill_owner_protocol::PROTOCOL_MAJOR,
        minor: 0,
        compatibility_epoch: talking_quill_owner_protocol::COMPATIBILITY_EPOCH,
        supported_feature_bits: FeatureBits::new(talking_quill_owner_protocol::BASE_V1),
        required_feature_bits: FeatureBits::new(talking_quill_owner_protocol::BASE_V1),
    }
}

fn policy() -> ReleasePolicy {
    ReleasePolicy {
        platform: Platform::Macos,
        architecture: Architecture::X64,
        owner_mode: OwnerMode::EnabledCandidate,
        release_build_digest: Bytes32::new([10; 32]),
        gateway_sha256: Bytes32::new([11; 32]),
        owner_sha256: Bytes32::new([12; 32]),
        gateway_signer_policy_digest: Bytes32::new([13; 32]),
        owner_signer_policy_digest: Bytes32::new([14; 32]),
        gateway_protocol: header(),
        owner_protocol: header(),
        predecessor: None,
    }
}

fn signature() -> PolicySignature {
    PolicySignature::from_der(vec![0x30, 0]).unwrap()
}

fn hello() -> Hello {
    Hello::new(
        Purpose::Capture,
        header(),
        Bytes32::new([20; 32]),
        Platform::Macos,
        Architecture::X64,
        Bytes32::new([10; 32]),
        Bytes32::new([11; 32]),
        Bytes32::new([15; 32]),
        Bytes32::new([13; 32]),
        Bytes32::new([16; 32]),
        policy().encode().unwrap(),
        signature(),
        Bytes32::new([17; 32]),
        None,
    )
    .unwrap()
}

fn challenge() -> Challenge {
    Challenge::new(
        header(),
        header().negotiate(header()).unwrap(),
        Purpose::Capture,
        AuthorityCeiling::Capture,
        Bytes32::new([21; 32]),
        Bytes32::new([22; 32]),
        Bytes32::new([23; 32]),
        Platform::Macos,
        Architecture::X64,
        Bytes32::new([10; 32]),
        Bytes32::new([12; 32]),
        Bytes32::new([15; 32]),
        Bytes32::new([14; 32]),
        Bytes32::new([16; 32]),
        policy().encode().unwrap(),
        signature(),
        Bytes32::new([17; 32]),
        None,
    )
    .unwrap()
}

fn run_authenticated_owner(mut stream: TcpStream) {
    let hello_body = read_outer_frame(&mut stream).unwrap().unwrap();
    let HandshakeMessage::Hello(hello) = parse_handshake_json(&hello_body).unwrap() else {
        panic!("expected hello");
    };
    let challenge = challenge();
    stream
        .write_all(&encode_outer_frame(&challenge.to_json().unwrap()).unwrap())
        .unwrap();

    let transcript = Transcript::build(
        &TranscriptInput::from_verified_handshake(&hello, &challenge, &ExactTestTrust).unwrap(),
    )
    .unwrap();
    let mut secret = [42; 32];
    let material = KeyAgreementMaterial::macos(&mut secret);
    let keys = AuthenticationKeys::derive(&transcript, &material).unwrap();
    let authenticate_body = read_outer_frame(&mut stream).unwrap().unwrap();
    let HandshakeMessage::Authenticate(authenticate) =
        parse_handshake_json(&authenticate_body).unwrap()
    else {
        panic!("expected authenticate");
    };
    let session = keys
        .establish_owner_session(&transcript, &authenticate.client_proof)
        .unwrap();
    let proofs = keys.proofs(&transcript);
    let authenticated = Authenticated::new(
        challenge.selected_protocol,
        Purpose::Capture,
        AuthorityCeiling::Capture,
        proofs.owner_proof,
    )
    .unwrap();
    let finish = AuthenticatedEnvelope::authenticated_finish(challenge.session_id, &authenticated)
        .unwrap()
        .encode_body(keys.owner_frame_key())
        .unwrap();
    stream
        .write_all(&encode_outer_frame(&finish).unwrap())
        .unwrap();

    let mut codec = OwnerSessionCodec::new(session, keys.owner_frame_key()).unwrap();
    let request_frame = read_outer_frame(&mut stream).unwrap().unwrap();
    let framed = encode_outer_frame(&request_frame).unwrap();
    let request = codec.receive_request(&framed).unwrap();
    assert!(matches!(request.request(), Request::LeaseAcquire(Empty {})));
    let response = Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
        capture_lease_id: Bytes32::new([24; 32]),
        capture_lease_epoch: U64String::try_from(1).unwrap(),
        state: AcquireState::Disabled,
    }));
    stream
        .write_all(&codec.encode_response(&request, &response).unwrap())
        .unwrap();
}

struct SlowProgressStream;

impl Read for SlowProgressStream {
    fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }
}

impl Write for SlowProgressStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(2));
        Ok(1)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn absolute_handshake_deadline_rejects_continuous_slow_progress() {
    let started = Instant::now();
    let result = authenticate_gateway(
        SlowProgressStream,
        hello(),
        &ExactTestTrust,
        |_challenge| {
            let mut secret = [42; 32];
            Ok(KeyAgreementMaterial::macos(&mut secret))
        },
        started + Duration::from_millis(20),
    );
    assert!(matches!(
        result,
        Err(talking_quill_helper::owner::handshake::GatewayHandshakeError::Timeout)
    ));
    assert!(started.elapsed() < Duration::from_millis(100));
}

struct InjectedAuthenticationFailure;

impl OwnerConnector for InjectedAuthenticationFailure {
    fn connect_capture(
        &mut self,
    ) -> Result<talking_quill_helper::owner::client::ConnectedOwner, ConnectError> {
        Err(ConnectError::Authentication)
    }
}

#[test]
fn launcher_provided_connector_consumes_injected_connector_without_discovery() {
    let mut connector =
        LauncherProvidedOwnerConnector::new(Box::new(InjectedAuthenticationFailure));
    assert_eq!(
        connector.connect_capture().unwrap_err(),
        ConnectError::Authentication
    );
}

#[test]
fn production_connector_stops_before_authentication_without_an_eligible_adjacent_owner() {
    let error = ProductionOwnerConnector::default()
        .connect_capture()
        .unwrap_err();
    #[cfg(windows)]
    assert!(matches!(
        error,
        ConnectError::Unavailable | ConnectError::Busy
    ));
    #[cfg(target_os = "macos")]
    assert_eq!(error, ConnectError::MacosProvisioningUnavailable);
    #[cfg(not(any(windows, target_os = "macos")))]
    assert_eq!(error, ConnectError::UnsupportedPlatform);
}

#[test]
fn generic_gateway_driver_completes_real_authentication_and_protocol_exchange() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let owner = thread::spawn(move || run_authenticated_owner(listener.accept().unwrap().0));
    let stream = TcpStream::connect(address).unwrap();
    stream.set_nonblocking(true).unwrap();
    let mut client: OwnerProtocolClient<'static> = authenticate_gateway(
        stream,
        hello(),
        &ExactTestTrust,
        |_challenge| {
            let mut secret = [42; 32];
            Ok(KeyAgreementMaterial::macos(&mut secret))
        },
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    let correlation = client
        .send_request(&Request::LeaseAcquire(Empty {}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client.poll().unwrap() {
            ClientPoll::Empty => {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
            ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response: Response::Success(SuccessResult::LeaseAcquire(_)),
            }) => {
                assert_eq!(correlation_sequence, correlation);
                break;
            }
            other => panic!("unexpected authenticated result: {other:?}"),
        }
    }
    client.abort();
    owner.join().unwrap();
}
