#![cfg(target_os = "macos")]

use std::collections::VecDeque;
use std::ffi::{CString, c_char};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(test)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::auth::{
    AuthenticationKeys, HandshakeTrustVerifier, KeyAgreementMaterial, Transcript, TranscriptInput,
};
use talking_quill_owner_protocol::envelope::AuthenticatedEnvelope;
use talking_quill_owner_protocol::framing::{FramingError, MAX_BODY_LENGTH, encode_outer_frame};
use talking_quill_owner_protocol::release_policy::{OwnerMode, ReleasePolicy};
use talking_quill_owner_protocol::schema::{
    Authenticated, AuthorityCeiling, Challenge, HandshakeMessage, Hello, Platform, ProtocolHeader,
    Purpose, parse_handshake_json,
};
use talking_quill_owner_protocol::{Bytes32, OwnerSessionCodec, StreamOrderedTransport};
use zeroize::Zeroizing;

use crate::runtime::{
    AuthenticatedConnection, AuthenticatedConnectionSource, ConnectionSourceError,
    Fixed32AccumulatorOutcome, accumulate_fixed_32, advance_bounded_write,
};
use crate::state::OwnerInstanceId;

#[cfg(test)]
use super::KeychainStore;
use super::config::MacosEndpointConfig;
use super::native_identity::NativePeerEvidence;
use super::native_keychain::NativeKeychainStore;
use super::runtime_directory::{MacosRuntimeDirectory, SOCKET_FILE, SocketInode};
use super::{AuthorizationPurpose, KeychainAccess, PeerEvidence, PeerRole, requirement_digest};

const MAX_HANDSHAKES: usize = 4;
const MAX_HANDSHAKE_STARTS_PER_SECOND: usize = 16;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const BROKER_CHILD_SOCKET_FD: RawFd = 100;
const BROKER_CHILD_OUTPUT_FD: RawFd = 101;
const BROKER_CHILD_REQUEST_FD: RawFd = 102;
const BROKER_SAFE_SOURCE_FD_MIN: RawFd = 200;
// Darwin's SOCK_CLOEXEC value is part of the socket ABI but is not exposed by
// every libc crate version supported by this workspace.
const DARWIN_SOCK_CLOEXEC: libc::c_int = 0x1000_0000;
const DARWIN_SOCK_NONBLOCK: libc::c_int = 0x2000_0000;
const BROKER_HEADER_BYTES: usize = 16;
const BROKER_REQUEST_PREFIX_BYTES: usize = 37;
const BROKER_MAX_CONFIG_BYTES: usize = 64 * 1024;
const BROKER_RESPONSE_PAYLOAD_BYTES: usize = 96;
const BROKER_RESPONSE_BYTES: usize = BROKER_HEADER_BYTES + BROKER_RESPONSE_PAYLOAD_BYTES;
const BROKER_VERSION: u16 = 1;
const BROKER_REQUEST_MAGIC: &[u8; 8] = b"TQMBREQ1";
const BROKER_RESPONSE_MAGIC: &[u8; 8] = b"TQMBRSP1";
static BROKER_REAPER: OnceLock<Sender<libc::pid_t>> = OnceLock::new();

pub struct MacosAuthenticatedConnectionSource {
    config: Option<Arc<MacosEndpointConfig>>,
    owner_instance: Option<Bytes32>,
    listener: Option<UnixListener>,
    socket_inode: Option<SocketInode>,
    results_tx: Sender<Option<AuthenticatedConnection>>,
    results_rx: Receiver<Option<AuthenticatedConnection>>,
    active: Arc<AtomicUsize>,
    cancellation: Arc<AtomicBool>,
    workers: Vec<HandshakeWorker>,
    recent_handshake_starts: VecDeque<Instant>,
    cleanup_poisoned: bool,
    cleanup_failure: Arc<AtomicBool>,
}

struct HandshakeWorker {
    control: UnixStream,
    join: std::thread::JoinHandle<()>,
}

struct SocketCleanup {
    directory: MacosRuntimeDirectory,
    name: String,
    inode: Option<SocketInode>,
    armed: bool,
    cleanup_failure: Arc<AtomicBool>,
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if self.armed {
            let certain = if let Some(inode) = self.inode {
                matches!(
                    self.directory
                        .unlink_named_socket_if_matches(&self.name, inode),
                    Ok(true)
                )
            } else {
                // Unknown identity can be undiscovered by quarantine, but it
                // cannot be proven deleted. Setup must retain singleton poison.
                let _ = self.directory.quarantine_socket_name(&self.name);
                false
            };
            if !certain {
                self.cleanup_failure.store(true, Ordering::Release);
            }
        }
    }
}

struct ProbeCandidate {
    stream: UnixStream,
    nonce: [u8; 32],
    received: usize,
    last_progress: Instant,
}

impl ProbeCandidate {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            nonce: [0; 32],
            received: 0,
            last_progress: Instant::now(),
        }
    }

    fn receive(&mut self, expected: &Bytes32) -> Fixed32AccumulatorOutcome {
        loop {
            let mut chunk = [0_u8; 32];
            let remaining = self.nonce.len() - self.received;
            let read = unsafe {
                libc::recv(
                    self.stream.as_raw_fd(),
                    chunk.as_mut_ptr().cast(),
                    remaining,
                    libc::MSG_DONTWAIT,
                )
            };
            if read > 0 {
                self.last_progress = Instant::now();
                let start = self.received;
                let disposition = accumulate_fixed_32(
                    &mut self.nonce,
                    &mut self.received,
                    &chunk[..read as usize],
                    expected.as_bytes(),
                );
                debug_assert!(self.received > start);
                if disposition != Fixed32AccumulatorOutcome::Pending {
                    return disposition;
                }
                continue;
            }
            if read == 0 {
                return Fixed32AccumulatorOutcome::Terminal;
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return Fixed32AccumulatorOutcome::Pending,
                _ => return Fixed32AccumulatorOutcome::Terminal,
            }
        }
    }
}

fn probe_state_matches(
    before: Option<SocketInode>,
    expected: SocketInode,
    after: Option<SocketInode>,
    nonce_matches: bool,
) -> bool {
    before == Some(expected) && after == Some(expected) && nonce_matches
}

fn begin_nonblocking_path_connect(
    path: &std::path::Path,
) -> Result<(OwnedFd, bool), super::MacosEndpointError> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() {
        return Err(super::MacosEndpointError::Handshake);
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    address.sun_len = u8::try_from(length).map_err(|_| super::MacosEndpointError::Handshake)?;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) {
        *target = *source as libc::c_char;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | DARWIN_SOCK_CLOEXEC | DARWIN_SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let status = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const address).cast(),
            length as libc::socklen_t,
        )
    };
    if status == 0 {
        return Ok((fd, true));
    }
    if !matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(code) if matches!(code, libc::EINPROGRESS | libc::EALREADY | libc::EWOULDBLOCK)
    ) {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((fd, false))
}

fn connect_so_error(fd: RawFd) -> Result<Option<()>, super::MacosEndpointError> {
    let mut error: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut error).cast(),
            &raw mut length,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    match error {
        0 => Ok(Some(())),
        libc::EINPROGRESS | libc::EALREADY | libc::EWOULDBLOCK => Ok(None),
        value => Err(std::io::Error::from_raw_os_error(value).into()),
    }
}

fn active_nonce_probe(
    listener: &UnixListener,
    directory: &MacosRuntimeDirectory,
    name: &str,
    path: &std::path::Path,
    expected: SocketInode,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    let before = directory.socket_inode_named(name)?;
    let nonce = Bytes32::random().map_err(|_| super::MacosEndpointError::Handshake)?;
    let (probe_fd, mut connected) = begin_nonblocking_path_connect(path)?;
    let mut queued_candidates = Vec::new();
    while !connected {
        require_not_cancelled(deadline, cancellation)?;
        // Free a saturated retained backlog, but retain every accepted stream:
        // connect may complete between poll and accept, so an unclassified
        // stream could already be the nonce-bearing probe.
        while let Ok((queued, _)) = listener.accept() {
            if queued_candidates.len() >= 512 {
                return Err(super::MacosEndpointError::Handshake);
            }
            queued_candidates.push(ProbeCandidate::new(queued));
        }
        let remaining = require_before(deadline)?;
        let timeout = i32::try_from(remaining.as_millis().min(10))
            .unwrap_or(10)
            .max(1);
        let mut descriptor = libc::pollfd {
            fd: probe_fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let status = unsafe { libc::poll(&raw mut descriptor, 1, timeout) };
        if status < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
        if status > 0 {
            connected = connect_so_error(probe_fd.as_raw_fd())?.is_some();
        }
    }
    let probe = unsafe { UnixStream::from_raw_fd(probe_fd.into_raw_fd()) };
    write_fd_until(probe.as_raw_fd(), nonce.as_bytes(), deadline, cancellation)?;
    for candidate in &mut queued_candidates {
        candidate.last_progress = Instant::now();
    }

    let mut matched = None;
    while matched.is_none() {
        require_not_cancelled(deadline, cancellation)?;
        while let Ok((candidate, _)) = listener.accept() {
            if queued_candidates.len() >= 512 {
                return Err(super::MacosEndpointError::Handshake);
            }
            require_cloexec(candidate.as_raw_fd())?;
            queued_candidates.push(ProbeCandidate::new(candidate));
        }
        let now = Instant::now();
        let mut index = 0;
        while index < queued_candidates.len() {
            match queued_candidates[index].receive(&nonce) {
                Fixed32AccumulatorOutcome::Match => {
                    matched = Some(queued_candidates.swap_remove(index).stream);
                    break;
                }
                Fixed32AccumulatorOutcome::Mismatch | Fixed32AccumulatorOutcome::Terminal => {
                    drop(queued_candidates.swap_remove(index));
                }
                Fixed32AccumulatorOutcome::Pending
                    if now.duration_since(queued_candidates[index].last_progress)
                        >= Duration::from_millis(25) =>
                {
                    // Idle expiry is terminal for this candidate and prevents
                    // fragmented hostile streams from owning the total probe deadline.
                    drop(queued_candidates.swap_remove(index));
                }
                Fixed32AccumulatorOutcome::Pending => index += 1,
            }
        }
        if matched.is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let accepted = matched.expect("matched probe stream");
    let (peer_pid, peer_token, peer_uid) =
        super::native_identity::socket_kernel_peer_identity(&accepted)?;
    let current_token = super::current_audit_token()?;
    if peer_pid != std::process::id()
        || peer_uid != unsafe { libc::geteuid() }
        || peer_token != current_token
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    let response: [u8; 32] = Sha256::digest(
        [
            b"talking-quill/socket-path-probe/v1\0".as_slice(),
            nonce.as_bytes(),
        ]
        .concat(),
    )
    .into();
    write_fd_until(accepted.as_raw_fd(), &response, deadline, cancellation)?;
    let mut observed = [0_u8; 32];
    read_fd_exact_until(probe.as_raw_fd(), &mut observed, deadline, cancellation)?;
    let after = directory.socket_inode_named(name)?;
    if probe_state_matches(before, expected, after, true) && observed == response {
        Ok(())
    } else {
        Err(super::MacosEndpointError::Handshake)
    }
}

impl std::fmt::Debug for MacosAuthenticatedConnectionSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MacosAuthenticatedConnectionSource(<redacted>)")
    }
}

impl MacosAuthenticatedConnectionSource {
    #[must_use]
    pub fn new(config: MacosEndpointConfig) -> Self {
        let (results_tx, results_rx) = channel();
        let cleanup_failure = Arc::new(AtomicBool::new(false));
        Self {
            config: Some(Arc::new(config)),
            owner_instance: None,
            listener: None,
            socket_inode: None,
            results_tx,
            results_rx,
            active: Arc::new(AtomicUsize::new(0)),
            cancellation: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            recent_handshake_starts: VecDeque::with_capacity(MAX_HANDSHAKE_STARTS_PER_SECOND),
            cleanup_poisoned: false,
            cleanup_failure,
        }
    }

    fn bind_listener(&mut self) -> Result<(), ConnectionSourceError> {
        let config = self.config.as_ref().ok_or(ConnectionSourceError)?;
        validate_running_owner(config)?;
        config
            .runtime_directory
            .revalidate()
            .map_err(|_| ConnectionSourceError)?;
        if let Some(stale) = config
            .runtime_directory
            .socket_inode()
            .map_err(|_| ConnectionSourceError)?
            && !config
                .runtime_directory
                .unlink_socket_if_matches(stale)
                .map_err(|_| ConnectionSourceError)?
        {
            return Err(ConnectionSourceError);
        }
        let random = Bytes32::random().map_err(|_| ConnectionSourceError)?;
        let suffix = random.as_bytes()[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let temporary_name = format!(".owner-v1.bind-{suffix}.sock");
        if config
            .runtime_directory
            .socket_inode_named(&temporary_name)
            .map_err(|_| ConnectionSourceError)?
            .is_some()
        {
            return Err(ConnectionSourceError);
        }
        let temporary_path = config.runtime_directory.path().join(&temporary_name);
        let listener = UnixListener::bind(&temporary_path).map_err(|_| ConnectionSourceError)?;
        // Arm name cleanup before any fallible identity lookup. Unknown names
        // are quarantined but never unlinked.
        let mut cleanup = SocketCleanup {
            directory: config.runtime_directory.clone(),
            name: temporary_name.clone(),
            inode: None,
            armed: true,
            cleanup_failure: Arc::clone(&self.cleanup_failure),
        };
        let inode = config
            .runtime_directory
            .socket_inode_named(&temporary_name)
            .map_err(|_| ConnectionSourceError)?
            .ok_or(ConnectionSourceError)?;
        cleanup.inode = Some(inode);
        config
            .runtime_directory
            .verify_listener_address(listener.as_raw_fd(), &temporary_path)
            .map_err(|_| ConnectionSourceError)?;
        config
            .runtime_directory
            .set_socket_private(&temporary_name)
            .map_err(|_| ConnectionSourceError)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ConnectionSourceError)?;
        active_nonce_probe(
            &listener,
            &config.runtime_directory,
            &temporary_name,
            &temporary_path,
            inode,
            Instant::now() + Duration::from_secs(1),
            &self.cancellation,
        )
        .map_err(|_| ConnectionSourceError)?;
        config
            .runtime_directory
            .install_socket_name(&temporary_name)
            .map_err(|_| ConnectionSourceError)?;
        cleanup.name = SOCKET_FILE.to_owned();
        config
            .runtime_directory
            .verify_published_socket_inode(inode)
            .map_err(|_| ConnectionSourceError)?;
        active_nonce_probe(
            &listener,
            &config.runtime_directory,
            SOCKET_FILE,
            &config.runtime_directory.socket_path(),
            inode,
            Instant::now() + Duration::from_secs(1),
            &self.cancellation,
        )
        .map_err(|_| ConnectionSourceError)?;
        self.listener = Some(listener);
        self.socket_inode = Some(inode);
        cleanup.armed = false;
        Ok(())
    }

    fn start_handshake(&mut self, stream: UnixStream) -> Result<(), ConnectionSourceError> {
        let config = Arc::clone(self.config.as_ref().ok_or(ConnectionSourceError)?);
        let owner_instance = self.owner_instance.ok_or(ConnectionSourceError)?;
        if self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_HANDSHAKES).then_some(count + 1)
            })
            .is_err()
        {
            return Ok(());
        }
        let sender = self.results_tx.clone();
        let active = Arc::clone(&self.active);
        let cancellation = Arc::clone(&self.cancellation);
        let cleanup_failure = Arc::clone(&self.cleanup_failure);
        let control = stream.try_clone().map_err(|_| {
            self.active.fetch_sub(1, Ordering::AcqRel);
            ConnectionSourceError
        })?;
        let join = std::thread::Builder::new()
            .name("tq-owner-macos-auth".into())
            .spawn(move || {
                let result = super::resilience::isolate_peer_rejection(authenticate(
                    stream,
                    &config,
                    owner_instance,
                    &cancellation,
                    &cleanup_failure,
                ));
                let _ = sender.send(result);
                active.fetch_sub(1, Ordering::AcqRel);
            })
            .map_err(|_| {
                self.active.fetch_sub(1, Ordering::AcqRel);
                ConnectionSourceError
            })?;
        self.workers.push(HandshakeWorker { control, join });
        Ok(())
    }

    fn permit_handshake_start_at(&mut self, now: Instant) -> bool {
        while self
            .recent_handshake_starts
            .front()
            .is_some_and(|started| now.duration_since(*started) >= Duration::from_secs(1))
        {
            self.recent_handshake_starts.pop_front();
        }
        if self.recent_handshake_starts.len() >= MAX_HANDSHAKE_STARTS_PER_SECOND {
            false
        } else {
            self.recent_handshake_starts.push_back(now);
            true
        }
    }

    fn permit_handshake_start(&mut self) -> bool {
        self.permit_handshake_start_at(Instant::now())
    }

    fn published_inode_valid(&self) -> bool {
        matches!(
            (&self.config, self.socket_inode),
            (Some(config), Some(expected))
                if matches!(config.runtime_directory.socket_inode(), Ok(Some(current)) if current == expected)
        )
    }

    fn reap_finished_workers(&mut self) -> Result<(), ConnectionSourceError> {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].join.is_finished() {
                let worker = self.workers.swap_remove(index);
                worker.join.join().map_err(|_| ConnectionSourceError)?;
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}

impl AuthenticatedConnectionSource for MacosAuthenticatedConnectionSource {
    fn bind_owner_instance(&mut self, owner: OwnerInstanceId) -> Result<(), ConnectionSourceError> {
        if self.owner_instance.is_some() {
            return Err(ConnectionSourceError);
        }
        self.owner_instance = Some(Bytes32::new(*owner.as_bytes()));
        self.bind_listener()
    }

    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        if self.cleanup_poisoned {
            return Err(ConnectionSourceError);
        }
        if self.listener.is_some() && !self.published_inode_valid() {
            self.cleanup_poisoned = true;
            self.cancellation.store(true, Ordering::Release);
            self.listener.take();
            return Err(ConnectionSourceError);
        }
        self.reap_finished_workers()?;
        loop {
            match self.results_rx.try_recv() {
                Ok(Some(connection)) => return Ok(Some(connection)),
                Ok(None) => continue, // rejected peers never poison the listener
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(ConnectionSourceError),
            }
        }
        let Some(listener) = &self.listener else {
            return Ok(None);
        };
        match listener.accept() {
            Ok((stream, _)) => {
                if !self.published_inode_valid() {
                    drop(stream);
                    self.cleanup_poisoned = true;
                    self.cancellation.store(true, Ordering::Release);
                    self.listener.take();
                    return Err(ConnectionSourceError);
                }
                if self.permit_handshake_start() {
                    self.start_handshake(stream)?;
                }
                Ok(None)
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(_) => Err(ConnectionSourceError),
        }
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        // Close accept and undiscover the endpoint first. Then shutdown every
        // accepted authentication socket and join every worker. Only success
        // permits ProductionRuntime to release its singleton locks.
        self.cancellation.store(true, Ordering::Release);
        self.listener.take();
        let mut failed = self.cleanup_poisoned || self.cleanup_failure.load(Ordering::Acquire);
        if let (Some(config), Some(inode)) = (&self.config, self.socket_inode.take()) {
            match config.runtime_directory.unlink_socket_if_matches(inode) {
                Ok(true) => {}
                Ok(false) | Err(_) => failed = true,
            }
        }
        for worker in &self.workers {
            let _ = worker.control.shutdown(Shutdown::Both);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while self.workers.iter().any(|worker| !worker.join.is_finished())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        for worker in self.workers.drain(..) {
            if worker.join.is_finished() {
                if worker.join.join().is_err() {
                    failed = true;
                }
            } else {
                // Native identity or Keychain APIs are not cancellable. Keep
                // singleton release poisoned and detach the unfinished worker.
                failed = true;
            }
        }
        while self.results_rx.try_recv().is_ok() {}
        if self.active.load(Ordering::Acquire) != 0 {
            failed = true;
        }
        if failed {
            self.cleanup_poisoned = true;
            Err(ConnectionSourceError)
        } else {
            Ok(())
        }
    }
}

impl Drop for MacosAuthenticatedConnectionSource {
    fn drop(&mut self) {
        let _ = self.shutdown_endpoint();
    }
}

fn validate_running_owner(config: &MacosEndpointConfig) -> Result<(), ConnectionSourceError> {
    if let Some(executable) = &config.validated_owner_executable {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(executable.as_raw_fd(), &raw mut stat) } == 0
            && (stat.st_mode & libc::S_IFMT) == libc::S_IFREG
            && stat.st_size > 0
        {
            return Ok(());
        }
        return Err(ConnectionSourceError);
    }
    #[cfg(test)]
    if config.runtime_directory.path().starts_with(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tmp"),
    ) {
        return Ok(());
    }
    Err(ConnectionSourceError)
}

struct BrokerAuthentication {
    connection_binding: Bytes32,
    secret: Zeroizing<[u8; 32]>,
}

struct BrokerChildGuard {
    pid: Option<libc::pid_t>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrokerWaitState {
    Running,
    ExitedSuccess,
    ExitedFailure,
    NoChild,
}

impl BrokerChildGuard {
    fn observe(&mut self) -> Result<BrokerWaitState, super::MacosEndpointError> {
        let Some(pid) = self.pid else {
            return Ok(BrokerWaitState::NoChild);
        };
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
        if result == pid {
            self.pid = None;
            return Ok(
                if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                    BrokerWaitState::ExitedSuccess
                } else {
                    BrokerWaitState::ExitedFailure
                },
            );
        }
        if result == 0 {
            return Ok(BrokerWaitState::Running);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            self.pid = None;
            Ok(BrokerWaitState::NoChild)
        } else if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(BrokerWaitState::Running)
        } else {
            Err(error.into())
        }
    }
}

impl Drop for BrokerChildGuard {
    fn drop(&mut self) {
        // Probe immediately before signaling. A successful waitpid or ECHILD
        // clears ownership, preventing a recycled PID from ever being killed.
        if !matches!(self.observe(), Ok(BrokerWaitState::Running)) {
            return;
        }
        if let Some(pid) = self.pid.take() {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            enqueue_broker_reap(pid);
        }
    }
}

fn run_native_auth_broker(
    stream: &UnixStream,
    config: &MacosEndpointConfig,
    purpose: Purpose,
    deadline: Instant,
    cancellation: &AtomicBool,
    cleanup_failure: &Arc<AtomicBool>,
) -> Result<BrokerAuthentication, super::MacosEndpointError> {
    require_cloexec(stream.as_raw_fd())?;
    let (request_read, request_write) = atomic_cloexec_socketpair()?;
    let (output_read, output_write) = atomic_cloexec_socketpair()?;
    set_nonblocking(request_write.as_raw_fd())?;
    set_no_sigpipe(request_write.as_raw_fd())?;
    set_nonblocking(output_read.as_raw_fd())?;

    let (broker_listener, mut broker_cleanup, broker_path) = create_broker_identity_listener(
        &config.runtime_directory,
        cleanup_failure,
        deadline,
        cancellation,
    )?;
    let validated_executable = config
        .validated_owner_executable
        .as_ref()
        .ok_or(super::MacosEndpointError::Handshake)?;
    let pid = spawn_broker(
        stream.as_raw_fd(),
        output_write.as_raw_fd(),
        request_read.as_raw_fd(),
        validated_executable.as_raw_fd(),
        &broker_path,
    )?;
    let mut child = BrokerChildGuard { pid: Some(pid) };
    drop(request_read);
    drop(output_write);

    let broker_before = config
        .runtime_directory
        .socket_inode_named(&broker_cleanup.name)?;
    let broker_stream = accept_until(&broker_listener, deadline, cancellation)?;
    let broker_after = config
        .runtime_directory
        .socket_inode_named(&broker_cleanup.name)?;
    if !probe_state_matches(
        broker_before,
        broker_cleanup
            .inode
            .ok_or(super::MacosEndpointError::Handshake)?,
        broker_after,
        true,
    ) {
        return Err(super::MacosEndpointError::Handshake);
    }
    require_not_cancelled(deadline, cancellation)?;
    let (broker_pid, broker_token, broker_uid) =
        super::native_identity::socket_kernel_peer_identity(&broker_stream)?;
    require_not_cancelled(deadline, cancellation)?;
    if broker_pid != pid as u32
        || broker_uid != config.peer_policy.console_uid
        || broker_token.pid() != pid as u32
        || broker_token.effective_uid() != config.peer_policy.console_uid
        || broker_token.audit_session_id() != config.peer_policy.audit_session_id
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    let broker_inode = broker_cleanup
        .inode
        .ok_or(super::MacosEndpointError::Handshake)?;
    if !config
        .runtime_directory
        .unlink_named_socket_if_matches(&broker_cleanup.name, broker_inode)?
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    broker_cleanup.armed = false;

    let nonce = Bytes32::random().map_err(|_| super::MacosEndpointError::Handshake)?;
    let request = encode_broker_request(purpose, nonce, &config.validated_resource_bytes)?;
    write_fd_until(
        request_write.as_raw_fd(),
        request.as_ref(),
        deadline,
        cancellation,
    )?;
    drop(request_write);

    let mut response = Zeroizing::new([0_u8; BROKER_RESPONSE_BYTES]);
    read_fd_exact_until(
        output_read.as_raw_fd(),
        response.as_mut(),
        deadline,
        cancellation,
    )?;
    require_fd_eof(output_read.as_raw_fd(), deadline, cancellation)?;
    wait_for_broker(&mut child, deadline, cancellation)?;
    parse_broker_response(response.as_ref(), purpose, nonce)
}

fn encode_broker_request(
    purpose: Purpose,
    nonce: Bytes32,
    validated_config: &[u8],
) -> Result<Zeroizing<Vec<u8>>, super::MacosEndpointError> {
    if validated_config.is_empty() || validated_config.len() > BROKER_MAX_CONFIG_BYTES {
        return Err(super::MacosEndpointError::Handshake);
    }
    let payload_bytes = BROKER_REQUEST_PREFIX_BYTES + validated_config.len();
    let mut frame = Zeroizing::new(vec![0_u8; BROKER_HEADER_BYTES + payload_bytes]);
    encode_broker_header(
        &mut frame[..BROKER_HEADER_BYTES],
        BROKER_REQUEST_MAGIC,
        purpose,
        payload_bytes,
    );
    frame[BROKER_HEADER_BYTES..BROKER_HEADER_BYTES + 32].copy_from_slice(nonce.as_bytes());
    frame[BROKER_HEADER_BYTES + 32] = purpose_tag(purpose);
    frame[BROKER_HEADER_BYTES + 33..BROKER_HEADER_BYTES + 37]
        .copy_from_slice(&(validated_config.len() as u32).to_be_bytes());
    frame[BROKER_HEADER_BYTES + BROKER_REQUEST_PREFIX_BYTES..].copy_from_slice(validated_config);
    Ok(frame)
}

fn parse_broker_response(
    frame: &[u8],
    purpose: Purpose,
    nonce: Bytes32,
) -> Result<BrokerAuthentication, super::MacosEndpointError> {
    validate_broker_header(
        &frame[..BROKER_HEADER_BYTES],
        BROKER_RESPONSE_MAGIC,
        purpose,
        BROKER_RESPONSE_PAYLOAD_BYTES,
    )?;
    let payload = &frame[BROKER_HEADER_BYTES..];
    if payload[..32] != nonce.as_bytes()[..] {
        return Err(super::MacosEndpointError::Handshake);
    }
    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&payload[32..64]);
    if binding.iter().all(|byte| *byte == 0) {
        return Err(super::MacosEndpointError::Handshake);
    }
    let mut secret = Zeroizing::new([0_u8; 32]);
    secret.copy_from_slice(&payload[64..96]);
    if secret.iter().all(|byte| *byte == 0) {
        return Err(super::MacosEndpointError::Handshake);
    }
    Ok(BrokerAuthentication {
        connection_binding: Bytes32::new(binding),
        secret,
    })
}

fn encode_broker_header(
    header: &mut [u8],
    magic: &[u8; 8],
    purpose: Purpose,
    payload_bytes: usize,
) {
    header[..8].copy_from_slice(magic);
    header[8..10].copy_from_slice(&BROKER_VERSION.to_be_bytes());
    header[10] = purpose_tag(purpose);
    header[11] = 0;
    header[12..16].copy_from_slice(&(payload_bytes as u32).to_be_bytes());
}

fn validate_broker_header(
    header: &[u8],
    magic: &[u8; 8],
    purpose: Purpose,
    payload_bytes: usize,
) -> Result<(), super::MacosEndpointError> {
    if header.len() != BROKER_HEADER_BYTES
        || &header[..8] != magic
        || u16::from_be_bytes(header[8..10].try_into().expect("version bytes")) != BROKER_VERSION
        || header[10] != purpose_tag(purpose)
        || header[11] != 0
        || u32::from_be_bytes(header[12..16].try_into().expect("length bytes")) as usize
            != payload_bytes
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    Ok(())
}

fn purpose_tag(purpose: Purpose) -> u8 {
    match purpose {
        Purpose::Observe => 1,
        Purpose::Capture => 2,
        Purpose::Maintenance => 3,
    }
}

fn purpose_from_tag(tag: u8) -> Result<Purpose, super::MacosEndpointError> {
    match tag {
        1 => Ok(Purpose::Observe),
        2 => Ok(Purpose::Capture),
        3 => Ok(Purpose::Maintenance),
        _ => Err(super::MacosEndpointError::Handshake),
    }
}

fn require_cloexec(fd: RawFd) -> Result<(), super::MacosEndpointError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        Err(super::MacosEndpointError::Handshake)
    } else {
        Ok(())
    }
}

fn atomic_cloexec_socketpair() -> Result<(OwnedFd, OwnedFd), super::MacosEndpointError> {
    let mut descriptors = [0; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | DARWIN_SOCK_CLOEXEC,
            0,
            descriptors.as_mut_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let first = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let second = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    require_cloexec(first.as_raw_fd())?;
    require_cloexec(second.as_raw_fd())?;
    Ok((first, second))
}

fn duplicate_safe_source(fd: RawFd) -> Result<OwnedFd, super::MacosEndpointError> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, BROKER_SAFE_SOURCE_FD_MIN) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
    require_cloexec(duplicate.as_raw_fd())?;
    Ok(duplicate)
}

fn set_no_sigpipe(fd: RawFd) -> Result<(), super::MacosEndpointError> {
    let enabled: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&raw const enabled).cast(),
            std::mem::size_of_val(&enabled) as libc::socklen_t,
        )
    } != 0
    {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

fn set_nonblocking(fd: RawFd) -> Result<(), super::MacosEndpointError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

fn create_broker_identity_listener(
    directory: &MacosRuntimeDirectory,
    cleanup_failure: &Arc<AtomicBool>,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(UnixListener, SocketCleanup, std::path::PathBuf), super::MacosEndpointError> {
    let random = Bytes32::random().map_err(|_| super::MacosEndpointError::Handshake)?;
    let suffix = random.as_bytes()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let name = format!(".broker-v1-{suffix}.sock");
    if directory.socket_inode_named(&name)?.is_some() {
        return Err(super::MacosEndpointError::Handshake);
    }
    let path = directory.path().join(&name);
    let listener = UnixListener::bind(&path)?;
    let mut cleanup = SocketCleanup {
        directory: directory.clone(),
        name: name.clone(),
        inode: None,
        armed: true,
        cleanup_failure: Arc::clone(cleanup_failure),
    };
    let inode = directory
        .socket_inode_named(&name)?
        .ok_or(super::MacosEndpointError::Handshake)?;
    cleanup.inode = Some(inode);
    directory.verify_listener_address(listener.as_raw_fd(), &path)?;
    directory.set_socket_private(&name)?;
    listener.set_nonblocking(true)?;
    active_nonce_probe(
        &listener,
        directory,
        &name,
        &path,
        inode,
        deadline,
        cancellation,
    )?;
    Ok((listener, cleanup, path))
}

fn accept_until(
    listener: &UnixListener,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<UnixStream, super::MacosEndpointError> {
    loop {
        poll_fd_until(listener.as_raw_fd(), libc::POLLIN, deadline, cancellation)?;
        match listener.accept() {
            Ok((stream, _)) => {
                require_cloexec(stream.as_raw_fd())?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn spawn_broker(
    socket_fd: RawFd,
    output_fd: RawFd,
    request_fd: RawFd,
    validated_executable_fd: RawFd,
    identity_socket_path: &std::path::Path,
) -> Result<libc::pid_t, super::MacosEndpointError> {
    // Snapshot every source atomically onto distinct CLOEXEC descriptors above
    // stdio and the fixed child range. File-action order can therefore never
    // overwrite a later source, even when the caller's descriptors are
    // 0/1/2 or 100/101/102.
    let socket_source = duplicate_safe_source(socket_fd)?;
    let output_source = duplicate_safe_source(output_fd)?;
    let request_source = duplicate_safe_source(request_fd)?;
    let executable_source = duplicate_safe_source(validated_executable_fd)?;
    let executable = CString::new(format!("/dev/fd/{}", executable_source.as_raw_fd()))
        .map_err(|_| super::MacosEndpointError::Handshake)?;
    let broker_arg = CString::new("--macos-auth-broker").expect("fixed broker argument");
    let identity_socket_path = CString::new(identity_socket_path.as_os_str().as_bytes())
        .map_err(|_| super::MacosEndpointError::Handshake)?;
    let path_env = CString::new("PATH=/usr/bin:/bin").expect("fixed environment");
    let lang_env = CString::new("LANG=C").expect("fixed environment");
    let dev_null = CString::new("/dev/null").expect("fixed path");
    let mut actions: libc::posix_spawn_file_actions_t = unsafe { std::mem::zeroed() };
    let mut attributes: libc::posix_spawnattr_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::posix_spawn_file_actions_init(&raw mut actions) } != 0 {
        return Err(super::MacosEndpointError::Handshake);
    }
    let mut actions_live = true;
    let initialize = (|| {
        for descriptor in [0, 1, 2] {
            if unsafe {
                libc::posix_spawn_file_actions_addopen(
                    &raw mut actions,
                    descriptor,
                    dev_null.as_ptr(),
                    libc::O_RDWR,
                    0,
                )
            } != 0
            {
                return Err(super::MacosEndpointError::Handshake);
            }
        }
        for (source, target) in [
            (socket_source.as_raw_fd(), BROKER_CHILD_SOCKET_FD),
            (output_source.as_raw_fd(), BROKER_CHILD_OUTPUT_FD),
            (request_source.as_raw_fd(), BROKER_CHILD_REQUEST_FD),
        ] {
            if unsafe { libc::posix_spawn_file_actions_adddup2(&raw mut actions, source, target) }
                != 0
            {
                return Err(super::MacosEndpointError::Handshake);
            }
        }
        if unsafe { libc::posix_spawnattr_init(&raw mut attributes) } != 0 {
            return Err(super::MacosEndpointError::Handshake);
        }
        if unsafe {
            libc::posix_spawnattr_setflags(
                &raw mut attributes,
                libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short,
            )
        } != 0
        {
            unsafe { libc::posix_spawnattr_destroy(&raw mut attributes) };
            return Err(super::MacosEndpointError::Handshake);
        }
        let mut pid = 0;
        let mut argv = [
            executable.as_ptr().cast_mut(),
            broker_arg.as_ptr().cast_mut(),
            identity_socket_path.as_ptr().cast_mut(),
            std::ptr::null_mut::<c_char>(),
        ];
        let mut environment = [
            path_env.as_ptr().cast_mut(),
            lang_env.as_ptr().cast_mut(),
            std::ptr::null_mut::<c_char>(),
        ];
        let status = unsafe {
            libc::posix_spawn(
                &raw mut pid,
                executable.as_ptr(),
                &actions,
                &attributes,
                argv.as_mut_ptr(),
                environment.as_mut_ptr(),
            )
        };
        unsafe { libc::posix_spawnattr_destroy(&raw mut attributes) };
        if status == 0 {
            Ok(pid)
        } else {
            Err(super::MacosEndpointError::Handshake)
        }
    })();
    if actions_live {
        unsafe { libc::posix_spawn_file_actions_destroy(&raw mut actions) };
        actions_live = false;
    }
    debug_assert!(!actions_live);
    initialize
}

fn require_not_cancelled(
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    if cancellation.load(Ordering::Acquire) {
        Err(super::MacosEndpointError::Handshake)
    } else {
        require_before(deadline).map(|_| ())
    }
}

fn poll_fd_until(
    fd: RawFd,
    events: i16,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    loop {
        if cancellation.load(Ordering::Acquire) {
            return Err(super::MacosEndpointError::Handshake);
        }
        let remaining = require_before(deadline)?;
        let timeout = i32::try_from(remaining.as_millis().min(10))
            .unwrap_or(10)
            .max(1);
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let status = unsafe { libc::poll(&raw mut descriptor, 1, timeout) };
        if status > 0 {
            if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                return Err(super::MacosEndpointError::Handshake);
            }
            return Ok(());
        }
        if status < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(std::io::Error::last_os_error().into());
        }
    }
}

fn write_fd_until(
    fd: RawFd,
    bytes: &[u8],
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    let mut offset = 0;
    while offset < bytes.len() {
        poll_fd_until(fd, libc::POLLOUT, deadline, cancellation)?;
        let written =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if written > 0 {
            advance_bounded_write(&mut offset, bytes.len(), written as usize)
                .map_err(|_| super::MacosEndpointError::Handshake)?;
        } else if written < 0
            && !matches!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
            )
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn read_fd_exact_until(
    fd: RawFd,
    bytes: &mut [u8],
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    let mut offset = 0;
    while offset < bytes.len() {
        poll_fd_until(fd, libc::POLLIN | libc::POLLHUP, deadline, cancellation)?;
        let read = unsafe {
            libc::read(
                fd,
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if read > 0 {
            offset += read as usize;
        } else if read == 0 {
            return Err(super::MacosEndpointError::Handshake);
        } else if !matches!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
        ) {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn require_fd_eof(
    fd: RawFd,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    loop {
        poll_fd_until(fd, libc::POLLIN | libc::POLLHUP, deadline, cancellation)?;
        let mut trailing = [0_u8; 1];
        match unsafe { libc::read(fd, trailing.as_mut_ptr().cast(), 1) } {
            0 => return Ok(()),
            1 => return Err(super::MacosEndpointError::Handshake),
            _ if matches!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
            ) => {}
            _ => return Err(std::io::Error::last_os_error().into()),
        }
    }
}

fn wait_for_broker(
    child: &mut BrokerChildGuard,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<(), super::MacosEndpointError> {
    loop {
        if cancellation.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(super::MacosEndpointError::Handshake);
        }
        match child.observe()? {
            BrokerWaitState::ExitedSuccess => return Ok(()),
            BrokerWaitState::ExitedFailure | BrokerWaitState::NoChild => {
                return Err(super::MacosEndpointError::Handshake);
            }
            BrokerWaitState::Running => {}
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn enqueue_broker_reap(pid: libc::pid_t) {
    let sender = BROKER_REAPER.get_or_init(|| {
        let (sender, receiver) = channel();
        std::thread::Builder::new()
            .name("tq-owner-macos-broker-reaper".into())
            .spawn(move || {
                let mut pending = Vec::new();
                loop {
                    if pending.is_empty() {
                        let Ok(pid) = receiver.recv() else { break };
                        pending.push(pid);
                    }
                    while let Ok(pid) = receiver.try_recv() {
                        if !pending.contains(&pid) {
                            pending.push(pid);
                        }
                    }
                    pending.retain(|pid| {
                        let mut status = 0;
                        let result = unsafe { libc::waitpid(*pid, &raw mut status, libc::WNOHANG) };
                        !(result == *pid
                            || (result < 0
                                && std::io::Error::last_os_error().raw_os_error()
                                    == Some(libc::ECHILD)))
                    });
                    if !pending.is_empty() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            })
            .expect("broker reaper thread");
        sender
    });
    let _ = sender.send(pid);
}

pub fn run_native_auth_broker_child(
    identity_socket_path: &std::path::Path,
) -> Result<(), super::MacosEndpointError> {
    // Connect before reading any request or producing any result. The parent
    // authenticates this accepted socket's kernel PID/audit token and exact
    // owner code identity, so the broker cannot authorize itself by assertion.
    let _identity_stream = UnixStream::connect(identity_socket_path)?;
    let mut request_file = unsafe { std::fs::File::from_raw_fd(BROKER_CHILD_REQUEST_FD) };
    let mut header = [0_u8; BROKER_HEADER_BYTES];
    request_file.read_exact(&mut header)?;
    let payload_bytes = u32::from_be_bytes(
        header[12..16]
            .try_into()
            .map_err(|_| super::MacosEndpointError::Handshake)?,
    ) as usize;
    if !(BROKER_REQUEST_PREFIX_BYTES..=BROKER_REQUEST_PREFIX_BYTES + BROKER_MAX_CONFIG_BYTES)
        .contains(&payload_bytes)
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    let mut payload = Zeroizing::new(vec![0_u8; payload_bytes]);
    request_file.read_exact(payload.as_mut())?;
    let mut trailing = [0_u8; 1];
    if request_file.read(&mut trailing)? != 0 {
        return Err(super::MacosEndpointError::Handshake);
    }
    let purpose = purpose_from_tag(payload[32])?;
    validate_broker_header(&header, BROKER_REQUEST_MAGIC, purpose, payload_bytes)?;
    let config_bytes = u32::from_be_bytes(
        payload[33..37]
            .try_into()
            .map_err(|_| super::MacosEndpointError::Handshake)?,
    ) as usize;
    if config_bytes == 0 || config_bytes != payload_bytes - BROKER_REQUEST_PREFIX_BYTES {
        return Err(super::MacosEndpointError::Handshake);
    }
    let mut nonce = [0_u8; 32];
    nonce.copy_from_slice(&payload[..32]);

    let config = MacosEndpointConfig::load_broker_policy_from_validated_bytes(
        &payload[BROKER_REQUEST_PREFIX_BYTES..],
    )?;
    super::native_identity::validate_retained_spawned_process_against(&config.peer_policy.owner)?;
    let stream = unsafe { UnixStream::from_raw_fd(BROKER_CHILD_SOCKET_FD) };
    let mut output = unsafe { std::fs::File::from_raw_fd(BROKER_CHILD_OUTPUT_FD) };
    let role = &config.peer_policy.gateway;
    let native = NativePeerEvidence::acquire(
        &stream,
        role.signing_identifier.clone(),
        role.canonical_executable_path.clone(),
        role.executable_sha256,
        config.peer_policy.release_build_digest,
        role.signing_identity,
    )?;
    let authorization_purpose = match purpose {
        Purpose::Capture => AuthorizationPurpose::Capture,
        Purpose::Observe | Purpose::Maintenance => AuthorizationPurpose::Maintenance,
    };
    let verified = PeerEvidence::new(&native).verify(
        &config.peer_policy,
        PeerRole::Gateway,
        authorization_purpose,
    )?;
    let mut keychain = NativeKeychainStore;
    let secret = Zeroizing::new(
        KeychainAccess::new(&mut keychain, &verified, &config.keychain_policy)
            .handshake_secret()?
            .into_authenticated_handshake_secret(),
    );
    let mut response = Zeroizing::new([0_u8; BROKER_RESPONSE_BYTES]);
    encode_broker_header(
        &mut response[..BROKER_HEADER_BYTES],
        BROKER_RESPONSE_MAGIC,
        purpose,
        BROKER_RESPONSE_PAYLOAD_BYTES,
    );
    response[BROKER_HEADER_BYTES..BROKER_HEADER_BYTES + 32].copy_from_slice(&nonce);
    response[BROKER_HEADER_BYTES + 32..BROKER_HEADER_BYTES + 64]
        .copy_from_slice(verified.connection_binding().as_bytes());
    response[BROKER_HEADER_BYTES + 64..].copy_from_slice(secret.as_ref());
    output.write_all(response.as_ref())?;
    output.flush()?;
    Ok(())
}

fn authenticate(
    mut stream: UnixStream,
    config: &MacosEndpointConfig,
    owner_instance: Bytes32,
    cancellation: &AtomicBool,
    cleanup_failure: &Arc<AtomicBool>,
) -> Result<AuthenticatedConnection, super::MacosEndpointError> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;

    require_before(deadline)?;
    let hello_body = read_outer_frame_until(&mut stream, deadline)?
        .ok_or(super::MacosEndpointError::Handshake)?;
    let HandshakeMessage::Hello(hello) = parse_handshake_json(&hello_body)? else {
        return Err(super::MacosEndpointError::Handshake);
    };
    validate_hello_before_broker(config, &hello)?;
    let broker = run_native_auth_broker(
        &stream,
        config,
        hello.purpose,
        deadline,
        cancellation,
        cleanup_failure,
    )?;
    authenticate_with_material(
        stream,
        config,
        owner_instance,
        hello,
        broker.connection_binding,
        broker.secret,
        deadline,
    )
}

#[cfg(test)]
fn authenticate_verified<S: KeychainStore>(
    stream: UnixStream,
    config: &MacosEndpointConfig,
    owner_instance: Bytes32,
    hello: Hello,
    verified: &super::VerifiedPeer<'_>,
    keychain: &mut S,
    deadline: Instant,
) -> Result<AuthenticatedConnection, super::MacosEndpointError> {
    let secret = Zeroizing::new(
        KeychainAccess::new(keychain, verified, &config.keychain_policy)
            .handshake_secret()?
            .into_authenticated_handshake_secret(),
    );
    authenticate_with_material(
        stream,
        config,
        owner_instance,
        hello,
        verified.connection_binding(),
        secret,
        deadline,
    )
}

fn validate_hello_before_broker(
    config: &MacosEndpointConfig,
    hello: &Hello,
) -> Result<(), super::MacosEndpointError> {
    hello.protocol.negotiate(owner_protocol_header())?;
    let client = hello
        .client_release_policy
        .decode()
        .map_err(|_| super::MacosEndpointError::Handshake)?;
    let owner = config
        .owner_release_policy
        .decode()
        .map_err(|_| super::MacosEndpointError::Handshake)?;
    if hello.client_nonce.as_bytes().iter().all(|byte| *byte == 0)
        || hello
            .platform_credential_binding_digest
            .as_bytes()
            .iter()
            .all(|byte| *byte == 0)
        || hello.platform != Platform::Macos
        || hello.architecture != config.architecture
        || hello.release_build_digest != config.peer_policy.release_build_digest
        || hello.executable_sha256 != config.peer_policy.gateway.executable_sha256
        || hello.installation_identity_digest != config.installation_identity_digest
        || hello.signer_policy_digest
            != requirement_digest(&config.peer_policy.gateway.designated_requirement)
        || hello.os_session_binding_digest
            != os_session_digest(
                config.peer_policy.console_uid,
                config.peer_policy.audit_session_id,
            )
        || hello.client_release_policy != config.gateway_release_policy
        || hello.client_release_policy_signature != config.gateway_release_policy_signature
        || !purpose_mode_valid(hello.purpose, &client, &owner)
    {
        return Err(super::MacosEndpointError::Handshake);
    }
    Ok(())
}

fn authenticate_with_material(
    mut stream: UnixStream,
    config: &MacosEndpointConfig,
    owner_instance: Bytes32,
    hello: Hello,
    connection_binding: Bytes32,
    mut secret: Zeroizing<[u8; 32]>,
    deadline: Instant,
) -> Result<AuthenticatedConnection, super::MacosEndpointError> {
    let owner_protocol = owner_protocol_header();
    let selected = hello.protocol.negotiate(owner_protocol)?;
    let challenge = Challenge::new(
        owner_protocol,
        selected,
        hello.purpose,
        ceiling(hello.purpose),
        Bytes32::random().map_err(|_| super::MacosEndpointError::Handshake)?,
        Bytes32::random().map_err(|_| super::MacosEndpointError::Handshake)?,
        owner_instance,
        Platform::Macos,
        config.architecture,
        config.peer_policy.release_build_digest,
        config.peer_policy.owner.executable_sha256,
        config.installation_identity_digest,
        requirement_digest(&config.peer_policy.owner.designated_requirement),
        os_session_digest(
            config.peer_policy.console_uid,
            config.peer_policy.audit_session_id,
        ),
        config.owner_release_policy.clone(),
        config.owner_release_policy_signature.clone(),
        connection_binding,
        None,
    )?;
    write_handshake(&mut stream, &challenge.to_json()?, deadline)?;

    let trust = ExactMacosTrust {
        config,
        connection_binding,
    };
    let transcript = Transcript::build(&TranscriptInput::from_verified_handshake(
        &hello, &challenge, &trust,
    )?)?;
    let material = KeyAgreementMaterial::macos(&mut secret);
    let keys = AuthenticationKeys::derive(&transcript, &material)?;

    let authenticate_body = read_outer_frame_until(&mut stream, deadline)?
        .ok_or(super::MacosEndpointError::Handshake)?;
    let HandshakeMessage::Authenticate(authenticate) = parse_handshake_json(&authenticate_body)?
    else {
        return Err(super::MacosEndpointError::Handshake);
    };
    let session = keys.establish_owner_session(&transcript, &authenticate.client_proof)?;
    let finish = Authenticated::new(
        selected,
        hello.purpose,
        ceiling(hello.purpose),
        keys.proofs(&transcript).owner_proof,
    )?;
    let finish_body = AuthenticatedEnvelope::authenticated_finish(challenge.session_id, &finish)?
        .encode_body(keys.owner_frame_key())?;
    write_all_until(&mut stream, &encode_outer_frame(&finish_body)?, deadline)?;
    require_before(deadline)?;
    stream.flush()?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    stream.set_nonblocking(true)?;
    let transport = StreamOrderedTransport::new(stream)?;
    let codec = OwnerSessionCodec::new(session, keys.owner_frame_key())?;
    Ok(AuthenticatedConnection::new(Box::new(transport), codec))
}

struct ExactMacosTrust<'a> {
    config: &'a MacosEndpointConfig,
    connection_binding: Bytes32,
}

impl HandshakeTrustVerifier for ExactMacosTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let config = self.config;
        let expected_client = config
            .gateway_release_policy
            .decode()
            .map_err(|_| talking_quill_owner_protocol::AuthenticationError::Trust)?;
        let expected_owner = config
            .owner_release_policy
            .decode()
            .map_err(|_| talking_quill_owner_protocol::AuthenticationError::Trust)?;
        let purpose_mode_valid = purpose_mode_valid(hello.purpose, client_policy, owner_policy);
        if !purpose_mode_valid
            || hello.platform != Platform::Macos
            || challenge.platform != Platform::Macos
            || hello.architecture != config.architecture
            || challenge.architecture != config.architecture
            || hello.release_build_digest != config.peer_policy.release_build_digest
            || challenge.release_build_digest != config.peer_policy.release_build_digest
            || hello.executable_sha256 != config.peer_policy.gateway.executable_sha256
            || challenge.executable_sha256 != config.peer_policy.owner.executable_sha256
            || hello.installation_identity_digest != config.installation_identity_digest
            || challenge.installation_identity_digest != config.installation_identity_digest
            || hello.signer_policy_digest
                != requirement_digest(&config.peer_policy.gateway.designated_requirement)
            || challenge.signer_policy_digest
                != requirement_digest(&config.peer_policy.owner.designated_requirement)
            || hello.os_session_binding_digest
                != os_session_digest(
                    config.peer_policy.console_uid,
                    config.peer_policy.audit_session_id,
                )
            || challenge.os_session_binding_digest != hello.os_session_binding_digest
            || hello.platform_credential_binding_digest != self.connection_binding
            || challenge.platform_credential_binding_digest != self.connection_binding
            || &expected_client != client_policy
            || &expected_owner != owner_policy
            || hello.client_release_policy_signature != config.gateway_release_policy_signature
            || challenge.owner_release_policy_signature != config.owner_release_policy_signature
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

fn require_before(deadline: Instant) -> Result<Duration, super::MacosEndpointError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(super::MacosEndpointError::Handshake)
}

fn read_outer_frame_until(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<Option<Vec<u8>>, super::MacosEndpointError> {
    let mut prefix = [0_u8; 4];
    if !read_exact_until(stream, &mut prefix, deadline, true)? {
        return Ok(None);
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if !(1..=MAX_BODY_LENGTH).contains(&length) {
        return Err(FramingError::InvalidLength.into());
    }
    let mut body = vec![0_u8; length];
    read_exact_until(stream, &mut body, deadline, false)?;
    Ok(Some(body))
}

fn read_exact_until(
    stream: &mut UnixStream,
    buffer: &mut [u8],
    deadline: Instant,
    clean_eof: bool,
) -> Result<bool, super::MacosEndpointError> {
    let mut offset = 0;
    while offset < buffer.len() {
        stream.set_read_timeout(Some(require_before(deadline)?))?;
        match stream.read(&mut buffer[offset..]) {
            Ok(0) if offset == 0 && clean_eof => return Ok(false),
            Ok(0) => return Err(FramingError::Truncated.into()),
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(super::MacosEndpointError::Handshake);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

fn write_all_until(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), super::MacosEndpointError> {
    let mut offset = 0;
    while offset < bytes.len() {
        stream.set_write_timeout(Some(require_before(deadline)?))?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(super::MacosEndpointError::Handshake),
            Ok(written) => offset += written,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(super::MacosEndpointError::Handshake);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn purpose_mode_valid(purpose: Purpose, client: &ReleasePolicy, owner: &ReleasePolicy) -> bool {
    match purpose {
        Purpose::Capture => {
            client == owner
                && client.owner_mode == OwnerMode::EnabledCandidate
                && client.predecessor.is_none()
                && owner.predecessor.is_none()
        }
        Purpose::Observe => client == owner,
        // Protocol-v1 can represent one-hop predecessor maintenance, but R5-M
        // has no authenticated previous-code enrollment. R8-M owns that
        // coordination; this endpoint accepts current exact policy only.
        Purpose::Maintenance => client == owner,
    }
}

fn write_handshake(
    stream: &mut UnixStream,
    body: &[u8],
    deadline: Instant,
) -> Result<(), super::MacosEndpointError> {
    write_all_until(stream, &encode_outer_frame(body)?, deadline)?;
    require_before(deadline)?;
    stream.flush()?;
    Ok(())
}

fn owner_protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

fn ceiling(purpose: Purpose) -> AuthorityCeiling {
    match purpose {
        Purpose::Observe => AuthorityCeiling::Observer,
        Purpose::Capture => AuthorityCeiling::Capture,
        Purpose::Maintenance => AuthorityCeiling::Maintenance,
    }
}

fn os_session_digest(uid: u32, audit_session_id: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-session/v1\0");
    digest.update(uid.to_be_bytes());
    digest.update(audit_session_id.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::fd::{AsRawFd, IntoRawFd};
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use talking_quill_owner_protocol::auth::{AuthenticationKeys, KeyAgreementMaterial};
    use talking_quill_owner_protocol::release_policy::{
        OwnerMode, PolicySignature, ReleasePolicy, ReleasePolicyPredecessor,
    };
    use talking_quill_owner_protocol::schema::{
        Architecture, Authenticate, HandshakeMessage, Hello, Platform, Purpose,
        parse_handshake_json,
    };

    use super::*;
    use crate::macos::{
        AuditToken, CodeIdentity, GATEWAY_SIGNING_IDENTIFIER, KeychainPolicy, KeychainSharingMode,
        LocalSigningIdentity, MacosFlockSingleton, MacosPeerPolicy, OWNER_SIGNING_IDENTIFIER,
        PeerCredentials, PeerEvidenceProvider, RequirementHash, RolePolicy,
        audit_token_connection_binding, self_signed_designated_requirement,
    };
    use crate::runtime::SingletonCoordinator;
    use crate::state::OwnerInstanceId;

    fn role(identifier: &str, path: PathBuf, marker: u8) -> RolePolicy {
        let hash = RequirementHash::new([marker; 20]);
        RolePolicy {
            signing_identifier: identifier.into(),
            designated_requirement: self_signed_designated_requirement(identifier, hash),
            canonical_executable_path: path,
            executable_sha256: Bytes32::new([marker; 32]),
            code_directory_hash: RequirementHash::new([marker; 20]),
            signing_identity: LocalSigningIdentity::LocallyTrustedSelfSigned {
                certificate_sha256: Bytes32::new([marker; 32]),
                requirement_certificate_hash: hash,
            },
            capture_authorized: true,
        }
    }

    struct HandshakeEvidence {
        identity: CodeIdentity,
        requirement: String,
        token: AuditToken,
        binding: Bytes32,
        uid: u32,
    }

    impl PeerEvidenceProvider for HandshakeEvidence {
        fn peer_credentials(&self) -> Result<PeerCredentials, super::super::IdentityError> {
            Ok(PeerCredentials {
                uid: self.uid,
                gid: 20,
            })
        }

        fn peer_pid(&self) -> Result<u32, super::super::IdentityError> {
            Ok(self.token.pid())
        }

        fn peer_audit_token(&self) -> Result<AuditToken, super::super::IdentityError> {
            Ok(self.token)
        }

        fn sec_code_identity(
            &self,
            _token: AuditToken,
        ) -> Result<CodeIdentity, super::super::IdentityError> {
            Ok(self.identity.clone())
        }

        fn evaluate_designated_requirement(
            &self,
            _token: AuditToken,
            requirement: &str,
        ) -> Result<bool, super::super::IdentityError> {
            Ok(requirement == self.requirement)
        }

        fn connection_binding(&self) -> Result<Bytes32, super::super::IdentityError> {
            Ok(self.binding)
        }
    }

    struct HandshakeStore;

    impl KeychainStore for HandshakeStore {
        fn read_owner_handshake_secret_without_ui(
            &self,
        ) -> Result<super::super::KeychainItem, super::super::KeychainError> {
            super::super::KeychainItem::from_bytes([7; 32])
        }

        fn write_maintenance_latch_without_ui(
            &mut self,
            _value: &[u8],
        ) -> Result<(), super::super::KeychainError> {
            Err(super::super::KeychainError::AccessDenied)
        }
    }

    struct AllowTrust;

    impl HandshakeTrustVerifier for AllowTrust {
        fn verify(
            &self,
            _hello: &Hello,
            _challenge: &Challenge,
            _client_policy: &ReleasePolicy,
            _owner_policy: &ReleasePolicy,
        ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
            Ok(())
        }
    }

    fn test_config(name: &str) -> (MacosEndpointConfig, PathBuf) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tmp")
            .join(format!("r5m-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let runtime_directory =
            super::super::runtime_directory::MacosRuntimeDirectory::for_test_path(root.clone())
                .expect("private test runtime");
        let executable = std::env::current_exe()
            .expect("test executable")
            .canonicalize()
            .expect("canonical test executable");
        let peer_policy = MacosPeerPolicy {
            console_uid: runtime_directory.uid(),
            audit_session_id: runtime_directory.audit_session_id(),
            release_build_digest: Bytes32::new([3; 32]),
            gateway: role(GATEWAY_SIGNING_IDENTIFIER, executable.clone(), 4),
            owner: role(OWNER_SIGNING_IDENTIFIER, executable.clone(), 5),
        };
        let mut bridge_policy = role("com.talkingquill.app.service-management", executable, 6);
        bridge_policy.capture_authorized = false;
        let architecture = if cfg!(target_arch = "aarch64") {
            Architecture::Arm64
        } else {
            Architecture::X64
        };
        let protocol = owner_protocol_header();
        let policy = ReleasePolicy {
            platform: Platform::Macos,
            architecture,
            owner_mode: OwnerMode::EnabledCandidate,
            release_build_digest: peer_policy.release_build_digest,
            gateway_sha256: peer_policy.gateway.executable_sha256,
            owner_sha256: peer_policy.owner.executable_sha256,
            gateway_signer_policy_digest: requirement_digest(
                &peer_policy.gateway.designated_requirement,
            ),
            owner_signer_policy_digest: requirement_digest(
                &peer_policy.owner.designated_requirement,
            ),
            gateway_protocol: protocol,
            owner_protocol: protocol,
            predecessor: None,
        }
        .encode()
        .expect("test release policy");
        let bridge_policy_blob = ReleasePolicy {
            platform: Platform::Macos,
            architecture,
            owner_mode: OwnerMode::EnabledCandidate,
            release_build_digest: peer_policy.release_build_digest,
            gateway_sha256: bridge_policy.executable_sha256,
            owner_sha256: bridge_policy.executable_sha256,
            gateway_signer_policy_digest: requirement_digest(&bridge_policy.designated_requirement),
            owner_signer_policy_digest: requirement_digest(&bridge_policy.designated_requirement),
            gateway_protocol: protocol,
            owner_protocol: protocol,
            predecessor: None,
        }
        .encode()
        .expect("test bridge release policy");
        let signature = PolicySignature::from_der(vec![0x30, 0x00]).expect("bounded DER");
        let keychain_policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::RequirementAcl)
                .expect("test keychain policy");
        (
            MacosEndpointConfig {
                runtime_directory,
                peer_policy,
                keychain_policy,
                bridge_policy,
                installation_identity_digest: Bytes32::new([8; 32]),
                gateway_release_policy: policy.clone(),
                gateway_release_policy_signature: signature.clone(),
                owner_release_policy: policy,
                owner_release_policy_signature: signature.clone(),
                bridge_release_policy: bridge_policy_blob,
                bridge_release_policy_signature: signature,
                architecture,
                outer_bundle_path: root.join("Talking Quill.app"),
                validated_owner_executable: None,
                validated_resource_bytes: Vec::new(),
            },
            root,
        )
    }

    #[test]
    fn capture_requires_current_enabled_policy_and_predecessor_maintenance_is_deferred() {
        let (config, root) = test_config("purpose-policy");
        let current = config
            .owner_release_policy
            .decode()
            .expect("current policy");
        assert!(purpose_mode_valid(Purpose::Capture, &current, &current));
        let mut disabled = current.clone();
        disabled.owner_mode = OwnerMode::SafeDisabled;
        assert!(!purpose_mode_valid(Purpose::Capture, &disabled, &disabled));
        assert!(purpose_mode_valid(Purpose::Observe, &disabled, &disabled));
        let mut successor = current.clone();
        successor.predecessor = Some(ReleasePolicyPredecessor {
            release_build_digest: current.release_build_digest,
            gateway_sha256: current.gateway_sha256,
            owner_sha256: current.owner_sha256,
            platform: current.platform,
            architecture: current.architecture,
        });
        assert!(!purpose_mode_valid(
            Purpose::Maintenance,
            &successor,
            &current
        ));
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn client_computable_binding_completes_gateway_owner_handshake() {
        let (config, root) = test_config("e2e-handshake");
        let gateway = &config.peer_policy.gateway;
        let token = AuditToken::new([
            0,
            config.peer_policy.console_uid,
            0,
            0,
            0,
            44,
            config.peer_policy.audit_session_id,
            0,
        ]);
        let binding = audit_token_connection_binding(token, config.peer_policy.console_uid);
        let evidence = HandshakeEvidence {
            identity: CodeIdentity {
                signing_identifier: gateway.signing_identifier.clone(),
                canonical_executable_path: gateway.canonical_executable_path.clone(),
                executable_sha256: gateway.executable_sha256,
                code_directory_hash: gateway.code_directory_hash,
                release_build_digest: config.peer_policy.release_build_digest,
                signing_identity: gateway.signing_identity,
                statically_valid: true,
            },
            requirement: gateway.designated_requirement.clone(),
            token,
            binding,
            uid: config.peer_policy.console_uid,
        };
        let verified = PeerEvidence::new(&evidence)
            .verify(
                &config.peer_policy,
                PeerRole::Gateway,
                AuthorizationPurpose::Capture,
            )
            .expect("accepted-socket peer evidence");
        let protocol = owner_protocol_header();
        let hello = Hello::new(
            Purpose::Capture,
            protocol,
            Bytes32::random().expect("client nonce"),
            Platform::Macos,
            config.architecture,
            config.peer_policy.release_build_digest,
            gateway.executable_sha256,
            config.installation_identity_digest,
            requirement_digest(&gateway.designated_requirement),
            os_session_digest(
                config.peer_policy.console_uid,
                config.peer_policy.audit_session_id,
            ),
            config.gateway_release_policy.clone(),
            config.gateway_release_policy_signature.clone(),
            binding,
            None,
        )
        .expect("gateway hello");
        let (mut owner_stream, mut gateway_stream) =
            UnixStream::pair().expect("gateway/owner stream");
        let client = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            write_handshake(&mut gateway_stream, &hello.to_json().unwrap(), deadline).unwrap();
            let challenge_body = read_outer_frame_until(&mut gateway_stream, deadline)
                .unwrap()
                .unwrap();
            let HandshakeMessage::Challenge(challenge) =
                parse_handshake_json(&challenge_body).unwrap()
            else {
                panic!("owner challenge");
            };
            assert_eq!(challenge.platform_credential_binding_digest, binding);
            let transcript = Transcript::build(
                &TranscriptInput::from_verified_handshake(&hello, &challenge, &AllowTrust).unwrap(),
            )
            .unwrap();
            let mut secret = [7_u8; 32];
            let material = KeyAgreementMaterial::macos(&mut secret);
            let keys = AuthenticationKeys::derive(&transcript, &material).unwrap();
            let proof = keys.proofs(&transcript).client_proof;
            write_handshake(
                &mut gateway_stream,
                &Authenticate::new(proof).to_json().unwrap(),
                deadline,
            )
            .unwrap();
            let finish = read_outer_frame_until(&mut gateway_stream, deadline)
                .unwrap()
                .unwrap();
            keys.pending_gateway_session(&transcript)
                .unwrap()
                .accept_authenticated_finish(&finish, &proof)
                .unwrap();
        });
        let mut store = HandshakeStore;
        let deadline = Instant::now() + Duration::from_secs(2);
        let owner_hello_body = read_outer_frame_until(&mut owner_stream, deadline)
            .expect("owner reads hello")
            .expect("hello frame");
        let HandshakeMessage::Hello(owner_hello) =
            parse_handshake_json(&owner_hello_body).expect("owner parses hello")
        else {
            panic!("gateway hello");
        };
        let connection = authenticate_verified(
            owner_stream,
            &config,
            Bytes32::new([12; 32]),
            owner_hello,
            &verified,
            &mut store,
            deadline,
        );
        assert!(connection.is_ok());
        client.join().expect("gateway handshake");
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn handshake_start_rate_and_worker_count_are_bounded() {
        let (config, root) = test_config("rate-limit");
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        let base = Instant::now();
        for _ in 0..MAX_HANDSHAKE_STARTS_PER_SECOND {
            assert!(source.permit_handshake_start_at(base));
        }
        assert!(!source.permit_handshake_start_at(base + Duration::from_millis(999)));
        assert!(source.permit_handshake_start_at(base + Duration::from_secs(1)));
        source.owner_instance = Some(Bytes32::new([21; 32]));
        source.active.store(MAX_HANDSHAKES, Ordering::Release);
        let (stream, _peer) = UnixStream::pair().expect("worker limit pair");
        source.start_handshake(stream).expect("bounded rejection");
        assert!(source.workers.is_empty());
        assert_eq!(source.active.load(Ordering::Acquire), MAX_HANDSHAKES);
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn broker_response_is_nonce_purpose_version_length_and_trailing_exact() {
        let nonce = Bytes32::new([41; 32]);
        let mut frame = Zeroizing::new([0_u8; BROKER_RESPONSE_BYTES]);
        encode_broker_header(
            &mut frame[..BROKER_HEADER_BYTES],
            BROKER_RESPONSE_MAGIC,
            Purpose::Capture,
            BROKER_RESPONSE_PAYLOAD_BYTES,
        );
        frame[BROKER_HEADER_BYTES..BROKER_HEADER_BYTES + 32].copy_from_slice(nonce.as_bytes());
        frame[BROKER_HEADER_BYTES + 32..BROKER_HEADER_BYTES + 64].fill(42);
        frame[BROKER_HEADER_BYTES + 64..].fill(43);
        assert!(parse_broker_response(frame.as_ref(), Purpose::Capture, nonce).is_ok());
        frame[8] ^= 1;
        assert!(parse_broker_response(frame.as_ref(), Purpose::Capture, nonce).is_err());
        frame[8] ^= 1;
        assert!(
            parse_broker_response(frame.as_ref(), Purpose::Capture, Bytes32::new([44; 32]))
                .is_err()
        );
        assert!(parse_broker_response(frame.as_ref(), Purpose::Observe, nonce).is_err());
        let (read, write) = atomic_cloexec_socketpair().expect("trailing-data socketpair");
        set_nonblocking(read.as_raw_fd()).expect("nonblocking read");
        let mut response_with_trailing = frame.to_vec();
        response_with_trailing.push(1);
        let writer = thread::spawn(move || {
            let mut file = unsafe { std::fs::File::from_raw_fd(write.into_raw_fd()) };
            file.write_all(&response_with_trailing).unwrap();
        });
        let cancellation = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut exact = [0_u8; BROKER_RESPONSE_BYTES];
        read_fd_exact_until(read.as_raw_fd(), &mut exact, deadline, &cancellation).unwrap();
        assert!(require_fd_eof(read.as_raw_fd(), deadline, &cancellation).is_err());
        writer.join().unwrap();
    }

    #[test]
    fn shutdown_cancellation_kills_broker_independently_of_deadline() {
        let child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("blocking broker fixture");
        let pid = child.id() as libc::pid_t;
        drop(child);
        let mut guard = BrokerChildGuard { pid: Some(pid) };
        let cancellation = AtomicBool::new(true);
        let started = Instant::now();
        assert!(
            wait_for_broker(&mut guard, started + Duration::from_secs(10), &cancellation).is_err()
        );
        drop(guard);
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn blocking_native_broker_is_killed_at_absolute_deadline() {
        let child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("blocking broker fixture");
        let pid = child.id() as libc::pid_t;
        drop(child);
        let mut guard = BrokerChildGuard { pid: Some(pid) };
        let cancellation = AtomicBool::new(false);
        let started = Instant::now();
        assert!(
            wait_for_broker(
                &mut guard,
                started + Duration::from_millis(50),
                &cancellation
            )
            .is_err()
        );
        drop(guard);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wait_state_clears_reaped_and_echild_ownership_before_drop() {
        let child = Command::new("/usr/bin/true").spawn().expect("exit fixture");
        let mut guard = BrokerChildGuard {
            pid: Some(child.id() as libc::pid_t),
        };
        drop(child);
        let deadline = Instant::now() + Duration::from_secs(1);
        while guard.observe().unwrap() == BrokerWaitState::Running {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(guard.pid.is_none(), "reaped PID ownership must clear");
        drop(guard);

        let mut not_a_child = BrokerChildGuard {
            pid: Some(unsafe { libc::getpid() }),
        };
        assert_eq!(not_a_child.observe().unwrap(), BrokerWaitState::NoChild);
        assert!(not_a_child.pid.is_none(), "ECHILD ownership must clear");
        drop(not_a_child);
    }

    #[test]
    fn atomic_ipc_and_safe_source_snapshots_cover_closed_stdio_and_fixed_fd_collisions() {
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork fixture");
        if pid == 0 {
            for fd in 0..=2 {
                unsafe { libc::close(fd) };
            }
            let Ok((first, second)) = atomic_cloexec_socketpair() else {
                unsafe { libc::_exit(10) }
            };
            if require_cloexec(first.as_raw_fd()).is_err()
                || require_cloexec(second.as_raw_fd()).is_err()
            {
                unsafe { libc::_exit(11) }
            }
            for (source, target) in [
                (first.as_raw_fd(), BROKER_CHILD_SOCKET_FD),
                (second.as_raw_fd(), BROKER_CHILD_OUTPUT_FD),
                (first.as_raw_fd(), BROKER_CHILD_REQUEST_FD),
            ] {
                if unsafe { libc::dup2(source, target) } != target {
                    unsafe { libc::_exit(12) }
                }
            }
            let snapshots = [
                duplicate_safe_source(BROKER_CHILD_SOCKET_FD),
                duplicate_safe_source(BROKER_CHILD_OUTPUT_FD),
                duplicate_safe_source(BROKER_CHILD_REQUEST_FD),
            ];
            if snapshots.iter().any(Result::is_err) {
                unsafe { libc::_exit(13) }
            }
            let descriptors = snapshots.map(|fd| fd.unwrap().as_raw_fd());
            if descriptors.iter().any(|fd| *fd < BROKER_SAFE_SOURCE_FD_MIN)
                || descriptors[0] == descriptors[1]
                || descriptors[0] == descriptors[2]
                || descriptors[1] == descriptors[2]
            {
                unsafe { libc::_exit(14) }
            }
            unsafe { libc::_exit(0) }
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0);
    }

    #[test]
    fn absolute_handshake_deadline_rejects_slowloris_progress() {
        let (mut owner, mut peer) = UnixStream::pair().expect("slowloris pair");
        let writer = thread::spawn(move || {
            for byte in [0_u8, 0, 0, 8] {
                if peer.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let started = Instant::now();
        assert!(read_outer_frame_until(&mut owner, started + Duration::from_millis(45)).is_err());
        assert!(started.elapsed() < Duration::from_millis(150));
        writer.join().expect("slow writer");
    }

    #[test]
    fn malformed_peers_do_not_kill_the_native_listener() {
        let (config, root) = test_config("listener");
        let socket = config.runtime_directory.socket_path();
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        source
            .bind_owner_instance(OwnerInstanceId::new([9; 32]).expect("owner"))
            .expect("bind native listener");

        for _ in 0..2 {
            let mut peer = UnixStream::connect(&socket).expect("connect malformed peer");
            peer.write_all(&[0, 0, 0, 0]).expect("send invalid frame");
            drop(peer);
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                assert!(source.poll_authenticated().is_ok());
                if source.active.load(Ordering::Acquire) == 0 {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "rejected handshake did not finish"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        drop(source);
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn unidentified_post_bind_cleanup_quarantines_without_unlinking() {
        let (config, root) = test_config("unknown-bind-cleanup");
        let name = ".owner-v1.bind-unknown.sock";
        let path = config.runtime_directory.path().join(name);
        let listener = UnixListener::bind(&path).expect("bind unknown fixture");
        let cleanup_failure = Arc::new(AtomicBool::new(false));
        let guard = SocketCleanup {
            directory: config.runtime_directory.clone(),
            name: name.into(),
            inode: None,
            armed: true,
            cleanup_failure: Arc::clone(&cleanup_failure),
        };
        drop(guard);
        assert!(cleanup_failure.load(Ordering::Acquire));
        assert!(!path.exists());
        drop(listener);
        // Unknown identity is deliberately retained under quarantine rather
        // than risking deletion of a raced replacement.
        assert!(
            fs::read_dir(config.runtime_directory.path())
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".unknown-"))
        );
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn setup_raii_replacement_failure_propagates_sticky_source_poison() {
        let (config, root) = test_config("setup-cleanup-poison");
        let name = ".setup-cleanup.sock";
        let path = config.runtime_directory.path().join(name);
        let original = UnixListener::bind(&path).unwrap();
        let inode = config
            .runtime_directory
            .socket_inode_named(name)
            .unwrap()
            .unwrap();
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        let guard = SocketCleanup {
            directory: source.config.as_ref().unwrap().runtime_directory.clone(),
            name: name.into(),
            inode: Some(inode),
            armed: true,
            cleanup_failure: Arc::clone(&source.cleanup_failure),
        };
        fs::remove_file(&path).unwrap();
        let replacement = UnixListener::bind(&path).unwrap();
        drop(guard);
        assert!(source.shutdown_endpoint().is_err());
        assert!(source.shutdown_endpoint().is_err());
        assert!(UnixStream::connect(&path).is_ok());
        drop(replacement);
        drop(original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn published_listener_accepts_connections_and_removes_its_path() {
        let (config, root) = test_config("bind-connect-remove");
        let socket = config.runtime_directory.socket_path();
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        source
            .bind_owner_instance(OwnerInstanceId::new([31; 32]).expect("owner"))
            .expect("publish listener");
        let peer = UnixStream::connect(&socket).expect("connect published listener");
        drop(peer);
        source.shutdown_endpoint().expect("remove endpoint");
        assert!(!socket.exists());
        drop(source);
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn endpoint_shutdown_closes_and_joins_stalled_authentication_workers() {
        let (config, root) = test_config("quiescence");
        let socket = config.runtime_directory.socket_path();
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        source
            .bind_owner_instance(OwnerInstanceId::new([11; 32]).expect("owner"))
            .expect("bind native listener");
        let _stalled_peer = UnixStream::connect(&socket).expect("connect stalled peer");
        let deadline = Instant::now() + Duration::from_secs(1);
        while source.active.load(Ordering::Acquire) == 0 {
            source.poll_authenticated().expect("accept stalled peer");
            assert!(Instant::now() < deadline, "worker was not accepted");
        }
        source.shutdown_endpoint().expect("quiesce workers");
        assert_eq!(source.active.load(Ordering::Acquire), 0);
        assert!(source.workers.is_empty());
        assert!(!socket.exists());
        drop(source);
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn probe_state_requires_exact_inode_before_after_and_nonce() {
        let expected = SocketInode {
            device: 1,
            inode: 2,
        };
        assert!(probe_state_matches(
            Some(expected),
            expected,
            Some(expected),
            true
        ));
        assert!(!probe_state_matches(None, expected, Some(expected), true));
        assert!(!probe_state_matches(
            Some(expected),
            expected,
            Some(SocketInode {
                device: 1,
                inode: 3
            }),
            true
        ));
        assert!(!probe_state_matches(
            Some(expected),
            expected,
            Some(expected),
            false
        ));
    }

    #[test]
    fn native_probe_candidate_retains_fragmented_stream_nonce() {
        let expected = Bytes32::new([19; 32]);
        let (candidate_stream, mut writer) = UnixStream::pair().unwrap();
        let mut candidate = ProbeCandidate::new(candidate_stream);
        writer.write_all(&[19; 2]).unwrap();
        assert_eq!(
            candidate.receive(&expected),
            Fixed32AccumulatorOutcome::Pending
        );
        assert_eq!(candidate.received, 2);
        writer.write_all(&[19; 11]).unwrap();
        assert_eq!(
            candidate.receive(&expected),
            Fixed32AccumulatorOutcome::Pending
        );
        assert_eq!(candidate.received, 13);
        writer.write_all(&[19; 19]).unwrap();
        assert_eq!(
            candidate.receive(&expected),
            Fixed32AccumulatorOutcome::Match
        );
        assert_eq!(candidate.received, 32);

        let (terminal_stream, terminal_peer) = UnixStream::pair().unwrap();
        let mut terminal = ProbeCandidate::new(terminal_stream);
        drop(terminal_peer);
        assert_eq!(
            terminal.receive(&expected),
            Fixed32AccumulatorOutcome::Terminal
        );
    }

    #[test]
    fn nonblocking_probe_is_bounded_when_path_does_not_reach_retained_listener() {
        let (config, root) = test_config("probe-timeout");
        let retained_name = ".probe-retained.sock";
        let retained_path = config.runtime_directory.path().join(retained_name);
        let retained = UnixListener::bind(&retained_path).unwrap();
        retained.set_nonblocking(true).unwrap();
        let retained_inode = config
            .runtime_directory
            .socket_inode_named(retained_name)
            .unwrap()
            .unwrap();
        let decoy_path = config.runtime_directory.path().join(".probe-decoy.sock");
        let decoy = UnixListener::bind(&decoy_path).unwrap();
        let cancellation = AtomicBool::new(false);
        let started = Instant::now();
        assert!(
            active_nonce_probe(
                &retained,
                &config.runtime_directory,
                retained_name,
                &decoy_path,
                retained_inode,
                started + Duration::from_millis(50),
                &cancellation,
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(decoy);
        drop(retained);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn relay_probe_child_fixture() {
        let Some(socket) = std::env::var_os("TQ_R5M_RELAY_SOCKET") else {
            return;
        };
        let marker = std::path::PathBuf::from(
            std::env::var_os("TQ_R5M_RELAY_MARKER").expect("relay marker"),
        );
        let socket = std::path::PathBuf::from(socket);
        let mut retained = UnixStream::connect(&socket).expect("connect retained listener");
        fs::remove_file(&socket).expect("replace retained pathname");
        let decoy = UnixListener::bind(&socket).expect("bind relay decoy");
        fs::write(&marker, b"ready").expect("publish relay readiness");
        let (mut probe, _) = decoy.accept().expect("accept probe client");
        let mut nonce = [0_u8; 32];
        probe.read_exact(&mut nonce).expect("read probe nonce");
        retained.write_all(&nonce).expect("relay nonce");
        let mut response = [0_u8; 32];
        if retained.read_exact(&mut response).is_ok() {
            let _ = probe.write_all(&response);
        }
    }

    #[test]
    fn active_probe_rejects_a_same_uid_cross_process_nonce_relay() {
        let (config, root) = test_config("probe-relay");
        let name = ".probe-relay.sock";
        let path = config.runtime_directory.path().join(name);
        let marker = root.join("relay-ready");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut relay = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "macos::endpoint::tests::relay_probe_child_fixture",
                "--nocapture",
            ])
            .env("TQ_R5M_RELAY_SOCKET", &path)
            .env("TQ_R5M_RELAY_MARKER", &marker)
            .spawn()
            .expect("spawn relay fixture");
        let ready_deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() {
            assert!(
                Instant::now() < ready_deadline,
                "relay fixture did not start"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let decoy_inode = config
            .runtime_directory
            .socket_inode_named(name)
            .unwrap()
            .unwrap();
        let cancellation = AtomicBool::new(false);
        assert!(
            active_nonce_probe(
                &listener,
                &config.runtime_directory,
                name,
                &path,
                decoy_inode,
                Instant::now() + Duration::from_secs(1),
                &cancellation,
            )
            .is_err()
        );
        let _ = relay.kill();
        let _ = relay.wait();
        drop(listener);
        let _ = fs::remove_file(&path);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonblocking_probe_drains_saturated_backlog_and_honors_cancellation() {
        let (config, root) = test_config("probe-backlog");
        let name = ".probe-backlog.sock";
        let path = config.runtime_directory.path().join(name);
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let inode = config
            .runtime_directory
            .socket_inode_named(name)
            .unwrap()
            .unwrap();
        let mut queued = Vec::new();
        for _ in 0..256 {
            if let Ok((fd, _)) = begin_nonblocking_path_connect(&path) {
                queued.push(fd);
            }
        }
        let cancellation = AtomicBool::new(false);
        active_nonce_probe(
            &listener,
            &config.runtime_directory,
            name,
            &path,
            inode,
            Instant::now() + Duration::from_secs(1),
            &cancellation,
        )
        .unwrap();
        cancellation.store(true, Ordering::Release);
        assert!(
            active_nonce_probe(
                &listener,
                &config.runtime_directory,
                name,
                &path,
                inode,
                Instant::now() + Duration::from_secs(10),
                &cancellation,
            )
            .is_err()
        );
        drop(queued);
        drop(listener);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn endpoint_shutdown_never_unlinks_a_replacement_socket() {
        let (config, root) = test_config("replacement");
        let socket = config.runtime_directory.socket_path();
        let mut source = MacosAuthenticatedConnectionSource::new(config);
        source
            .bind_owner_instance(OwnerInstanceId::new([10; 32]).expect("owner"))
            .expect("bind native listener");
        let replacement_path = socket.clone();
        let (replacement_tx, replacement_rx) = std::sync::mpsc::channel();
        let replacer = thread::spawn(move || {
            fs::remove_file(&replacement_path).expect("replace source socket pathname");
            let replacement =
                UnixListener::bind(&replacement_path).expect("bind replacement socket");
            replacement_tx.send(replacement).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while source.poll_authenticated().is_ok() {
            assert!(Instant::now() < deadline, "replacement was not detected");
            thread::yield_now();
        }
        replacer.join().unwrap();
        let replacement = replacement_rx.recv().unwrap();
        assert!(source.shutdown_endpoint().is_err());
        assert!(
            source.shutdown_endpoint().is_err(),
            "cleanup poison is sticky"
        );
        assert!(UnixStream::connect(&socket).is_ok());
        drop(replacement);
        fs::remove_file(&socket).expect("remove replacement socket");
        drop(source);
        fs::remove_dir_all(root).expect("remove test runtime");
    }

    #[test]
    fn native_flock_singleton_rejects_a_second_owner_and_releases() {
        let (config, root) = test_config("singleton");
        let directory = config.runtime_directory;
        let maintenance = directory
            .open_lock_file(super::super::runtime_directory::MAINTENANCE_FILE)
            .expect("maintenance lock");
        assert_eq!(
            unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_EX) },
            0
        );
        let mut excluded = MacosFlockSingleton::new(directory.clone());
        assert_eq!(excluded.try_acquire(), Ok(false));
        assert_eq!(
            unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_UN) },
            0
        );

        let mut first = MacosFlockSingleton::new(directory.clone());
        let mut second = MacosFlockSingleton::new(directory.clone());
        assert_eq!(first.try_acquire(), Ok(true));
        assert_eq!(second.try_acquire(), Ok(false));

        // Owner retains LOCK_SH for its lifetime, so an R8 maintenance
        // exclusive acquisition cannot enter after owner election.
        let maintenance_racer = directory
            .open_lock_file(super::super::runtime_directory::MAINTENANCE_FILE)
            .expect("maintenance racer lock");
        assert_eq!(
            unsafe { libc::flock(maintenance_racer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB,) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EWOULDBLOCK)
        );

        first.release();
        assert_eq!(
            unsafe { libc::flock(maintenance_racer.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB,) },
            0
        );
        assert_eq!(
            unsafe { libc::flock(maintenance_racer.as_raw_fd(), libc::LOCK_UN) },
            0
        );
        assert_eq!(second.try_acquire(), Ok(true));
        second.release();
        fs::remove_dir_all(root).expect("remove test runtime");
    }
}
