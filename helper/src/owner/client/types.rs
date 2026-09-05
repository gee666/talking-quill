//! Owner connection contracts, errors, and injectable clock.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::client::OwnerProtocolClient;
use talking_quill_owner_protocol::schema::ErrorCode;
use thiserror::Error;

#[doc(hidden)]
pub trait OwnerClock: Send {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

#[doc(hidden)]
pub use OwnerClock as MaintenanceClock;

pub(super) struct SystemOwnerClock;

impl OwnerClock for SystemOwnerClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
/// Authentication result supplied by the platform connector.
pub struct ConnectedOwner {
    pub client: OwnerProtocolClient<'static>,
    /// Bounded, locally verified artifact identity. It is reporting data, not
    /// an authorization decision (authorization completed before this value).
    pub build_id: String,
}

impl std::fmt::Debug for ConnectedOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectedOwner(<redacted>)")
    }
}

/// Platform-specific owner connector. Implementations must return
/// only an authenticated protocol-v1 client. Credentials may not come from
/// argv, environment, stdio, or a user-readable file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureRevocation {
    Confirmed,
    Unavailable,
    Failed,
}

/// Independently owned authority-control or transport-abort path. It must not
/// share locks with connect, renew, or event polling and must return promptly.
pub trait OwnerShutdownControl: Send + Sync {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation;
}

#[derive(Debug, Default)]
struct UnavailableOwnerShutdownControl;

impl OwnerShutdownControl for UnavailableOwnerShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        CaptureRevocation::Unavailable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerProcessState {
    Running,
    Exited,
    Unknown,
}

impl OwnerProcessState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Unknown => "unknown",
        }
    }
}

pub trait OwnerConnector: Send {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError>;
    #[cfg(feature = "windows-installed-acceptance")]
    fn connect_existing_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        Err(ConnectError::Unavailable)
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(
        &self,
    ) -> Option<crate::gateway::AcceptanceEndpointObservability> {
        None
    }
    fn owner_process_state(&self) -> OwnerProcessState {
        OwnerProcessState::Unknown
    }
    fn shutdown_control(&self) -> Arc<dyn OwnerShutdownControl> {
        Arc::new(UnavailableOwnerShutdownControl)
    }
    fn connect_maintenance(&mut self) -> Result<ConnectedOwner, ConnectError> {
        Err(ConnectError::Unavailable)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum ConnectError {
    #[error("keyboard owner endpoint is unavailable")]
    Unavailable,
    #[error("keyboard owner authentication failed")]
    Authentication,
    #[error("keyboard owner is incompatible")]
    Incompatible,
    #[error("keyboard owner is busy or draining")]
    Busy,
    #[error("keyboard owner singleton collision")]
    SingletonCollision,
    #[error("platform connector did not deliver a private connection offer")]
    PrivateOfferUnavailable,
    #[error("macOS owner socket and Keychain identity are not provisioned")]
    MacosProvisioningUnavailable,
    #[error("keyboard owner has no production endpoint on this platform")]
    UnsupportedPlatform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerClientDiagnostic {
    pub category: &'static str,
    pub operation: &'static str,
    pub correlation_status: &'static str,
    pub transport_status: &'static str,
}

#[derive(Debug, Error)]
pub enum OwnerClientError {
    #[error(transparent)]
    Connect(#[from] ConnectError),
    #[error("owner connection closed before a certain response")]
    Disconnected,
    #[error("owner request timed out with an uncertain result")]
    Uncertain,
    #[error("owner protocol response did not match the request")]
    Protocol,
    #[error("owner rejected acquisition before granting a capability: {0:?}")]
    AcquireRejected(ErrorCode),
    #[error("owner rejected the operation: {0:?}")]
    Rejected(ErrorCode),
    #[error("owner command sequence exhausted")]
    SequenceExhausted,
    #[error("owner protocol I/O failed")]
    Transport,
    #[error("owner operation was cancelled before its absolute deadline")]
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerEventDisposition {
    Continue,
    Terminal,
}
