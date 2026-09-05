//! Capture association and disabled-first connection reconciliation.

use super::{
    ConnectError, ConnectedOwner, LEASE_RENEW_INTERVAL, OWNER_CALL_TIMEOUT, OwnerClientDiagnostic,
    OwnerClientError, OwnerClock, OwnerConnector, OwnerEventDisposition, SystemOwnerClock,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::client::{ClientError, ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::{
    CaptureCommandParams, Empty, ErrorCode, HealthResult, LeaseAcquireResult, LeaseDisposition,
    PasteInjectParams, PasteResult, PermissionsResult, ReplaceConfigurationParams, Request,
    Response, SessionMode, SessionSetModeParams, SetEnabledParams, SuccessResult,
};
use talking_quill_owner_protocol::{Bytes32, GatewayMessage, U64String};

mod configuration;
mod diagnostics;
mod exchange;
mod lease;
mod paste;
mod queries;

use diagnostics::client_failure_diagnostic;

const EVENT_PUMP_FRAME_BUDGET: usize = 64;
const EVENT_PUMP_TIME_BUDGET: Duration = Duration::from_millis(5);

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
}

impl Drop for OwnerCaptureClient {
    fn drop(&mut self) {
        if !self.client.is_closed() {
            self.client.abort();
        }
    }
}
