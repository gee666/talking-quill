//! Mapping between owner protocol v1 and Electron-facing gateway semantics.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
#[cfg(feature = "windows-installed-acceptance")]
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, bounded};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, ActivationKey,
    EventPhase, KeyboardEvent, NativeTargetToken, ProfileId, SessionCaptureMode, SessionKey,
    Shortcut, ShortcutModifiers,
};
use talking_quill_owner_protocol::schema as wire;
use talking_quill_owner_protocol::{Bytes32, GatewayMessage, U64String};

use crate::gateway::{
    ActivationCaptureGate, CallbackGate, ClipboardTextHash, FrontApp, GatewayBackend, HookStatus,
    KeyboardOwnerSnapshot, KeyboardOwnerState, PasteFailure, PasteResult, PermissionState,
    Permissions, PlatformError, PlatformShutdown, ShutdownOwnerDisposition, TerminalReason,
    TerminalSignal, TransactionObservabilitySnapshot, WindowBounds,
};
use crate::protocol::Outbound;

#[cfg(feature = "windows-installed-acceptance")]
use super::client::acceptance_observability_without_lease;
use super::client::{
    CaptureRevocation, ConnectError, OwnerCaptureClient, OwnerClientDiagnostic, OwnerClientError,
    OwnerConnector, OwnerEventDisposition, OwnerMaintenanceClient, OwnerProcessState,
    OwnerShutdownControl,
};

const RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(5);
const ACTOR_POLL_INTERVAL: Duration = Duration::from_millis(10);
const GATEWAY_COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const GATEWAY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_QUEUE_CAPACITY: usize = 32;
#[cfg(feature = "windows-installed-acceptance")]
const ACCEPTANCE_LEASE_RENEWAL_PAUSE: Duration = Duration::from_millis(6_500);
#[cfg(feature = "windows-installed-acceptance")]
const ACCEPTANCE_LEASE_RENEWAL_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseKnowledge {
    NeverAcquired,
    Held,
    Uncertain,
    Released,
}

#[derive(Debug, Default)]
struct GatewayEventCounters {
    received: AtomicU64,
    v10_accepted: AtomicU64,
}

#[derive(Clone)]
struct PublishedState {
    snapshot: KeyboardOwnerSnapshot,
    permissions: Permissions,
    last_error: PlatformError,
    shutdown_disposition: Option<ShutdownOwnerDisposition>,
}

#[derive(Clone)]
struct ReconcileBudget {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

struct ActorState {
    owner: Option<OwnerCaptureClient>,
    lease: LeaseKnowledge,
    desired_bindings: Option<wire::Bindings>,
    desired_enabled: bool,
    desired_session_mode: wire::SessionMode,
    runtime_rollback: bool,
    next_connect: Instant,
    reconnect_backoff: Duration,
    reconcile_budget: Option<ReconcileBudget>,
    ever_connected: bool,
    #[cfg(feature = "windows-installed-acceptance")]
    acceptance_safe_disabled: bool,
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

struct CommandEnvelope {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    command: ActorCommand,
    reply: Sender<Result<ActorValue, PlatformError>>,
}

enum ActorCommand {
    Establish,
    Service {
        now: Instant,
    },
    RefreshHealth,
    Configure {
        bindings: wire::Bindings,
        enabled: bool,
    },
    Session(wire::SessionMode),
    Paste(wire::PasteInjectParams),
    FrontApp,
    Permissions,
    Observability,
    #[cfg(feature = "windows-installed-acceptance")]
    AcceptanceEndpointObservability,
    #[cfg(feature = "windows-installed-acceptance")]
    AcceptancePauseLeaseRenewal,
    Maintenance {
        acquire: wire::MaintenanceAcquireParams,
        transaction_id: Bytes32,
        operation: wire::MaintenanceOperation,
    },
    Shutdown,
}

enum ActorValue {
    Unit,
    Paste(wire::PasteResult),
    FrontApp(wire::FrontAppMetadataResult),
    Permissions(wire::PermissionsResult),
    Observability(Box<wire::ObservabilityResult>),
    #[cfg(feature = "windows-installed-acceptance")]
    AcceptanceEndpointObservability(crate::gateway::AcceptanceEndpointObservability),
    #[cfg(feature = "windows-installed-acceptance")]
    AcceptancePauseLeaseRenewal(Box<crate::gateway::AcceptancePauseLeaseRenewalResult>),
    Maintenance(Bytes32),
    Shutdown(PlatformShutdown),
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

impl OwnerGatewayBackend {
    pub fn connect_with(
        connector: Box<dyn OwnerConnector>,
        outbound: Sender<Outbound>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        let admission_gate = Arc::new(CallbackGate::new());
        if capture_gate.is_open() {
            admission_gate.open();
        }
        Self::connect_with_spawner_and_gate(
            connector,
            outbound,
            capture_gate,
            admission_gate,
            None,
            &SystemOwnerWorkerSpawner,
        )
    }

    #[doc(hidden)]
    pub fn connect_with_spawner(
        connector: Box<dyn OwnerConnector>,
        outbound: Sender<Outbound>,
        capture_gate: ActivationCaptureGate,
        spawner: &dyn OwnerWorkerSpawner,
    ) -> Result<Self, PlatformError> {
        let admission_gate = Arc::new(CallbackGate::new());
        if capture_gate.is_open() {
            admission_gate.open();
        }
        Self::connect_with_spawner_and_gate(
            connector,
            outbound,
            capture_gate,
            admission_gate,
            None,
            spawner,
        )
    }

    #[doc(hidden)]
    pub fn connect_with_terminal(
        connector: Box<dyn OwnerConnector>,
        outbound: Sender<Outbound>,
        capture_gate: ActivationCaptureGate,
        admission_gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        Self::connect_with_spawner_and_gate(
            connector,
            outbound,
            capture_gate,
            admission_gate,
            Some(terminal),
            &SystemOwnerWorkerSpawner,
        )
    }

    #[doc(hidden)]
    pub fn connect_with_admission_gate(
        connector: Box<dyn OwnerConnector>,
        outbound: Sender<Outbound>,
        capture_gate: ActivationCaptureGate,
        admission_gate: Arc<CallbackGate>,
    ) -> Result<Self, PlatformError> {
        Self::connect_with_spawner_and_gate(
            connector,
            outbound,
            capture_gate,
            admission_gate,
            None,
            &SystemOwnerWorkerSpawner,
        )
    }

    fn connect_with_spawner_and_gate(
        connector: Box<dyn OwnerConnector>,
        outbound: Sender<Outbound>,
        capture_gate: ActivationCaptureGate,
        admission_gate: Arc<CallbackGate>,
        terminal: Option<Arc<TerminalSignal>>,
        spawner: &dyn OwnerWorkerSpawner,
    ) -> Result<Self, PlatformError> {
        let shutdown_control = connector.shutdown_control();
        let published = Arc::new(Mutex::new(PublishedState {
            snapshot: KeyboardOwnerSnapshot::unavailable(),
            permissions: Permissions::unknown(),
            last_error: PlatformError::OwnerUnavailable,
            shutdown_disposition: None,
        }));
        let event_counters = Arc::new(GatewayEventCounters::default());
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let (commands_tx, commands_rx) = bounded(COMMAND_QUEUE_CAPACITY);
        let (done_tx, actor_done) = bounded(1);
        let (started_tx, started_rx) = bounded(1);
        let actor_published = Arc::clone(&published);
        let actor_gate = Arc::clone(&admission_gate);
        let actor_events = Arc::clone(&event_counters);
        let actor_terminal = terminal.clone();
        let actor_shutdown = Arc::clone(&shutdown_requested);
        let actor = spawner
            .spawn(Box::new(move || {
                let state = ActorState {
                    owner: None,
                    lease: LeaseKnowledge::NeverAcquired,
                    desired_bindings: None,
                    desired_enabled: false,
                    desired_session_mode: wire::SessionMode::Off,
                    runtime_rollback: capture_gate.runtime_rollback_active(),
                    next_connect: Instant::now(),
                    reconnect_backoff: RECONNECT_INITIAL_BACKOFF,
                    reconcile_budget: None,
                    ever_connected: false,
                    #[cfg(feature = "windows-installed-acceptance")]
                    acceptance_safe_disabled: false,
                };
                let _ = started_tx.try_send(());
                owner_actor_loop(
                    state,
                    connector,
                    commands_rx,
                    outbound,
                    actor_gate,
                    actor_events,
                    actor_published,
                    actor_terminal,
                    actor_shutdown,
                );
                let _ = done_tx.try_send(());
            }))
            .map_err(|_| PlatformError::OwnerUnavailable)?;
        if started_rx
            .recv_timeout(super::client::OWNER_CALL_TIMEOUT)
            .is_err()
        {
            shutdown_requested.store(true, Ordering::Release);
            return Err(PlatformError::OwnerUnavailable);
        }
        let backend = Self {
            commands: commands_tx,
            published,
            admission_gate,
            shutdown_control,
            shutdown_requested,
            event_counters,
            actor_done,
            actor: Some(actor),
        };
        let _ = backend.call_actor_until(
            ActorCommand::Establish,
            Instant::now() + super::client::OWNER_CALL_TIMEOUT,
        );
        Ok(backend)
    }

    fn call_actor(&self, command: ActorCommand) -> Result<ActorValue, PlatformError> {
        self.call_actor_until(command, Instant::now() + GATEWAY_COMMAND_TIMEOUT)
    }

    fn call_actor_until(
        &self,
        command: ActorCommand,
        deadline: Instant,
    ) -> Result<ActorValue, PlatformError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply_tx, reply_rx) = bounded(1);
        let envelope = CommandEnvelope {
            deadline,
            cancelled: Arc::clone(&cancelled),
            command,
            reply: reply_tx,
        };
        if self
            .commands
            .send_timeout(envelope, deadline.saturating_duration_since(Instant::now()))
            .is_err()
        {
            cancelled.store(true, Ordering::Release);
            let _ = self.shutdown_control.revoke_capture_and_cancel_io();
            return Err(PlatformError::OwnerUnavailable);
        }
        match reply_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => {
                cancelled.store(true, Ordering::Release);
                let _ = self.shutdown_control.revoke_capture_and_cancel_io();
                Err(PlatformError::OwnerUnavailable)
            }
        }
    }

    #[doc(hidden)]
    pub fn configure_activation_until(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
        deadline: Instant,
    ) -> Result<(), PlatformError> {
        let bindings = wire_bindings(bindings)?;
        match self.call_actor_until(ActorCommand::Configure { bindings, enabled }, deadline)? {
            ActorValue::Unit => Ok(()),
            _ => Err(PlatformError::NativeFailure),
        }
    }

    fn published_snapshot(&self) -> PublishedState {
        self.published
            .lock()
            .map(|value| value.clone())
            .unwrap_or(PublishedState {
                snapshot: KeyboardOwnerSnapshot::unavailable(),
                permissions: Permissions::unknown(),
                last_error: PlatformError::OwnerUnavailable,
                shutdown_disposition: None,
            })
    }
}

#[allow(clippy::too_many_arguments)]
fn owner_actor_loop(
    mut state: ActorState,
    mut connector: Box<dyn OwnerConnector>,
    commands: Receiver<CommandEnvelope>,
    outbound: Sender<Outbound>,
    admission_gate: Arc<CallbackGate>,
    event_counters: Arc<GatewayEventCounters>,
    published: Arc<Mutex<PublishedState>>,
    terminal: Option<Arc<TerminalSignal>>,
    shutdown_requested: Arc<AtomicBool>,
) {
    while !shutdown_requested.load(Ordering::Acquire) {
        if let Ok(envelope) = commands.recv_timeout(ACTOR_POLL_INTERVAL) {
            if state.owner.is_none()
                && state.reconcile_budget.as_ref().is_some_and(|budget| {
                    budget.cancelled.load(Ordering::Acquire) || Instant::now() >= budget.deadline
                })
            {
                fail_closed_actor(&mut state, &published, PlatformError::OwnerUnavailable);
            }
            if envelope.cancelled.load(Ordering::Acquire) || Instant::now() >= envelope.deadline {
                fail_closed_actor(&mut state, &published, PlatformError::OwnerUnavailable);
                continue;
            }
            let shutdown = matches!(envelope.command, ActorCommand::Shutdown);
            state.reconcile_budget = Some(ReconcileBudget {
                deadline: envelope.deadline,
                cancelled: Arc::clone(&envelope.cancelled),
            });
            #[cfg(feature = "windows-installed-acceptance")]
            let acceptance_reconnect_blocked = state.acceptance_safe_disabled
                && state.owner.is_none()
                && command_requires_capture(&envelope.command);
            #[cfg(not(feature = "windows-installed-acceptance"))]
            let acceptance_reconnect_blocked = false;
            let result = if acceptance_reconnect_blocked {
                Err(PlatformError::OwnerUnavailable)
            } else if state.owner.is_none() && command_requires_capture(&envelope.command) {
                connect_and_reconcile_until(
                    &mut state,
                    connector.as_mut(),
                    &outbound,
                    &admission_gate,
                    &event_counters,
                    &published,
                    terminal.as_ref(),
                    envelope.deadline,
                    &envelope.cancelled,
                )
                .and_then(|_| {
                    execute_actor_command(
                        &mut state,
                        connector.as_mut(),
                        envelope.command,
                        envelope.deadline,
                        &envelope.cancelled,
                        &published,
                    )
                })
            } else {
                execute_actor_command(
                    &mut state,
                    connector.as_mut(),
                    envelope.command,
                    envelope.deadline,
                    &envelope.cancelled,
                    &published,
                )
            };
            if envelope.cancelled.load(Ordering::Acquire) || Instant::now() >= envelope.deadline {
                fail_closed_actor(&mut state, &published, PlatformError::OwnerUnavailable);
            } else {
                let _ = envelope.reply.try_send(result);
            }
            if shutdown {
                return;
            }
        }
        if state.owner.is_some() {
            let envelope = internal_service_envelope();
            let ActorCommand::Service { now } = envelope.command else {
                unreachable!();
            };
            let result = {
                let owner = state.owner.as_mut().expect("owner checked");
                match owner.service_until(now, envelope.deadline, &envelope.cancelled) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let renewal_failed = matches!(
                            error,
                            OwnerClientError::Uncertain | OwnerClientError::Cancelled
                        ) || owner
                            .last_failure()
                            .is_some_and(|failure| failure.operation == "lease.renew");
                        Err((error, renewal_failed))
                    }
                }
            };
            match result {
                Ok(()) => {
                    if let Some(owner) = state.owner.as_ref() {
                        publish_owner(owner, &published);
                    }
                }
                Err((error, renewal_failed)) => {
                    report_actor_error(&state, connector.as_ref(), &error, terminal.as_ref());
                    let mapped = map_client_error(&error);
                    if renewal_failed {
                        fail_closed_actor(&mut state, &published, mapped);
                    } else {
                        disconnect_actor(&mut state, &published, mapped);
                        state.next_connect = Instant::now() + state.reconnect_backoff;
                    }
                }
            }
        } else if Instant::now() >= state.next_connect
            && !{
                #[cfg(feature = "windows-installed-acceptance")]
                {
                    state.acceptance_safe_disabled
                }
                #[cfg(not(feature = "windows-installed-acceptance"))]
                {
                    false
                }
            }
            && let Some(budget) = state.reconcile_budget.clone()
        {
            if budget.cancelled.load(Ordering::Acquire) || Instant::now() >= budget.deadline {
                fail_closed_actor(&mut state, &published, PlatformError::OwnerUnavailable);
            } else {
                let _ = connect_and_reconcile_until(
                    &mut state,
                    connector.as_mut(),
                    &outbound,
                    &admission_gate,
                    &event_counters,
                    &published,
                    terminal.as_ref(),
                    budget.deadline,
                    &budget.cancelled,
                );
            }
        }
    }
}

fn internal_service_envelope() -> CommandEnvelope {
    let scheduled_at = Instant::now();
    let (reply, _receiver) = bounded(1);
    CommandEnvelope {
        deadline: scheduled_at + super::client::OWNER_CALL_TIMEOUT,
        cancelled: Arc::new(AtomicBool::new(false)),
        command: ActorCommand::Service { now: scheduled_at },
        reply,
    }
}

const fn command_requires_capture(command: &ActorCommand) -> bool {
    #[cfg(feature = "windows-installed-acceptance")]
    if matches!(
        command,
        ActorCommand::AcceptanceEndpointObservability | ActorCommand::AcceptancePauseLeaseRenewal
    ) {
        return false;
    }
    !matches!(
        command,
        ActorCommand::Configure { enabled: false, .. }
            | ActorCommand::Session(wire::SessionMode::Off)
            | ActorCommand::Maintenance { .. }
            | ActorCommand::Shutdown
    )
}

fn execute_actor_command(
    state: &mut ActorState,
    connector: &mut dyn OwnerConnector,
    command: ActorCommand,
    deadline: Instant,
    cancelled: &Arc<AtomicBool>,
    published: &Arc<Mutex<PublishedState>>,
) -> Result<ActorValue, PlatformError> {
    if let ActorCommand::Shutdown = command {
        let result = if state.lease == LeaseKnowledge::NeverAcquired {
            PlatformShutdown::quiescent(None)
        } else if let Some(owner) = state.owner.as_mut() {
            match owner.release_and_exit_when_neutral_until(deadline, cancelled) {
                Ok(disposition) => {
                    let mapped = match disposition {
                        wire::LeaseDisposition::Neutral => ShutdownOwnerDisposition::Neutral,
                        wire::LeaseDisposition::Draining => ShutdownOwnerDisposition::Draining,
                    };
                    if let Ok(mut value) = published.lock() {
                        value.shutdown_disposition = Some(mapped);
                    }
                    state.lease = LeaseKnowledge::Released;
                    state.owner = None;
                    PlatformShutdown::quiescent(None)
                }
                Err(_) => PlatformShutdown {
                    terminal_reason: None,
                    observability_quiescent: false,
                },
            }
        } else {
            PlatformShutdown {
                terminal_reason: None,
                observability_quiescent: false,
            }
        };
        return Ok(ActorValue::Shutdown(result));
    }
    if matches!(command, ActorCommand::Establish) {
        return Ok(ActorValue::Unit);
    }
    #[cfg(feature = "windows-installed-acceptance")]
    if matches!(command, ActorCommand::AcceptanceEndpointObservability) {
        if state.owner.is_none() {
            return Err(PlatformError::OwnerUnavailable);
        }
        return connector
            .acceptance_endpoint_observability()
            .map(ActorValue::AcceptanceEndpointObservability)
            .ok_or(PlatformError::OwnerUnavailable);
    }
    #[cfg(feature = "windows-installed-acceptance")]
    if matches!(command, ActorCommand::AcceptancePauseLeaseRenewal) {
        state.acceptance_safe_disabled = true;
        state.desired_bindings = None;
        state.desired_enabled = false;
        state.desired_session_mode = wire::SessionMode::Off;
        let result = (|| {
            if deadline.saturating_duration_since(Instant::now())
                < ACCEPTANCE_LEASE_RENEWAL_PAUSE + Duration::from_secs(1)
            {
                return Err(PlatformError::OwnerUnavailable);
            }
            let endpoint_before = connector
                .acceptance_endpoint_observability()
                .ok_or(PlatformError::OwnerUnavailable)?;
            let (before_timestamp_ms, before) = {
                let owner = state
                    .owner
                    .as_mut()
                    .ok_or(PlatformError::OwnerUnavailable)?;
                owner
                    .force_capture_safe_disabled_until(deadline, cancelled)
                    .map_err(|error| map_client_error(&error))?;
                let timestamp = unix_timestamp_ms()?;
                let observability = owner
                    .observability_until(deadline, cancelled)
                    .map_err(|error| map_client_error(&error))?;
                (timestamp, observability)
            };
            std::thread::sleep(ACCEPTANCE_LEASE_RENEWAL_PAUSE);
            state.owner.take();
            state.lease = LeaseKnowledge::Uncertain;
            let observer = connector
                .connect_existing_capture()
                .map_err(|_| PlatformError::OwnerUnavailable)?;
            let endpoint_after = connector
                .acceptance_endpoint_observability()
                .ok_or(PlatformError::OwnerUnavailable)?;
            if endpoint_after.owner.process_id != endpoint_before.owner.process_id
                || endpoint_after.owner.creation_marker != endpoint_before.owner.creation_marker
                || endpoint_after.gateway.process_id != endpoint_before.gateway.process_id
                || endpoint_after.gateway.creation_marker != endpoint_before.gateway.creation_marker
            {
                return Err(PlatformError::OwnerSecurityFault);
            }
            let after = acceptance_observability_without_lease(observer, deadline, cancelled)
                .map_err(|error| map_client_error(&error))?;
            let after_timestamp_ms = unix_timestamp_ms()?;
            let before_owner = owner_observability_from_wire(&before.owner);
            let after_owner = owner_observability_from_wire(&after.owner);
            if before_owner.lease_expired.checked_add(1) != Some(after_owner.lease_expired)
                || after_owner.lease_renewed != before_owner.lease_renewed
                || after_timestamp_ms.saturating_sub(before_timestamp_ms) < 6_500
            {
                return Err(PlatformError::OwnerSecurityFault);
            }
            Ok(crate::gateway::AcceptancePauseLeaseRenewalResult {
                pause_duration_ms: 6_500,
                before_timestamp_ms,
                after_timestamp_ms,
                before: before_owner,
                after: after_owner,
            })
        })();
        fail_closed_actor(state, published, PlatformError::OwnerSecurityFault);
        return result
            .map(Box::new)
            .map(ActorValue::AcceptancePauseLeaseRenewal);
    }
    if let ActorCommand::Configure {
        bindings,
        enabled: false,
    } = &command
        && state.owner.is_none()
    {
        state.desired_bindings = Some(bindings.clone());
        state.desired_enabled = false;
        return Ok(ActorValue::Unit);
    }
    if matches!(command, ActorCommand::Session(wire::SessionMode::Off)) && state.owner.is_none() {
        state.desired_session_mode = wire::SessionMode::Off;
        return Ok(ActorValue::Unit);
    }
    if let ActorCommand::Maintenance {
        acquire,
        transaction_id,
        operation,
    } = &command
    {
        let result =
            OwnerMaintenanceClient::acquire_until(connector, acquire.clone(), deadline, cancelled)
                .and_then(|mut maintenance| {
                    maintenance.prepare_until(*transaction_id, *operation, deadline, cancelled)
                })
                .map(ActorValue::Maintenance)
                .map_err(|error| map_client_error(&error));
        if let Err(error) = result {
            fail_closed_actor(state, published, error);
        }
        return result;
    }
    let owner = state.owner.as_mut().ok_or_else(|| {
        published
            .lock()
            .map(|v| v.last_error)
            .unwrap_or(PlatformError::OwnerUnavailable)
    })?;
    let result = match command {
        ActorCommand::RefreshHealth => owner
            .refresh_health_until(deadline, cancelled)
            .map(|_| ActorValue::Unit),
        ActorCommand::Configure { bindings, enabled } => {
            let result = owner
                .configure_until(bindings.clone(), enabled, deadline, cancelled)
                .and_then(|_| owner.refresh_health_until(deadline, cancelled).map(|_| ()));
            if result.is_ok() {
                state.desired_bindings = Some(bindings);
                state.desired_enabled = enabled;
            }
            result.map(|_| ActorValue::Unit)
        }
        ActorCommand::Session(mode) => {
            let result = owner.set_session_mode_until(mode, deadline, cancelled);
            if result.is_ok() {
                state.desired_session_mode = mode;
            }
            result.map(|_| ActorValue::Unit)
        }
        ActorCommand::Paste(params) => owner
            .paste_until(params, deadline, cancelled)
            .map(ActorValue::Paste),
        ActorCommand::FrontApp => {
            if !owner.supports_front_app_metadata() {
                return Err(PlatformError::OwnerIncompatible);
            }
            owner
                .front_app_metadata_until(deadline, cancelled)
                .map(ActorValue::FrontApp)
        }
        ActorCommand::Permissions => owner
            .refresh_permissions_until(deadline, cancelled)
            .cloned()
            .map(ActorValue::Permissions),
        ActorCommand::Observability => owner
            .observability_until(deadline, cancelled)
            .map(Box::new)
            .map(ActorValue::Observability),
        #[cfg(feature = "windows-installed-acceptance")]
        ActorCommand::AcceptanceEndpointObservability
        | ActorCommand::AcceptancePauseLeaseRenewal => unreachable!(),
        ActorCommand::Establish
        | ActorCommand::Service { .. }
        | ActorCommand::Maintenance { .. }
        | ActorCommand::Shutdown => unreachable!(),
    };
    match result {
        Ok(value) => {
            publish_owner(owner, published);
            Ok(value)
        }
        Err(error) => {
            let mapped = map_client_error(&error);
            if matches!(
                error,
                OwnerClientError::Rejected(wire::ErrorCode::Busy | wire::ErrorCode::Unavailable)
            ) && owner.refresh_health_until(deadline, cancelled).is_ok()
            {
                publish_owner(owner, published);
                Err(mapped)
            } else {
                fail_closed_actor(state, published, mapped);
                Err(mapped)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn connect_and_reconcile_until(
    state: &mut ActorState,
    connector: &mut dyn OwnerConnector,
    outbound: &Sender<Outbound>,
    admission_gate: &Arc<CallbackGate>,
    event_counters: &Arc<GatewayEventCounters>,
    published: &Arc<Mutex<PublishedState>>,
    terminal: Option<&Arc<TerminalSignal>>,
    deadline: Instant,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), PlatformError> {
    let outbound = outbound.clone();
    let gate = Arc::clone(admission_gate);
    let counters = Arc::clone(event_counters);
    match OwnerCaptureClient::connect_until(
        connector,
        move |message| map_event(message, &outbound, &gate, &counters),
        deadline,
        cancelled,
    ) {
        Ok(mut owner) => {
            let reconciled = if state.runtime_rollback {
                owner
                    .runtime_rollback_until(deadline, cancelled)
                    .and_then(|_| owner.refresh_health_until(deadline, cancelled).map(|_| ()))
            } else if let Some(bindings) = state.desired_bindings.clone() {
                owner
                    .configure_until(bindings, state.desired_enabled, deadline, cancelled)
                    .and_then(|_| {
                        owner.set_session_mode_until(
                            state.desired_session_mode,
                            deadline,
                            cancelled,
                        )
                    })
                    .and_then(|_| owner.refresh_health_until(deadline, cancelled).map(|_| ()))
            } else {
                Ok(())
            };
            match reconciled {
                Ok(()) => {
                    if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                        fail_closed_actor(state, published, PlatformError::OwnerUnavailable);
                        return Err(PlatformError::OwnerUnavailable);
                    }
                    state.lease = LeaseKnowledge::Held;
                    state.ever_connected = true;
                    state.reconnect_backoff = RECONNECT_INITIAL_BACKOFF;
                    publish_owner(&owner, published);
                    state.owner = Some(owner);
                    Ok(())
                }
                Err(error) => {
                    report_actor_error(state, connector, &error, terminal);
                    let mapped = map_client_error(&error);
                    fail_closed_actor(state, published, mapped);
                    Err(mapped)
                }
            }
        }
        Err(error) => {
            if !matches!(
                error,
                OwnerClientError::Connect(_) | OwnerClientError::AcquireRejected(_)
            ) {
                state.lease = LeaseKnowledge::Uncertain;
            }
            let mapped = map_client_error(&error);
            if matches!(
                error,
                OwnerClientError::Uncertain | OwnerClientError::Cancelled
            ) {
                fail_closed_actor(state, published, mapped);
            } else if let Ok(mut value) = published.lock() {
                value.last_error = mapped;
            }
            state.next_connect = Instant::now() + state.reconnect_backoff;
            state.reconnect_backoff = (state.reconnect_backoff * 2).min(RECONNECT_MAX_BACKOFF);
            Err(mapped)
        }
    }
}

fn report_actor_error(
    state: &ActorState,
    connector: &dyn OwnerConnector,
    error: &OwnerClientError,
    terminal: Option<&Arc<TerminalSignal>>,
) {
    let diagnostic = state
        .owner
        .as_ref()
        .and_then(OwnerCaptureClient::last_failure)
        .unwrap_or(OwnerClientDiagnostic {
            category: client_error_category(error),
            operation: "gateway.actor",
            correlation_status: "unknown",
            transport_status: "unknown",
        });
    let _ = terminal;
    let _ = crate::report_owner_connection_diagnostic(
        diagnostic,
        "not_attempted",
        connector.owner_process_state(),
    );
}

fn publish_owner(owner: &OwnerCaptureClient, published: &Arc<Mutex<PublishedState>>) {
    if let Ok(mut value) = published.lock() {
        value.snapshot = snapshot_from_owner(owner);
        value.permissions = permissions_from_wire(owner.permissions());
        value.last_error = PlatformError::OwnerUnavailable;
    }
}

fn disconnect_actor(
    state: &mut ActorState,
    published: &Arc<Mutex<PublishedState>>,
    error: PlatformError,
) {
    if state.owner.take().is_some() {
        state.lease = LeaseKnowledge::Uncertain;
    }
    if let Ok(mut value) = published.lock() {
        value.last_error = error;
        value.snapshot = KeyboardOwnerSnapshot::unavailable();
        value.snapshot.state = match error {
            PlatformError::OwnerDraining => KeyboardOwnerState::Draining,
            PlatformError::OwnerRollback => KeyboardOwnerState::SafeDisabled,
            PlatformError::OwnerSecurityFault | PlatformError::OwnerAuthentication => {
                KeyboardOwnerState::Degraded
            }
            _ => KeyboardOwnerState::Unavailable,
        };
        value.permissions = Permissions::unknown();
    }
}

fn fail_closed_actor(
    state: &mut ActorState,
    published: &Arc<Mutex<PublishedState>>,
    error: PlatformError,
) {
    state.desired_bindings = None;
    state.desired_enabled = false;
    state.desired_session_mode = wire::SessionMode::Off;
    state.reconcile_budget = None;
    disconnect_actor(state, published, error);
}

fn client_error_category(error: &OwnerClientError) -> &'static str {
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

fn map_client_error(error: &OwnerClientError) -> PlatformError {
    match error {
        OwnerClientError::Connect(ConnectError::Authentication) => {
            PlatformError::OwnerAuthentication
        }
        OwnerClientError::Connect(ConnectError::Incompatible) => PlatformError::OwnerIncompatible,
        OwnerClientError::Connect(ConnectError::Busy) => PlatformError::OwnerBusy,
        OwnerClientError::Connect(ConnectError::SingletonCollision) => {
            PlatformError::OwnerSingletonCollision
        }
        OwnerClientError::Connect(_) => PlatformError::OwnerUnavailable,
        OwnerClientError::AcquireRejected(wire::ErrorCode::Busy)
        | OwnerClientError::Rejected(wire::ErrorCode::Busy) => PlatformError::OwnerBusy,
        OwnerClientError::AcquireRejected(
            wire::ErrorCode::Draining | wire::ErrorCode::InvalidState,
        )
        | OwnerClientError::Rejected(wire::ErrorCode::Draining | wire::ErrorCode::InvalidState) => {
            PlatformError::OwnerDraining
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Incompatible)
        | OwnerClientError::Rejected(wire::ErrorCode::Incompatible) => {
            PlatformError::OwnerIncompatible
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Rollback)
        | OwnerClientError::Rejected(wire::ErrorCode::Rollback) => PlatformError::OwnerRollback,
        OwnerClientError::AcquireRejected(wire::ErrorCode::SecurityFault)
        | OwnerClientError::Rejected(wire::ErrorCode::SecurityFault) => {
            PlatformError::OwnerSecurityFault
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Indeterminate)
        | OwnerClientError::Rejected(wire::ErrorCode::Indeterminate)
        | OwnerClientError::Uncertain => PlatformError::Indeterminate,
        OwnerClientError::AcquireRejected(wire::ErrorCode::NativeFailure)
        | OwnerClientError::Rejected(wire::ErrorCode::NativeFailure) => {
            PlatformError::NativeFailure
        }
        OwnerClientError::Cancelled => PlatformError::OwnerUnavailable,
        _ => PlatformError::OwnerUnavailable,
    }
}

impl GatewayBackend for OwnerGatewayBackend {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        Self::connect_with_spawner_and_gate(
            Box::new(ProductionOwnerConnector::default()),
            outbound,
            capture_gate,
            gate,
            Some(terminal),
            &SystemOwnerWorkerSpawner,
        )
    }

    fn hook_status(&self) -> HookStatus {
        let _ = self.call_actor(ActorCommand::RefreshHealth);
        let state = self.published_snapshot();
        if !state.snapshot.hook_healthy {
            HookStatus::Unavailable
        } else if self.event_counters.received.load(Ordering::Relaxed) > 0 {
            HookStatus::PhysicalObserved
        } else {
            HookStatus::InstalledUnobserved
        }
    }
    fn keyboard_owner(&self) -> KeyboardOwnerSnapshot {
        self.published_snapshot().snapshot
    }
    fn keyboard_capture_available(&self) -> bool {
        self.published_snapshot().snapshot.capture_available()
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let bindings = wire_bindings(bindings)?;
        match self.call_actor(ActorCommand::Configure { bindings, enabled })? {
            ActorValue::Unit => Ok(()),
            _ => Err(PlatformError::NativeFailure),
        }
    }
    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError> {
        let mode = match mode {
            SessionCaptureMode::Off => wire::SessionMode::Off,
            SessionCaptureMode::Recording => wire::SessionMode::Recording,
            SessionCaptureMode::CancelOnly => wire::SessionMode::CancelOnly,
        };
        match self.call_actor(ActorCommand::Session(mode))? {
            ActorValue::Unit => Ok(()),
            _ => Err(PlatformError::NativeFailure),
        }
    }
    fn inject_paste(&self) -> PasteResult {
        PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        }
    }
    fn inject_paste_for_activation_with_clipboard_hash(
        &self,
        context: ActivationContext,
        expected: ClipboardTextHash,
    ) -> PasteResult {
        let operation_id = match Bytes32::random() {
            Ok(value) => value,
            Err(_) => {
                return PasteResult {
                    submitted: false,
                    reason: Some(PasteFailure::Unavailable),
                };
            }
        };
        let snapshot = self.published_snapshot().snapshot;
        let owner_instance_id =
            decode_opaque32(&snapshot.instance_id).unwrap_or(Bytes32::new([1; 32]));
        let target_token = context
            .target_token()
            .map(|value| wire::WireToken::new(value.as_str().to_owned()))
            .transpose()
            .ok()
            .flatten();
        let params = wire::PasteInjectParams {
            capture_lease_id: Bytes32::new([1; 32]),
            capture_lease_epoch: U64String::try_from(1).unwrap(),
            command_sequence: U64String::try_from(1).unwrap(),
            operation_id,
            owner_instance_id,
            activation_generation: U64String::try_from(context.activation_generation().get())
                .unwrap(),
            target_token,
            fallback_text_sha256: Bytes32::new(*expected.as_bytes()),
        };
        match self.call_actor(ActorCommand::Paste(params)) {
            Ok(ActorValue::Paste(wire::PasteResult::Committed { .. })) => PasteResult {
                submitted: true,
                reason: None,
            },
            Ok(ActorValue::Paste(wire::PasteResult::Indeterminate { .. }))
            | Err(PlatformError::Indeterminate) => PasteResult {
                submitted: false,
                reason: Some(PasteFailure::Indeterminate),
            },
            Ok(ActorValue::Paste(wire::PasteResult::ClipboardOnly { reason })) => PasteResult {
                submitted: false,
                reason: Some(map_paste_error(reason)),
            },
            _ => PasteResult {
                submitted: false,
                reason: Some(PasteFailure::Unavailable),
            },
        }
    }
    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        let ActorValue::FrontApp(value) = self.call_actor(ActorCommand::FrontApp)? else {
            return Err(PlatformError::NativeFailure);
        };
        if !value.available {
            return Err(PlatformError::OwnerUnavailable);
        }
        Ok(FrontApp {
            process_name: value.process_name.ok_or(PlatformError::OwnerIncompatible)?,
            window_title: value.window_title.ok_or(PlatformError::OwnerIncompatible)?,
            window_bounds: value.window_bounds.map(|b| WindowBounds {
                x: b.x,
                y: b.y,
                width: b.width,
                height: b.height,
            }),
        })
    }
    fn permissions(&self) -> Permissions {
        match self.call_actor(ActorCommand::Permissions) {
            Ok(ActorValue::Permissions(value)) => permissions_from_wire(&value),
            _ => Permissions::unknown(),
        }
    }
    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        match self.call_actor(ActorCommand::Observability) {
            Ok(ActorValue::Observability(value)) => {
                let mut result = observability_from_wire(&value);
                result.registered_input.gateway_received =
                    self.event_counters.received.load(Ordering::Relaxed);
                result.registered_input.v10_notification_accepted =
                    self.event_counters.v10_accepted.load(Ordering::Relaxed);
                result
            }
            _ => TransactionObservabilitySnapshot::default(),
        }
    }
    fn owner_observability(&self) -> crate::gateway::OwnerObservabilitySnapshot {
        match self.call_actor(ActorCommand::Observability) {
            Ok(ActorValue::Observability(value)) => owner_observability_from_wire(&value.owner),
            _ => Default::default(),
        }
    }
    fn runtime_owner_observability(&self) -> crate::gateway::RuntimeOwnerObservabilitySnapshot {
        match self.call_actor(ActorCommand::Observability) {
            Ok(ActorValue::Observability(value)) => {
                crate::gateway::RuntimeOwnerObservabilitySnapshot {
                    native: observability_from_wire(&value),
                    owner: owner_observability_from_wire(&value.owner),
                }
            }
            _ => Default::default(),
        }
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_endpoint_observability(
        &self,
    ) -> Option<crate::gateway::AcceptanceEndpointObservability> {
        match self.call_actor(ActorCommand::AcceptanceEndpointObservability) {
            Ok(ActorValue::AcceptanceEndpointObservability(value)) => Some(value),
            _ => None,
        }
    }
    #[cfg(feature = "windows-installed-acceptance")]
    fn acceptance_pause_lease_renewal(
        &self,
    ) -> Result<crate::gateway::AcceptancePauseLeaseRenewalResult, PlatformError> {
        match self.call_actor_until(
            ActorCommand::AcceptancePauseLeaseRenewal,
            Instant::now() + ACCEPTANCE_LEASE_RENEWAL_TIMEOUT,
        )? {
            ActorValue::AcceptancePauseLeaseRenewal(value) => Ok(*value),
            _ => Err(PlatformError::NativeFailure),
        }
    }
    fn prepare_maintenance(
        &self,
        request: crate::gateway::MaintenanceRequest,
    ) -> Result<[u8; 32], PlatformError> {
        let acquire = maintenance_acquire_params(&request)?;
        let transaction_id = maintenance_digest(
            b"talking-quill/maintenance-transaction/v1\0",
            &request.transaction_id,
        )?;
        let operation = match request.operation {
            crate::gateway::MaintenanceOperation::Update => wire::MaintenanceOperation::Update,
            crate::gateway::MaintenanceOperation::Uninstall => {
                wire::MaintenanceOperation::Uninstall
            }
            crate::gateway::MaintenanceOperation::Rollback => wire::MaintenanceOperation::Rollback,
        };
        match self.call_actor(ActorCommand::Maintenance {
            acquire,
            transaction_id,
            operation,
        })? {
            ActorValue::Maintenance(value) => Ok(*value.as_bytes()),
            _ => Err(PlatformError::NativeFailure),
        }
    }
    fn shutdown(&mut self) -> PlatformShutdown {
        let deadline = Instant::now() + GATEWAY_SHUTDOWN_TIMEOUT;
        let cooperative_deadline = deadline
            .checked_sub(Duration::from_secs(1))
            .unwrap_or(deadline);
        self.admission_gate.close();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply_tx, reply_rx) = bounded(1);
        let envelope = CommandEnvelope {
            deadline,
            cancelled: Arc::clone(&cancelled),
            command: ActorCommand::Shutdown,
            reply: reply_tx,
        };
        let sent = self
            .commands
            .send_timeout(
                envelope,
                cooperative_deadline.saturating_duration_since(Instant::now()),
            )
            .is_ok();
        let cooperative = sent.then(|| {
            reply_rx.recv_timeout(cooperative_deadline.saturating_duration_since(Instant::now()))
        });
        let result = match cooperative {
            Some(Ok(Ok(ActorValue::Shutdown(value)))) => value,
            _ => {
                cancelled.store(true, Ordering::Release);
                self.shutdown_requested.store(true, Ordering::Release);
                let revocation = self.shutdown_control.revoke_capture_and_cancel_io();
                PlatformShutdown {
                    terminal_reason: (revocation != CaptureRevocation::Confirmed)
                        .then_some(TerminalReason::OwnerThreadUnresponsive),
                    observability_quiescent: false,
                }
            }
        };
        if self
            .actor_done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
        {
            if let Some(actor) = self.actor.take() {
                let _ = actor.join();
            }
        } else if result.terminal_reason.is_none() {
            return PlatformShutdown {
                terminal_reason: Some(TerminalReason::OwnerThreadUnresponsive),
                observability_quiescent: false,
            };
        }
        result
    }
    fn shutdown_owner_disposition(&self) -> ShutdownOwnerDisposition {
        self.published_snapshot()
            .shutdown_disposition
            .unwrap_or(ShutdownOwnerDisposition::Draining)
    }
}

impl Drop for OwnerGatewayBackend {
    fn drop(&mut self) {
        if self.actor.is_some() {
            let _ = self.shutdown();
        }
    }
}

fn decode_opaque32(value: &str) -> Option<Bytes32> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, output) in bytes.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(Bytes32::new(bytes))
}

fn snapshot_from_owner(owner: &OwnerCaptureClient) -> KeyboardOwnerSnapshot {
    let health = owner.health();
    KeyboardOwnerSnapshot {
        model: "out_of_process",
        protocol_version: 1,
        state: match health.reported_state {
            wire::OwnerReportedState::Starting => KeyboardOwnerState::SafeDisabled,
            wire::OwnerReportedState::IdleNeutral => KeyboardOwnerState::Idle,
            wire::OwnerReportedState::LeaseDisabled => KeyboardOwnerState::LeasedDisabled,
            wire::OwnerReportedState::LeaseEnabled => KeyboardOwnerState::LeasedEnabled,
            wire::OwnerReportedState::LeaseDraining
            | wire::OwnerReportedState::OrphanCancelling
            | wire::OwnerReportedState::OrphanDraining => KeyboardOwnerState::Draining,
            wire::OwnerReportedState::MaintenanceDraining
            | wire::OwnerReportedState::MaintenanceReady => KeyboardOwnerState::Maintenance,
            wire::OwnerReportedState::DegradedDraining => KeyboardOwnerState::Degraded,
            wire::OwnerReportedState::Stopping => KeyboardOwnerState::Unavailable,
        },
        instance_id: encode_opaque(health.owner_instance_id.as_bytes()),
        build_id: owner.build_id().to_owned(),
        lease_epoch: Some(owner.lease_epoch()),
        authenticated: true,
        keyboard_build_eligible: health.keyboard_build_eligible,
        permissions_eligible: health.permissions_eligible,
        hook_healthy: health.hook_healthy
            && health.process_state == wire::ProcessState::Healthy
            && matches!(
                health.reported_state,
                wire::OwnerReportedState::IdleNeutral
                    | wire::OwnerReportedState::LeaseDisabled
                    | wire::OwnerReportedState::LeaseEnabled
                    | wire::OwnerReportedState::LeaseDraining
            ),
        rollback_latched: health.rollback_latched,
    }
}

fn map_event(
    message: GatewayMessage,
    outbound: &Sender<Outbound>,
    admission_gate: &CallbackGate,
    counters: &GatewayEventCounters,
) -> OwnerEventDisposition {
    match message {
        GatewayMessage::Event(wire::Event::Activation(event)) => {
            saturating_increment(&counters.received);
            let Some(binding) = core_binding(&event) else {
                return OwnerEventDisposition::Terminal;
            };
            let Some(generation) = ActivationGeneration::new(event.activation_generation.get())
            else {
                return OwnerEventDisposition::Terminal;
            };
            let mut context = ActivationContext::target_unavailable(generation);
            if let Some(token) = event.target_token.as_ref() {
                let Ok(token) = NativeTargetToken::new(token.as_str()) else {
                    return OwnerEventDisposition::Terminal;
                };
                context = context.with_target_token(token);
            }
            let value = if let Some(held_ms) = event.held_ms {
                KeyboardEvent::ActivationComplete {
                    binding,
                    context,
                    held_ms: held_ms.get(),
                }
            } else {
                KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: map_phase(event.phase),
                }
            };
            if !publish_owner_event(admission_gate, outbound, Outbound::Event(value)) {
                return OwnerEventDisposition::Terminal;
            }
            saturating_increment(&counters.v10_accepted);
        }
        GatewayMessage::Event(wire::Event::RegisteredObservation(event)) => {
            saturating_increment(&counters.received);
            if !publish_owner_event(
                admission_gate,
                outbound,
                Outbound::RegisteredObservation(event.generation.get()),
            ) {
                return OwnerEventDisposition::Terminal;
            }
            saturating_increment(&counters.v10_accepted);
        }
        GatewayMessage::Event(wire::Event::SessionKey(event)) => {
            let key = match event.key {
                wire::SessionKey::Escape => SessionKey::Escape,
                wire::SessionKey::Enter => SessionKey::Enter,
            };
            if !publish_owner_event(
                admission_gate,
                outbound,
                Outbound::Event(KeyboardEvent::SessionKey {
                    key,
                    phase: map_phase(event.phase),
                }),
            ) {
                return OwnerEventDisposition::Terminal;
            }
        }
        GatewayMessage::Event(wire::Event::AudioDevicesChanged(_)) => {
            if !publish_owner_event(admission_gate, outbound, Outbound::InputDevicesChanged) {
                return OwnerEventDisposition::Terminal;
            }
        }
        GatewayMessage::Event(wire::Event::HealthChanged(_)) => {}
        GatewayMessage::Event(wire::Event::TerminalDegraded(_)) => {
            return OwnerEventDisposition::Terminal;
        }
        GatewayMessage::PredecessorTerminal(wire::PredecessorTerminalEvent::LeaseUnavailable {
            ..
        }) => return OwnerEventDisposition::Terminal,
        GatewayMessage::PredecessorTerminal(_) => {}
        GatewayMessage::Event(wire::Event::PasteCommitted(_)) | GatewayMessage::Response { .. } => {
            return OwnerEventDisposition::Terminal;
        }
    }
    OwnerEventDisposition::Continue
}

#[cfg(feature = "windows-installed-acceptance")]
fn unix_timestamp_ms() -> Result<u64, PlatformError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PlatformError::OwnerSecurityFault)?
        .as_millis();
    u64::try_from(millis).map_err(|_| PlatformError::OwnerSecurityFault)
}

fn owner_observability_from_wire(
    value: &wire::OwnerCounters,
) -> crate::gateway::OwnerObservabilitySnapshot {
    crate::gateway::OwnerObservabilitySnapshot {
        starts: value.starts.get(),
        clean_exits: value.clean_exits.get(),
        abnormal_exits: value.abnormal_exits.get(),
        singleton_collisions: value.singleton_collisions.get(),
        auth_attempts: value.auth_attempts.get(),
        auth_failures: crate::gateway::OwnerAuthFailureCounters {
            cross_user: value.auth_failures.cross_user.get(),
            wrong_session: value.auth_failures.wrong_session.get(),
            code_identity: value.auth_failures.code_identity.get(),
            mac: value.auth_failures.mac.get(),
            protocol: value.auth_failures.protocol.get(),
        },
        lease_acquired: value.lease_acquired.get(),
        lease_renewed: value.lease_renewed.get(),
        lease_expired: value.lease_expired.get(),
        lease_disconnected: value.lease_disconnected.get(),
        lease_released_neutral: value.lease_released_neutral.get(),
        lease_released_draining: value.lease_released_draining.get(),
        drain_duration_ms_total: value.drain_duration_ms_total.get(),
        drain_duration_ms_max: value.drain_duration_ms_max.get(),
        maintenance_postponed: value.maintenance_postponed.get(),
        handoff_succeeded: value.handoff_succeeded.get(),
        handoff_failed: value.handoff_failed.get(),
        degraded: value.degraded.get(),
        hook_recoveries: value.hook_recoveries.get(),
    }
}

fn observability_from_wire(value: &wire::ObservabilityResult) -> TransactionObservabilitySnapshot {
    let registered = value.registered_input.as_ref();
    TransactionObservabilitySnapshot {
        registered_input: crate::gateway::RegisteredInputCounters {
            hook_installed: registered.map_or(0, |value| value.hook_installed.get()),
            pump_alive: registered.map_or(0, |value| value.pump_alive.get()),
            hc_action_callbacks: registered.map_or(0, |value| value.hc_action_callbacks.get()),
            physical_callbacks: registered.map_or(0, |value| value.physical_callbacks.get()),
            physical_callbacks_filtered: registered
                .map_or(0, |value| value.physical_callbacks_filtered.get()),
            registered_candidate_callbacks: registered
                .map_or(0, |value| value.registered_candidate_callbacks.get()),
            registered_match_callbacks: registered
                .map_or(0, |value| value.registered_match_callbacks.get()),
            registered_release_callbacks: registered
                .map_or(0, |value| value.registered_release_callbacks.get()),
            callback_channel_accepted: registered
                .map_or(0, |value| value.callback_channel_accepted.get()),
            callback_channel_rejected: registered
                .map_or(0, |value| value.callback_channel_rejected.get()),
            adapter_dequeued: registered.map_or(0, |value| value.adapter_dequeued.get()),
            owner_admitted: registered.map_or(0, |value| value.owner_admitted.get()),
            owner_flushed: registered.map_or(0, |value| value.owner_flushed.get()),
            owner_rejected: registered.map_or(0, |value| value.owner_rejected.get()),
            gateway_received: 0,
            v10_notification_accepted: 0,
            electron_received: 0,
            observation_accepted: 0,
        },
        transactions: crate::gateway::TransactionCounters {
            started: value.transactions.started.get(),
            committed: value.transactions.committed.get(),
            replayed: value.transactions.replayed.get(),
            cancelled: value.transactions.cancelled.get(),
            journal_high_water: value.transactions.journal_high_water.get(),
            cancellation_reasons: crate::gateway::CancellationReasonCounters {
                invalid_continuation: value
                    .transactions
                    .cancellation_reasons
                    .invalid_continuation
                    .get(),
                modifier_changed: value
                    .transactions
                    .cancellation_reasons
                    .modifier_changed
                    .get(),
                alt_gr: value.transactions.cancellation_reasons.alt_gr.get(),
                journal_overflow: value
                    .transactions
                    .cancellation_reasons
                    .journal_overflow
                    .get(),
                configuration_replaced: value
                    .transactions
                    .cancellation_reasons
                    .configuration_replaced
                    .get(),
                revision_mismatch: value
                    .transactions
                    .cancellation_reasons
                    .revision_mismatch
                    .get(),
                gate_closed: value.transactions.cancellation_reasons.gate_closed.get(),
                shutdown: value.transactions.cancellation_reasons.shutdown.get(),
                helper_disconnected: value
                    .transactions
                    .cancellation_reasons
                    .helper_disconnected
                    .get(),
                secure_desktop: value.transactions.cancellation_reasons.secure_desktop.get(),
                timeout: value.transactions.cancellation_reasons.timeout.get(),
                activation_delivery_failed: value
                    .transactions
                    .cancellation_reasons
                    .activation_delivery_failed
                    .get(),
                neutralization_failed: value
                    .transactions
                    .cancellation_reasons
                    .neutralization_failed
                    .get(),
                replay_failed: value.transactions.cancellation_reasons.replay_failed.get(),
                effect_protocol_violation: value
                    .transactions
                    .cancellation_reasons
                    .effect_protocol_violation
                    .get(),
                physical_state_mismatch: value
                    .transactions
                    .cancellation_reasons
                    .physical_state_mismatch
                    .get(),
                target_changed: value.transactions.cancellation_reasons.target_changed.get(),
            },
        },
        replay: crate::gateway::EffectOutcomeCounters {
            attempted: value.replay.attempted.get(),
            succeeded: value.replay.succeeded.get(),
            partial: value.replay.partial.get(),
            failed: value.replay.failed.get(),
        },
        dummy: crate::gateway::EffectOutcomeCounters {
            attempted: value.dummy.attempted.get(),
            succeeded: value.dummy.succeeded.get(),
            partial: value.dummy.partial.get(),
            failed: value.dummy.failed.get(),
        },
        native_paste: crate::gateway::NativePasteCounters {
            target_validation_fallbacks: value.native_paste.target_validation_fallbacks.get(),
            modifier_wait_duration_ms_total: value
                .native_paste
                .modifier_wait_duration_ms_total
                .get(),
            modifier_wait_duration_ms_max: value.native_paste.modifier_wait_duration_ms_max.get(),
            modifier_timeouts: value.native_paste.modifier_timeouts.get(),
            shutdown_ownership_deadlines: value.native_paste.shutdown_ownership_deadlines.get(),
        },
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

fn wire_bindings(bindings: ActivationBindings) -> Result<wire::Bindings, PlatformError> {
    let values = bindings
        .iter()
        .map(|binding| {
            let shortcut = binding.shortcut();
            let modifiers = shortcut.modifiers();
            let keys = shortcut
                .keys()
                .iter()
                .map(|key| wire_letter(*key))
                .collect();
            Ok(wire::Binding::new(
                wire::ProfileId::new(binding.profile_id().as_str().to_owned())
                    .map_err(|_| PlatformError::NativeFailure)?,
                wire::BindingShortcut::new(
                    wire::Modifiers::new(
                        modifiers.ctrl,
                        modifiers.alt,
                        modifiers.shift,
                        modifiers.meta,
                    ),
                    keys,
                )
                .map_err(|_| PlatformError::NativeFailure)?,
            ))
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    wire::Bindings::new(values).map_err(|_| PlatformError::NativeFailure)
}

fn core_binding(event: &wire::ActivationEvent) -> Option<ActivationBinding> {
    let (ctrl, alt, shift, meta) = event.shortcut.modifiers().values();
    let keys = event
        .shortcut
        .keys()
        .iter()
        .map(|key| ActivationKey::from_index(*key as u8))
        .collect::<Option<Vec<_>>>()?;
    Some(ActivationBinding::new(
        ProfileId::new(event.profile_id.as_str()).ok()?,
        Shortcut::new(
            ShortcutModifiers {
                ctrl,
                alt,
                shift,
                meta,
            },
            &keys,
        )
        .ok()?,
    ))
}

fn wire_letter(key: ActivationKey) -> wire::Letter {
    const LETTERS: [wire::Letter; 26] = [
        wire::Letter::A,
        wire::Letter::B,
        wire::Letter::C,
        wire::Letter::D,
        wire::Letter::E,
        wire::Letter::F,
        wire::Letter::G,
        wire::Letter::H,
        wire::Letter::I,
        wire::Letter::J,
        wire::Letter::K,
        wire::Letter::L,
        wire::Letter::M,
        wire::Letter::N,
        wire::Letter::O,
        wire::Letter::P,
        wire::Letter::Q,
        wire::Letter::R,
        wire::Letter::S,
        wire::Letter::T,
        wire::Letter::U,
        wire::Letter::V,
        wire::Letter::W,
        wire::Letter::X,
        wire::Letter::Y,
        wire::Letter::Z,
    ];
    LETTERS[usize::from(key.index())]
}
fn saturating_increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1).min(9_007_199_254_740_991))
    });
}

fn publish_owner_event(
    admission_gate: &CallbackGate,
    outbound: &Sender<Outbound>,
    event: Outbound,
) -> bool {
    let Some(_delivery) = admission_gate.try_acquire_delivery() else {
        return false;
    };
    outbound.try_send(event).is_ok()
}

fn map_phase(value: wire::Phase) -> EventPhase {
    match value {
        wire::Phase::Down => EventPhase::Down,
        wire::Phase::Up => EventPhase::Up,
    }
}
fn permissions_from_wire(value: &wire::PermissionsResult) -> Permissions {
    Permissions {
        accessibility: permission(value.accessibility),
        input_monitoring: permission(value.input_monitoring),
        event_post: permission(value.event_post),
    }
}
fn permission(value: wire::PermissionState) -> PermissionState {
    match value {
        wire::PermissionState::Granted => PermissionState::Granted,
        wire::PermissionState::Denied => PermissionState::Denied,
        wire::PermissionState::Unknown => PermissionState::Unknown,
        wire::PermissionState::NotRequired => PermissionState::NotApplicable,
    }
}
fn map_paste_error(value: wire::PasteRefusalReason) -> PasteFailure {
    match value {
        wire::PasteRefusalReason::PermissionDenied => PasteFailure::PermissionDenied,
        wire::PasteRefusalReason::ConflictingModifiers => PasteFailure::ConflictingModifiers,
        wire::PasteRefusalReason::SecureInput => PasteFailure::SecureInput,
        wire::PasteRefusalReason::NativeRejected => PasteFailure::OsRejected,
        wire::PasteRefusalReason::TargetUnavailable
        | wire::PasteRefusalReason::ClipboardChanged
        | wire::PasteRefusalReason::NativeUnavailable => PasteFailure::Unavailable,
    }
}
fn encode_opaque(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(all(test, target_os = "macos"))]
mod macos_maintenance_tests {
    use super::*;

    #[test]
    fn maintenance_hex_is_canonical_bytes_not_a_text_hash() {
        let encoded = (0_u8..32)
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        let decoded = maintenance_digest(b"ignored-domain", &encoded).unwrap();
        let expected: [u8; 32] = (0_u8..32).collect::<Vec<_>>().try_into().unwrap();
        assert_eq!(decoded.as_bytes(), &expected);
        assert!(maintenance_digest(b"ignored-domain", &encoded.to_uppercase()).is_err());
        assert!(maintenance_digest(b"ignored-domain", "abcd").is_err());
    }
}
