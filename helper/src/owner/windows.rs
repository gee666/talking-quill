#![cfg(windows)]
//! Windows adjacent keyboard-owner connector.

use std::io::{Read, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::auth::{
    EphemeralP256Secret, HandshakeTrustVerifier, KeyAgreementMaterial, PeerRole,
};
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature, ReleasePolicy};
use talking_quill_owner_protocol::schema::{
    Architecture, Challenge, Hello, Platform, ProtocolHeader, Purpose,
};
use talking_quill_windows_owner_ipc::channel::{ChannelPurpose, StablePipeBinding};
use talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture;

use super::client::{
    CaptureRevocation, ConnectError, ConnectedOwner, OwnerProcessState, OwnerShutdownControl,
};
use super::handshake::{GatewayHandshakeError, authenticate_gateway};

struct ExactStablePipeTrust<'a> {
    binding: &'a StablePipeBinding,
}
impl HandshakeTrustVerifier for ExactStablePipeTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let b = self.binding;
        let expected_policy_proof =
            talking_quill_windows_owner_ipc::installed::protected_policy_proof(b);
        if hello.platform != Platform::Windows
            || challenge.platform != Platform::Windows
            || hello.purpose != purpose(b.purpose)
            || challenge.purpose != purpose(b.purpose)
            || hello.architecture != architecture(b.architecture)
            || challenge.architecture != architecture(b.architecture)
            || hello.executable_sha256 != digest(b.gateway_sha256.as_bytes())
            || challenge.executable_sha256 != digest(b.owner_sha256.as_bytes())
            || hello.release_build_digest != digest(b.release_build_digest.as_bytes())
            || challenge.release_build_digest != digest(b.release_build_digest.as_bytes())
            || hello.installation_identity_digest != install_digest(&b.installation_id)
            || challenge.installation_identity_digest != install_digest(&b.installation_id)
            || hello.signer_policy_digest != digest(b.gateway_role_digest.as_bytes())
            || challenge.signer_policy_digest != digest(b.owner_role_digest.as_bytes())
            || hello.client_release_policy_digest != digest(b.release_policy_digest.as_bytes())
            || challenge.owner_release_policy_digest != digest(b.release_policy_digest.as_bytes())
            || hello.client_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || challenge.owner_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || hello.os_session_binding_digest != session_digest(b)
            || challenge.os_session_binding_digest != session_digest(b)
            || hello.platform_credential_binding_digest != Bytes32::new(b.credential_binding_digest)
            || challenge.platform_credential_binding_digest
                != Bytes32::new(b.credential_binding_digest)
            || client_policy.gateway_sha256 != hello.executable_sha256
            || owner_policy.owner_sha256 != challenge.executable_sha256
            || client_policy != owner_policy
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}
fn purpose(value: ChannelPurpose) -> Purpose {
    match value {
        ChannelPurpose::Observe => Purpose::Observe,
        ChannelPurpose::Capture => Purpose::Capture,
    }
}
fn architecture(value: WindowsArchitecture) -> Architecture {
    match value {
        WindowsArchitecture::X64 => Architecture::X64,
        WindowsArchitecture::Arm64 => Architecture::Arm64,
    }
}
fn digest(value: &[u8; 32]) -> Bytes32 {
    Bytes32::new(*value)
}
fn install_digest(value: &str) -> Bytes32 {
    domain_digest(b"", value)
}
fn domain_digest(domain: &[u8], value: &str) -> Bytes32 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(value.as_bytes());
    Bytes32::new(hash.finalize().into())
}
#[cfg(feature = "windows-installed-acceptance")]
fn lower_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn session_digest(binding: &StablePipeBinding) -> Bytes32 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"TQKO-WINDOWS-SESSION-V1\0");
    hash.update(binding.session.user_sid_digest);
    hash.update(binding.session.logon_sid_digest);
    hash.update(binding.session.wts_session_id.to_be_bytes());
    hash.update(binding.session.integrity_rid.to_be_bytes());
    hash.update(binding.owner_integrity_rid.to_be_bytes());
    Bytes32::new(hash.finalize().into())
}

#[derive(Debug)]
struct ActiveWindowsStream {
    generation: u64,
    reader: usize,
    writer: usize,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct WindowsStreamRegistry {
    active: Mutex<Option<ActiveWindowsStream>>,
    issuing_thread: Mutex<Option<(u64, OwnedHandle)>>,
    issuing_thread_stopped: Condvar,
    next_generation: AtomicU64,
    shutdown_requested: AtomicBool,
}

struct IssuingIoGuard {
    registry: Arc<WindowsStreamRegistry>,
    generation: u64,
}

impl Drop for IssuingIoGuard {
    fn drop(&mut self) {
        if let Ok(mut issuing) = self.registry.issuing_thread.lock()
            && issuing
                .as_ref()
                .is_some_and(|(generation, _)| *generation == self.generation)
        {
            issuing.take();
            self.registry.issuing_thread_stopped.notify_all();
        }
    }
}

#[derive(Debug)]
struct WindowsShutdownControl {
    registry: Arc<WindowsStreamRegistry>,
}

impl OwnerShutdownControl for WindowsShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        self.registry
            .shutdown_requested
            .store(true, Ordering::Release);
        let Ok(mut active) = self.registry.active.lock() else {
            return CaptureRevocation::Failed;
        };
        let Some(active_stream) = active.as_ref() else {
            // No lease can be acquired after shutdown_requested becomes true.
            return CaptureRevocation::Confirmed;
        };
        let generation = active_stream.generation;
        active_stream.cancelled.store(true, Ordering::Release);
        unsafe {
            // CancelIoEx handles overlapped work. CancelSynchronousIo below
            // targets the exact thread that issued a blocking cloned-handle write.
            windows_sys::Win32::System::IO::CancelIoEx(
                active_stream.reader as windows_sys::Win32::Foundation::HANDLE,
                std::ptr::null(),
            );
            windows_sys::Win32::System::IO::CancelIoEx(
                active_stream.writer as windows_sys::Win32::Foundation::HANDLE,
                std::ptr::null(),
            );
        }
        let Ok(mut issuing) = self.registry.issuing_thread.lock() else {
            return CaptureRevocation::Failed;
        };
        if let Some((issuing_generation, thread)) = issuing.as_ref()
            && *issuing_generation == generation
        {
            unsafe {
                windows_sys::Win32::System::IO::CancelSynchronousIo(thread.as_raw_handle());
            }
            let waited = self.registry.issuing_thread_stopped.wait_timeout_while(
                issuing,
                Duration::from_millis(500),
                |value| {
                    value
                        .as_ref()
                        .is_some_and(|(issuing_generation, _)| *issuing_generation == generation)
                },
            );
            let Ok((updated, timeout)) = waited else {
                return CaptureRevocation::Failed;
            };
            issuing = updated;
            if timeout.timed_out()
                && issuing
                    .as_ref()
                    .is_some_and(|(issuing_generation, _)| *issuing_generation == generation)
            {
                return CaptureRevocation::Failed;
            }
        }
        drop(issuing);
        active.take();
        CaptureRevocation::Confirmed
    }
}

pub(crate) struct LocalChildStream {
    reader: std::fs::File,
    writer: std::fs::File,
    registry: Arc<WindowsStreamRegistry>,
    generation: u64,
    cancelled: Arc<AtomicBool>,
    verified_peers: Option<(
        talking_quill_windows_owner_ipc::peer::VerifiedPeer,
        talking_quill_windows_owner_ipc::peer::VerifiedPeer,
    )>,
}

impl LocalChildStream {
    fn new(
        reader: std::fs::File,
        writer: std::fs::File,
        registry: Arc<WindowsStreamRegistry>,
    ) -> Result<Self, ConnectError> {
        if registry.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        let generation = registry.next_generation.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let active = ActiveWindowsStream {
            generation,
            reader: reader.as_raw_handle() as usize,
            writer: writer.as_raw_handle() as usize,
            cancelled: Arc::clone(&cancelled),
        };
        let mut slot = registry
            .active
            .lock()
            .map_err(|_| ConnectError::Unavailable)?;
        if registry.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        if slot.is_some() {
            return Err(ConnectError::Busy);
        }
        *slot = Some(active);
        drop(slot);
        Ok(Self {
            reader,
            writer,
            registry,
            generation,
            cancelled,
            verified_peers: None,
        })
    }

    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }

    fn begin_issuing_io(&self) -> std::io::Result<IssuingIoGuard> {
        self.check_cancelled()?;
        let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThread() };
        let mut duplicate = std::ptr::null_mut();
        let duplicated = unsafe {
            windows_sys::Win32::Foundation::DuplicateHandle(
                process,
                thread,
                process,
                &mut duplicate,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        let mut issuing = self
            .registry
            .issuing_thread
            .lock()
            .map_err(|_| std::io::Error::other("private pipe I/O registry poisoned"))?;
        self.check_cancelled()?;
        if issuing.is_some() {
            return Err(std::io::Error::other("concurrent private pipe I/O"));
        }
        *issuing = Some((self.generation, handle));
        Ok(IssuingIoGuard {
            registry: Arc::clone(&self.registry),
            generation: self.generation,
        })
    }
}

impl Drop for LocalChildStream {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.active.lock()
            && active
                .as_ref()
                .is_some_and(|value| value.generation == self.generation)
        {
            active.take();
        }
    }
}

impl Read for LocalChildStream {
    fn read(&mut self, value: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        let mut available = 0_u32;
        if unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                self.reader.as_raw_handle(),
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
        let length = value.len().min(available as usize);
        self.reader.read(&mut value[..length])
    }
}
impl Write for LocalChildStream {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let guard = self.begin_issuing_io()?;
        let result = self.writer.write(value);
        drop(guard);
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let guard = self.begin_issuing_io()?;
        let result = self.writer.flush();
        drop(guard);
        result
    }
}

#[derive(Debug, Default)]
pub struct LocalOwnerConnector {
    launched: Option<talking_quill_windows_owner_ipc::peer::VerifiedPeer>,
    streams: Arc<WindowsStreamRegistry>,
    #[cfg(feature = "windows-installed-acceptance")]
    acceptance_observability: Option<crate::gateway::AcceptanceEndpointObservability>,
}

struct AuthenticatedLocalOwner {
    connected: ConnectedOwner,
    #[cfg(feature = "windows-installed-acceptance")]
    acceptance_observability: crate::gateway::AcceptanceEndpointObservability,
}

impl super::client::OwnerConnector for LocalOwnerConnector {
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        Arc::new(WindowsShutdownControl {
            registry: Arc::clone(&self.streams),
        })
    }

    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(
        &self,
    ) -> Option<crate::gateway::AcceptanceEndpointObservability> {
        self.acceptance_observability.clone()
    }

    fn owner_process_state(&self) -> OwnerProcessState {
        self.launched
            .as_ref()
            .map_or(OwnerProcessState::Unknown, |owner| {
                if owner.still_running() {
                    OwnerProcessState::Running
                } else {
                    OwnerProcessState::Exited
                }
            })
    }

    #[cfg(feature = "windows-installed-acceptance")]
    fn connect_existing_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        if self.streams.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        let authenticated = connect_stable_owner(Arc::clone(&self.streams), None)?;
        self.acceptance_observability = Some(authenticated.acceptance_observability);
        Ok(authenticated.connected)
    }

    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        if self.streams.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        if let Ok(authenticated) = connect_stable_owner(Arc::clone(&self.streams), None) {
            #[cfg(feature = "windows-installed-acceptance")]
            {
                self.acceptance_observability = Some(authenticated.acceptance_observability);
            }
            return Ok(authenticated.connected);
        }
        let launched = launch_stable_owner()?;
        let expected = (launched.facts.process_id, launched.facts.creation_marker);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match connect_stable_owner(Arc::clone(&self.streams), Some(expected)) {
                Ok(authenticated) => {
                    self.launched = Some(launched);
                    #[cfg(feature = "windows-installed-acceptance")]
                    {
                        self.acceptance_observability =
                            Some(authenticated.acceptance_observability);
                    }
                    return Ok(authenticated.connected);
                }
                Err(_) if launched.still_running() && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn launch_stable_owner() -> Result<talking_quill_windows_owner_ipc::peer::VerifiedPeer, ConnectError>
{
    use std::os::windows::process::CommandExt as _;
    let gateway = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| ConnectError::Unavailable)?;
    let owner = gateway
        .parent()
        .map(|parent| parent.join("talking-quill-keyboard-owner.exe"))
        .ok_or(ConnectError::Unavailable)?
        .canonicalize()
        .map_err(|_| ConnectError::Unavailable)?;
    if owner.parent() != gateway.parent() {
        return Err(ConnectError::Authentication);
    }
    let _locked = open_verified_image(&owner)?;
    let child = std::process::Command::new(owner)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| ConnectError::Unavailable)?;
    talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(child.id())
        .map_err(|_| ConnectError::Authentication)
}

fn connect_stable_owner(
    streams: Arc<WindowsStreamRegistry>,
    expected_launch: Option<(u32, u64)>,
) -> Result<AuthenticatedLocalOwner, ConnectError> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    };
    let name = talking_quill_windows_owner_ipc::endpoint::current_pipe_name()
        .map_err(|_| ConnectError::Unavailable)?;
    let wide: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain([0])
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(ConnectError::Unavailable);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let server_pid =
        talking_quill_windows_owner_ipc::peer::named_pipe_server_pid(handle.as_handle())
            .map_err(|_| ConnectError::Authentication)?;
    let gateway = talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(unsafe {
        windows_sys::Win32::System::Threading::GetCurrentProcessId()
    })
    .map_err(|_| ConnectError::Authentication)?;
    let owner = talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(server_pid)
        .map_err(|_| ConnectError::Authentication)?;
    if expected_launch.is_some_and(|(process_id, creation_marker)| {
        owner.facts.process_id != process_id || owner.facts.creation_marker != creation_marker
    }) {
        return Err(ConnectError::Authentication);
    }
    let release = talking_quill_windows_owner_ipc::installed::InstalledRelease::from_peer_facts(
        &gateway.facts,
        &owner.facts,
        ChannelPurpose::Capture,
    )
    .map_err(|_| ConnectError::Authentication)?;
    if !gateway.still_running() || !owner.still_running() {
        return Err(ConnectError::Authentication);
    }
    let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
    let mut duplicate = std::ptr::null_mut();
    if unsafe {
        windows_sys::Win32::Foundation::DuplicateHandle(
            process,
            handle.as_raw_handle(),
            process,
            &mut duplicate,
            0,
            0,
            windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(ConnectError::Unavailable);
    }
    let mut stream = LocalChildStream::new(
        unsafe { std::fs::File::from_raw_handle(handle.into_raw_handle()) },
        unsafe { std::fs::File::from_raw_handle(duplicate) },
        streams,
    )?;
    stream.verified_peers = Some((gateway, owner));
    authenticate_local_peer(release.binding, release.policy.to_vec(), stream)
}

fn open_verified_image(path: &std::path::Path) -> Result<std::fs::File, ConnectError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(ConnectError::Unavailable);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle).into() })
}

fn authenticate_local_peer(
    binding: StablePipeBinding,
    release_policy: Vec<u8>,
    stream: LocalChildStream,
) -> Result<AuthenticatedLocalOwner, ConnectError> {
    let policy = PolicyBlob::from_bytes(
        release_policy
            .try_into()
            .map_err(|_| ConnectError::Authentication)?,
    )
    .map_err(|_| ConnectError::Authentication)?;
    let signature = PolicySignature::from_windows_manifest_proof(
        talking_quill_windows_owner_ipc::installed::protected_policy_proof(&binding),
    )
    .map_err(|_| ConnectError::Authentication)?;
    let ephemeral = EphemeralP256Secret::random().map_err(|_| ConnectError::Authentication)?;
    let hello = Hello::new(
        Purpose::Capture,
        protocol_header(),
        Bytes32::random().map_err(|_| ConnectError::Authentication)?,
        Platform::Windows,
        architecture(binding.architecture),
        digest(binding.release_build_digest.as_bytes()),
        digest(binding.gateway_sha256.as_bytes()),
        install_digest(&binding.installation_id),
        digest(binding.gateway_role_digest.as_bytes()),
        session_digest(&binding),
        policy,
        signature,
        Bytes32::new(binding.credential_binding_digest),
        Some(ephemeral.public_key().clone()),
    )
    .map_err(|_| ConnectError::Authentication)?;
    let verifier = ExactStablePipeTrust { binding: &binding };
    let client = authenticate_gateway(
        stream,
        hello,
        &verifier,
        |challenge| {
            let peer = challenge
                .owner_ephemeral_public_key
                .as_ref()
                .ok_or(GatewayHandshakeError::Authentication)?;
            KeyAgreementMaterial::windows_peer(PeerRole::Gateway, &ephemeral, peer)
                .map_err(|_| GatewayHandshakeError::Authentication)
        },
        Instant::now() + Duration::from_secs(3),
    )
    .map_err(|_| ConnectError::Authentication)?;
    #[cfg(feature = "windows-installed-acceptance")]
    let acceptance_observability = crate::gateway::AcceptanceEndpointObservability {
        endpoint_version: 2,
        peer_authenticated: true,
        release_build_digest: lower_hex(binding.release_build_digest.as_bytes()),
        manifest_sha256: lower_hex(binding.manifest_sha256.as_bytes()),
        gateway: crate::gateway::AcceptanceEndpointPeerFacts {
            process_id: binding.gateway_process_id,
            creation_marker: binding.gateway_creation_marker.to_string(),
            integrity_rid: binding.session.integrity_rid,
            session_id: binding.session.wts_session_id,
            user_sid_hash: lower_hex(&binding.session.user_sid_digest),
        },
        owner: crate::gateway::AcceptanceEndpointPeerFacts {
            process_id: binding.owner_process_id,
            creation_marker: binding.owner_creation_marker.to_string(),
            integrity_rid: binding.owner_integrity_rid,
            session_id: binding.session.wts_session_id,
            user_sid_hash: lower_hex(&binding.session.user_sid_digest),
        },
    };
    Ok(AuthenticatedLocalOwner {
        connected: ConnectedOwner {
            client,
            build_id: binding.build_id,
        },
        #[cfg(feature = "windows-installed-acceptance")]
        acceptance_observability,
    })
}
