#![cfg(windows)]
//! Windows adjacent keyboard-owner connector.

mod authentication;
mod stream;

use std::os::windows::io::{AsHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

use talking_quill_windows_owner_ipc::channel::ChannelPurpose;

use super::client::{ConnectError, ConnectedOwner, OwnerProcessState, OwnerShutdownControl};
use authentication::authenticate_local_peer;
pub(crate) use stream::LocalChildStream;
use stream::{WindowsShutdownControl, WindowsStreamRegistry};

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
        // Keep the exact process handle even when startup takes longer than one
        // connect attempt. Otherwise retries spawn singleton contenders forever.
        if self
            .launched
            .as_ref()
            .is_none_or(|owner| !owner.still_running())
        {
            self.launched = Some(launch_stable_owner()?);
        }
        let launched = self.launched.as_ref().ok_or(ConnectError::Unavailable)?;
        let expected = (launched.facts.process_id, launched.facts.creation_marker);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match connect_stable_owner(Arc::clone(&self.streams), Some(expected)) {
                Ok(authenticated) => {
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
        // Forward bounded failure diagnostics through the gateway's existing
        // stderr reader, including native owner panics and connection loss.
        .stderr(std::process::Stdio::inherit())
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
