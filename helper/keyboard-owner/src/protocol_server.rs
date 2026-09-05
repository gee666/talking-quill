//! Transport-independent owner protocol server and state/executor integration.
//!
//! This library has no process entry point, OS endpoint, singleton, launcher,
//! service, package startup, or credential path. Every connection is an
//! explicitly supplied authenticated ordered transport.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
fn record_starvation_evidence(event: &str) {
    use std::io::Write as _;
    let Some(path) = std::env::var_os("TALKING_QUILL_TEST_OWNER_LIFECYCLE_EVIDENCE") else {
        return;
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{{\"timestamp\":{timestamp},\"event\":\"{event}\"}}");
    }
}

#[cfg(not(debug_assertions))]
fn record_starvation_evidence(_event: &str) {}

use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationKey, EventPhase,
    KeyboardEvent, NativeTargetToken, OWNER_ADMITTED_EFFECT_CAPACITY, ProfileId,
    SessionCaptureMode, SessionKey, Shortcut, ShortcutModifiers,
};
use talking_quill_owner_protocol::schema::{
    AcquireState, ActivationEvent, AudioDevicesChangedEvent, BindingShortcut, ConfigurationResult,
    EnabledResult, ErrorBody, ErrorCode, Event, HealthResult, LeaseAcquireResult,
    LeaseDisposition as WireLeaseDisposition, MaintenanceAcquireResult, MaintenanceAcquireState,
    MaintenanceOperation as WireMaintenanceOperation, MaintenancePrepareResult, Modifiers,
    OwnerReportedState, PasteCommitState, PasteCommittedEvent, PasteRefusalReason, PasteResult,
    Phase, ProcessState as WireProcessState, ProfileId as WireProfileId, Purpose,
    RegisteredObservationEvent, ReleaseResult, RenewResult, Request, Response, RollbackResult,
    SessionKey as WireSessionKey, SessionKeyEvent, SessionMode, SessionModeResult, SuccessResult,
    TerminalDegradedEvent, TerminalDegradedReason, WireToken,
};
use talking_quill_owner_protocol::transport::{
    FlushReceipt, OrderedTransport, Progress as TransportProgress, ReceiveResult, TransportError,
};
use talking_quill_owner_protocol::{
    Bytes32, CapabilityKind, CapabilitySequenceValidator, Counter, OwnerSessionCodec,
    ReceivedRequest, SessionCodecError, U64String,
};
use thiserror::Error;

use crate::adapter::{
    AdapterEventDisposition, AdapterEventRejection, BoundedShutdownOutcome, BrokerEvent,
    CapabilityIdSource, NativeAdapter, NativeAdapterExecutor, NativeAdapterPump,
    PasteCommitOutcome, PasteRefusal,
};
use crate::executor::{ExecutorCommand, ExecutorResult, OwnerExecutor, PasteExecutorRequest};
use crate::state::{
    AdmissionState, BuildDigest, CandidateOwnership, CapabilityRef, CommandSequence,
    ConfigurationRevision, ConnectionId, ControllerLossReason, ControllerState, KeyboardOwnerState,
    LeaseDisposition, MaintenanceCommand, MaintenanceHandoff, MaintenanceOperation,
    MaintenanceRequest, MaintenanceTransactionId, NativeActionFailure, NativeOwnership,
    NativeOwnershipObservation, NativeReadiness, OwnerActivationGeneration, OwnerInstanceId,
    PasteAuthorization, PasteOperationId, PasteOwnership, PredecessorTerminalEvent,
    PredecessorTerminalOffer, ProcessState, ReportedState, RequiredAction, RequiredActionKind,
    ResponseCorrelation, ResponseStage, TerminalOwnership, TerminalReason,
    TerminalUnavailableReason, Transition, TransitionError, TransitionErrorKind,
};

const MAX_CONNECTIONS: usize = 8;
const MAX_PENDING_TRANSPORT_FLUSHES: usize = 16;
const MAX_SYNCHRONOUS_CLOSE_ATTEMPTS: usize = 8;
const CAPABILITY_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

struct ServerConnection {
    endpoint: Box<dyn OrderedTransport>,
    codec: OwnerSessionCodec,
    capture_sequence: Option<CapabilitySequenceValidator>,
    maintenance_sequence: Option<CapabilitySequenceValidator>,
    capability_deadline: Option<Instant>,
    pending_flushes: VecDeque<PendingFlush>,
    pending_close: Option<ResponseCorrelation>,
    closed: bool,
}

impl fmt::Debug for ServerConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ServerConnection(<redacted>)")
    }
}

struct PendingFlush {
    receipt: FlushReceipt,
    completion: FlushCompletion,
}

#[derive(Clone, Copy)]
enum FlushCompletion {
    RegisteredObservation,
    Ordinary,
    AdmittedEvent,
    PredecessorTerminal(PredecessorTerminalOffer),
    FinalResponse(ResponseCorrelation),
    PlannedExitResponse,
}

struct QueuedEvent {
    connection: ConnectionId,
    event: Event,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct ClosingEventScope {
    connection: ConnectionId,
    authority: CapabilityRef,
    bindings: Option<ActivationBindings>,
    session_mode: Option<SessionCaptureMode>,
}

impl fmt::Debug for ClosingEventScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClosingEventScope(<redacted>)")
    }
}

impl fmt::Debug for QueuedEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QueuedEvent(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DriveSummary {
    lease_disposition: Option<LeaseDisposition>,
    response_stage: Option<ResponseStage>,
    paste_refusal: Option<PasteRefusal>,
    paste_waiting: bool,
    paste_failure: Option<NativeActionFailure>,
    native_failure: Option<NativeActionFailure>,
}

impl DriveSummary {
    fn merge(&mut self, other: Self) {
        if other.lease_disposition.is_some() {
            self.lease_disposition = other.lease_disposition;
        }
        if other.response_stage.is_some() {
            self.response_stage = other.response_stage;
        }
        if other.paste_refusal.is_some() {
            self.paste_refusal = other.paste_refusal;
        }
        self.paste_waiting |= other.paste_waiting;
        if other.paste_failure.is_some() {
            self.paste_failure = other.paste_failure;
        }
        if self.native_failure.is_none()
            || other.native_failure == Some(NativeActionFailure::Indeterminate)
        {
            self.native_failure = other.native_failure.or(self.native_failure);
        }
    }
}

struct DispatchResult {
    response: Response,
    final_flush: Option<ResponseCorrelation>,
    planned_exit: bool,
}

enum Incoming {
    PeerClosed,
    ProtocolFault,
    Request(Box<ReceivedRequest>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchError {
    Semantic(ErrorCode),
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerPump {
    Empty,
    Backpressured,
    Processed,
    PeerClosed,
    FatalClosed,
}

/// The in-memory protocol coordinator. The state and executor remain alive
/// after a connection fault so already-owned input can continue draining.
pub struct OwnerProtocolServer<'a, E: OwnerExecutor> {
    state: KeyboardOwnerState,
    executor: E,
    connections: HashMap<ConnectionId, ServerConnection>,
    queued_events: VecDeque<QueuedEvent>,
    terminal_draining_sent: HashMap<ConnectionId, bool>,
    maintenance_request: Option<MaintenanceRequest>,
    active_bindings: Option<ActivationBindings>,
    active_activation: Option<(ActivationBinding, ActivationContext)>,
    activation_generation_high_water: u64,
    observation_generation_high_water: u64,
    adapter_event_high_water: u64,
    adapter_event_in_flight: Option<crate::adapter::AdapterEventId>,
    deferred_adapter_acknowledgements:
        VecDeque<(crate::adapter::AdapterEventId, AdapterEventDisposition)>,
    deferred_controller_losses: VecDeque<(ConnectionId, ControllerLossReason)>,
    active_session_keys: u8,
    active_session_mode: Option<SessionCaptureMode>,
    closing_event_scope: Option<ClosingEventScope>,
    active_paste: Option<PasteExecutorRequest>,
    deferred_close: Option<RequiredAction>,
    pending_close_confirmation: Option<RequiredAction>,
    exit_requested: bool,
    planned_exit_when_neutral: bool,
    planned_exit_response_flushed: bool,
    planned_exit_terminal_pending: bool,
    heartbeat_timeout: Duration,
    registered_owner_admitted: u64,
    registered_owner_flushed: u64,
    registered_owner_rejected: u64,
    lease_acquired: u64,
    lease_renewed: u64,
    lease_expired: u64,
    lease_disconnected: u64,
    lease_released_neutral: u64,
    lease_released_draining: u64,
    lifetime: PhantomData<&'a ()>,
}

impl<E: OwnerExecutor> fmt::Debug for OwnerProtocolServer<'_, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerProtocolServer(<redacted>)")
    }
}

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    pub fn start(owner_instance: OwnerInstanceId, mut executor: E) -> Result<Self, ServerError> {
        let mut state = KeyboardOwnerState::new(owner_instance);
        if !executor.seed_startup_physical_snapshot() {
            return Err(ServerError::StartupSnapshot);
        }
        state.confirm_startup_snapshot_seeded()?;
        state.startup_completed()?;
        state.observe_native_readiness(executor.readiness())?;
        Ok(Self {
            state,
            executor,
            connections: HashMap::new(),
            queued_events: VecDeque::with_capacity(OWNER_ADMITTED_EFFECT_CAPACITY),
            terminal_draining_sent: HashMap::new(),
            maintenance_request: None,
            active_bindings: None,
            active_activation: None,
            activation_generation_high_water: 0,
            observation_generation_high_water: 0,
            adapter_event_high_water: 0,
            adapter_event_in_flight: None,
            deferred_adapter_acknowledgements: VecDeque::new(),
            deferred_controller_losses: VecDeque::new(),
            active_session_keys: 0,
            active_session_mode: None,
            closing_event_scope: None,
            active_paste: None,
            deferred_close: None,
            pending_close_confirmation: None,
            exit_requested: false,
            planned_exit_when_neutral: false,
            planned_exit_response_flushed: false,
            planned_exit_terminal_pending: false,
            heartbeat_timeout: CAPABILITY_HEARTBEAT_TIMEOUT,
            registered_owner_admitted: 0,
            registered_owner_flushed: 0,
            registered_owner_rejected: 0,
            lease_acquired: 0,
            lease_renewed: 0,
            lease_expired: 0,
            lease_disconnected: 0,
            lease_released_neutral: 0,
            lease_released_draining: 0,
            lifetime: PhantomData,
        })
    }

    /// Compatibility name retained while existing owner tests migrate.
    pub fn start_fake(owner_instance: OwnerInstanceId, executor: E) -> Result<Self, ServerError> {
        Self::start(owner_instance, executor)
    }

    pub fn attach_connection<T>(
        &mut self,
        connection: ConnectionId,
        endpoint: T,
        codec: OwnerSessionCodec,
    ) -> Result<(), ServerError>
    where
        T: OrderedTransport + 'static,
    {
        self.attach_boxed_connection(connection, Box::new(endpoint), codec)
    }

    /// R5-W/R5-M authenticated endpoint hook. The endpoint provider must
    /// complete platform authentication before transferring this connected
    /// transport and its matching session codec to the owner runtime.
    pub fn attach_boxed_connection(
        &mut self,
        connection: ConnectionId,
        endpoint: Box<dyn OrderedTransport>,
        codec: OwnerSessionCodec,
    ) -> Result<(), ServerError> {
        if endpoint.is_test_only() != codec.is_test_only() {
            return Err(ServerError::TransportAuthenticationBoundary);
        }
        self.sweep_closed_connections();
        self.reap_closed_connection(connection);
        if self
            .connections
            .values()
            .filter(|active| !active.closed)
            .count()
            >= MAX_CONNECTIONS
        {
            return Err(ServerError::ConnectionCapacity);
        }
        if self.connections.contains_key(&connection) {
            return Err(ServerError::DuplicateConnection);
        }
        self.connections.insert(
            connection,
            ServerConnection {
                endpoint,
                codec,
                capture_sequence: None,
                maintenance_sequence: None,
                capability_deadline: None,
                pending_flushes: VecDeque::with_capacity(MAX_PENDING_TRANSPORT_FLUSHES),
                pending_close: None,
                closed: false,
            },
        );
        Ok(())
    }

    #[must_use]
    pub const fn state(&self) -> &KeyboardOwnerState {
        &self.state
    }

    #[must_use]
    pub const fn executor(&self) -> &E {
        &self.executor
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn executor_mut(&mut self) -> &mut E {
        &mut self.executor
    }

    #[must_use]
    pub const fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    pub const fn planned_exit_when_neutral(&self) -> bool {
        self.planned_exit_when_neutral
    }

    pub fn planned_exit_route_complete(&self) -> bool {
        self.planned_exit_when_neutral
            && self.planned_exit_response_flushed
            && !self.planned_exit_terminal_pending
            && self.terminal_draining_sent.is_empty()
    }

    #[must_use]
    pub fn queued_event_count(&self) -> usize {
        self.queued_events.len()
    }

    #[must_use]
    pub fn has_connection_capacity(&self) -> bool {
        self.connections
            .values()
            .filter(|active| !active.closed)
            .count()
            < MAX_CONNECTIONS
    }

    #[must_use]
    pub const fn has_deferred_close(&self) -> bool {
        self.deferred_close.is_some() || self.pending_close_confirmation.is_some()
    }

    /// Retries a previously bounded-out close without processing any new wire
    /// request. Until this succeeds the state remains non-enableable.
    pub fn service_deferred_close(&mut self) -> Result<bool, ServerError> {
        let Some(action) = self.deferred_close.take() else {
            return Ok(false);
        };
        self.drive_close_iteratively(action, &mut None, None)?;
        Ok(true)
    }

    pub fn pump(&mut self, connection: ConnectionId) -> Result<ServerPump, ServerError> {
        if self
            .connections
            .get(&connection)
            .and_then(|active| active.capability_deadline)
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.expire_connection(connection)?;
            return Ok(ServerPump::FatalClosed);
        }
        match self.service_transport_close(connection) {
            Ok(TransportProgress::Pending) => return Ok(ServerPump::Backpressured),
            Ok(TransportProgress::Complete) => {}
            Err(_) => {
                self.teardown_connection(connection, ControllerLossReason::Eof)?;
                return Ok(ServerPump::FatalClosed);
            }
        }
        if self
            .connections
            .get(&connection)
            .is_some_and(|active| active.closed)
        {
            self.reap_closed_connection(connection);
            return Ok(ServerPump::Processed);
        }
        let flush_progress = match self.service_transport_flush(connection) {
            Ok(progress) => progress,
            Err(_) => {
                self.teardown_connection(connection, ControllerLossReason::Eof)?;
                return Ok(ServerPump::FatalClosed);
            }
        };
        if flush_progress == TransportProgress::Pending
            || self
                .connections
                .get(&connection)
                .is_some_and(|active| !active.pending_flushes.is_empty())
        {
            return Ok(ServerPump::Backpressured);
        }
        if self
            .connections
            .get(&connection)
            .is_some_and(|active| active.closed)
        {
            self.reap_closed_connection(connection);
            return Ok(ServerPump::Processed);
        }
        if self.deferred_close.is_some() {
            self.service_deferred_close()?;
        }
        self.flush_admitted_events()?;
        let received = {
            let Some(active) = self.connections.get_mut(&connection) else {
                return Err(ServerError::UnknownConnection);
            };
            if active.closed {
                return Ok(ServerPump::PeerClosed);
            }
            match active.endpoint.try_receive() {
                Ok(ReceiveResult::Empty) => return Ok(ServerPump::Empty),
                Ok(ReceiveResult::PeerClosed) => {
                    active.closed = true;
                    Incoming::PeerClosed
                }
                Ok(ReceiveResult::Frame(frame)) => match active.codec.receive_request(&frame) {
                    Ok(request) => Incoming::Request(Box::new(request)),
                    Err(error) => {
                        active.endpoint.abort();
                        active.closed = true;
                        let _ = error;
                        Incoming::ProtocolFault
                    }
                },
                Err(error) => {
                    active.endpoint.abort();
                    active.closed = true;
                    let _ = error;
                    Incoming::ProtocolFault
                }
            }
        };
        let request = match received {
            Incoming::Request(request) => request,
            Incoming::PeerClosed => {
                self.teardown_connection(connection, ControllerLossReason::Eof)?;
                return Ok(ServerPump::PeerClosed);
            }
            Incoming::ProtocolFault => {
                self.teardown_connection(connection, ControllerLossReason::ProtocolFault)?;
                return Ok(ServerPump::FatalClosed);
            }
        };

        if request.request().method().is_capability_mutation()
            && self
                .validate_capability_request(connection, request.request())
                .is_err()
        {
            self.teardown_connection(connection, ControllerLossReason::ProtocolFault)?;
            return Ok(ServerPump::FatalClosed);
        }

        let dispatch = match self.dispatch(connection, &request) {
            Ok(value) => value,
            Err(DispatchError::Semantic(code)) => DispatchResult {
                response: Response::Error(ErrorBody::new(code)),
                final_flush: None,
                planned_exit: false,
            },
            Err(DispatchError::Fatal) => {
                self.teardown_connection(connection, ControllerLossReason::ProtocolFault)?;
                return Ok(ServerPump::FatalClosed);
            }
        };

        let progress = if let Some(correlation) = dispatch.final_flush {
            self.state.begin_final_response_flush(correlation)?;
            let frame = match self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)?
                .codec
                .encode_response(&request, &dispatch.response)
            {
                Ok(frame) => frame,
                Err(_) => {
                    self.teardown_connection(connection, ControllerLossReason::Eof)?;
                    return Ok(ServerPump::FatalClosed);
                }
            };
            self.enqueue_transport_frame(
                connection,
                frame,
                FlushCompletion::FinalResponse(correlation),
            )
        } else if dispatch.planned_exit {
            let frame = self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)?
                .codec
                .encode_response(&request, &dispatch.response)?;
            self.enqueue_transport_frame(connection, frame, FlushCompletion::PlannedExitResponse)
        } else {
            self.enqueue_and_flush_response(connection, &request, &dispatch.response)
        };
        match progress {
            Ok(TransportProgress::Complete)
                if self
                    .connections
                    .get(&connection)
                    .is_some_and(|active| active.pending_close.is_some()) =>
            {
                Ok(ServerPump::Backpressured)
            }
            Ok(TransportProgress::Complete) => Ok(ServerPump::Processed),
            Ok(TransportProgress::Pending) => Ok(ServerPump::Backpressured),
            Err(_) => {
                self.teardown_connection(connection, ControllerLossReason::Eof)?;
                Ok(ServerPump::FatalClosed)
            }
        }
    }

    fn health(&self) -> HealthResult {
        let status = self.state.status();
        HealthResult {
            owner_instance_id: Bytes32::new(*self.state.owner_instance().as_bytes()),
            reported_state: wire_reported_state(status.reported_state),
            process_state: wire_process_state(status.process_state),
            rollback_latched: status.rollback_latched,
            native_state_unknown: status.native_state_unknown,
            maintenance_sealed: status.maintenance_sealed,
            keyboard_build_eligible: status.keyboard_build_eligible,
            paste_ready: status.paste_ready,
            permissions_eligible: status.permissions_eligible,
            hook_healthy: status.hook_healthy,
        }
    }

    fn ensure_effect_queue_consistent(&self) -> Result<(), ServerError> {
        (usize::from(self.state.ownership().admitted_effects()) == self.queued_events.len())
            .then_some(())
            .ok_or(ServerError::ExecutorContract)
    }

    fn retire_one_admitted_effect(&mut self) -> Result<(), ServerError> {
        let current = self.state.ownership().admitted_effects();
        if current == 0 {
            return Err(ServerError::ExecutorContract);
        }
        let transition = self.state.set_broker_admitted_effects(current - 1)?;
        self.drive_transition(transition, None).map(|_| ())
    }

    fn retire_all_admitted_effects(&mut self) -> Result<(), ServerError> {
        let transition = self.state.set_broker_admitted_effects(0)?;
        self.drive_transition(transition, None).map(|_| ())
    }
}

impl<A, C> OwnerProtocolServer<'_, NativeAdapterExecutor<A, C>>
where
    A: NativeAdapter,
    C: CapabilityIdSource,
{
    /// Pumps exactly one adapter event and acknowledges it exactly once after
    /// the B2 state/event admission decision has linearized. Rejection never
    /// requeues or retries an event.
    pub fn pump_native_adapter(&mut self) -> NativeAdapterPump {
        if self.deferred_close.is_some() {
            let _ = self.service_deferred_close();
            return NativeAdapterPump::ClosePending;
        }
        self.pump_executor_adapter_event(false)
            .map_or(NativeAdapterPump::Empty, NativeAdapterPump::Processed)
    }

    /// Controller authority is already gone, so no terminal route remains to
    /// service. The platform owns the one bounded drain and reports whether it
    /// stopped neutral or terminal-incomplete.
    pub fn orphan_retirement_policy(&self) -> crate::OrphanRetirementPolicy {
        self.executor.orphan_retirement_policy()
    }

    pub fn bounded_orphan_shutdown(&mut self) -> BoundedShutdownOutcome {
        self.executor.bounded_orphan_shutdown()
    }
}

const fn authoritative_control_event(event: BrokerEvent) -> bool {
    matches!(
        event,
        BrokerEvent::OwnershipChanged(_)
            | BrokerEvent::ReadinessChanged(_)
            | BrokerEvent::RecoverableNativeFault
    )
}

mod wire;
use wire::*;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("keyboard owner startup physical snapshot failed")]
    StartupSnapshot,
    #[error("keyboard owner connection capacity exceeded")]
    ConnectionCapacity,
    #[error("keyboard owner connection ID is duplicated")]
    DuplicateConnection,
    #[error("keyboard owner connection is unknown")]
    UnknownConnection,
    #[error("keyboard owner capability protocol fault")]
    CapabilityProtocol,
    #[error("keyboard owner close retry budget exhausted with work retained")]
    CloseRetryExhausted,
    #[error("keyboard owner executor returned an invalid completion")]
    ExecutorContract,
    #[error("keyboard owner event is not scoped to the enabled capture lease")]
    EventScope,
    #[error("keyboard owner admitted-event capacity exceeded")]
    EventCapacity,
    #[error("test authentication cannot cross a production transport boundary")]
    TransportAuthenticationBoundary,
    #[error("owner transport already has a bounded pending write")]
    TransportBackpressure,
    #[error("keyboard owner event delivery failed")]
    EventDelivery,
    #[error("keyboard owner predecessor terminal route is unavailable")]
    TerminalRoute,
    #[error(transparent)]
    State(#[from] TransitionError),
    #[error(transparent)]
    Session(#[from] SessionCodecError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}

mod actions;
mod confirmation;
mod disconnect;
mod dispatch;
mod events;
mod observation;
mod transport;
