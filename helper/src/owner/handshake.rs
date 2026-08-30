//! Platform-neutral gateway side of owner-protocol v1 authentication.
//!
//! Platform connectors retain responsibility for acquiring an authenticated OS
//! stream, constructing/verifying identity-bound handshake fields, and consuming
//! platform key material. This driver owns only the strict wire
//! order and atomic transition into an authenticated protocol client.

use std::io::{Read, Write};
use std::thread;
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::client::OwnerProtocolClient;
use talking_quill_owner_protocol::framing::{MAX_BODY_LENGTH, encode_outer_frame};
use talking_quill_owner_protocol::schema::{
    Authenticate, Challenge, HandshakeMessage, Hello, parse_handshake_json,
};
use talking_quill_owner_protocol::{
    AuthenticationKeys, GatewaySessionCodec, HandshakeTrustVerifier, KeyAgreementMaterial,
    StreamOrderedTransport, Transcript, TranscriptInput,
};
use thiserror::Error;

const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Debug, Error)]
pub enum GatewayHandshakeError {
    #[error("owner handshake timed out")]
    Timeout,
    #[error("owner closed during authentication")]
    PeerClosed,
    #[error("owner handshake I/O failed")]
    Io,
    #[error("owner handshake framing was invalid")]
    Framing,
    #[error("owner handshake message was invalid or out of order")]
    Protocol,
    #[error("owner handshake trust or proof verification failed")]
    Authentication,
    #[error("authenticated owner transport initialization failed")]
    Transport,
}

/// Drives `hello -> challenge -> authenticate -> authenticated` exactly once.
///
/// `stream` must already be a private, kernel-authenticated platform endpoint
/// and must translate temporary readiness failures to `WouldBlock`. `verifier`
/// must bind the challenge to that held endpoint before `key_material` consumes
/// the corresponding private secret. The driver never discovers credentials and
/// never retries an uncertain proof write.
pub fn authenticate_gateway<S, V, K>(
    mut stream: S,
    hello: Hello,
    verifier: &V,
    key_material: K,
    deadline: Instant,
) -> Result<OwnerProtocolClient<'static>, GatewayHandshakeError>
where
    S: Read + Write + Send + 'static,
    V: HandshakeTrustVerifier,
    K: FnOnce(&Challenge) -> Result<KeyAgreementMaterial, GatewayHandshakeError>,
{
    check_deadline(deadline)?;
    let hello_body = hello
        .to_json()
        .map_err(|_| GatewayHandshakeError::Protocol)?;
    check_deadline(deadline)?;
    write_frame(&mut stream, &hello_body, deadline)?;
    check_deadline(deadline)?;

    let challenge_body = read_frame(&mut stream, deadline)?;
    check_deadline(deadline)?;
    let HandshakeMessage::Challenge(challenge) =
        parse_handshake_json(&challenge_body).map_err(|_| GatewayHandshakeError::Protocol)?
    else {
        return Err(GatewayHandshakeError::Protocol);
    };
    let transcript = Transcript::build(
        &TranscriptInput::from_verified_handshake(&hello, &challenge, verifier)
            .map_err(|_| GatewayHandshakeError::Authentication)?,
    )
    .map_err(|_| GatewayHandshakeError::Authentication)?;
    check_deadline(deadline)?;

    // Platform key acquisition consumes independently verified key material.
    check_deadline(deadline)?;
    let material = key_material(&challenge)?;
    check_deadline(deadline)?;
    let keys = AuthenticationKeys::derive(&transcript, &material)
        .map_err(|_| GatewayHandshakeError::Authentication)?;
    check_deadline(deadline)?;
    let proofs = keys.proofs(&transcript);
    let authenticate = Authenticate::new(proofs.client_proof)
        .to_json()
        .map_err(|_| GatewayHandshakeError::Protocol)?;

    // Recheck immediately around disclosure of the one-use client proof.
    check_deadline(deadline)?;
    write_frame(&mut stream, &authenticate, deadline)?;
    check_deadline(deadline)?;

    let finish = read_frame(&mut stream, deadline)?;
    check_deadline(deadline)?;
    let session = keys
        .pending_gateway_session(&transcript)
        .and_then(|pending| pending.accept_authenticated_finish(&finish, &proofs.client_proof))
        .map_err(|_| GatewayHandshakeError::Authentication)?;
    check_deadline(deadline)?;
    let codec = GatewaySessionCodec::new(session, keys.gateway_frame_key())
        .map_err(|_| GatewayHandshakeError::Transport)?;
    check_deadline(deadline)?;
    let transport =
        StreamOrderedTransport::new(stream).map_err(|_| GatewayHandshakeError::Transport)?;
    check_deadline(deadline)?;
    OwnerProtocolClient::new(transport, codec).map_err(|_| GatewayHandshakeError::Transport)
}

fn write_frame(
    stream: &mut impl Write,
    body: &[u8],
    deadline: Instant,
) -> Result<(), GatewayHandshakeError> {
    let frame = encode_outer_frame(body).map_err(|_| GatewayHandshakeError::Framing)?;
    let mut offset = 0;
    while offset < frame.len() {
        check_deadline(deadline)?;
        match stream.write(&frame[offset..]) {
            Ok(0) => return Err(GatewayHandshakeError::PeerClosed),
            Ok(written) => {
                offset += written;
                check_deadline(deadline)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(_) => return Err(GatewayHandshakeError::Io),
        }
    }
    loop {
        check_deadline(deadline)?;
        match stream.flush() {
            Ok(()) => {
                check_deadline(deadline)?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(_) => return Err(GatewayHandshakeError::Io),
        }
    }
}

fn read_frame(stream: &mut impl Read, deadline: Instant) -> Result<Vec<u8>, GatewayHandshakeError> {
    let mut prefix = [0_u8; 4];
    read_exact(stream, &mut prefix, deadline)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_BODY_LENGTH {
        return Err(GatewayHandshakeError::Framing);
    }
    let mut body = vec![0_u8; length];
    read_exact(stream, &mut body, deadline)?;
    Ok(body)
}

fn read_exact(
    stream: &mut impl Read,
    buffer: &mut [u8],
    deadline: Instant,
) -> Result<(), GatewayHandshakeError> {
    let mut offset = 0;
    while offset < buffer.len() {
        check_deadline(deadline)?;
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => return Err(GatewayHandshakeError::PeerClosed),
            Ok(read) => {
                offset += read;
                check_deadline(deadline)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => wait(deadline)?,
            Err(_) => return Err(GatewayHandshakeError::Io),
        }
    }
    Ok(())
}

fn check_deadline(deadline: Instant) -> Result<(), GatewayHandshakeError> {
    if Instant::now() >= deadline {
        Err(GatewayHandshakeError::Timeout)
    } else {
        Ok(())
    }
}

fn wait(deadline: Instant) -> Result<(), GatewayHandshakeError> {
    check_deadline(deadline)?;
    thread::sleep(POLL_INTERVAL);
    Ok(())
}
