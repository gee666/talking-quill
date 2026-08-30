#![cfg(target_os = "macos")]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::auth::{HandshakeTrustVerifier, KeyAgreementMaterial};
use talking_quill_owner_protocol::release_policy::ReleasePolicy;
use talking_quill_owner_protocol::schema::{Challenge, Hello, Platform, ProtocolHeader, Purpose};

use crate::owner::client::{
    CaptureRevocation, ConnectError, ConnectedOwner, OwnerConnector, OwnerShutdownControl,
};
use crate::owner::handshake::authenticate_gateway;

use super::config::{InstalledConfig, current_audit_token};
use super::keychain::read_handshake_secret;
use super::service_management::MacosLoginItemService;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
struct ActiveMacosStream {
    generation: u64,
    fd: RawFd,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct MacosStreamRegistry {
    active: Mutex<Option<ActiveMacosStream>>,
    next_generation: AtomicU64,
    shutdown_requested: AtomicBool,
}

#[derive(Debug, Default)]
pub struct MacosOwnerConnector {
    streams: Arc<MacosStreamRegistry>,
}

impl OwnerConnector for MacosOwnerConnector {
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        Arc::new(MacosShutdownControl {
            streams: Arc::clone(&self.streams),
        })
    }

    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.connect(Purpose::Capture)
    }

    fn connect_maintenance(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.connect(Purpose::Maintenance)
    }
}

impl MacosOwnerConnector {
    #[cfg(test)]
    pub(crate) fn register_test_stream(
        &self,
        stream: UnixStream,
    ) -> Result<impl Read + Write + Send + 'static, ConnectError> {
        MacosOwnerStream::new_capture(stream, Arc::clone(&self.streams))
    }

    #[cfg(test)]
    pub(crate) fn register_test_maintenance_stream(
        &self,
        stream: UnixStream,
    ) -> impl Read + Write + Send + 'static {
        MacosOwnerStream::new_untracked(stream)
    }

    fn connect(&mut self, purpose: Purpose) -> Result<ConnectedOwner, ConnectError> {
        self.check_capture_connect_allowed(purpose)?;
        let removal_pending = super::maintenance::durable_removal_pending()
            .map_err(|_| ConnectError::MacosProvisioningUnavailable)?;
        let config =
            InstalledConfig::load().map_err(|_| ConnectError::MacosProvisioningUnavailable)?;
        self.check_capture_connect_allowed(purpose)?;
        validate_gateway_bytes(&config)?;
        if removal_pending {
            super::maintenance::resume_durable_removal_if_needed(&config)
                .map_err(|_| ConnectError::MacosProvisioningUnavailable)?;
        }
        super::provisioning::provision_if_missing(&config)
            .map_err(|_| ConnectError::MacosProvisioningUnavailable)?;
        MacosLoginItemService
            .ensure_registered(&config)
            .map_err(|_| ConnectError::MacosProvisioningUnavailable)?;
        self.check_capture_connect_allowed(purpose)?;
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let stream = loop {
            match self.connect_socket_bounded(&config.socket_path, purpose, deadline) {
                Ok(stream) => break stream,
                Err(ConnectError::Unavailable) if Instant::now() < deadline => {
                    self.check_capture_connect_allowed(purpose)?;
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        };
        let stream = if matches!(purpose, Purpose::Capture) {
            MacosOwnerStream::new_capture(stream, Arc::clone(&self.streams))?
        } else {
            MacosOwnerStream::new_untracked(stream)
        };
        let token = current_audit_token().map_err(|_| ConnectError::Authentication)?;
        let uid = unsafe { libc::geteuid() };
        let binding = audit_binding(token, uid);
        let hello = Hello::new(
            purpose,
            protocol_header(),
            Bytes32::random().map_err(|_| ConnectError::Authentication)?,
            Platform::Macos,
            config.architecture,
            config.release_build_digest,
            config.gateway.executable_sha256,
            config.installation_identity_digest,
            requirement_digest(&config.gateway.designated_requirement),
            os_session_digest(uid, token[6]),
            config.gateway_release_policy.clone(),
            config.gateway_release_policy_signature.clone(),
            binding,
            None,
        )
        .map_err(|_| ConnectError::Incompatible)?;
        let verifier = ExactOwnerTrust { config: &config };
        let client = authenticate_gateway(
            stream,
            hello,
            &verifier,
            |_| {
                let mut secret = read_handshake_secret()
                    .map_err(|_| crate::owner::handshake::GatewayHandshakeError::Authentication)?;
                Ok(KeyAgreementMaterial::macos(&mut secret))
            },
            Instant::now() + HANDSHAKE_TIMEOUT,
        )
        .map_err(|_| ConnectError::Authentication)?;
        Ok(ConnectedOwner {
            client,
            build_id: hex(config.release_build_digest.as_bytes()),
        })
    }

    fn connect_socket_bounded(
        &self,
        path: &std::path::Path,
        purpose: Purpose,
        deadline: Instant,
    ) -> Result<UnixStream, ConnectError> {
        self.check_capture_connect_allowed(purpose)?;
        let bytes = path.as_os_str().as_bytes();
        let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
        if bytes.is_empty() || bytes.len() >= address.sun_path.len() {
            return Err(ConnectError::Unavailable);
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        address.sun_len = u8::try_from(std::mem::size_of::<libc::sockaddr_un>())
            .map_err(|_| ConnectError::Unavailable)?;
        for (target, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
            *target = source as libc::c_char;
        }
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if raw < 0 {
            return Err(ConnectError::Unavailable);
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } != 0
        {
            return Err(ConnectError::Unavailable);
        }
        let connected = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&raw const address).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if connected != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(ConnectError::Unavailable);
            }
            loop {
                self.check_capture_connect_allowed(purpose)?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ConnectError::Unavailable);
                }
                let mut poll_fd = libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let timeout = remaining.min(Duration::from_millis(25)).as_millis() as libc::c_int;
                let ready = unsafe { libc::poll(&raw mut poll_fd, 1, timeout) };
                if ready < 0 {
                    return Err(ConnectError::Unavailable);
                }
                if ready == 0 {
                    continue;
                }
                let mut socket_error = 0;
                let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                if unsafe {
                    libc::getsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_ERROR,
                        (&raw mut socket_error).cast(),
                        &raw mut length,
                    )
                } != 0
                    || socket_error != 0
                {
                    return Err(ConnectError::Unavailable);
                }
                break;
            }
        }
        Ok(UnixStream::from(fd))
    }

    fn check_capture_connect_allowed(&self, purpose: Purpose) -> Result<(), ConnectError> {
        if matches!(purpose, Purpose::Capture)
            && self.streams.shutdown_requested.load(Ordering::Acquire)
        {
            Err(ConnectError::Unavailable)
        } else {
            Ok(())
        }
    }
}

struct MacosOwnerStream {
    stream: UnixStream,
    registration: Option<(Arc<MacosStreamRegistry>, u64)>,
    cancelled: Arc<AtomicBool>,
}

impl MacosOwnerStream {
    fn new_capture(
        stream: UnixStream,
        streams: Arc<MacosStreamRegistry>,
    ) -> Result<Self, ConnectError> {
        if streams.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        let generation = streams.next_generation.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let active = ActiveMacosStream {
            generation,
            fd: stream.as_raw_fd(),
            cancelled: Arc::clone(&cancelled),
        };
        let mut slot = streams
            .active
            .lock()
            .map_err(|_| ConnectError::Unavailable)?;
        if streams.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        if slot.is_some() {
            return Err(ConnectError::Busy);
        }
        *slot = Some(active);
        drop(slot);
        Ok(Self {
            stream,
            registration: Some((streams, generation)),
            cancelled,
        })
    }

    fn new_untracked(stream: UnixStream) -> Self {
        Self {
            stream,
            registration: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

impl Read for MacosOwnerStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        self.stream.read(buffer)
    }
}

impl Write for MacosOwnerStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.check_cancelled()?;
        self.stream.flush()
    }
}

impl Drop for MacosOwnerStream {
    fn drop(&mut self) {
        let Some((streams, generation)) = self.registration.as_ref() else {
            return;
        };
        if let Ok(mut active) = streams.active.lock()
            && active
                .as_ref()
                .is_some_and(|value| value.generation == *generation)
        {
            active.take();
        }
    }
}

struct MacosShutdownControl {
    streams: Arc<MacosStreamRegistry>,
}
impl OwnerShutdownControl for MacosShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        self.streams
            .shutdown_requested
            .store(true, Ordering::Release);
        let Ok(active) = self.streams.active.try_lock() else {
            return CaptureRevocation::Failed;
        };
        let Some(active) = active.as_ref() else {
            // No capture stream can register after shutdown_requested is set.
            return CaptureRevocation::Confirmed;
        };
        active.cancelled.store(true, Ordering::Release);
        let result = unsafe { libc::shutdown(active.fd, libc::SHUT_RDWR) };
        if result == 0 {
            CaptureRevocation::Confirmed
        } else {
            CaptureRevocation::Failed
        }
    }
}

struct ExactOwnerTrust<'a> {
    config: &'a InstalledConfig,
}
impl HandshakeTrustVerifier for ExactOwnerTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let expected = self
            .config
            .owner_release_policy
            .decode()
            .map_err(|_| talking_quill_owner_protocol::AuthenticationError::Trust)?;
        if challenge.platform != Platform::Macos
            || challenge.architecture != self.config.architecture
            || challenge.release_build_digest != self.config.release_build_digest
            || challenge.executable_sha256 != self.config.owner.executable_sha256
            || challenge.installation_identity_digest != self.config.installation_identity_digest
            || challenge.owner_release_policy != self.config.owner_release_policy
            || challenge.owner_release_policy_signature
                != self.config.owner_release_policy_signature
            || hello.platform_credential_binding_digest
                != challenge.platform_credential_binding_digest
            || owner_policy != &expected
            || client_policy.gateway_sha256 != self.config.gateway.executable_sha256
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

fn validate_gateway_bytes(config: &InstalledConfig) -> Result<(), ConnectError> {
    let current = std::env::current_exe().map_err(|_| ConnectError::Authentication)?;
    let canonical = current
        .canonicalize()
        .map_err(|_| ConnectError::Authentication)?;
    if canonical != config.gateway.canonical_executable_path {
        return Err(ConnectError::Authentication);
    }
    let mut file = File::open(canonical).map_err(|_| ConnectError::Authentication)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ConnectError::Authentication)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if hash.finalize().as_slice() != config.gateway.executable_sha256.as_bytes() {
        return Err(ConnectError::Authentication);
    }
    Ok(())
}

fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

fn audit_binding(token: [u32; 8], uid: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-token-binding/v1\0");
    for word in token {
        digest.update(word.to_be_bytes());
    }
    digest.update(uid.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}
fn os_session_digest(uid: u32, audit: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-session/v1\0");
    digest.update(uid.to_be_bytes());
    digest.update(audit.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}
fn requirement_digest(requirement: &str) -> Bytes32 {
    Bytes32::new(Sha256::digest(requirement.as_bytes()).into())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
