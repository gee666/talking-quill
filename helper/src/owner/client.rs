//! Strict owner-protocol v1 client state machine used by the non-suppressing gateway.
//!
//! Authentication and endpoint discovery are owned by [`OwnerConnector`].  This
//! module starts at an already authenticated stream, never retries a mutation,
//! and makes every new capture connection reconcile disabled-first.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::client::{ClientError, ClientPoll, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::{
    MaintenanceAcquireParams, MaintenanceAcquireResult, MaintenanceCommandParams,
    MaintenanceOperation, MaintenancePrepareParams, Request, Response, SuccessResult,
};
use talking_quill_owner_protocol::{Bytes32, GatewayMessage, U64String};

#[cfg(feature = "windows-installed-acceptance")]
mod acceptance;
mod capture;
mod types;

#[cfg(feature = "windows-installed-acceptance")]
pub use acceptance::acceptance_observability_without_lease;
pub use capture::OwnerCaptureClient;
use types::SystemOwnerClock;
pub use types::{
    CaptureRevocation, ConnectError, ConnectedOwner, OwnerClientDiagnostic, OwnerClientError,
    OwnerConnector, OwnerEventDisposition, OwnerProcessState, OwnerShutdownControl,
};
#[doc(hidden)]
pub use types::{MaintenanceClock, OwnerClock};

pub const OWNER_CALL_TIMEOUT: Duration = Duration::from_secs(2);
pub const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(1);
const MAINTENANCE_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
