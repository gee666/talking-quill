//! Windows stable per-WTS-session gateway endpoint for the detached owner.
//!
//! The owner keeps the first-instance, local-only named pipe across gateway
//! disconnects. Authentication derives peer process, token, image, file, and
//! installed-release facts from Windows before the protocol-v1 P-256 proof.

use std::fmt;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::auth::{
    AuthenticationKeys, EphemeralP256Secret, HandshakeTrustVerifier, KeyAgreementMaterial,
    PeerRole, Transcript, TranscriptInput,
};
use talking_quill_owner_protocol::envelope::AuthenticatedEnvelope;
use talking_quill_owner_protocol::framing::{MAX_BODY_LENGTH, encode_outer_frame};
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};
use talking_quill_owner_protocol::schema::{
    Architecture, Authenticated, AuthorityCeiling, Challenge, HandshakeMessage, Hello, Platform,
    ProtocolHeader, Purpose, parse_handshake_json,
};
use talking_quill_owner_protocol::{Bytes32, OwnerSessionCodec, StreamOrderedTransport};
use talking_quill_windows_owner_ipc::channel::{ChannelPurpose, StablePipeBinding};
use thiserror::Error;

use crate::runtime::{
    AuthenticatedConnection, AuthenticatedConnectionSource, ConnectionSourceError,
};
use crate::state::OwnerInstanceId;

const HANDSHAKE_QUEUE: usize = 4;
const ENDPOINT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_AUTHENTICATION_GRACE: Duration = Duration::from_secs(8);
#[cfg(windows)]
const PRIVATE_PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Cloneable connected byte stream. Production supplies two service-created
/// anonymous-pipe directions; tests may use a connected loopback stream.
pub trait PrivateDuplexStream: Read + Write + Send + 'static {
    /// Switch the connected stream to nonblocking mode after the blocking,
    /// bounded authentication handshake has completed.
    fn set_nonblocking_private(&self) -> std::io::Result<()>;
}

impl PrivateDuplexStream for std::net::TcpStream {
    fn set_nonblocking_private(&self) -> std::io::Result<()> {
        self.set_nonblocking(true)
    }
}

#[derive(Debug)]
enum WorkerResult {
    Authenticated(Box<AuthenticatedConnection>),
    Rejected,
}

struct AuthenticationWorker {
    shutdown: OwnerShutdownControl,
    done: Receiver<()>,
    join: thread::JoinHandle<()>,
}

#[derive(Clone)]
struct OwnerShutdownControl {
    cancelled: Arc<AtomicBool>,
}

impl OwnerShutdownControl {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    fn revoke_blocked_io(&self, worker: &thread::JoinHandle<()>) {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::System::IO::CancelSynchronousIo;

        self.cancelled.store(true, Ordering::Release);
        // SAFETY: the JoinHandle owns the authentication thread handle. The
        // source joins that thread immediately after cancelling its blocked I/O.
        unsafe { CancelSynchronousIo(worker.as_raw_handle()) };
    }
}

/// Persistent stable named-pipe endpoint. The first instance is created before
/// native startup publishes readiness and remains owner-controlled across
/// gateway disconnects.
pub struct WindowsNamedPipeConnectionSource {
    name: String,
    owner_instance: Option<Bytes32>,
    results: Option<Receiver<WorkerResult>>,
    worker: Option<AuthenticationWorker>,
    terminal: bool,
}

impl fmt::Debug for WindowsNamedPipeConnectionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WindowsNamedPipeConnectionSource(<redacted>)")
    }
}

impl WindowsNamedPipeConnectionSource {
    pub fn for_current_session() -> Result<Self, ConnectionSourceError> {
        let name = talking_quill_windows_owner_ipc::endpoint::current_pipe_name()
            .map_err(|_| ConnectionSourceError)?;
        Ok(Self {
            name,
            owner_instance: None,
            results: None,
            worker: None,
            terminal: false,
        })
    }
}

impl AuthenticatedConnectionSource for WindowsNamedPipeConnectionSource {
    fn bind_owner_instance(&mut self, owner: OwnerInstanceId) -> Result<(), ConnectionSourceError> {
        if self.owner_instance.is_some() {
            return Err(ConnectionSourceError);
        }
        let security =
            talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon()
                .map_err(|_| ConnectionSourceError)?;
        let first = talking_quill_windows_owner_ipc::endpoint::create_server_instance(
            &self.name, &security, true,
        )
        .map_err(|_| ConnectionSourceError)?;
        let owner_instance = Bytes32::new(*owner.as_bytes());
        self.owner_instance = Some(owner_instance);
        let name = self.name.clone();
        let (sender, receiver) = sync_channel(HANDSHAKE_QUEUE);
        let (done_sender, done) = sync_channel(1);
        let shutdown = OwnerShutdownControl::new();
        let cancelled = shutdown.signal();
        let join = thread::Builder::new()
            .name("tq-owner-named-pipe-auth".into())
            .spawn(move || {
                let mut listener = first;
                loop {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    if connect_named_pipe(&listener, &cancelled).is_err() {
                        break;
                    }
                    let security = match talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon() {
                        Ok(value) => value,
                        Err(_) => break,
                    };
                    let next = match talking_quill_windows_owner_ipc::endpoint::create_server_instance(
                        &name,
                        &security,
                        false,
                    ) {
                        Ok(value) => value,
                        Err(_) => break,
                    };
                    let accepted = listener;
                    listener = next;
                    let result = peer_offer(accepted)
                        .and_then(|offer| authenticate_owner_peer(offer, owner_instance, &cancelled))
                        .map(Box::new)
                        .map(WorkerResult::Authenticated)
                        .unwrap_or(WorkerResult::Rejected);
                    if sender.try_send(result).is_err() && cancelled.load(Ordering::Acquire) {
                        break;
                    }
                }
                let _ = done_sender.try_send(());
            })
            .map_err(|_| ConnectionSourceError)?;
        self.results = Some(receiver);
        self.worker = Some(AuthenticationWorker {
            shutdown,
            done,
            join,
        });
        Ok(())
    }

    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        if self.terminal {
            return Ok(None);
        }
        let Some(results) = &self.results else {
            return Ok(None);
        };
        match results.try_recv() {
            Ok(WorkerResult::Authenticated(connection)) => Ok(Some(*connection)),
            Ok(WorkerResult::Rejected) | Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ConnectionSourceError),
        }
    }

    fn initial_authentication_grace(&self) -> Option<Duration> {
        Some(INITIAL_AUTHENTICATION_GRACE)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        self.terminal = true;
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        let deadline = Instant::now() + ENDPOINT_SHUTDOWN_TIMEOUT;
        worker.shutdown.cancelled.store(true, Ordering::Release);
        worker.shutdown.revoke_blocked_io(&worker.join);
        if worker
            .done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            return Err(ConnectionSourceError);
        }
        while !worker.join.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if !worker.join.is_finished() {
            return Err(ConnectionSourceError);
        }
        let worker = self.worker.take().ok_or(ConnectionSourceError)?;
        worker.join.join().map_err(|_| ConnectionSourceError)
    }
}

impl Drop for WindowsNamedPipeConnectionSource {
    fn drop(&mut self) {
        if self.shutdown_endpoint().is_err() && self.worker.is_some() {
            // The absolute endpoint budget expired while a thread still owned
            // pipe authority. Process termination is the only bounded outcome
            // that lets Windows close every listener and peer handle without
            // detaching that authority or releasing the singleton first.
            std::process::abort();
        }
    }
}

fn connect_named_pipe(
    listener: &std::os::windows::io::OwnedHandle,
    cancelled: &AtomicBool,
) -> Result<(), WindowsEndpointError> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::Foundation::{
        ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(WindowsEndpointError::Io(std::io::Error::last_os_error()));
    }
    let event = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(event) };
    let mut overlapped = windows_sys::Win32::System::IO::OVERLAPPED::default();
    overlapped.hEvent = event.as_raw_handle();
    let connected = unsafe {
        windows_sys::Win32::System::Pipes::ConnectNamedPipe(
            listener.as_raw_handle(),
            &mut overlapped,
        )
    };
    if connected != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
        return Ok(());
    }
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        return Err(error.into());
    }
    loop {
        if cancelled.load(Ordering::Acquire) {
            unsafe {
                windows_sys::Win32::System::IO::CancelIoEx(listener.as_raw_handle(), &overlapped)
            };
            let mut transferred = 0;
            unsafe {
                windows_sys::Win32::System::IO::GetOverlappedResult(
                    listener.as_raw_handle(),
                    &overlapped,
                    &mut transferred,
                    1,
                )
            };
            return Err(WindowsEndpointError::Cancelled);
        }
        match unsafe { WaitForSingleObject(event.as_raw_handle(), 50) } {
            WAIT_OBJECT_0 => {
                let mut transferred = 0;
                if unsafe {
                    windows_sys::Win32::System::IO::GetOverlappedResult(
                        listener.as_raw_handle(),
                        &overlapped,
                        &mut transferred,
                        0,
                    )
                } == 0
                {
                    return Err(WindowsEndpointError::Io(std::io::Error::last_os_error()));
                }
                return Ok(());
            }
            WAIT_TIMEOUT => {}
            _ => return Err(WindowsEndpointError::Io(std::io::Error::last_os_error())),
        }
    }
}

struct WindowsOverlappedPipeStream {
    handle: std::os::windows::io::OwnedHandle,
    _gateway: talking_quill_windows_owner_ipc::peer::VerifiedPeer,
    _owner: talking_quill_windows_owner_ipc::peer::VerifiedPeer,
}

impl fmt::Debug for WindowsOverlappedPipeStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WindowsOverlappedPipeStream(<redacted>)")
    }
}

impl WindowsOverlappedPipeStream {
    fn transfer(&self, buffer: *mut u8, length: usize, write: bool) -> std::io::Result<usize> {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
        use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
        use windows_sys::Win32::System::Threading::CreateEventW;
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let event = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(event) };
        let mut overlapped = windows_sys::Win32::System::IO::OVERLAPPED::default();
        overlapped.hEvent = event.as_raw_handle();
        let handle = self.handle.as_raw_handle();
        let started = if write {
            unsafe {
                windows_sys::Win32::Storage::FileSystem::WriteFile(
                    handle,
                    buffer,
                    length.min(u32::MAX as usize) as u32,
                    std::ptr::null_mut(),
                    &mut overlapped,
                )
            }
        } else {
            unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    handle,
                    buffer,
                    length.min(u32::MAX as usize) as u32,
                    std::ptr::null_mut(),
                    &mut overlapped,
                )
            }
        };
        if started == 0
            && std::io::Error::last_os_error().raw_os_error() != Some(ERROR_IO_PENDING as i32)
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut transferred = 0;
        if unsafe {
            windows_sys::Win32::System::IO::GetOverlappedResultEx(
                handle,
                &overlapped,
                &mut transferred,
                PRIVATE_PIPE_WRITE_TIMEOUT.as_millis() as u32,
                0,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            unsafe { windows_sys::Win32::System::IO::CancelIoEx(handle, &overlapped) };
            let mut ignored = 0;
            unsafe {
                windows_sys::Win32::System::IO::GetOverlappedResult(
                    handle,
                    &overlapped,
                    &mut ignored,
                    1,
                )
            };
            return Err(error);
        }
        Ok(transferred as usize)
    }
}

impl Read for WindowsOverlappedPipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::io::AsRawHandle as _;
        let mut available = 0;
        if unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                self.handle.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if available == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        self.transfer(
            buffer.as_mut_ptr(),
            buffer.len().min(available as usize),
            false,
        )
    }
}

impl Write for WindowsOverlappedPipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.transfer(buffer.as_ptr().cast_mut(), buffer.len(), true)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl PrivateDuplexStream for WindowsOverlappedPipeStream {
    fn set_nonblocking_private(&self) -> std::io::Result<()> {
        Ok(())
    }
}

struct WindowsPeerOffer {
    binding: StablePipeBinding,
    stream: WindowsOverlappedPipeStream,
    owner_release_policy: PolicyBlob,
    owner_release_policy_signature: PolicySignature,
    deadline: Instant,
}

fn peer_offer(
    accepted: std::os::windows::io::OwnedHandle,
) -> Result<WindowsPeerOffer, WindowsEndpointError> {
    use std::os::windows::io::AsHandle as _;
    let gateway_pid =
        talking_quill_windows_owner_ipc::peer::named_pipe_client_pid(accepted.as_handle())?;
    let gateway =
        talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(gateway_pid)?;
    let owner = talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(unsafe {
        windows_sys::Win32::System::Threading::GetCurrentProcessId()
    })?;
    let release = talking_quill_windows_owner_ipc::installed::InstalledRelease::from_peer_facts(
        &gateway.facts,
        &owner.facts,
        ChannelPurpose::Capture,
    )?;
    if !gateway.still_running() || !owner.still_running() {
        return Err(WindowsEndpointError::Trust);
    }
    let stream = WindowsOverlappedPipeStream {
        handle: accepted,
        _gateway: gateway,
        _owner: owner,
    };
    let policy_proof =
        talking_quill_windows_owner_ipc::installed::protected_policy_proof(&release.binding);
    Ok(WindowsPeerOffer {
        binding: release.binding,
        stream,
        owner_release_policy: PolicyBlob::from_bytes(release.policy)?,
        owner_release_policy_signature: PolicySignature::from_windows_manifest_proof(policy_proof)?,
        deadline: Instant::now() + Duration::from_secs(3),
    })
}

struct ExactStablePipeTrust<'a> {
    binding: &'a StablePipeBinding,
}

impl HandshakeTrustVerifier for ExactStablePipeTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &talking_quill_owner_protocol::ReleasePolicy,
        owner_policy: &talking_quill_owner_protocol::ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let binding = self.binding;
        let expected_policy_proof =
            talking_quill_windows_owner_ipc::installed::protected_policy_proof(binding);
        let purpose = map_purpose(binding.purpose);
        let architecture = map_architecture(binding.architecture);
        let expected_session = session_digest(binding);
        if hello.platform != Platform::Windows
            || challenge.platform != Platform::Windows
            || hello.purpose != purpose
            || challenge.purpose != purpose
            || hello.architecture != architecture
            || challenge.architecture != architecture
            || hello.executable_sha256 != digest(binding.gateway_sha256.as_bytes())
            || challenge.executable_sha256 != digest(binding.owner_sha256.as_bytes())
            || hello.release_build_digest != digest(binding.release_build_digest.as_bytes())
            || challenge.release_build_digest != digest(binding.release_build_digest.as_bytes())
            || hello.installation_identity_digest != install_digest(&binding.installation_id)
            || challenge.installation_identity_digest != install_digest(&binding.installation_id)
            || hello.signer_policy_digest != digest(binding.gateway_role_digest.as_bytes())
            || challenge.signer_policy_digest != digest(binding.owner_role_digest.as_bytes())
            || hello.client_release_policy_digest
                != digest(binding.release_policy_digest.as_bytes())
            || challenge.owner_release_policy_digest
                != digest(binding.release_policy_digest.as_bytes())
            || hello.client_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || challenge.owner_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || hello.os_session_binding_digest != expected_session
            || challenge.os_session_binding_digest != expected_session
            || hello.platform_credential_binding_digest
                != Bytes32::new(binding.credential_binding_digest)
            || challenge.platform_credential_binding_digest
                != Bytes32::new(binding.credential_binding_digest)
            || client_policy.gateway_sha256 != hello.executable_sha256
            || owner_policy.owner_sha256 != challenge.executable_sha256
            || client_policy != owner_policy
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

fn authenticate_owner_peer(
    mut offer: WindowsPeerOffer,
    owner_instance: Bytes32,
    cancelled: &AtomicBool,
) -> Result<AuthenticatedConnection, WindowsEndpointError> {
    let deadline = offer.deadline;
    offer.stream.set_nonblocking_private()?;
    let hello_body = read_handshake_frame(&mut offer.stream, deadline, cancelled)?;
    let HandshakeMessage::Hello(hello) = parse_handshake_json(&hello_body)? else {
        return Err(WindowsEndpointError::HandshakeOrder);
    };
    let binding = &offer.binding;
    let owner_protocol = owner_protocol_header();
    let selected = hello.protocol.negotiate(owner_protocol)?;
    let ephemeral = EphemeralP256Secret::random()?;
    let purpose = map_purpose(binding.purpose);
    if hello.purpose != purpose {
        return Err(WindowsEndpointError::Trust);
    }
    let challenge = Challenge::new(
        owner_protocol,
        selected,
        purpose,
        map_ceiling(binding.purpose),
        Bytes32::random().map_err(|_| WindowsEndpointError::Random)?,
        Bytes32::random().map_err(|_| WindowsEndpointError::Random)?,
        owner_instance,
        Platform::Windows,
        map_architecture(binding.architecture),
        digest(binding.release_build_digest.as_bytes()),
        digest(binding.owner_sha256.as_bytes()),
        install_digest(&binding.installation_id),
        digest(binding.owner_role_digest.as_bytes()),
        session_digest(binding),
        offer.owner_release_policy.clone(),
        offer.owner_release_policy_signature.clone(),
        Bytes32::new(binding.credential_binding_digest),
        Some(ephemeral.public_key().clone()),
    )?;
    write_handshake(
        &mut offer.stream,
        &challenge.to_json()?,
        deadline,
        cancelled,
    )?;
    let transcript = Transcript::build(&TranscriptInput::from_verified_handshake(
        &hello,
        &challenge,
        &ExactStablePipeTrust { binding },
    )?)?;
    let peer_key = hello
        .client_ephemeral_public_key
        .as_ref()
        .ok_or(WindowsEndpointError::Trust)?;
    let material = KeyAgreementMaterial::windows_peer(PeerRole::Owner, &ephemeral, peer_key)?;
    let keys = AuthenticationKeys::derive(&transcript, &material)?;
    let authenticate_body = read_handshake_frame(&mut offer.stream, deadline, cancelled)?;
    let HandshakeMessage::Authenticate(authenticate) = parse_handshake_json(&authenticate_body)?
    else {
        return Err(WindowsEndpointError::HandshakeOrder);
    };
    let session = keys.establish_owner_session(&transcript, &authenticate.client_proof)?;
    let proofs = keys.proofs(&transcript);
    let finish = Authenticated::new(
        selected,
        purpose,
        map_ceiling(binding.purpose),
        proofs.owner_proof,
    )?;
    let finish_body = AuthenticatedEnvelope::authenticated_finish(challenge.session_id, &finish)?
        .encode_body(keys.owner_frame_key())?;
    write_all_until(
        &mut offer.stream,
        &encode_outer_frame(&finish_body)?,
        deadline,
        cancelled,
    )?;
    offer.stream.flush()?;
    let transport = StreamOrderedTransport::new(offer.stream)?;
    let codec = OwnerSessionCodec::new(session, keys.owner_frame_key())?;
    Ok(AuthenticatedConnection::new(Box::new(transport), codec))
}

fn write_handshake(
    stream: &mut impl Write,
    body: &[u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<(), WindowsEndpointError> {
    write_all_until(stream, &encode_outer_frame(body)?, deadline, cancelled)?;
    stream.flush()?;
    Ok(())
}

fn read_handshake_frame(
    stream: &mut impl Read,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, WindowsEndpointError> {
    let mut prefix = [0_u8; 4];
    read_exact_until(stream, &mut prefix, deadline, cancelled)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_BODY_LENGTH {
        return Err(WindowsEndpointError::Framing(
            talking_quill_owner_protocol::framing::FramingError::InvalidLength,
        ));
    }
    let mut body = vec![0_u8; length];
    read_exact_until(stream, &mut body, deadline, cancelled)?;
    Ok(body)
}

fn read_exact_until(
    stream: &mut impl Read,
    buffer: &mut [u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<(), WindowsEndpointError> {
    let mut offset = 0;
    while offset < buffer.len() {
        if cancelled.load(Ordering::Acquire) {
            return Err(WindowsEndpointError::Cancelled);
        }
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => return Err(WindowsEndpointError::PeerClosed),
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(WindowsEndpointError::HandshakeTimeout);
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn write_all_until(
    stream: &mut impl Write,
    buffer: &[u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<(), WindowsEndpointError> {
    let mut offset = 0;
    while offset < buffer.len() {
        if cancelled.load(Ordering::Acquire) {
            return Err(WindowsEndpointError::Cancelled);
        }
        match stream.write(&buffer[offset..]) {
            Ok(0) => return Err(WindowsEndpointError::PeerClosed),
            Ok(written) => offset += written,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(WindowsEndpointError::HandshakeTimeout);
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn owner_protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

fn map_purpose(value: ChannelPurpose) -> Purpose {
    match value {
        ChannelPurpose::Observe => Purpose::Observe,
        ChannelPurpose::Capture => Purpose::Capture,
    }
}

fn map_ceiling(value: ChannelPurpose) -> AuthorityCeiling {
    match value {
        ChannelPurpose::Observe => AuthorityCeiling::Observer,
        ChannelPurpose::Capture => AuthorityCeiling::Capture,
    }
}

fn map_architecture(
    value: talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture,
) -> Architecture {
    match value {
        talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture::X64 => {
            Architecture::X64
        }
        talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture::Arm64 => {
            Architecture::Arm64
        }
    }
}

fn digest(value: &[u8; 32]) -> Bytes32 {
    Bytes32::new(*value)
}

fn install_digest(value: &str) -> Bytes32 {
    use sha2::{Digest as _, Sha256};
    Bytes32::new(Sha256::digest(value.as_bytes()).into())
}

fn session_digest(binding: &StablePipeBinding) -> Bytes32 {
    use sha2::{Digest as _, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"TQKO-WINDOWS-SESSION-V1\0");
    hash.update(binding.session.user_sid_digest);
    hash.update(binding.session.logon_sid_digest);
    hash.update(binding.session.wts_session_id.to_be_bytes());
    hash.update(binding.session.integrity_rid.to_be_bytes());
    hash.update(binding.owner_integrity_rid.to_be_bytes());
    Bytes32::new(hash.finalize().into())
}

#[derive(Debug, Error)]
enum WindowsEndpointError {
    #[error("private owner peer closed during authentication")]
    PeerClosed,
    #[error("private owner handshake order is invalid")]
    HandshakeOrder,
    #[error("private owner handshake timed out")]
    HandshakeTimeout,
    #[error("private owner handshake was cancelled")]
    Cancelled,
    #[error("private owner trust binding is invalid")]
    Trust,
    #[error("operating-system randomness is unavailable")]
    Random,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Framing(#[from] talking_quill_owner_protocol::framing::FramingError),
    #[error(transparent)]
    Schema(#[from] talking_quill_owner_protocol::schema::SchemaError),
    #[error(transparent)]
    Protocol(#[from] talking_quill_owner_protocol::schema::ProtocolSelectionError),
    #[error(transparent)]
    Authentication(#[from] talking_quill_owner_protocol::AuthenticationError),
    #[error(transparent)]
    Envelope(#[from] talking_quill_owner_protocol::envelope::EnvelopeError),
    #[error(transparent)]
    Session(#[from] talking_quill_owner_protocol::SessionCodecError),
    #[error(transparent)]
    Transport(#[from] talking_quill_owner_protocol::TransportError),
    #[error(transparent)]
    Peer(#[from] talking_quill_windows_owner_ipc::peer::PeerError),
    #[error(transparent)]
    Installed(#[from] talking_quill_windows_owner_ipc::installed::InstalledReleaseError),
    #[error(transparent)]
    ReleasePolicy(#[from] talking_quill_owner_protocol::release_policy::ReleasePolicyError),
}
