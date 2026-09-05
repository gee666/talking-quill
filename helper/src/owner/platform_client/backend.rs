use super::{
    GATEWAY_COMMAND_TIMEOUT, OwnerGatewayBackend, OwnerWorkerSpawner, SystemOwnerWorkerSpawner,
    actor::{
        ActorCommand, ActorState, ActorValue, CommandEnvelope, LeaseKnowledge, PublishedState,
        owner_actor_loop,
    },
    client::{self, OwnerConnector},
    conversions::wire_bindings,
    events::GatewayEventCounters,
    reconcile::RECONNECT_INITIAL_BACKOFF,
};
use crate::{
    gateway::{
        ActivationCaptureGate, CallbackGate, KeyboardOwnerSnapshot, Permissions, PlatformError,
        TerminalSignal,
    },
    protocol::Outbound,
};
use crossbeam_channel::{Sender, bounded};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use talking_quill_keyboard_core::ActivationBindings;
use talking_quill_owner_protocol::schema as wire;

const COMMAND_QUEUE_CAPACITY: usize = 32;

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

    pub(super) fn connect_with_spawner_and_gate(
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
        if started_rx.recv_timeout(client::OWNER_CALL_TIMEOUT).is_err() {
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
            Instant::now() + client::OWNER_CALL_TIMEOUT,
        );
        Ok(backend)
    }

    pub(super) fn call_actor(&self, command: ActorCommand) -> Result<ActorValue, PlatformError> {
        self.call_actor_until(command, Instant::now() + GATEWAY_COMMAND_TIMEOUT)
    }

    pub(super) fn call_actor_until(
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

    pub(super) fn published_snapshot(&self) -> PublishedState {
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
