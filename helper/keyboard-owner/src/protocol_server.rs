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

    /// Admits a core keyboard event after binding, owner-instance, generation,
    /// and current capture scope have been derived and validated locally.
    fn admit_keyboard_event_inner(
        &mut self,
        event: KeyboardEvent,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        let (connection, epoch) = self.current_capture_for_event(before_close_barrier)?;
        let registered_activation = matches!(
            event,
            KeyboardEvent::Activation { .. } | KeyboardEvent::ActivationComplete { .. }
        );
        let bindings = if before_close_barrier {
            self.closing_event_scope
                .and_then(|scope| scope.bindings)
                .ok_or(ServerError::EventScope)?
        } else {
            self.active_bindings.ok_or(ServerError::EventScope)?
        };
        let wire = match event {
            KeyboardEvent::Activation {
                binding,
                context,
                phase,
            } => {
                if !bindings.iter().any(|configured| configured == binding) {
                    return Err(ServerError::EventScope);
                }
                let generation = context.activation_generation().get();
                match phase {
                    EventPhase::Down
                        if self.active_activation.is_none()
                            && generation > self.activation_generation_high_water => {}
                    EventPhase::Up if self.active_activation == Some((binding, context)) => {}
                    _ => return Err(ServerError::EventScope),
                }
                Event::Activation(wire_activation_event(
                    self.state.owner_instance(),
                    epoch.get(),
                    binding,
                    context.target_token(),
                    generation,
                    phase,
                    None,
                )?)
            }
            KeyboardEvent::ActivationComplete {
                binding,
                context,
                held_ms,
            } => {
                let generation = context.activation_generation().get();
                if !bindings.iter().any(|configured| configured == binding)
                    || self.active_activation.is_some()
                    || generation <= self.activation_generation_high_water
                {
                    return Err(ServerError::EventScope);
                }
                Event::Activation(wire_activation_event(
                    self.state.owner_instance(),
                    epoch.get(),
                    binding,
                    context.target_token(),
                    generation,
                    EventPhase::Up,
                    Some(held_ms),
                )?)
            }
            KeyboardEvent::SessionKey { key, phase } => {
                let bit = match key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                let phase_valid = match phase {
                    EventPhase::Down => {
                        self.active_session_keys & bit == 0
                            && if before_close_barrier {
                                self.closing_event_scope
                                    .and_then(|scope| scope.session_mode)
                                    .is_some_and(|mode| mode.allows(key))
                            } else {
                                self.state
                                    .applied_session_mode()
                                    .is_some_and(|mode| mode.allows(key))
                            }
                    }
                    EventPhase::Up => self.active_session_keys & bit != 0,
                };
                if !phase_valid {
                    return Err(ServerError::EventScope);
                }
                Event::SessionKey(SessionKeyEvent {
                    capture_lease_epoch: wire_u64(epoch.get()),
                    key: match key {
                        SessionKey::Escape => WireSessionKey::Escape,
                        SessionKey::Enter => WireSessionKey::Enter,
                    },
                    phase: wire_phase(phase),
                })
            }
        };
        if let Err(error) = self.admit_capture_event(connection, wire) {
            if registered_activation {
                self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            }
            return Err(error);
        }
        if registered_activation {
            self.registered_owner_admitted = self.registered_owner_admitted.saturating_add(1);
        }
        match event {
            KeyboardEvent::Activation {
                binding,
                context,
                phase,
            } => {
                let generation = context.activation_generation().get();
                if phase == EventPhase::Down {
                    self.active_activation = Some((binding, context));
                    self.activation_generation_high_water = generation;
                } else {
                    self.active_activation = None;
                }
            }
            KeyboardEvent::ActivationComplete { context, .. } => {
                self.activation_generation_high_water = context.activation_generation().get();
            }
            KeyboardEvent::SessionKey { key, phase } => {
                let bit = match key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                match phase {
                    EventPhase::Down => self.active_session_keys |= bit,
                    EventPhase::Up => self.active_session_keys &= !bit,
                }
            }
        }
        Ok(())
    }

    fn admit_registered_observation_inner(
        &mut self,
        generation: u64,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        let (connection, epoch) = self.current_capture_for_event(before_close_barrier)?;
        let negotiated = self.connections.get(&connection).is_some_and(|active| {
            active
                .codec
                .supports_feature(talking_quill_owner_protocol::REGISTERED_INPUT_OBSERVABILITY_V1)
        });
        if !negotiated || generation == 0 || generation <= self.observation_generation_high_water {
            self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            return Err(ServerError::EventScope);
        }
        let wire = Event::RegisteredObservation(RegisteredObservationEvent {
            capture_lease_epoch: wire_u64(epoch.get()),
            generation: wire_u64(generation),
        });
        if let Err(error) = self.admit_capture_event(connection, wire) {
            self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            return Err(error);
        }
        self.observation_generation_high_water = generation;
        self.registered_owner_admitted = self.registered_owner_admitted.saturating_add(1);
        Ok(())
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn admit_keyboard_event(&mut self, event: KeyboardEvent) -> Result<(), ServerError> {
        self.admit_keyboard_event_inner(event, false)
    }

    fn admit_audio_devices_changed_inner(
        &mut self,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        let (connection, epoch) = self.current_capture_for_event(before_close_barrier)?;
        self.admit_capture_event(
            connection,
            Event::AudioDevicesChanged(AudioDevicesChangedEvent {
                capture_lease_epoch: wire_u64(epoch.get()),
            }),
        )
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn admit_audio_devices_changed(&mut self) -> Result<(), ServerError> {
        self.admit_audio_devices_changed_inner(false)
    }

    fn publish_terminal_degraded_inner(&mut self) -> Result<(), ServerError> {
        let (connection, _) = self.current_capture()?;
        let status = self.state.status();
        let reason = if status.native_state_unknown {
            TerminalDegradedReason::OwnershipUnknown
        } else if status.process_state == ProcessState::Degraded {
            TerminalDegradedReason::NativeFault
        } else {
            return Err(ServerError::EventScope);
        };
        self.send_direct_event(
            connection,
            &Event::TerminalDegraded(TerminalDegradedEvent { reason }),
        )
    }

    /// Constructs health from the authoritative W1 state rather than accepting
    /// caller-provided wire status.
    pub fn publish_health_changed(&mut self, connection: ConnectionId) -> Result<(), ServerError> {
        let purpose = self
            .connections
            .get(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .purpose();
        let current_route = match purpose {
            Purpose::Observe => true,
            Purpose::Capture => self.capture_authority(connection).is_some(),
            Purpose::Maintenance => self.maintenance_authority(connection).is_some(),
        };
        if !current_route || self.terminal_draining_sent.contains_key(&connection) {
            return Err(ServerError::EventScope);
        }
        self.send_direct_event(connection, &Event::HealthChanged(self.health()))
    }

    fn admit_capture_event(
        &mut self,
        connection: ConnectionId,
        event: Event,
    ) -> Result<(), ServerError> {
        if self.queued_events.len() >= OWNER_ADMITTED_EFFECT_CAPACITY {
            // The ninth item is never admitted. Existing accepted effects cross
            // the writer boundary before fail-closed degradation proceeds.
            self.flush_admitted_events()?;
            let transition = self.state.recoverable_native_fault()?;
            self.drive_transition(transition, None)?;
            return Err(ServerError::EventCapacity);
        }
        let next = self
            .state
            .ownership()
            .admitted_effects()
            .checked_add(1)
            .ok_or(ServerError::EventCapacity)?;
        let transition = self.state.set_broker_admitted_effects(next)?;
        // Queue insertion and the authoritative admitted-effect count form one
        // local commit. Preserve the queue entry even if fail-closed follow-up
        // work fails so count and queue can never diverge.
        self.queued_events
            .push_back(QueuedEvent { connection, event });
        self.drive_transition(transition, None)?;
        self.ensure_effect_queue_consistent()?;
        Ok(())
    }

    pub fn flush_admitted_events(&mut self) -> Result<(), ServerError> {
        while let Some((connection, event)) = self
            .queued_events
            .front()
            .map(|queued| (queued.connection, queued.event.clone()))
        {
            if self
                .connections
                .get(&connection)
                .is_some_and(|active| !active.pending_flushes.is_empty())
            {
                return Ok(());
            }
            let send = self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)
                .and_then(|active| active.codec.encode_event(&event).map_err(Into::into))
                .and_then(|frame| {
                    self.enqueue_transport_frame(connection, frame, FlushCompletion::AdmittedEvent)
                });
            if send.is_err() {
                self.queued_events.clear();
                let _ = self.retire_all_admitted_effects();
                if self.adapter_event_in_flight.is_some() {
                    self.abort_connection(connection);
                    if !self
                        .deferred_controller_losses
                        .iter()
                        .any(|(active, _)| *active == connection)
                    {
                        self.deferred_controller_losses
                            .push_back((connection, ControllerLossReason::Eof));
                    }
                } else {
                    let _ = self.teardown_connection(connection, ControllerLossReason::Eof);
                }
                self.ensure_effect_queue_consistent()?;
                return Err(ServerError::EventDelivery);
            }
            if send? == TransportProgress::Pending {
                return Ok(());
            }
            self.ensure_effect_queue_consistent()?;
        }
        Ok(())
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    fn observe_native_ownership_inner(
        &mut self,
        ownership: NativeOwnership,
    ) -> Result<(), ServerError> {
        self.observe_native_observation_inner(NativeOwnershipObservation {
            candidate: ownership.candidate(),
            activation_drain_keys: u16::from(ownership.activation_drain_keys()),
            session_drain_keys: u16::from(ownership.session_drain_keys()),
            replay_cleanup_edges: u16::from(ownership.replay_cleanup_edges()),
            paste: ownership.paste(),
            conservative_native_work: ownership.conservative_native_work(),
            admitted_effects: u16::from(ownership.admitted_effects()),
        })
    }

    /// Applies a raw aggregate adapter observation without masking max+1
    /// values. Broker-admitted effects remain owner-authoritative; a mismatch
    /// closes fresh admission before the event is rejected.
    fn observe_native_observation_inner(
        &mut self,
        observation: NativeOwnershipObservation,
    ) -> Result<(), ServerError> {
        if usize::from(observation.admitted_effects) != self.queued_events.len()
            || observation.admitted_effects != u16::from(self.state.ownership().admitted_effects())
        {
            let transition = self.state.adapter_stream_desynchronized();
            self.drive_external_transition(transition)?;
            return Err(ServerError::ExecutorContract);
        }
        let transition = self.state.observe_native_observation(observation);
        self.drive_external_transition(transition)?;
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    fn observe_native_readiness_inner(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<(), ServerError> {
        let transition = self.state.observe_native_readiness(readiness);
        self.drive_external_transition(transition)
    }

    fn observe_recoverable_native_fault_inner(&mut self) -> Result<(), ServerError> {
        let transition = self.state.recoverable_native_fault();
        self.drive_external_transition(transition)?;
        if self.current_capture().is_ok() {
            self.publish_terminal_degraded_inner()?;
        }
        Ok(())
    }

    fn confirm_paste_claimed_inner(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
        {
            return Err(ServerError::EventScope);
        }
        let transition = self.state.confirm_paste_claimed(authorization)?;
        self.drive_transition(transition, None).map(|_| ())
    }

    fn publish_paste_committed_inner(
        &mut self,
        authorization: PasteAuthorization,
        indeterminate: bool,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
        {
            return Err(ServerError::EventScope);
        }
        let notification_route = self
            .current_capture()
            .ok()
            .filter(|(_, authority)| authority.epoch() == authorization.capture_epoch());
        let transition = if indeterminate {
            self.state.confirm_paste_indeterminate(authorization)?
        } else {
            self.state.confirm_paste_completed(authorization)?
        };
        self.drive_transition(transition, None)?;
        if !indeterminate {
            self.active_paste = None;
        }
        // Native completion is authoritative even when capture authority was
        // already revoked or its notification route fails. Never ask the
        // adapter to retry a claimed one-shot paste.
        if let Some((connection, authority)) = notification_route {
            let _ = self.send_direct_event(
                connection,
                &Event::PasteCommitted(PasteCommittedEvent {
                    capture_lease_epoch: wire_u64(authority.epoch().get()),
                    operation_id: Bytes32::new(*authorization.operation().as_bytes()),
                    state: if indeterminate {
                        PasteCommitState::Indeterminate
                    } else {
                        PasteCommitState::Committed
                    },
                }),
            );
        }
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    fn confirm_paste_completed_inner(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
            || self.state.ownership().paste() != PasteOwnership::Indeterminate
        {
            return Err(ServerError::EventScope);
        }
        let transition = self.state.confirm_paste_completed(authorization)?;
        self.drive_transition(transition, None)?;
        self.active_paste = None;
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_ownership(
        &mut self,
        ownership: NativeOwnership,
    ) -> Result<(), ServerError> {
        self.observe_native_ownership_inner(ownership)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_observation(
        &mut self,
        observation: NativeOwnershipObservation,
    ) -> Result<(), ServerError> {
        self.observe_native_observation_inner(observation)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_readiness(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<(), ServerError> {
        self.observe_native_readiness_inner(readiness)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_recoverable_native_fault(&mut self) -> Result<(), ServerError> {
        self.observe_recoverable_native_fault_inner()
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn confirm_paste_claimed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        self.confirm_paste_claimed_inner(authorization)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn publish_paste_committed(
        &mut self,
        authorization: PasteAuthorization,
        indeterminate: bool,
    ) -> Result<(), ServerError> {
        self.publish_paste_committed_inner(authorization, indeterminate)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn confirm_paste_completed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        self.confirm_paste_completed_inner(authorization)
    }

    pub fn expire_connection(&mut self, connection: ConnectionId) -> Result<(), ServerError> {
        self.lease_expired = self.lease_expired.saturating_add(1);
        record_starvation_evidence("lease_expired");
        let result = self.teardown_connection(connection, ControllerLossReason::HeartbeatExpired);
        record_starvation_evidence(if result.is_ok() {
            "lease_expiry_transport_close"
        } else {
            "lease_expiry_close_failed"
        });
        result
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn set_heartbeat_timeout_for_test(&mut self, timeout: Duration) {
        self.heartbeat_timeout = timeout;
    }

    fn renew_capability_deadline(&mut self, connection: ConnectionId) -> Result<(), DispatchError> {
        record_starvation_evidence("lease_deadline_renewed");
        let deadline = Instant::now()
            .checked_add(self.heartbeat_timeout)
            .ok_or(DispatchError::Fatal)?;
        self.connections
            .get_mut(&connection)
            .ok_or(DispatchError::Fatal)?
            .capability_deadline = Some(deadline);
        Ok(())
    }

    fn dispatch(
        &mut self,
        connection: ConnectionId,
        received: &ReceivedRequest,
    ) -> Result<DispatchResult, DispatchError> {
        let request = received.request();
        let mut final_flush = None;
        let response = match request {
            Request::LeaseAcquire(_) => {
                let capability = self
                    .executor
                    .allocate_capability_id()
                    .ok_or(DispatchError::Semantic(ErrorCode::Unavailable))?;
                let authenticated = self.state.authenticate_observer(connection);
                self.apply_state(authenticated, None)?;
                if let Err(error) = self.state.acquire_capture_lease(connection, capability) {
                    let _ = self
                        .state
                        .controller_lost(connection, ControllerLossReason::Eof);
                    return Err(self.handle_transition_error(error, None));
                }
                let authority = self
                    .capture_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                self.active_bindings = None;
                self.active_activation = None;
                self.active_session_keys = 0;
                self.active_session_mode = None;
                self.active_paste = None;
                let sequence = CapabilitySequenceValidator::new(
                    CapabilityKind::Capture,
                    Bytes32::new(*authority.id().as_bytes()),
                    authority.epoch().get(),
                )
                .map_err(|_| DispatchError::Fatal)?;
                self.connections
                    .get_mut(&connection)
                    .ok_or(DispatchError::Fatal)?
                    .capture_sequence = Some(sequence);
                self.renew_capability_deadline(connection)?;
                self.lease_acquired = self.lease_acquired.saturating_add(1);
                Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
                    capture_lease_id: Bytes32::new(*authority.id().as_bytes()),
                    capture_lease_epoch: wire_u64(authority.epoch().get()),
                    state: AcquireState::Disabled,
                }))
            }
            Request::MaintenanceAcquire(params) => {
                let capability = self
                    .executor
                    .allocate_capability_id()
                    .ok_or(DispatchError::Semantic(ErrorCode::Unavailable))?;
                let maintenance = maintenance_request(params).ok_or(DispatchError::Fatal)?;
                if self
                    .maintenance_request
                    .is_some_and(|existing| existing != maintenance)
                {
                    return Err(DispatchError::Semantic(ErrorCode::InvalidState));
                }
                let transition =
                    self.state
                        .acquire_maintenance(connection, capability, maintenance);
                let summary = self.apply_state(transition, None)?;
                if summary.response_stage != Some(ResponseStage::MaintenanceAcquireReady) {
                    return Err(DispatchError::Semantic(
                        if summary.native_failure.is_some() {
                            ErrorCode::NativeFailure
                        } else {
                            ErrorCode::Draining
                        },
                    ));
                }
                self.maintenance_request = Some(maintenance);
                let authority = self
                    .maintenance_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                let sequence = CapabilitySequenceValidator::new(
                    CapabilityKind::Maintenance,
                    Bytes32::new(*authority.id().as_bytes()),
                    authority.epoch().get(),
                )
                .map_err(|_| DispatchError::Fatal)?;
                self.connections
                    .get_mut(&connection)
                    .ok_or(DispatchError::Fatal)?
                    .maintenance_sequence = Some(sequence);
                self.renew_capability_deadline(connection)?;
                Response::Success(SuccessResult::MaintenanceAcquire(
                    MaintenanceAcquireResult {
                        maintenance_capability_id: Bytes32::new(*authority.id().as_bytes()),
                        maintenance_capability_epoch: wire_u64(authority.epoch().get()),
                        state: if self.state.ownership().is_native_neutral() {
                            MaintenanceAcquireState::Sealed
                        } else {
                            MaintenanceAcquireState::Draining
                        },
                    },
                ))
            }
            Request::HealthGet(_) => Response::Success(SuccessResult::Health(self.health())),
            Request::PermissionsGet(_) => {
                Response::Success(SuccessResult::Permissions(self.executor.permissions()))
            }
            Request::ObservabilityGet(_) => {
                let mut observability = self.executor.observability();
                let negotiated = self.connections.get(&connection).is_some_and(|active| {
                    active.codec.supports_feature(
                        talking_quill_owner_protocol::REGISTERED_INPUT_OBSERVABILITY_V1,
                    )
                });
                if !negotiated {
                    observability.registered_input = None;
                }
                observability.owner.lease_acquired = wire_counter(self.lease_acquired);
                observability.owner.lease_renewed = wire_counter(self.lease_renewed);
                observability.owner.lease_expired = wire_counter(self.lease_expired);
                observability.owner.lease_disconnected = wire_counter(self.lease_disconnected);
                observability.owner.lease_released_neutral =
                    wire_counter(self.lease_released_neutral);
                observability.owner.lease_released_draining =
                    wire_counter(self.lease_released_draining);
                if let Some(registered) = observability.registered_input.as_mut() {
                    registered.owner_admitted = wire_counter(self.registered_owner_admitted);
                    registered.owner_flushed = wire_counter(self.registered_owner_flushed);
                    registered.owner_rejected = wire_counter(self.registered_owner_rejected);
                }
                Response::Success(SuccessResult::Observability(Box::new(observability)))
            }
            Request::FrontAppGet(_) => {
                Response::Success(SuccessResult::FrontApp(self.executor.front_app()))
            }
            Request::FrontAppMetadataGet(_) => Response::Success(SuccessResult::FrontAppMetadata(
                self.executor.front_app_metadata(),
            )),
            Request::LeaseRenew(params) => {
                self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Renew,
                    None,
                )?;
                self.renew_capability_deadline(connection)?;
                self.lease_renewed = self.lease_renewed.saturating_add(1);
                Response::Success(SuccessResult::Renew(RenewResult { renewed: true }))
            }
            Request::SessionReconcileOff(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::ReconcileSessionOff,
                    None,
                )?;
                require_native_success(summary)?;
                self.active_session_mode = Some(SessionCaptureMode::Off);
                Response::Success(SuccessResult::SessionMode(SessionModeResult {
                    mode: SessionMode::Off,
                }))
            }
            Request::SessionSetMode(params) => {
                let mode = state_session_mode(params.mode);
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::SetSessionMode(mode),
                    None,
                )?;
                require_native_success(summary)?;
                self.active_session_mode = Some(mode);
                Response::Success(SuccessResult::SessionMode(SessionModeResult {
                    mode: params.mode,
                }))
            }
            Request::CaptureReplaceConfiguration(params) => {
                let bindings = core_bindings(&params.bindings)
                    .map_err(|_| DispatchError::Semantic(ErrorCode::InvalidState))?;
                let revision = ConfigurationRevision::new(params.revision.get())
                    .ok_or(DispatchError::Fatal)?;
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::ReplaceConfiguration { revision, bindings },
                    None,
                )?;
                require_native_success(summary)?;
                self.active_bindings = Some(bindings);
                self.active_activation = None;
                self.active_session_keys = 0;
                Response::Success(SuccessResult::Configuration(ConfigurationResult {
                    revision: params.revision,
                }))
            }
            Request::CaptureSetEnabled(params) => {
                let command = if params.enabled {
                    crate::state::CaptureCommand::Enable
                } else {
                    crate::state::CaptureCommand::Disable
                };
                let summary =
                    self.apply_capture(connection, params.command_sequence.get(), command, None)?;
                require_native_success(summary)?;
                if !params.enabled {
                    self.active_activation = None;
                    self.active_session_keys = 0;
                }
                Response::Success(SuccessResult::Enabled(EnabledResult {
                    enabled: params.enabled,
                }))
            }
            Request::PasteInject(params) => {
                let authority = self
                    .capture_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                let operation = PasteOperationId::new(*params.operation_id.as_bytes())
                    .ok_or(DispatchError::Fatal)?;
                let owner_instance = OwnerInstanceId::new(*params.owner_instance_id.as_bytes())
                    .ok_or(DispatchError::Fatal)?;
                let generation = OwnerActivationGeneration::new(params.activation_generation.get())
                    .ok_or(DispatchError::Fatal)?;
                let authorization = PasteAuthorization::new(
                    operation,
                    owner_instance,
                    authority.epoch(),
                    generation,
                );
                let target_token = params
                    .target_token
                    .as_ref()
                    .map(|value| NativeTargetToken::new(value.as_str()))
                    .transpose()
                    .map_err(|_| DispatchError::Fatal)?;
                let context = PasteExecutorRequest {
                    authorization,
                    target_token,
                    fallback_text_sha256: params.fallback_text_sha256,
                };
                let summary = match self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::BeginPaste(authorization),
                    Some(context),
                ) {
                    Ok(summary) => summary,
                    Err(error) => {
                        if self.state.ownership().paste() == PasteOwnership::Indeterminate {
                            self.active_paste = Some(context);
                        }
                        return Err(error);
                    }
                };
                let result = if let Some(reason) = summary.paste_refusal {
                    PasteResult::ClipboardOnly {
                        reason: reason.wire_reason(),
                    }
                } else if summary.paste_waiting {
                    self.active_paste = Some(context);
                    PasteResult::Waiting {
                        operation_id: params.operation_id,
                    }
                } else if self.state.ownership().paste() == PasteOwnership::Indeterminate {
                    self.active_paste = Some(context);
                    PasteResult::Indeterminate {
                        operation_id: params.operation_id,
                    }
                } else if summary.paste_failure == Some(NativeActionFailure::FailedNotApplied) {
                    PasteResult::ClipboardOnly {
                        reason: PasteRefusalReason::NativeRejected,
                    }
                } else {
                    return Err(DispatchError::Semantic(ErrorCode::NativeFailure));
                };
                Response::Success(SuccessResult::Paste(result))
            }
            Request::LeaseRelease(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Release,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                match disposition {
                    LeaseDisposition::Neutral => {
                        self.lease_released_neutral = self.lease_released_neutral.saturating_add(1);
                    }
                    LeaseDisposition::Draining => {
                        self.lease_released_draining =
                            self.lease_released_draining.saturating_add(1);
                    }
                }
                self.planned_exit_when_neutral = false;
                Response::Success(SuccessResult::Release(ReleaseResult {
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::OwnerExitWhenNeutral(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Release,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                match disposition {
                    LeaseDisposition::Neutral => {
                        self.lease_released_neutral = self.lease_released_neutral.saturating_add(1);
                    }
                    LeaseDisposition::Draining => {
                        self.lease_released_draining =
                            self.lease_released_draining.saturating_add(1);
                    }
                }
                self.planned_exit_when_neutral = true;
                self.planned_exit_terminal_pending = disposition == LeaseDisposition::Draining;
                Response::Success(SuccessResult::Release(ReleaseResult {
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::RuntimeRollback(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::RuntimeRollback,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                Response::Success(SuccessResult::Rollback(RollbackResult {
                    latched: true,
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::MaintenanceRenew(params) => {
                self.apply_maintenance(
                    connection,
                    params.command_sequence.get(),
                    MaintenanceCommand::Renew,
                    None,
                )?;
                self.renew_capability_deadline(connection)?;
                Response::Success(SuccessResult::Renew(RenewResult { renewed: true }))
            }
            Request::MaintenancePrepare(params) => {
                let expected = self.maintenance_request.ok_or(DispatchError::Fatal)?;
                if params.transaction_id.as_bytes() != expected.transaction().as_bytes()
                    || maintenance_operation(params.operation) != expected.operation()
                {
                    // B1 already consumed this authenticated capability
                    // sequence. Consume the same sequence in W1 without native
                    // work or lease-liveness renewal before rejecting it.
                    self.apply_maintenance(
                        connection,
                        params.command_sequence.get(),
                        MaintenanceCommand::ConsumeSemanticRejection,
                        None,
                    )?;
                    return Err(DispatchError::Semantic(ErrorCode::InvalidState));
                }
                let correlation = ResponseCorrelation::new(received.transport_sequence())
                    .ok_or(DispatchError::Fatal)?;
                let summary = self.apply_maintenance(
                    connection,
                    params.command_sequence.get(),
                    MaintenanceCommand::Prepare {
                        operation: maintenance_operation(params.operation),
                        response_correlation: correlation,
                    },
                    None,
                )?;
                if summary.response_stage != Some(ResponseStage::FinalResponseReady) {
                    return Err(DispatchError::Semantic(
                        if summary.native_failure.is_some() {
                            ErrorCode::NativeFailure
                        } else {
                            ErrorCode::Draining
                        },
                    ));
                }
                final_flush = Some(correlation);
                Response::Success(SuccessResult::MaintenancePrepare(
                    MaintenancePrepareResult {
                        ready_to_exit: true,
                        owner_handoff: Bytes32::new(*expected.owner_handoff().as_bytes()),
                    },
                ))
            }
        };
        Ok(DispatchResult {
            response,
            final_flush,
            planned_exit: matches!(request, Request::OwnerExitWhenNeutral(_)),
        })
    }

    /// Closes every attached authority route before signal/session shutdown.
    /// Native ownership remains in this server and continues through the
    /// ordinary orphan cancellation/drain path.
    pub fn detach_all_for_shutdown(&mut self) -> Result<(), ServerError> {
        let connections = self.connections.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for connection in connections {
            if self.connections.contains_key(&connection)
                && let Err(error) = self.teardown_connection(connection, ControllerLossReason::Eof)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Applies the process rollback latch through the same ordered native close
    /// path used by a protocol rollback command.
    pub fn latch_runtime_rollback(&mut self) -> Result<(), ServerError> {
        let transition = self.state.latch_runtime_rollback();
        self.drive_external_transition(transition)
    }

    /// Attempts a neutral, quiescent idle exit. Busy/draining are returned to
    /// the outer runtime so it can keep pumping rather than invent neutrality.
    pub fn request_idle_exit(&mut self) -> Result<(), ServerError> {
        let transition = self.state.request_idle_exit();
        self.drive_external_transition(transition)
    }

    /// Explicit containment for an outer-loop/provider failure. This sacrifices
    /// future availability, closes fresh admission, and preserves drain work.
    pub fn recover_fatal_runtime_fault(&mut self) -> Result<(), ServerError> {
        let transition = self.state.recoverable_native_fault();
        self.drive_external_transition(transition)
    }

    fn apply_capture(
        &mut self,
        connection: ConnectionId,
        sequence: u64,
        command: crate::state::CaptureCommand,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        let authority = self
            .capture_authority(connection)
            .ok_or(DispatchError::Fatal)?;
        let sequence = CommandSequence::new(sequence).ok_or(DispatchError::Fatal)?;
        let transition = self
            .state
            .apply_capture_command(connection, authority, sequence, command);
        self.apply_state(transition, paste)
    }

    fn apply_maintenance(
        &mut self,
        connection: ConnectionId,
        sequence: u64,
        command: MaintenanceCommand,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        let authority = self
            .maintenance_authority(connection)
            .ok_or(DispatchError::Fatal)?;
        let sequence = CommandSequence::new(sequence).ok_or(DispatchError::Fatal)?;
        let transition = self
            .state
            .apply_maintenance_command(connection, authority, sequence, command);
        self.apply_state(transition, paste)
    }

    fn apply_broker_event(
        &mut self,
        event: BrokerEvent,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        match event {
            BrokerEvent::Keyboard(event) => {
                self.admit_keyboard_event_inner(event, before_close_barrier)
            }
            BrokerEvent::RegisteredObservation { generation } => {
                self.admit_registered_observation_inner(generation, before_close_barrier)
            }
            BrokerEvent::AudioInputDevicesChanged => {
                self.admit_audio_devices_changed_inner(before_close_barrier)
            }
            BrokerEvent::OwnershipChanged(observation) => {
                self.observe_native_observation_inner(observation)
            }
            BrokerEvent::ReadinessChanged(readiness) => {
                self.observe_native_readiness_inner(readiness)
            }
            BrokerEvent::PasteClaimed(authorization) => {
                self.confirm_paste_claimed_inner(authorization)
            }
            BrokerEvent::PasteFinished {
                authorization,
                outcome,
            } => self.publish_paste_committed_inner(
                authorization,
                outcome == PasteCommitOutcome::Indeterminate,
            ),
            BrokerEvent::PasteIndeterminateResolved(authorization) => {
                self.confirm_paste_completed_inner(authorization)
            }
            BrokerEvent::RecoverableNativeFault => self.observe_recoverable_native_fault_inner(),
        }
    }

    fn latch_adapter_desynchronization(&mut self) {
        let transition = self.state.adapter_stream_desynchronized();
        let _ = self.drive_external_transition(transition);
    }

    fn pump_executor_adapter_event(
        &mut self,
        before_close_barrier: bool,
    ) -> Option<AdapterEventDisposition> {
        let (envelope, sequence_valid) = self.executor.try_next_adapter_event()?;
        let event = envelope.event();
        let nested = self.adapter_event_in_flight.is_some();
        if !nested {
            self.adapter_event_in_flight = Some(envelope.id());
        }
        let server_sequence_valid = self
            .adapter_event_high_water
            .checked_add(1)
            .is_some_and(|expected| expected == envelope.id().get());
        let disposition = if sequence_valid && server_sequence_valid {
            // Advance before semantic application so a reentrant close barrier
            // recognizes the current event. Its acknowledgement remains
            // ordered after application and before any nested acknowledgements.
            self.adapter_event_high_water = envelope.id().get();
            let preflush_error = if matches!(
                event,
                BrokerEvent::Keyboard(_)
                    | BrokerEvent::RegisteredObservation { .. }
                    | BrokerEvent::AudioInputDevicesChanged
            ) {
                None
            } else {
                self.flush_admitted_events().err()
            };
            // A later authoritative fact is applied even when delivery of an
            // earlier semantic notification failed and tore down its route.
            let application = self.apply_broker_event(event, before_close_barrier);
            match application {
                Ok(()) => AdapterEventDisposition::Accepted,
                Err(_error) if authoritative_control_event(event) => {
                    AdapterEventDisposition::Accepted
                }
                Err(error) => AdapterEventDisposition::Rejected(adapter_rejection(
                    event,
                    preflush_error.as_ref().unwrap_or(&error),
                )),
            }
        } else {
            self.latch_adapter_desynchronization();
            AdapterEventDisposition::Rejected(AdapterEventRejection::InvalidTransition)
        };
        if nested {
            self.deferred_adapter_acknowledgements
                .push_back((envelope.id(), disposition));
        } else {
            self.executor
                .acknowledge_adapter_event(envelope.id(), disposition);
            self.adapter_event_in_flight = None;
            while let Some((id, deferred)) = self.deferred_adapter_acknowledgements.pop_front() {
                self.executor.acknowledge_adapter_event(id, deferred);
            }
            self.finalize_acknowledged_close_confirmation();
            self.service_deferred_controller_losses();
        }
        Some(disposition)
    }

    fn service_deferred_controller_losses(&mut self) {
        while let Some((connection, reason)) = self.deferred_controller_losses.pop_front() {
            let _ = self.handle_connection_loss(connection, reason);
            self.reap_closed_connection(connection);
        }
    }

    fn finalize_acknowledged_close_confirmation(&mut self) {
        let Some(action) = self.pending_close_confirmation.take() else {
            return;
        };
        let mut summary = DriveSummary::default();
        match self.confirm_executor_result(action, ExecutorResult::Applied, &mut summary) {
            Ok(Some(transition)) => {
                if self.state.admission() == AdmissionState::Closed {
                    self.closing_event_scope = None;
                }
                let _ = self.drive_transition(transition, None);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = self.drive_confirmation_error(&error, None);
            }
        }
    }

    fn drain_adapter_events_through(
        &mut self,
        through_event: Option<crate::adapter::AdapterEventId>,
    ) {
        let target = through_event.map_or(0, crate::adapter::AdapterEventId::get);
        if target < self.adapter_event_high_water {
            self.latch_adapter_desynchronization();
        }
        while self.adapter_event_high_water < target {
            let before = self.adapter_event_high_water;
            if self.pump_executor_adapter_event(true).is_none()
                || self.adapter_event_high_water == before
            {
                self.latch_adapter_desynchronization();
                break;
            }
        }
        // Delivery failure retires the semantic notification and tears down
        // its controller route; it is not an adapter event-sequence fault.
        let _ = self.flush_admitted_events();
    }

    fn drive_external_transition(
        &mut self,
        transition: Result<Transition, TransitionError>,
    ) -> Result<(), ServerError> {
        match transition {
            Ok(transition) => {
                self.drive_transition(transition, None)?;
                Ok(())
            }
            Err(error) => {
                let actions = error.actions().as_slice().to_vec();
                let offer = error.terminal_offer();
                self.drive_actions_and_offer(&actions, offer, None)?;
                Err(ServerError::State(error))
            }
        }
    }

    fn apply_state(
        &mut self,
        transition: Result<Transition, TransitionError>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        match transition {
            Ok(transition) => match self.drive_transition(transition, paste) {
                Ok(summary) => Ok(summary),
                Err(ServerError::CloseRetryExhausted) => Err(DispatchError::Fatal),
                Err(_) => Err(DispatchError::Semantic(ErrorCode::NativeFailure)),
            },
            Err(error) => Err(self.handle_transition_error(error, paste)),
        }
    }

    fn handle_transition_error(
        &mut self,
        error: TransitionError,
        paste: Option<PasteExecutorRequest>,
    ) -> DispatchError {
        let actions = error.actions().as_slice().to_vec();
        let offer = error.terminal_offer();
        if self
            .drive_actions_and_offer(&actions, offer, paste)
            .is_err()
        {
            return DispatchError::Fatal;
        }
        if matches!(
            error.kind(),
            TransitionErrorKind::ProtocolFault | TransitionErrorKind::WrongController
        ) {
            DispatchError::Fatal
        } else {
            DispatchError::Semantic(error_code(error.kind()))
        }
    }

    fn drive_transition(
        &mut self,
        transition: Transition,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        let actions = transition.actions().as_slice().to_vec();
        let offer = transition.terminal_offer();
        let mut summary = DriveSummary {
            lease_disposition: transition.lease_disposition(),
            response_stage: transition.response_stage(),
            ..DriveSummary::default()
        };
        summary.merge(self.drive_actions_and_offer(&actions, offer, paste)?);
        Ok(summary)
    }

    fn drive_actions_and_offer(
        &mut self,
        actions: &[RequiredAction],
        offer: Option<PredecessorTerminalOffer>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        let mut summary = DriveSummary::default();
        let mut pending_offer = offer;
        if self.state.admission() == AdmissionState::Closed
            && let Some(offer) = pending_offer.take()
        {
            self.emit_terminal_offer(offer)?;
        }
        for action in actions.iter().copied() {
            if matches!(
                action,
                RequiredAction::CloseFreshAdmission { .. }
                    | RequiredAction::EmergencyCloseFreshAdmission
            ) {
                summary.merge(self.drive_close_iteratively(action, &mut pending_offer, paste)?);
                continue;
            }
            if action == RequiredAction::ExitOwner {
                // W1 has already authorized exit after final flush. This is an
                // outer-loop directive, not a fallible native confirmation.
                let _ = self.executor.execute(ExecutorCommand::State(action));
                self.exit_requested = true;
                continue;
            }
            let command = if matches!(action, RequiredAction::AdmitPaste { .. }) {
                ExecutorCommand::AdmitPaste {
                    action,
                    request: paste.ok_or(ServerError::ExecutorContract)?,
                }
            } else {
                ExecutorCommand::State(action)
            };
            let result = self.executor.execute(command);
            let transition = match self.confirm_executor_result(action, result, &mut summary) {
                Ok(transition) => transition,
                Err(error) => {
                    let recovery = self.drive_confirmation_error(&error, paste);
                    self.fail_pending_terminal_offer(&mut pending_offer);
                    recovery?;
                    return Err(error);
                }
            };
            if let Some(transition) = transition {
                summary.merge(self.drive_transition(transition, paste)?);
            }
        }
        if pending_offer.is_some() {
            // Closure could not be established, so do not publish revocation
            // ahead of the native boundary. Retire the best-effort route.
            self.fail_pending_terminal_offer(&mut pending_offer);
        }
        self.advance_predecessor_terminal()?;
        Ok(summary)
    }

    fn drive_close_iteratively(
        &mut self,
        initial_action: RequiredAction,
        pending_offer: &mut Option<PredecessorTerminalOffer>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        self.ensure_closing_event_scope(*pending_offer);
        let mut summary = DriveSummary::default();
        let mut action = initial_action;
        for _ in 0..MAX_SYNCHRONOUS_CLOSE_ATTEMPTS {
            let mut result = self.executor.execute(ExecutorCommand::State(action));
            if let ExecutorResult::AdmissionClosed { through_event } = result {
                self.drain_adapter_events_through(through_event);
                if self.adapter_event_in_flight.is_some() {
                    // The native barrier is authoritative, but reducer close
                    // and dependent effects must follow FIFO acknowledgements
                    // for the event that triggered this close.
                    self.pending_close_confirmation = Some(action);
                    return Ok(summary);
                }
                result = ExecutorResult::Applied;
            }
            let transition = match self.confirm_executor_result(action, result, &mut summary) {
                Ok(Some(transition)) => transition,
                Ok(None) => {
                    self.fail_pending_terminal_offer(pending_offer);
                    return Err(ServerError::ExecutorContract);
                }
                Err(error) => {
                    let recovery = self.drive_confirmation_error(&error, paste);
                    self.fail_pending_terminal_offer(pending_offer);
                    recovery?;
                    return Err(ServerError::ExecutorContract);
                }
            };
            summary.lease_disposition =
                transition.lease_disposition().or(summary.lease_disposition);
            summary.response_stage = transition.response_stage().or(summary.response_stage);
            if self.state.admission() == AdmissionState::Closed {
                self.closing_event_scope = None;
                if let Some(offer) = pending_offer.take() {
                    // Close is confirmed before revocation, and revocation is
                    // emitted before the transition's dependent work.
                    self.emit_terminal_offer(offer)?;
                }
                summary.merge(self.drive_transition(transition, paste)?);
                return Ok(summary);
            }
            let retry_actions = transition.actions().as_slice();
            if transition.terminal_offer().is_some()
                || retry_actions.len() != 1
                || !matches!(
                    retry_actions[0],
                    RequiredAction::CloseFreshAdmission { .. }
                        | RequiredAction::EmergencyCloseFreshAdmission
                )
            {
                self.fail_pending_terminal_offer(pending_offer);
                return Err(ServerError::ExecutorContract);
            }
            action = retry_actions[0];
        }
        self.fail_pending_terminal_offer(pending_offer);
        self.deferred_close = Some(action);
        Err(ServerError::CloseRetryExhausted)
    }

    fn ensure_closing_event_scope(&mut self, offer: Option<PredecessorTerminalOffer>) {
        if self.closing_event_scope.is_some() {
            return;
        }
        let route = self
            .current_capture()
            .ok()
            .or_else(|| offer.map(|offer| (offer.connection(), offer.authority())));
        if let Some((connection, authority)) = route {
            self.closing_event_scope = Some(ClosingEventScope {
                connection,
                authority,
                bindings: self.active_bindings,
                session_mode: self.active_session_mode,
            });
        }
    }

    fn drive_confirmation_error(
        &mut self,
        error: &ServerError,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<(), ServerError> {
        let ServerError::State(error) = error else {
            return Ok(());
        };
        let mut actions = error.actions().as_slice().to_vec();
        if let Some(index) = actions.iter().position(|action| {
            matches!(
                action,
                RequiredAction::CloseFreshAdmission { .. }
                    | RequiredAction::EmergencyCloseFreshAdmission
            )
        }) {
            // Retain one close directive for the outer iterative service loop;
            // never recursively start a fresh close retry budget from a
            // confirmation error.
            self.deferred_close.get_or_insert(actions.remove(index));
        }
        let offer = error.terminal_offer();
        if actions.is_empty() && offer.is_none() {
            return Ok(());
        }
        self.drive_actions_and_offer(&actions, offer, paste)?;
        Ok(())
    }

    fn fail_pending_terminal_offer(
        &mut self,
        pending_offer: &mut Option<PredecessorTerminalOffer>,
    ) {
        if let Some(offer) = pending_offer.take() {
            let _ = self.state.fail_predecessor_terminal_write(offer);
        }
    }

    fn confirm_executor_result(
        &mut self,
        action: RequiredAction,
        result: ExecutorResult,
        summary: &mut DriveSummary,
    ) -> Result<Option<Transition>, ServerError> {
        let transition = match (action, result) {
            (RequiredAction::CloseFreshAdmission { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_admission_closed(token)?)
            }
            (RequiredAction::EmergencyCloseFreshAdmission, ExecutorResult::Applied) => {
                Some(self.state.confirm_emergency_admission_closed()?)
            }
            (RequiredAction::OpenFreshAdmission { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_admission_opened(token)?)
            }
            (RequiredAction::ApplySessionMode { token, mode }, ExecutorResult::Applied) => {
                Some(self.state.confirm_session_mode_applied(token, mode)?)
            }
            (
                RequiredAction::ApplyConfiguration { token, request, .. },
                ExecutorResult::Applied,
            ) => Some(self.state.confirm_configuration_applied(token, request)?),
            (
                RequiredAction::AdmitPaste {
                    token,
                    authorization,
                },
                ExecutorResult::PasteWaiting,
            ) => {
                summary.paste_waiting = true;
                Some(self.state.confirm_paste_waiting(token, authorization)?)
            }
            (
                RequiredAction::AdmitPaste {
                    token,
                    authorization,
                },
                ExecutorResult::PasteRefused(reason),
            ) => {
                summary.paste_refusal = Some(reason);
                Some(self.state.confirm_paste_refused(token, authorization)?)
            }
            (
                RequiredAction::CancelCandidate { token },
                ExecutorResult::CandidateCancelled(ownership),
            ) => Some(self.state.confirm_candidate_cancelled(token, ownership)?),
            (
                RequiredAction::CancelWaitingPaste {
                    token,
                    authorization,
                },
                ExecutorResult::Applied,
            ) => Some(
                self.state
                    .confirm_waiting_paste_cancelled(token, authorization)?,
            ),
            (
                RequiredAction::PersistMaintenanceRecord { token, request },
                ExecutorResult::Applied,
            ) => Some(self.state.confirm_maintenance_persisted(token, request)?),
            (RequiredAction::StopNativeAdapter { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_native_stopped(token)?)
            }
            (RequiredAction::ContinueNativeDrain, ExecutorResult::Applied) => None,
            (action, ExecutorResult::AdmissionClosed { .. }) => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_adapter_contract(action.token())?)
            }
            (action, ExecutorResult::ContractViolation) => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_adapter_contract(action.token())?)
            }
            (action, ExecutorResult::Failed(failure)) if action.token().is_some() => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(failure);
                }
                summary.native_failure = Some(failure);
                Some(
                    self.state
                        .fail_native_action(action.token().expect("checked token"), failure)?,
                )
            }
            (action, _) if action.token().is_some() => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_action(
                    action.token().expect("checked token"),
                    NativeActionFailure::Indeterminate,
                )?)
            }
            _ => {
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.recoverable_native_fault()?)
            }
        };
        Ok(transition)
    }

    fn validate_capability_request(
        &mut self,
        connection: ConnectionId,
        request: &Request,
    ) -> Result<(), ServerError> {
        let active = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?;
        let validator = match request {
            Request::MaintenanceRenew(_) | Request::MaintenancePrepare(_) => {
                active.maintenance_sequence.as_mut()
            }
            _ => active.capture_sequence.as_mut(),
        }
        .ok_or(ServerError::CapabilityProtocol)?;
        validator
            .accept(request)
            .map_err(|_| ServerError::CapabilityProtocol)
    }

    fn enqueue_transport_frame(
        &mut self,
        connection: ConnectionId,
        frame: Vec<u8>,
        completion: FlushCompletion,
    ) -> Result<TransportProgress, ServerError> {
        let (receipt, progress) = {
            let active = self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)?;
            if active.pending_flushes.len() >= MAX_PENDING_TRANSPORT_FLUSHES {
                return Err(ServerError::TransportBackpressure);
            }
            let receipt = active.endpoint.try_send(frame)?;
            let progress = if active.pending_flushes.is_empty() {
                active.endpoint.flush(receipt)?
            } else {
                TransportProgress::Pending
            };
            (receipt, progress)
        };
        match progress {
            TransportProgress::Pending => {
                self.connections
                    .get_mut(&connection)
                    .ok_or(ServerError::UnknownConnection)?
                    .pending_flushes
                    .push_back(PendingFlush {
                        receipt,
                        completion,
                    });
            }
            TransportProgress::Complete => {
                self.complete_transport_flush(connection, completion)?;
            }
        }
        Ok(progress)
    }

    fn service_transport_close(
        &mut self,
        connection: ConnectionId,
    ) -> Result<TransportProgress, ServerError> {
        let Some(correlation) = self
            .connections
            .get(&connection)
            .and_then(|active| active.pending_close)
        else {
            return Ok(TransportProgress::Complete);
        };
        let progress = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .endpoint
            .close()?;
        if progress == TransportProgress::Complete {
            self.finish_final_transport_close(connection, correlation)?;
        }
        Ok(progress)
    }

    fn service_transport_flush(
        &mut self,
        connection: ConnectionId,
    ) -> Result<TransportProgress, ServerError> {
        let Some(receipt) = self
            .connections
            .get(&connection)
            .and_then(|active| active.pending_flushes.front())
            .map(|pending| pending.receipt)
        else {
            return Ok(TransportProgress::Complete);
        };
        let progress = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .endpoint
            .flush(receipt)?;
        if progress == TransportProgress::Complete {
            let completion = self
                .connections
                .get_mut(&connection)
                .and_then(|active| active.pending_flushes.pop_front())
                .ok_or(ServerError::ExecutorContract)?
                .completion;
            self.complete_transport_flush(connection, completion)?;
        }
        Ok(progress)
    }

    fn complete_transport_flush(
        &mut self,
        connection: ConnectionId,
        completion: FlushCompletion,
    ) -> Result<(), ServerError> {
        match completion {
            FlushCompletion::Ordinary => {}
            FlushCompletion::AdmittedEvent => {
                let queued = self
                    .queued_events
                    .pop_front()
                    .ok_or(ServerError::ExecutorContract)?;
                if queued.connection != connection {
                    return Err(ServerError::ExecutorContract);
                }
                let registered_activation = matches!(
                    queued.event,
                    Event::Activation(_) | Event::RegisteredObservation(_)
                );
                self.retire_one_admitted_effect()?;
                if registered_activation {
                    self.registered_owner_flushed = self.registered_owner_flushed.saturating_add(1);
                }
                self.ensure_effect_queue_consistent()?;
            }
            FlushCompletion::PredecessorTerminal(offer) => {
                self.state.confirm_predecessor_terminal_written(offer)?;
                match offer.event() {
                    PredecessorTerminalEvent::LeaseRevoked(_) => {
                        self.terminal_draining_sent.insert(connection, false);
                    }
                    PredecessorTerminalEvent::LeaseDraining(_) => {
                        self.terminal_draining_sent.insert(connection, true);
                    }
                    PredecessorTerminalEvent::LeaseNeutral
                    | PredecessorTerminalEvent::LeaseUnavailable(_) => {
                        if matches!(offer.event(), PredecessorTerminalEvent::LeaseNeutral) {
                            self.planned_exit_terminal_pending = false;
                        }
                        self.terminal_draining_sent.remove(&connection);
                    }
                }
            }
            FlushCompletion::PlannedExitResponse => {
                self.planned_exit_response_flushed = true;
            }
            FlushCompletion::FinalResponse(correlation) => {
                let progress = self
                    .connections
                    .get_mut(&connection)
                    .ok_or(ServerError::UnknownConnection)?
                    .endpoint
                    .close()?;
                if progress == TransportProgress::Pending {
                    self.connections
                        .get_mut(&connection)
                        .ok_or(ServerError::UnknownConnection)?
                        .pending_close = Some(correlation);
                } else {
                    self.finish_final_transport_close(connection, correlation)?;
                }
            }
        }
        Ok(())
    }

    fn finish_final_transport_close(
        &mut self,
        connection: ConnectionId,
        correlation: ResponseCorrelation,
    ) -> Result<(), ServerError> {
        let active = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?;
        active.pending_close = None;
        active.closed = true;
        let transition = self.state.confirm_final_response_flushed(correlation)?;
        self.drive_transition(transition, None)?;
        self.reap_closed_connection(connection);
        Ok(())
    }

    fn enqueue_and_flush_response(
        &mut self,
        connection: ConnectionId,
        request: &ReceivedRequest,
        response: &Response,
    ) -> Result<TransportProgress, ServerError> {
        let frame = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .encode_response(request, response)?;
        self.enqueue_transport_frame(connection, frame, FlushCompletion::Ordinary)
    }

    fn encode_enqueue_flush_event(
        &mut self,
        connection: ConnectionId,
        event: &Event,
    ) -> Result<TransportProgress, ServerError> {
        let frame = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .encode_event(event)?;
        self.enqueue_transport_frame(connection, frame, FlushCompletion::Ordinary)
    }

    fn send_direct_event(
        &mut self,
        connection: ConnectionId,
        event: &Event,
    ) -> Result<(), ServerError> {
        if self.encode_enqueue_flush_event(connection, event).is_ok() {
            return Ok(());
        }
        let _ = self.teardown_connection(connection, ControllerLossReason::Eof);
        Err(ServerError::EventDelivery)
    }

    fn emit_terminal_offer(&mut self, offer: PredecessorTerminalOffer) -> Result<(), ServerError> {
        let event = wire_terminal_event(offer);
        let connection = offer.connection();
        let frame = self
            .connections
            .get_mut(&connection)
            .filter(|active| !active.closed)
            .ok_or(ServerError::TerminalRoute)
            .and_then(|active| {
                active
                    .codec
                    .encode_predecessor_terminal(&event)
                    .map_err(Into::into)
            });
        let write = frame.and_then(|frame| {
            self.enqueue_transport_frame(
                connection,
                frame,
                FlushCompletion::PredecessorTerminal(offer),
            )
        });
        if write.is_err() {
            if let Some(active) = self.connections.get_mut(&connection) {
                active.endpoint.abort();
                active.closed = true;
            }
            self.state.fail_predecessor_terminal_write(offer)?;
            self.terminal_draining_sent.remove(&connection);
            self.reap_closed_connection(connection);
        }
        Ok(())
    }

    fn advance_predecessor_terminal(&mut self) -> Result<(), ServerError> {
        let Some((&connection, &draining_sent)) = self.terminal_draining_sent.iter().next() else {
            return Ok(());
        };
        let status = self.state.status();
        let candidate = if status.native_state_unknown {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                    TerminalUnavailableReason::OwnershipUnknown,
                ))
        } else if status.process_state == ProcessState::Degraded {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                    TerminalUnavailableReason::NativeFault,
                ))
        } else if self.state.ownership().is_native_neutral() {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
        } else if !draining_sent {
            terminal_ownership(self.state.ownership()).and_then(|ownership| {
                self.state
                    .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseDraining(ownership))
            })
        } else {
            None
        };
        if let Some(offer) = candidate {
            debug_assert_eq!(offer.connection(), connection);
            self.emit_terminal_offer(offer)?;
        }
        Ok(())
    }

    fn retire_queued_events_for_connection(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        let before = self.queued_events.len();
        self.queued_events
            .retain(|queued| queued.connection != connection);
        let removed = before - self.queued_events.len();
        if removed == 0 {
            return Ok(());
        }
        let removed = u8::try_from(removed).map_err(|_| ServerError::ExecutorContract)?;
        let current = self.state.ownership().admitted_effects();
        let remaining = current
            .checked_sub(removed)
            .ok_or(ServerError::ExecutorContract)?;
        let transition = self.state.set_broker_admitted_effects(remaining)?;
        self.drive_transition(transition, None)?;
        self.ensure_effect_queue_consistent()
    }

    fn handle_connection_loss(
        &mut self,
        connection: ConnectionId,
        reason: ControllerLossReason,
    ) -> Result<(), ServerError> {
        match self.state.controller() {
            ControllerState::AuthenticatedObserver { connection: active }
            | ControllerState::CaptureLeaseDisabled {
                connection: active, ..
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active, ..
            }
            | ControllerState::MaintenanceExclusive {
                connection: active, ..
            } if active == connection => match self.state.controller_lost(connection, reason) {
                Ok(transition) => {
                    self.drive_transition(transition, None)?;
                }
                Err(error) => {
                    let _ = self.handle_transition_error(error, None);
                }
            },
            _ => self.retire_disconnected_predecessor(connection)?,
        }
        if self.capture_authority(connection).is_none() {
            self.active_activation = None;
            self.active_session_keys = 0;
            self.active_session_mode = None;
        }
        if self.state.ownership().paste() == PasteOwnership::None {
            self.active_paste = None;
        }
        Ok(())
    }

    fn retire_disconnected_predecessor(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        if !self.terminal_draining_sent.contains_key(&connection) {
            return Ok(());
        }
        let status = self.state.status();
        let event = if status.native_state_unknown {
            PredecessorTerminalEvent::LeaseUnavailable(TerminalUnavailableReason::OwnershipUnknown)
        } else if status.process_state == ProcessState::Degraded {
            PredecessorTerminalEvent::LeaseUnavailable(TerminalUnavailableReason::NativeFault)
        } else if self.state.ownership().is_native_neutral() {
            PredecessorTerminalEvent::LeaseNeutral
        } else if let Some(ownership) = terminal_ownership(self.state.ownership()) {
            PredecessorTerminalEvent::LeaseDraining(ownership)
        } else {
            return Ok(());
        };
        if let Some(offer) = self.state.offer_predecessor_terminal(event) {
            self.state.fail_predecessor_terminal_write(offer)?;
            self.terminal_draining_sent.remove(&connection);
        }
        Ok(())
    }

    fn abort_connection(&mut self, connection: ConnectionId) {
        if let Some(active) = self.connections.get_mut(&connection) {
            active.endpoint.abort();
            active.closed = true;
        }
    }

    fn teardown_connection(
        &mut self,
        connection: ConnectionId,
        reason: ControllerLossReason,
    ) -> Result<(), ServerError> {
        let held_capture_lease = matches!(
            self.state.controller(),
            ControllerState::CaptureLeaseDisabled { connection: active, .. }
                | ControllerState::CaptureLeaseEnabled { connection: active, .. }
                if active == connection
        );
        if held_capture_lease && reason != ControllerLossReason::HeartbeatExpired {
            self.lease_disconnected = self.lease_disconnected.saturating_add(1);
        }
        let pending_failure = self.fail_pending_transport_writes(connection);
        self.abort_connection(connection);
        let retirement = self.retire_queued_events_for_connection(connection);
        let controller_loss = self.handle_connection_loss(connection, reason);
        self.reap_closed_connection(connection);
        match (pending_failure, retirement, controller_loss) {
            (Err(error), _, _) | (Ok(()), Err(error), _) | (Ok(()), Ok(()), Err(error)) => {
                Err(error)
            }
            (Ok(()), Ok(()), Ok(())) => Ok(()),
        }
    }

    fn fail_pending_transport_writes(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        let pending = self
            .connections
            .get_mut(&connection)
            .map(|active| {
                active.pending_close = None;
                active.pending_flushes.drain(..).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for write in pending {
            if let FlushCompletion::PredecessorTerminal(offer) = write.completion {
                self.state.fail_predecessor_terminal_write(offer)?;
                self.terminal_draining_sent.remove(&connection);
            }
        }
        Ok(())
    }

    fn reap_closed_connection(&mut self, connection: ConnectionId) {
        let is_controller = match self.state.controller() {
            ControllerState::AuthenticatedObserver { connection: active }
            | ControllerState::CaptureLeaseDisabled {
                connection: active, ..
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active, ..
            }
            | ControllerState::MaintenanceExclusive {
                connection: active, ..
            } => active == connection,
            ControllerState::NoController => false,
        };
        let removable = self
            .connections
            .get(&connection)
            .is_some_and(|active| active.closed)
            && !is_controller
            && !self.terminal_draining_sent.contains_key(&connection)
            && !self
                .queued_events
                .iter()
                .any(|queued| queued.connection == connection)
            && self
                .closing_event_scope
                .is_none_or(|scope| scope.connection != connection);
        if removable {
            self.connections.remove(&connection);
        }
    }

    fn sweep_closed_connections(&mut self) {
        let candidates = self
            .connections
            .iter()
            .filter_map(|(connection, active)| active.closed.then_some(*connection))
            .collect::<Vec<_>>();
        for connection in candidates {
            self.reap_closed_connection(connection);
        }
    }

    fn capture_authority(&self, connection: ConnectionId) -> Option<CapabilityRef> {
        match self.state.controller() {
            ControllerState::CaptureLeaseDisabled {
                connection: active,
                authority,
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active,
                authority,
            } if active == connection => Some(authority),
            _ => None,
        }
    }

    fn maintenance_authority(&self, connection: ConnectionId) -> Option<CapabilityRef> {
        match self.state.controller() {
            ControllerState::MaintenanceExclusive {
                connection: active,
                authority,
            } if active == connection => Some(authority),
            _ => None,
        }
    }

    fn current_capture(&self) -> Result<(ConnectionId, CapabilityRef), ServerError> {
        match self.state.controller() {
            ControllerState::CaptureLeaseDisabled {
                connection,
                authority,
            }
            | ControllerState::CaptureLeaseEnabled {
                connection,
                authority,
            } => Ok((connection, authority)),
            _ => Err(ServerError::EventScope),
        }
    }

    fn current_capture_for_event(
        &self,
        before_close_barrier: bool,
    ) -> Result<(ConnectionId, crate::state::CapabilityEpoch), ServerError> {
        if before_close_barrier {
            let scope = self.closing_event_scope.ok_or(ServerError::EventScope)?;
            if !matches!(
                self.state.admission(),
                AdmissionState::Closing | AdmissionState::Unknown
            ) {
                return Err(ServerError::EventScope);
            }
            return Ok((scope.connection, scope.authority.epoch()));
        }
        let (connection, authority) = self.current_capture()?;
        if self.state.admission() != AdmissionState::Open {
            return Err(ServerError::EventScope);
        }
        Ok((connection, authority.epoch()))
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

fn adapter_rejection(event: BrokerEvent, error: &ServerError) -> AdapterEventRejection {
    match error {
        ServerError::EventScope
            if matches!(
                event,
                BrokerEvent::Keyboard(_)
                    | BrokerEvent::RegisteredObservation { .. }
                    | BrokerEvent::AudioInputDevicesChanged
            ) =>
        {
            AdapterEventRejection::AdmissionClosed
        }
        ServerError::EventScope => AdapterEventRejection::StaleScope,
        ServerError::EventCapacity => AdapterEventRejection::Capacity,
        ServerError::EventDelivery
        | ServerError::TerminalRoute
        | ServerError::Session(_)
        | ServerError::Transport(_)
        | ServerError::TransportBackpressure => AdapterEventRejection::DeliveryFailed,
        ServerError::StartupSnapshot
        | ServerError::ConnectionCapacity
        | ServerError::DuplicateConnection
        | ServerError::UnknownConnection
        | ServerError::TransportAuthenticationBoundary
        | ServerError::CapabilityProtocol
        | ServerError::CloseRetryExhausted
        | ServerError::ExecutorContract
        | ServerError::State(_) => AdapterEventRejection::InvalidTransition,
    }
}

fn require_native_success(summary: DriveSummary) -> Result<(), DispatchError> {
    if summary.native_failure.is_some() {
        Err(DispatchError::Semantic(ErrorCode::NativeFailure))
    } else {
        Ok(())
    }
}

fn maintenance_request(
    params: &talking_quill_owner_protocol::schema::MaintenanceAcquireParams,
) -> Option<MaintenanceRequest> {
    let transaction = MaintenanceTransactionId::new(*params.transaction_id().as_bytes())?;
    let source = BuildDigest::new(*params.source_build_digest().as_bytes())?;
    let target = params
        .target_build_digest()
        .and_then(|value| BuildDigest::new(*value.as_bytes()));
    let target_owner = params
        .target_owner_sha256()
        .and_then(|value| BuildDigest::new(*value.as_bytes()));
    let owner_handoff = Bytes32::random()
        .ok()
        .and_then(|value| MaintenanceHandoff::new(*value.as_bytes()))?;
    MaintenanceRequest::new(
        transaction,
        maintenance_operation(params.operation()),
        source,
        target,
        target_owner,
        owner_handoff,
    )
}

const fn maintenance_operation(operation: WireMaintenanceOperation) -> MaintenanceOperation {
    match operation {
        WireMaintenanceOperation::Update => MaintenanceOperation::Update,
        WireMaintenanceOperation::Uninstall => MaintenanceOperation::Uninstall,
        WireMaintenanceOperation::Rollback => MaintenanceOperation::Rollback,
    }
}

const fn state_session_mode(mode: SessionMode) -> SessionCaptureMode {
    match mode {
        SessionMode::Off => SessionCaptureMode::Off,
        SessionMode::Recording => SessionCaptureMode::Recording,
        SessionMode::CancelOnly => SessionCaptureMode::CancelOnly,
    }
}

fn core_bindings(
    bindings: &talking_quill_owner_protocol::schema::Bindings,
) -> Result<ActivationBindings, ()> {
    let converted = bindings
        .as_slice()
        .iter()
        .map(|binding| {
            let profile = ProfileId::new(binding.profile_id().as_str()).map_err(|_| ())?;
            let (ctrl, alt, shift, meta) = binding.shortcut().modifiers().values();
            let keys = binding
                .shortcut()
                .keys()
                .iter()
                .map(|letter| ActivationKey::from_index(*letter as u8).ok_or(()))
                .collect::<Result<Vec<_>, _>>()?;
            let shortcut = Shortcut::new(
                ShortcutModifiers {
                    ctrl,
                    alt,
                    shift,
                    meta,
                },
                &keys,
            )
            .map_err(|_| ())?;
            Ok(ActivationBinding::new(profile, shortcut))
        })
        .collect::<Result<Vec<_>, ()>>()?;
    ActivationBindings::new(&converted).map_err(|_| ())
}

fn wire_activation_event(
    owner_instance: OwnerInstanceId,
    capture_epoch: u64,
    binding: ActivationBinding,
    target_token: Option<NativeTargetToken>,
    generation: u64,
    phase: EventPhase,
    held_ms: Option<u64>,
) -> Result<ActivationEvent, ServerError> {
    let profile_id = WireProfileId::new(binding.profile_id().as_str().to_owned())
        .map_err(|_| ServerError::EventScope)?;
    let shortcut = binding.shortcut();
    let modifiers = shortcut.modifiers();
    let keys = shortcut
        .keys()
        .iter()
        .copied()
        .map(wire_letter)
        .collect::<Vec<_>>();
    let shortcut = BindingShortcut::new(
        Modifiers::new(
            modifiers.ctrl,
            modifiers.alt,
            modifiers.shift,
            modifiers.meta,
        ),
        keys,
    )
    .map_err(|_| ServerError::EventScope)?;
    let target_token = target_token
        .map(|token| WireToken::new(token.as_str().to_owned()))
        .transpose()
        .map_err(|_| ServerError::EventScope)?;
    Ok(ActivationEvent {
        capture_lease_epoch: wire_u64(capture_epoch),
        owner_instance_id: Bytes32::new(*owner_instance.as_bytes()),
        profile_id,
        shortcut,
        activation_generation: wire_u64(generation),
        target_token,
        phase: wire_phase(phase),
        held_ms: held_ms.map(wire_u64),
    })
}

const fn wire_letter(key: ActivationKey) -> talking_quill_owner_protocol::schema::Letter {
    use talking_quill_owner_protocol::schema::Letter;
    match key {
        ActivationKey::A => Letter::A,
        ActivationKey::B => Letter::B,
        ActivationKey::C => Letter::C,
        ActivationKey::D => Letter::D,
        ActivationKey::E => Letter::E,
        ActivationKey::F => Letter::F,
        ActivationKey::G => Letter::G,
        ActivationKey::H => Letter::H,
        ActivationKey::I => Letter::I,
        ActivationKey::J => Letter::J,
        ActivationKey::K => Letter::K,
        ActivationKey::L => Letter::L,
        ActivationKey::M => Letter::M,
        ActivationKey::N => Letter::N,
        ActivationKey::O => Letter::O,
        ActivationKey::P => Letter::P,
        ActivationKey::Q => Letter::Q,
        ActivationKey::R => Letter::R,
        ActivationKey::S => Letter::S,
        ActivationKey::T => Letter::T,
        ActivationKey::U => Letter::U,
        ActivationKey::V => Letter::V,
        ActivationKey::W => Letter::W,
        ActivationKey::X => Letter::X,
        ActivationKey::Y => Letter::Y,
        ActivationKey::Z => Letter::Z,
    }
}

const fn wire_phase(phase: EventPhase) -> Phase {
    match phase {
        EventPhase::Down => Phase::Down,
        EventPhase::Up => Phase::Up,
    }
}

fn terminal_ownership(ownership: NativeOwnership) -> Option<TerminalOwnership> {
    let mut present = Vec::with_capacity(5);
    if ownership.candidate() != CandidateOwnership::None {
        present.push(TerminalOwnership::Candidate);
    }
    if ownership.activation_drain_keys() != 0 {
        present.push(TerminalOwnership::Activation);
    }
    if ownership.session_drain_keys() != 0 {
        present.push(TerminalOwnership::Session);
    }
    if ownership.replay_cleanup_edges() != 0 {
        present.push(TerminalOwnership::ReplayCleanup);
    }
    if ownership.paste() != PasteOwnership::None {
        present.push(TerminalOwnership::Paste);
    }
    match present.as_slice() {
        [] => None,
        [only] => Some(*only),
        _ => Some(TerminalOwnership::Multiple),
    }
}

fn wire_terminal_event(
    offer: PredecessorTerminalOffer,
) -> talking_quill_owner_protocol::schema::PredecessorTerminalEvent {
    use talking_quill_owner_protocol::schema::{
        LeaseDisposition as WireDisposition, OwnershipKind, PredecessorTerminalEvent as WireEvent,
        RevocationReason, TerminalUnavailableReason as WireUnavailable,
    };
    let lease_id = Bytes32::new(*offer.authority().id().as_bytes());
    let lease_epoch = wire_u64(offer.authority().epoch().get());
    let terminal_sequence = wire_u64(offer.terminal_sequence());
    match offer.event() {
        PredecessorTerminalEvent::LeaseRevoked(reason) => WireEvent::LeaseRevoked {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            reason: match reason {
                TerminalReason::Eof => RevocationReason::Eof,
                TerminalReason::Heartbeat => RevocationReason::Heartbeat,
                TerminalReason::Maintenance => RevocationReason::Maintenance,
                TerminalReason::Release => RevocationReason::Release,
                TerminalReason::Protocol => RevocationReason::Protocol,
                TerminalReason::Rollback => RevocationReason::Rollback,
            },
        },
        PredecessorTerminalEvent::LeaseDraining(ownership) => WireEvent::LeaseDraining {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            ownership: match ownership {
                TerminalOwnership::Candidate => OwnershipKind::Candidate,
                TerminalOwnership::Activation => OwnershipKind::Activation,
                TerminalOwnership::Session => OwnershipKind::Session,
                TerminalOwnership::ReplayCleanup => OwnershipKind::ReplayCleanup,
                TerminalOwnership::Paste => OwnershipKind::Paste,
                TerminalOwnership::Multiple => OwnershipKind::Multiple,
            },
        },
        PredecessorTerminalEvent::LeaseNeutral => WireEvent::LeaseNeutral {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            disposition: WireDisposition::Neutral,
        },
        PredecessorTerminalEvent::LeaseUnavailable(reason) => WireEvent::LeaseUnavailable {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            reason: match reason {
                TerminalUnavailableReason::NativeFault => WireUnavailable::NativeFault,
                TerminalUnavailableReason::OwnershipUnknown => WireUnavailable::OwnershipUnknown,
            },
        },
    }
}

const fn wire_reported_state(state: ReportedState) -> OwnerReportedState {
    match state {
        ReportedState::Starting => OwnerReportedState::Starting,
        ReportedState::IdleNeutral => OwnerReportedState::IdleNeutral,
        ReportedState::LeaseDisabled => OwnerReportedState::LeaseDisabled,
        ReportedState::LeaseEnabled => OwnerReportedState::LeaseEnabled,
        ReportedState::LeaseDraining => OwnerReportedState::LeaseDraining,
        ReportedState::OrphanCancelling => OwnerReportedState::OrphanCancelling,
        ReportedState::OrphanDraining => OwnerReportedState::OrphanDraining,
        ReportedState::MaintenanceDraining => OwnerReportedState::MaintenanceDraining,
        ReportedState::DegradedDraining => OwnerReportedState::DegradedDraining,
        ReportedState::MaintenanceReady => OwnerReportedState::MaintenanceReady,
        ReportedState::Stopping => OwnerReportedState::Stopping,
    }
}

const fn wire_process_state(state: ProcessState) -> WireProcessState {
    match state {
        ProcessState::Starting => WireProcessState::Starting,
        ProcessState::Healthy => WireProcessState::Healthy,
        ProcessState::RollbackLatched => WireProcessState::RollbackLatched,
        ProcessState::Degraded => WireProcessState::Degraded,
        ProcessState::StoppingNative => WireProcessState::StoppingNative,
        ProcessState::FlushingResponse => WireProcessState::FlushingResponse,
        ProcessState::Exiting => WireProcessState::Exiting,
    }
}

const fn wire_disposition(disposition: LeaseDisposition) -> WireLeaseDisposition {
    match disposition {
        LeaseDisposition::Neutral => WireLeaseDisposition::Neutral,
        LeaseDisposition::Draining => WireLeaseDisposition::Draining,
    }
}

fn wire_counter(value: u64) -> Counter {
    Counter::new(value.min(9_007_199_254_740_991)).expect("counter is JS-safe")
}

fn wire_u64(value: u64) -> U64String {
    U64String::try_from(value).expect("state identities are nonzero")
}

const fn error_code(kind: TransitionErrorKind) -> ErrorCode {
    match kind {
        TransitionErrorKind::Busy => ErrorCode::Busy,
        TransitionErrorKind::Draining => ErrorCode::Draining,
        TransitionErrorKind::RollbackLatched => ErrorCode::Rollback,
        TransitionErrorKind::Degraded
        | TransitionErrorKind::NativeReadinessRequired
        | TransitionErrorKind::Starting
        | TransitionErrorKind::StartupSnapshotRequired => ErrorCode::Unavailable,
        TransitionErrorKind::ProtocolFault | TransitionErrorKind::WrongController => {
            ErrorCode::SecurityFault
        }
        TransitionErrorKind::ActionIdExhausted
        | TransitionErrorKind::NativeConfirmationMismatch
        | TransitionErrorKind::InvalidOwnershipTransition
        | TransitionErrorKind::ResponseFlushMismatch => ErrorCode::NativeFailure,
        TransitionErrorKind::PasteUnavailable => ErrorCode::Unavailable,
        TransitionErrorKind::MaintenanceSealed
        | TransitionErrorKind::LeaseMustBeDisabled
        | TransitionErrorKind::AdmissionTransitionPending
        | TransitionErrorKind::ConfigurationRequired
        | TransitionErrorKind::SessionOffReconciliationRequired
        | TransitionErrorKind::InvalidConfigurationRevision
        | TransitionErrorKind::PasteScopeMismatch
        | TransitionErrorKind::MaintenanceTransactionMismatch
        | TransitionErrorKind::MaintenanceOperationMismatch
        | TransitionErrorKind::EpochExhausted
        | TransitionErrorKind::Stopping => ErrorCode::InvalidState,
    }
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("fake owner startup physical snapshot failed")]
    StartupSnapshot,
    #[error("fake owner connection capacity exceeded")]
    ConnectionCapacity,
    #[error("fake owner connection ID is duplicated")]
    DuplicateConnection,
    #[error("fake owner connection is unknown")]
    UnknownConnection,
    #[error("fake owner capability protocol fault")]
    CapabilityProtocol,
    #[error("fake owner close retry budget exhausted with work retained")]
    CloseRetryExhausted,
    #[error("fake owner executor returned an invalid completion")]
    ExecutorContract,
    #[error("fake owner event is not scoped to the enabled capture lease")]
    EventScope,
    #[error("fake owner admitted-event capacity exceeded")]
    EventCapacity,
    #[error("test authentication cannot cross a production transport boundary")]
    TransportAuthenticationBoundary,
    #[error("owner transport already has a bounded pending write")]
    TransportBackpressure,
    #[error("fake owner event delivery failed")]
    EventDelivery,
    #[error("fake owner predecessor terminal route is unavailable")]
    TerminalRoute,
    #[error(transparent)]
    State(#[from] TransitionError),
    #[error(transparent)]
    Session(#[from] SessionCodecError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}
