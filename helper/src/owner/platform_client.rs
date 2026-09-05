//! Mapping between owner protocol v1 and Electron-facing gateway semantics.

#[cfg(not(any(windows, target_os = "macos")))]
use super::client::CaptureRevocation;
use super::client::{self, ConnectError, OwnerConnector, OwnerProcessState, OwnerShutdownControl};
use crate::gateway::{CallbackGate, PlatformError};
use actor::{CommandEnvelope, PublishedState};
use crossbeam_channel::{Receiver, Sender};
use events::GatewayEventCounters;
use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    thread::JoinHandle,
    time::Duration,
};
use talking_quill_owner_protocol::{Bytes32, schema as wire};

mod actor;
mod actor_commands;
mod backend;
mod conversions;
mod errors;
mod events;
mod gateway;
#[cfg(all(test, target_os = "macos"))]
mod macos_maintenance_tests;
mod observability;
mod reconcile;

const GATEWAY_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Default)]
pub struct ProductionOwnerConnector {
    #[cfg(windows)]
    windows: super::windows::LocalOwnerConnector,
    #[cfg(target_os = "macos")]
    macos: super::macos::MacosOwnerConnector,
}

pub struct LauncherProvidedOwnerConnector {
    inner: Box<dyn OwnerConnector>,
}

impl std::fmt::Debug for LauncherProvidedOwnerConnector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LauncherProvidedOwnerConnector(<redacted>)")
    }
}

impl LauncherProvidedOwnerConnector {
    #[must_use]
    pub fn new(inner: Box<dyn OwnerConnector>) -> Self {
        Self { inner }
    }
}

impl OwnerConnector for LauncherProvidedOwnerConnector {
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(
        &self,
    ) -> Option<crate::gateway::AcceptanceEndpointObservability> {
        self.inner.acceptance_endpoint_observability()
    }
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        self.inner.shutdown_control()
    }
    fn owner_process_state(&self) -> OwnerProcessState {
        self.inner.owner_process_state()
    }
    fn connect_capture(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        self.inner.connect_capture()
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn connect_existing_capture(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        self.inner.connect_existing_capture()
    }
    fn connect_maintenance(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        self.inner.connect_maintenance()
    }
}

impl OwnerConnector for ProductionOwnerConnector {
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(
        &self,
    ) -> Option<crate::gateway::AcceptanceEndpointObservability> {
        #[cfg(windows)]
        return self.windows.acceptance_endpoint_observability();
        #[cfg(not(windows))]
        return None;
    }
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        #[cfg(windows)]
        return self.windows.shutdown_control();
        #[cfg(target_os = "macos")]
        return self.macos.shutdown_control();
        #[cfg(not(any(windows, target_os = "macos")))]
        return Arc::new(UnsupportedShutdownControl);
    }
    fn owner_process_state(&self) -> OwnerProcessState {
        #[cfg(windows)]
        return self.windows.owner_process_state();
        #[cfg(target_os = "macos")]
        return self.macos.owner_process_state();
        #[cfg(not(any(windows, target_os = "macos")))]
        return OwnerProcessState::Unknown;
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn connect_existing_capture(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        #[cfg(windows)]
        return self.windows.connect_existing_capture();
        #[cfg(not(windows))]
        return Err(ConnectError::UnsupportedPlatform);
    }
    fn connect_capture(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        #[cfg(windows)]
        return self.windows.connect_capture();
        #[cfg(target_os = "macos")]
        return self.macos.connect_capture();
        #[cfg(not(any(windows, target_os = "macos")))]
        return Err(ConnectError::UnsupportedPlatform);
    }
    fn connect_maintenance(&mut self) -> Result<super::client::ConnectedOwner, ConnectError> {
        #[cfg(windows)]
        return Err(ConnectError::Unavailable);
        #[cfg(target_os = "macos")]
        return self.macos.connect_maintenance();
        #[cfg(not(any(windows, target_os = "macos")))]
        return Err(ConnectError::UnsupportedPlatform);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct UnsupportedShutdownControl;
#[cfg(not(any(windows, target_os = "macos")))]
impl OwnerShutdownControl for UnsupportedShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        CaptureRevocation::Unavailable
    }
}

#[doc(hidden)]
pub trait OwnerWorkerSpawner {
    fn spawn(&self, worker: Box<dyn FnOnce() + Send + 'static>) -> std::io::Result<JoinHandle<()>>;
}

struct SystemOwnerWorkerSpawner;
impl OwnerWorkerSpawner for SystemOwnerWorkerSpawner {
    fn spawn(&self, worker: Box<dyn FnOnce() + Send + 'static>) -> std::io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("talking-quill-owner-actor".into())
            .spawn(worker)
    }
}

pub struct OwnerGatewayBackend {
    commands: Sender<CommandEnvelope>,
    published: Arc<Mutex<PublishedState>>,
    admission_gate: Arc<CallbackGate>,
    shutdown_control: Arc<dyn OwnerShutdownControl>,
    shutdown_requested: Arc<AtomicBool>,
    event_counters: Arc<GatewayEventCounters>,
    actor_done: Receiver<()>,
    actor: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for OwnerGatewayBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OwnerGatewayBackend(<redacted>)")
    }
}

#[cfg(target_os = "macos")]
fn maintenance_digest(_domain: &[u8], value: &str) -> Result<Bytes32, PlatformError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PlatformError::OwnerIncompatible);
    }
    let mut bytes = [0_u8; 32];
    for (target, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *target = u8::from_str_radix(
            std::str::from_utf8(pair).map_err(|_| PlatformError::OwnerIncompatible)?,
            16,
        )
        .map_err(|_| PlatformError::OwnerIncompatible)?;
    }
    Ok(Bytes32::new(bytes))
}

#[cfg(not(target_os = "macos"))]
fn maintenance_digest(domain: &[u8], value: &str) -> Result<Bytes32, PlatformError> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(value.as_bytes());
    Ok(Bytes32::new(hash.finalize().into()))
}

fn maintenance_acquire_params(
    request: &crate::gateway::MaintenanceRequest,
) -> Result<wire::MaintenanceAcquireParams, PlatformError> {
    let transaction_id = maintenance_digest(
        b"talking-quill/maintenance-transaction/v1\0",
        &request.transaction_id,
    )?;
    let source_build_digest =
        maintenance_digest(b"talking-quill/build-id/v1\0", &request.source_build_id)?;
    Ok(match request.operation {
        crate::gateway::MaintenanceOperation::Uninstall => {
            wire::MaintenanceAcquireParams::Uninstall {
                transaction_id,
                source_build_digest,
            }
        }
        crate::gateway::MaintenanceOperation::Update
        | crate::gateway::MaintenanceOperation::Rollback => {
            let target_build_digest = maintenance_digest(
                b"talking-quill/build-id/v1\0",
                request
                    .target_build_id
                    .as_deref()
                    .ok_or(PlatformError::OwnerIncompatible)?,
            )?;
            let target_owner_sha256 = Bytes32::new(
                request
                    .target_owner_sha256
                    .ok_or(PlatformError::OwnerIncompatible)?,
            );
            if matches!(
                request.operation,
                crate::gateway::MaintenanceOperation::Update
            ) {
                wire::MaintenanceAcquireParams::Update {
                    transaction_id,
                    source_build_digest,
                    target_build_digest,
                    target_owner_sha256,
                }
            } else {
                wire::MaintenanceAcquireParams::Rollback {
                    transaction_id,
                    source_build_digest,
                    target_build_digest,
                    target_owner_sha256,
                }
            }
        }
    })
}
