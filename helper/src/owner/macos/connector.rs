#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::auth::KeyAgreementMaterial;
use talking_quill_owner_protocol::schema::{Hello, Platform, Purpose};

use crate::owner::client::{
    CaptureRevocation, ConnectError, ConnectedOwner, OwnerConnector, OwnerShutdownControl,
};
use crate::owner::handshake::authenticate_gateway;

use super::config::{InstalledConfig, current_audit_token};
use super::keychain::read_handshake_secret;
use super::service_management::MacosLoginItemService;

mod socket;
mod trust;

use trust::{
    ExactOwnerTrust, audit_binding, hex, os_session_digest, protocol_header, requirement_digest,
    validate_gateway_bytes,
};

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
