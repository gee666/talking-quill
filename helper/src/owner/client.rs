//! Strict owner-protocol v1 client state machine used by the non-suppressing gateway.
//!
//! Authentication and endpoint discovery are owned by [`OwnerConnector`].  This
//! module starts at an already authenticated stream, never retries a mutation,
//! and makes every new capture connection reconcile disabled-first.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::client::{ClientError, ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::{
    CaptureCommandParams, Empty, ErrorCode, HealthResult, LeaseAcquireResult, LeaseDisposition,
    MaintenanceAcquireParams, MaintenanceAcquireResult, MaintenanceCommandParams,
    MaintenanceOperation, MaintenancePrepareParams, PasteInjectParams, PasteResult,
    PermissionsResult, ReplaceConfigurationParams, Request, Response, SessionMode,
    SessionSetModeParams, SetEnabledParams, SuccessResult,
};
use talking_quill_owner_protocol::{Bytes32, GatewayMessage, U64String};
use thiserror::Error;

pub const OWNER_CALL_TIMEOUT: Duration = Duration::from_secs(2);
pub const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(1);
const MAINTENANCE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const EVENT_PUMP_FRAME_BUDGET: usize = 64;
const EVENT_PUMP_TIME_BUDGET: Duration = Duration::from_millis(5);

#[doc(hidden)]
pub trait OwnerClock: Send {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

#[doc(hidden)]
pub use OwnerClock as MaintenanceClock;

struct SystemOwnerClock;

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

#[cfg(feature = "windows-installed-acceptance")]
pub fn acceptance_observability_without_lease(
    connected: ConnectedOwner,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
    let mut client = connected.client;
    let request = Request::ObservabilityGet(Empty {});
    let correlation = client
        .send_request(&request)
        .map_err(|_| OwnerClientError::Transport)?;
    loop {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            client.abort();
            return Err(OwnerClientError::Cancelled);
        }
        match client.poll() {
            Ok(ClientPoll::Empty) => std::thread::sleep(Duration::from_millis(1)),
            Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
            Ok(ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response,
            })) if correlation_sequence == correlation => match response {
                Response::Success(SuccessResult::Observability(value)) => return Ok(*value),
                Response::Success(_) => return Err(OwnerClientError::Protocol),
                Response::Error(error) => return Err(OwnerClientError::Rejected(error.code())),
            },
            Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                client.abort();
                return Err(OwnerClientError::Protocol);
            }
            Ok(ClientPoll::Message(_)) => {}
            Err(_) => {
                client.abort();
                return Err(OwnerClientError::Transport);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerEventDisposition {
    Continue,
    Terminal,
}

/// One exact-build capture association. Dropping it aborts the connection;
/// callers must explicitly release when they need a reported disposition.
pub struct OwnerCaptureClient {
    client: OwnerProtocolClient<'static>,
    build_id: String,
    lease: LeaseAcquireResult,
    next_command_sequence: u64,
    health: HealthResult,
    permissions: PermissionsResult,
    enabled: bool,
    revision: u64,
    event_handler: Box<dyn FnMut(GatewayMessage) -> OwnerEventDisposition + Send>,
    next_renewal: Instant,
    last_failure: Option<OwnerClientDiagnostic>,
    clock: Box<dyn OwnerClock>,
}

impl std::fmt::Debug for OwnerCaptureClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnerCaptureClient(<redacted>)")
    }
}

impl OwnerCaptureClient {
    /// Connects, acquires a disabled lease, reads authoritative health and
    /// permissions, then consumes command sequence 1 reconciling session capture
    /// off. No enable/configuration can precede this sequence.
    pub fn connect(
        connector: &mut dyn OwnerConnector,
        event_handler: impl FnMut(GatewayMessage) -> OwnerEventDisposition + Send + 'static,
    ) -> Result<Self, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        Self::connect_until(
            connector,
            event_handler,
            Instant::now() + OWNER_CALL_TIMEOUT,
            &cancelled,
        )
    }

    pub fn connect_until(
        connector: &mut dyn OwnerConnector,
        event_handler: impl FnMut(GatewayMessage) -> OwnerEventDisposition + Send + 'static,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<Self, OwnerClientError> {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(OwnerClientError::Cancelled);
        }
        let ConnectedOwner { client, build_id } = connector.connect_capture()?;
        if build_id.is_empty() || build_id.len() > 128 {
            return Err(OwnerClientError::Connect(ConnectError::Incompatible));
        }
        let placeholder = HealthResult {
            owner_instance_id: Bytes32::new([1; 32]),
            reported_state: talking_quill_owner_protocol::schema::OwnerReportedState::Starting,
            process_state: talking_quill_owner_protocol::schema::ProcessState::Starting,
            rollback_latched: false,
            native_state_unknown: true,
            maintenance_sealed: false,
            keyboard_build_eligible: false,
            paste_ready: false,
            permissions_eligible: false,
            hook_healthy: false,
        };
        let permissions = PermissionsResult {
            accessibility: talking_quill_owner_protocol::schema::PermissionState::Unknown,
            input_monitoring: talking_quill_owner_protocol::schema::PermissionState::Unknown,
            event_post: talking_quill_owner_protocol::schema::PermissionState::Unknown,
        };
        let mut this = Self {
            client,
            build_id,
            lease: LeaseAcquireResult {
                capture_lease_id: Bytes32::new([1; 32]),
                capture_lease_epoch: U64String::try_from(1)
                    .map_err(|_| OwnerClientError::Protocol)?,
                state: talking_quill_owner_protocol::schema::AcquireState::Disabled,
            },
            next_command_sequence: 1,
            health: placeholder,
            permissions,
            enabled: false,
            revision: 0,
            event_handler: Box::new(event_handler),
            next_renewal: Instant::now() + LEASE_RENEW_INTERVAL,
            last_failure: None,
            clock: Box::new(SystemOwnerClock),
        };
        this.check_budget(deadline, cancelled)?;
        this.lease = match this.call_until(Request::LeaseAcquire(Empty {}), deadline, cancelled) {
            Ok(SuccessResult::LeaseAcquire(value)) => value,
            Ok(_) => return this.protocol_failure("lease.acquire", "matched"),
            // Any authenticated, correlation-matched semantic error proves
            // that lease.acquire did not grant a capability. The error code
            // affects reporting, not acquisition certainty.
            Err(OwnerClientError::Rejected(code)) => {
                return Err(OwnerClientError::AcquireRejected(code));
            }
            Err(error) => return Err(error),
        };
        let health_result = this.call_until(Request::HealthGet(Empty {}), deadline, cancelled);
        this.health = match health_result {
            Ok(SuccessResult::Health(value)) => value,
            Ok(_) => {
                let error = this
                    .protocol_failure::<()>("health.get", "matched")
                    .unwrap_err();
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
            Err(error) => {
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
        };
        let permissions_result =
            this.call_until(Request::PermissionsGet(Empty {}), deadline, cancelled);
        this.permissions = match permissions_result {
            Ok(SuccessResult::Permissions(value)) => value,
            Ok(_) => {
                let error = this
                    .protocol_failure::<()>("permissions.get", "matched")
                    .unwrap_err();
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
            Err(error) => {
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
        };
        let params = match this.capture_params_without_renewal() {
            Ok(params) => params,
            Err(error) => {
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
        };
        let reconcile_result =
            this.call_until(Request::SessionReconcileOff(params), deadline, cancelled);
        match reconcile_result {
            Ok(SuccessResult::SessionMode(value)) if value.mode == SessionMode::Off => {}
            Ok(_) => {
                let error = this
                    .protocol_failure::<()>("session.reconcile_off", "matched")
                    .unwrap_err();
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
            Err(error) => {
                this.report_post_grant_failure(connector, &error)?;
                return Err(error);
            }
        }
        this.next_renewal = this.clock.now() + LEASE_RENEW_INTERVAL;
        this.last_failure = None;
        Ok(this)
    }

    #[must_use]
    pub fn build_id(&self) -> &str {
        &self.build_id
    }
    #[must_use]
    pub const fn health(&self) -> &HealthResult {
        &self.health
    }
    #[must_use]
    pub const fn permissions(&self) -> &PermissionsResult {
        &self.permissions
    }
    #[must_use]
    pub fn lease_epoch(&self) -> u64 {
        self.lease.capture_lease_epoch.get()
    }
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn last_failure(&self) -> Option<OwnerClientDiagnostic> {
        self.last_failure
    }

    pub fn refresh_health(&mut self) -> Result<&HealthResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.refresh_health_until(deadline, &cancelled)
    }

    pub fn refresh_health_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<&HealthResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        self.health = match self.call_until(Request::HealthGet(Empty {}), deadline, cancelled)? {
            SuccessResult::Health(value) => value,
            _ => return self.protocol_failure("health.get", "matched"),
        };
        Ok(&self.health)
    }

    pub fn refresh_permissions(&mut self) -> Result<&PermissionsResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.refresh_permissions_until(deadline, &cancelled)
    }

    pub fn refresh_permissions_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<&PermissionsResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        self.permissions =
            match self.call_until(Request::PermissionsGet(Empty {}), deadline, cancelled)? {
                SuccessResult::Permissions(value) => value,
                _ => return self.protocol_failure("permissions.get", "matched"),
            };
        Ok(&self.permissions)
    }

    /// Full replacement followed by an explicit enable decision. If capture was
    /// enabled, disabling is a separate preceding mutation. No request is ever
    /// retransmitted after a timeout/disconnect.
    pub fn configure(
        &mut self,
        bindings: talking_quill_owner_protocol::schema::Bindings,
        enabled: bool,
    ) -> Result<(), OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.configure_until(bindings, enabled, deadline, &cancelled)
    }

    pub fn configure_until(
        &mut self,
        bindings: talking_quill_owner_protocol::schema::Bindings,
        enabled: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if self.enabled {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.check_budget(deadline, cancelled)?;
        self.revision = match self.revision.checked_add(1) {
            Some(revision) => revision,
            None => return self.sequence_failure("capture.replace_configuration"),
        };
        let base = self.capture_params_until(deadline, cancelled)?;
        let revision = match U64String::try_from(self.revision) {
            Ok(revision) => revision,
            Err(_) => {
                return self.protocol_failure("capture.replace_configuration", "not_established");
            }
        };
        let params = ReplaceConfigurationParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            revision,
            bindings,
        };
        match self.call_until(
            Request::CaptureReplaceConfiguration(params),
            deadline,
            cancelled,
        )? {
            SuccessResult::Configuration(value) if value.revision.get() == self.revision => {}
            _ => return self.protocol_failure("capture.replace_configuration", "matched"),
        }
        self.check_budget(deadline, cancelled)?;
        self.set_enabled_until(enabled, deadline, cancelled)
    }

    pub fn set_session_mode(&mut self, mode: SessionMode) -> Result<(), OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.set_session_mode_until(mode, deadline, &cancelled)
    }

    pub fn set_session_mode_until(
        &mut self,
        mode: SessionMode,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        let base = self.capture_params_until(deadline, cancelled)?;
        let params = SessionSetModeParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            mode,
        };
        match self.call_until(Request::SessionSetMode(params), deadline, cancelled)? {
            SuccessResult::SessionMode(value) if value.mode == mode => Ok(()),
            _ => self.protocol_failure("session.set_mode", "matched"),
        }
    }

    pub fn paste(&mut self, params: PasteInjectParams) -> Result<PasteResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.paste_until(params, deadline, &cancelled)
    }

    pub fn paste_until(
        &mut self,
        mut params: PasteInjectParams,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<PasteResult, OwnerClientError> {
        self.last_failure = None;
        let operation_id = params.operation_id;
        let base = self.capture_params_until(deadline, cancelled)?;
        params.capture_lease_id = base.capture_lease_id;
        params.capture_lease_epoch = base.capture_lease_epoch;
        params.command_sequence = base.command_sequence;
        match self.call_until(Request::PasteInject(params), deadline, cancelled)? {
            SuccessResult::Paste(PasteResult::Waiting { .. }) => {
                self.wait_for_paste_completion(operation_id, deadline, cancelled)
            }
            SuccessResult::Paste(value) => Ok(value),
            _ => self.protocol_failure("paste.inject", "matched"),
        }
    }

    fn wait_for_paste_completion(
        &mut self,
        operation_id: Bytes32,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<PasteResult, OwnerClientError> {
        self.last_failure = None;
        let mut budget_started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            if let Err(error) = self.check_budget(deadline, cancelled) {
                self.last_failure = Some(OwnerClientDiagnostic {
                    category: "uncertain",
                    operation: "paste.await_commit",
                    correlation_status: "pending",
                    transport_status: "open",
                });
                return Err(error);
            }
            if frames >= EVENT_PUMP_FRAME_BUDGET
                || self.clock.now().saturating_duration_since(budget_started)
                    >= EVENT_PUMP_TIME_BUDGET
            {
                frames = 0;
                budget_started = self.clock.now();
                std::thread::yield_now();
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => self.clock.sleep(Duration::from_millis(1)),
                Ok(ClientPoll::PeerClosed) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "disconnected",
                        operation: "paste.await_commit",
                        correlation_status: "pending",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Event(
                    talking_quill_owner_protocol::schema::Event::PasteCommitted(event),
                ))) if event.operation_id == operation_id => {
                    self.last_failure = None;
                    return Ok(match event.state {
                        talking_quill_owner_protocol::schema::PasteCommitState::Committed => {
                            PasteResult::Committed { operation_id }
                        }
                        talking_quill_owner_protocol::schema::PasteCommitState::Indeterminate => {
                            PasteResult::Indeterminate { operation_id }
                        }
                    });
                }
                Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "protocol",
                        operation: "paste.await_commit",
                        correlation_status: "unexpected_response",
                        transport_status: "open",
                    });
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, "paste.await_commit")?;
                }
                Err(error) => {
                    self.last_failure = Some(client_failure_diagnostic(
                        "paste.await_commit",
                        "pending",
                        &error,
                    ));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub fn renew_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        let params = self.capture_params_without_renewal()?;
        match self.call_until(Request::LeaseRenew(params), deadline, cancelled)? {
            SuccessResult::Renew(value) if value.renewed => {
                self.next_renewal = self.clock.now() + LEASE_RENEW_INTERVAL;
                self.last_failure = None;
                Ok(())
            }
            _ => self.protocol_failure("lease.renew", "matched"),
        }
    }

    #[cfg(feature = "windows-installed-acceptance")]
    pub fn force_capture_safe_disabled_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        if self.enabled {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.set_session_mode_until(SessionMode::Off, deadline, cancelled)
    }

    pub fn runtime_rollback(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.runtime_rollback_until(deadline, &cancelled)
    }

    pub fn runtime_rollback_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.last_failure = None;
        let params = self.capture_params_until(deadline, cancelled)?;
        match self.call_until(Request::RuntimeRollback(params), deadline, cancelled)? {
            SuccessResult::Rollback(value) if value.latched => {
                self.enabled = false;
                Ok(value.disposition)
            }
            _ => self.protocol_failure("runtime.rollback", "matched"),
        }
    }

    /// Pumps unsolicited events and renews under the actor's single internal
    /// command envelope.
    pub fn service_until(
        &mut self,
        now: Instant,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if now >= self.next_renewal {
            self.renew_until(deadline, cancelled)?;
        }
        let started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            self.check_budget(deadline, cancelled)?;
            if frames >= EVENT_PUMP_FRAME_BUDGET
                || self.clock.now().saturating_duration_since(started) >= EVENT_PUMP_TIME_BUDGET
            {
                if self.clock.now() >= self.next_renewal {
                    self.renew_until(deadline, cancelled)?;
                }
                self.last_failure = None;
                return Ok(());
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => {
                    self.last_failure = None;
                    return Ok(());
                }
                Ok(ClientPoll::PeerClosed) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "disconnected",
                        operation: "service.poll",
                        correlation_status: "none",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "protocol",
                        operation: "service.poll",
                        correlation_status: "unexpected_response",
                        transport_status: "open",
                    });
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, "service.poll")?;
                    if self.clock.now() >= self.next_renewal {
                        self.renew_until(deadline, cancelled)?;
                    }
                }
                Err(error) => {
                    self.last_failure =
                        Some(client_failure_diagnostic("service.poll", "none", &error));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub fn release(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.release_with_exit_until(false, deadline, &cancelled)
    }

    pub fn release_and_exit_when_neutral(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.release_and_exit_when_neutral_until(deadline, &cancelled)
    }

    pub fn release_and_exit_when_neutral_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.release_with_exit_until(true, deadline, cancelled)
    }

    fn release_with_exit_until(
        &mut self,
        exit_when_neutral: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.last_failure = None;
        if self.enabled && !exit_when_neutral {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.check_budget(deadline, cancelled)?;
        let params = self.capture_params_until(deadline, cancelled)?;
        let request = if exit_when_neutral {
            Request::OwnerExitWhenNeutral(params)
        } else {
            Request::LeaseRelease(params)
        };
        match self.call_until(request, deadline, cancelled)? {
            SuccessResult::Release(value)
                if exit_when_neutral && value.disposition == LeaseDisposition::Draining =>
            {
                self.wait_for_planned_neutral(deadline, cancelled)
            }
            SuccessResult::Release(value) => Ok(value.disposition),
            _ => self.protocol_failure("lease.release", "matched"),
        }
    }

    fn wait_for_planned_neutral(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        loop {
            self.check_budget(deadline, cancelled)?;
            match self.client.poll() {
                Ok(ClientPoll::Empty) => self.clock.sleep(Duration::from_millis(1)),
                Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(
                    talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseNeutral {
                        ..
                    },
                ))) => return Ok(LeaseDisposition::Neutral),
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(
                    talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseDraining {
                        ..
                    },
                ))) => {}
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(_))) => {
                    self.client.abort();
                    return Err(OwnerClientError::Uncertain);
                }
                Ok(ClientPoll::Message(message)) => {
                    self.handle_unsolicited(message, "owner.exit_when_neutral")?;
                }
                Err(_) => {
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub fn front_app(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.renew_if_due_until(deadline, &cancelled)?;
        match self.call_until(Request::FrontAppGet(Empty {}), deadline, &cancelled)? {
            SuccessResult::FrontApp(value) => Ok(value),
            _ => self.protocol_failure("front_app.get", "matched"),
        }
    }

    #[must_use]
    pub fn supports_front_app_metadata(&self) -> bool {
        self.client
            .supports_feature(talking_quill_owner_protocol::FRONT_APP_METADATA_V1)
    }

    pub fn front_app_metadata(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppMetadataResult, OwnerClientError>
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.front_app_metadata_until(deadline, &cancelled)
    }

    pub fn front_app_metadata_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppMetadataResult, OwnerClientError>
    {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if !self.supports_front_app_metadata() {
            self.last_failure = Some(OwnerClientDiagnostic {
                category: "rejected",
                operation: "front_app.metadata_get",
                correlation_status: "not_established",
                transport_status: "open",
            });
            return Err(OwnerClientError::Rejected(ErrorCode::Incompatible));
        }
        self.renew_if_due_until(deadline, cancelled)?;
        match self.call_until(Request::FrontAppMetadataGet(Empty {}), deadline, cancelled)? {
            SuccessResult::FrontAppMetadata(value) => Ok(value),
            _ => self.protocol_failure("front_app.metadata_get", "matched"),
        }
    }

    pub fn observability(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.observability_until(deadline, &cancelled)
    }

    pub fn observability_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        match self.call_until(Request::ObservabilityGet(Empty {}), deadline, cancelled)? {
            SuccessResult::Observability(value) => Ok(*value),
            _ => self.protocol_failure("observability.get", "matched"),
        }
    }

    fn set_enabled_until(
        &mut self,
        enabled: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        let base = self.capture_params_until(deadline, cancelled)?;
        let params = SetEnabledParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            enabled,
        };
        match self.call_until(Request::CaptureSetEnabled(params), deadline, cancelled)? {
            SuccessResult::Enabled(value) if value.enabled == enabled => {
                self.enabled = enabled;
                Ok(())
            }
            _ => self.protocol_failure("capture.set_enabled", "matched"),
        }
    }

    fn capture_params_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<CaptureCommandParams, OwnerClientError> {
        self.check_budget(deadline, cancelled)?;
        self.renew_if_due_until(deadline, cancelled)?;
        self.check_budget(deadline, cancelled)?;
        self.capture_params_without_renewal()
    }

    fn renew_if_due_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.check_budget(deadline, cancelled)?;
        if self.clock.now() >= self.next_renewal {
            self.renew_until(deadline, cancelled)
        } else {
            Ok(())
        }
    }

    fn capture_params_without_renewal(&mut self) -> Result<CaptureCommandParams, OwnerClientError> {
        let sequence = self.next_command_sequence;
        self.next_command_sequence = match sequence.checked_add(1) {
            Some(next) => next,
            None => return self.sequence_failure("command.sequence"),
        };
        let command_sequence = match U64String::try_from(sequence) {
            Ok(sequence) => sequence,
            Err(_) => return self.protocol_failure("command.sequence", "not_established"),
        };
        Ok(CaptureCommandParams {
            capture_lease_id: self.lease.capture_lease_id,
            capture_lease_epoch: self.lease.capture_lease_epoch,
            command_sequence,
        })
    }

    fn handle_unsolicited(
        &mut self,
        message: GatewayMessage,
        operation: &'static str,
    ) -> Result<(), OwnerClientError> {
        if let GatewayMessage::Event(talking_quill_owner_protocol::schema::Event::HealthChanged(
            health,
        )) = &message
        {
            self.health = health.clone();
        }
        if (self.event_handler)(message) == OwnerEventDisposition::Terminal {
            self.last_failure = Some(OwnerClientDiagnostic {
                category: "protocol",
                operation,
                correlation_status: "none",
                transport_status: "open",
            });
            self.client.abort();
            Err(OwnerClientError::Protocol)
        } else {
            Ok(())
        }
    }

    fn call_until(
        &mut self,
        request: Request,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<SuccessResult, OwnerClientError> {
        let operation = request.method().as_str();
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        let correlation = self.client.send_request(&request).map_err(|error| {
            self.last_failure = Some(client_failure_diagnostic(
                operation,
                "not_established",
                &error,
            ));
            self.client.abort();
            OwnerClientError::Transport
        })?;
        let mut budget_started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            if let Err(error) = self.check_budget(deadline, cancelled) {
                self.last_failure = Some(OwnerClientDiagnostic {
                    category: "uncertain",
                    operation,
                    correlation_status: "pending",
                    transport_status: "open",
                });
                return Err(error);
            }
            if frames >= EVENT_PUMP_FRAME_BUDGET
                || self.clock.now().saturating_duration_since(budget_started)
                    >= EVENT_PUMP_TIME_BUDGET
            {
                frames = 0;
                budget_started = self.clock.now();
                std::thread::yield_now();
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => self.clock.sleep(Duration::from_millis(1)),
                Ok(ClientPoll::PeerClosed) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "disconnected",
                        operation,
                        correlation_status: "pending",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Response {
                    correlation_sequence,
                    response,
                })) => {
                    if correlation_sequence != correlation {
                        self.last_failure = Some(OwnerClientDiagnostic {
                            category: "protocol",
                            operation,
                            correlation_status: "mismatched",
                            transport_status: "open",
                        });
                        self.client.abort();
                        return Err(OwnerClientError::Protocol);
                    }
                    return match response {
                        Response::Success(value) => {
                            self.last_failure = None;
                            Ok(value)
                        }
                        Response::Error(error) => {
                            self.last_failure = Some(OwnerClientDiagnostic {
                                category: "rejected",
                                operation,
                                correlation_status: "matched",
                                transport_status: "open",
                            });
                            Err(OwnerClientError::Rejected(error.code()))
                        }
                    };
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, operation)?;
                }
                Err(error) => {
                    self.last_failure =
                        Some(client_failure_diagnostic(operation, "pending", &error));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    fn operation_deadline(&self) -> Instant {
        self.clock.now() + OWNER_CALL_TIMEOUT
    }

    fn check_budget(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        if cancelled.load(Ordering::Acquire) {
            self.client.abort();
            Err(OwnerClientError::Cancelled)
        } else if self.clock.now() >= deadline {
            self.client.abort();
            Err(OwnerClientError::Uncertain)
        } else {
            Ok(())
        }
    }

    #[doc(hidden)]
    pub fn replace_clock(&mut self, clock: Box<dyn OwnerClock>) {
        self.clock = clock;
    }

    fn report_post_grant_failure(
        &self,
        connector: &dyn OwnerConnector,
        error: &OwnerClientError,
    ) -> Result<(), OwnerClientError> {
        let diagnostic = self.last_failure.unwrap_or(OwnerClientDiagnostic {
            category: owner_client_error_category(error),
            operation: "connect.reconcile",
            correlation_status: "unknown",
            transport_status: "unknown",
        });
        let _ = crate::report_owner_connection_diagnostic(
            diagnostic,
            "not_attempted",
            connector.owner_process_state(),
        );
        Ok(())
    }

    fn protocol_failure<T>(
        &mut self,
        operation: &'static str,
        correlation_status: &'static str,
    ) -> Result<T, OwnerClientError> {
        self.last_failure = Some(OwnerClientDiagnostic {
            category: "protocol",
            operation,
            correlation_status,
            transport_status: "open",
        });
        self.client.abort();
        Err(OwnerClientError::Protocol)
    }

    fn sequence_failure<T>(&mut self, operation: &'static str) -> Result<T, OwnerClientError> {
        self.last_failure = Some(OwnerClientDiagnostic {
            category: "sequence_exhausted",
            operation,
            correlation_status: "not_established",
            transport_status: "open",
        });
        self.client.abort();
        Err(OwnerClientError::SequenceExhausted)
    }
}

fn owner_client_error_category(error: &OwnerClientError) -> &'static str {
    match error {
        OwnerClientError::Connect(_) => "connect",
        OwnerClientError::Disconnected => "disconnected",
        OwnerClientError::Uncertain => "uncertain",
        OwnerClientError::Protocol => "protocol",
        OwnerClientError::AcquireRejected(_) => "acquire_rejected",
        OwnerClientError::Rejected(_) => "rejected",
        OwnerClientError::SequenceExhausted => "sequence_exhausted",
        OwnerClientError::Transport => "transport",
        OwnerClientError::Cancelled => "cancelled",
    }
}

fn client_failure_diagnostic(
    operation: &'static str,
    correlation_status: &'static str,
    error: &ClientError,
) -> OwnerClientDiagnostic {
    let (category, transport_status) = match error {
        ClientError::Transport(talking_quill_owner_protocol::TransportError::PeerClosed) => {
            ("disconnected", "eof")
        }
        ClientError::Transport(_) => ("transport", "error"),
        ClientError::Codec(_) | ClientError::TransportAuthenticationBoundary => {
            ("protocol", "open")
        }
        ClientError::Closed => ("disconnected", "closed"),
        ClientError::Backpressured => ("transport", "backpressured"),
    };
    OwnerClientDiagnostic {
        category,
        operation,
        correlation_status,
        transport_status,
    }
}

/// Separate maintenance authority. Its capability and sequence can never be
/// used on the capture connection.
pub struct OwnerMaintenanceClient {
    client: OwnerProtocolClient<'static>,
    capability: MaintenanceAcquireResult,
    next_command_sequence: u64,
    next_renewal: Instant,
    clock: Box<dyn OwnerClock>,
}

impl std::fmt::Debug for OwnerMaintenanceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwnerMaintenanceClient(<redacted>)")
    }
}

impl OwnerMaintenanceClient {
    pub fn acquire(
        connector: &mut dyn OwnerConnector,
        params: MaintenanceAcquireParams,
    ) -> Result<Self, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        Self::acquire_until(
            connector,
            params,
            Instant::now() + OWNER_CALL_TIMEOUT,
            &cancelled,
        )
    }

    pub fn acquire_until(
        connector: &mut dyn OwnerConnector,
        params: MaintenanceAcquireParams,
        deadline: Instant,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Self, OwnerClientError> {
        Self::acquire_with_clock_until(
            connector,
            params,
            Box::new(SystemOwnerClock),
            deadline,
            cancelled,
        )
    }

    #[doc(hidden)]
    pub fn acquire_with_clock(
        connector: &mut dyn OwnerConnector,
        params: MaintenanceAcquireParams,
        clock: Box<dyn OwnerClock>,
    ) -> Result<Self, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = clock.now() + Duration::from_secs(24 * 60 * 60);
        Self::acquire_with_clock_until(connector, params, clock, deadline, &cancelled)
    }

    fn acquire_with_clock_until(
        connector: &mut dyn OwnerConnector,
        params: MaintenanceAcquireParams,
        clock: Box<dyn OwnerClock>,
        deadline: Instant,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Self, OwnerClientError> {
        check_maintenance_budget(clock.as_ref(), deadline, cancelled)?;
        let ConnectedOwner { client, build_id } = connector.connect_maintenance()?;
        check_maintenance_budget(clock.as_ref(), deadline, cancelled)?;
        if build_id.is_empty() || build_id.len() > 128 {
            return Err(OwnerClientError::Protocol);
        }
        let mut this = Self {
            client,
            capability: MaintenanceAcquireResult {
                maintenance_capability_id: Bytes32::new([1; 32]),
                maintenance_capability_epoch: U64String::try_from(1)
                    .map_err(|_| OwnerClientError::Protocol)?,
                state: talking_quill_owner_protocol::schema::MaintenanceAcquireState::Draining,
            },
            next_command_sequence: 1,
            next_renewal: clock.now() + LEASE_RENEW_INTERVAL,
            clock,
        };
        this.capability =
            match this.call_until(Request::MaintenanceAcquire(params), deadline, cancelled) {
                Ok(SuccessResult::MaintenanceAcquire(value)) => value,
                Ok(_) => return Err(OwnerClientError::Protocol),
                Err(OwnerClientError::Rejected(code)) => {
                    return Err(OwnerClientError::AcquireRejected(code));
                }
                Err(error) => return Err(error),
            };
        Ok(this)
    }

    pub fn prepare(
        &mut self,
        transaction_id: Bytes32,
        operation: MaintenanceOperation,
    ) -> Result<Bytes32, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.prepare_until(
            transaction_id,
            operation,
            self.clock.now() + Duration::from_secs(24 * 60 * 60),
            &cancelled,
        )
    }

    pub fn prepare_until(
        &mut self,
        transaction_id: Bytes32,
        operation: MaintenanceOperation,
        deadline: Instant,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Bytes32, OwnerClientError> {
        check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)?;
        let sequence = self.next_command_sequence;
        self.next_command_sequence = sequence
            .checked_add(1)
            .ok_or(OwnerClientError::SequenceExhausted)?;
        let params = MaintenancePrepareParams {
            maintenance_capability_id: self.capability.maintenance_capability_id,
            maintenance_capability_epoch: self.capability.maintenance_capability_epoch,
            command_sequence: U64String::try_from(sequence)
                .map_err(|_| OwnerClientError::Protocol)?,
            transaction_id,
            operation,
        };
        check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)?;
        let prepare_correlation = self
            .client
            .send_request(&Request::MaintenancePrepare(params))
            .map_err(|_| {
                self.client.abort();
                OwnerClientError::Transport
            })?;
        self.wait_for_prepare(prepare_correlation, deadline, cancelled)
    }

    pub fn renew(&mut self) -> Result<(), OwnerClientError> {
        let sequence = self.next_command_sequence;
        self.next_command_sequence = sequence
            .checked_add(1)
            .ok_or(OwnerClientError::SequenceExhausted)?;
        let params = MaintenanceCommandParams {
            maintenance_capability_id: self.capability.maintenance_capability_id,
            maintenance_capability_epoch: self.capability.maintenance_capability_epoch,
            command_sequence: U64String::try_from(sequence)
                .map_err(|_| OwnerClientError::Protocol)?,
        };
        match self.call_bounded(Request::MaintenanceRenew(params))? {
            SuccessResult::Renew(value) if value.renewed => {
                self.next_renewal = self.clock.now() + LEASE_RENEW_INTERVAL;
                Ok(())
            }
            _ => Err(OwnerClientError::Protocol),
        }
    }

    fn call_bounded(&mut self, request: Request) -> Result<SuccessResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.call_until(request, self.clock.now() + OWNER_CALL_TIMEOUT, &cancelled)
    }

    fn call_until(
        &mut self,
        request: Request,
        deadline: Instant,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<SuccessResult, OwnerClientError> {
        check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)?;
        self.call_inner(request, Some((deadline, cancelled)))
    }

    fn wait_for_prepare(
        &mut self,
        prepare_correlation: u64,
        deadline: Instant,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Bytes32, OwnerClientError> {
        let mut pending_renew = None;
        let mut renew_correlation = None;
        loop {
            if let Err(error) = check_maintenance_budget(self.clock.as_ref(), deadline, cancelled) {
                self.client.abort();
                return Err(error);
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => {
                    if renew_correlation.is_none()
                        && pending_renew.is_none()
                        && self.clock.now() >= self.next_renewal
                    {
                        pending_renew = Some(Request::MaintenanceRenew(self.maintenance_params()?));
                    }
                    if let Some(request) = pending_renew.take() {
                        check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)?;
                        match self.client.send_request(&request) {
                            Ok(correlation) => renew_correlation = Some(correlation),
                            Err(ClientError::Backpressured) => pending_renew = Some(request),
                            Err(_) => {
                                self.client.abort();
                                return Err(OwnerClientError::Transport);
                            }
                        }
                    }
                    let remaining = deadline.saturating_duration_since(self.clock.now());
                    self.clock.sleep(MAINTENANCE_POLL_INTERVAL.min(remaining));
                }
                Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
                Ok(ClientPoll::Message(GatewayMessage::Response {
                    correlation_sequence,
                    response,
                })) if correlation_sequence == prepare_correlation => {
                    return match response {
                        Response::Success(SuccessResult::MaintenancePrepare(value))
                            if value.ready_to_exit =>
                        {
                            Ok(value.owner_handoff)
                        }
                        Response::Success(_) => Err(OwnerClientError::Protocol),
                        Response::Error(error) => Err(OwnerClientError::Rejected(error.code())),
                    };
                }
                Ok(ClientPoll::Message(GatewayMessage::Response {
                    correlation_sequence,
                    response,
                })) if Some(correlation_sequence) == renew_correlation => match response {
                    Response::Success(SuccessResult::Renew(value)) if value.renewed => {
                        renew_correlation = None;
                        self.next_renewal = self.clock.now() + LEASE_RENEW_INTERVAL;
                    }
                    Response::Error(error) => {
                        return Err(OwnerClientError::Rejected(error.code()));
                    }
                    _ => return Err(OwnerClientError::Protocol),
                },
                Ok(ClientPoll::Message(_)) => {
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Err(_) => {
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    fn maintenance_params(&mut self) -> Result<MaintenanceCommandParams, OwnerClientError> {
        let sequence = self.next_command_sequence;
        self.next_command_sequence = sequence
            .checked_add(1)
            .ok_or(OwnerClientError::SequenceExhausted)?;
        Ok(MaintenanceCommandParams {
            maintenance_capability_id: self.capability.maintenance_capability_id,
            maintenance_capability_epoch: self.capability.maintenance_capability_epoch,
            command_sequence: U64String::try_from(sequence)
                .map_err(|_| OwnerClientError::Protocol)?,
        })
    }

    fn call_inner(
        &mut self,
        request: Request,
        budget: Option<(Instant, &Arc<AtomicBool>)>,
    ) -> Result<SuccessResult, OwnerClientError> {
        if let Some((deadline, cancelled)) = budget {
            check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)?;
        }
        let correlation = self.client.send_request(&request).map_err(|_| {
            self.client.abort();
            OwnerClientError::Transport
        })?;
        loop {
            match self.client.poll() {
                Ok(ClientPoll::Empty) => {
                    if let Some((deadline, cancelled)) = budget
                        && let Err(error) =
                            check_maintenance_budget(self.clock.as_ref(), deadline, cancelled)
                    {
                        self.client.abort();
                        return Err(error);
                    }
                    self.clock.sleep(Duration::from_millis(1));
                }
                Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
                Ok(ClientPoll::Message(GatewayMessage::Response {
                    correlation_sequence,
                    response,
                })) if correlation_sequence == correlation => {
                    return match response {
                        Response::Success(value) => Ok(value),
                        Response::Error(error) => Err(OwnerClientError::Rejected(error.code())),
                    };
                }
                Ok(ClientPoll::Message(_)) => {
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Err(_) => {
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }
}

fn check_maintenance_budget(
    clock: &dyn OwnerClock,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<(), OwnerClientError> {
    if cancelled.load(Ordering::Acquire) || clock.now() >= deadline {
        Err(OwnerClientError::Cancelled)
    } else {
        Ok(())
    }
}

impl Drop for OwnerMaintenanceClient {
    fn drop(&mut self) {
        if !self.client.is_closed() {
            self.client.abort();
        }
    }
}

impl Drop for OwnerCaptureClient {
    fn drop(&mut self) {
        if !self.client.is_closed() {
            self.client.abort();
        }
    }
}
