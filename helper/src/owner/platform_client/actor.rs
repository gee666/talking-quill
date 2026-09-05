use super::{
    GATEWAY_COMMAND_TIMEOUT,
    actor_commands::execute_actor_command,
    client::{self, OwnerCaptureClient, OwnerClientError, OwnerConnector},
    errors::map_client_error,
    events::GatewayEventCounters,
    reconcile::{
        connect_and_reconcile_until, disconnect_actor, fail_closed_actor, publish_owner,
        report_actor_error,
    },
};
use crate::{
    gateway::{
        CallbackGate, KeyboardOwnerSnapshot, Permissions, PlatformError, PlatformShutdown,
        ShutdownOwnerDisposition, TerminalSignal,
    },
    protocol::Outbound,
};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::{Bytes32, schema as wire};

const ACTOR_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LeaseKnowledge {
    NeverAcquired,
    Held,
    Uncertain,
    Released,
}

#[derive(Clone)]
pub(super) struct PublishedState {
    pub(super) snapshot: KeyboardOwnerSnapshot,
    pub(super) permissions: Permissions,
    pub(super) last_error: PlatformError,
    pub(super) shutdown_disposition: Option<ShutdownOwnerDisposition>,
}

#[derive(Clone)]
pub(super) struct ReconcileBudget {
    pub(super) deadline: Instant,
    pub(super) cancelled: Arc<AtomicBool>,
}

pub(super) struct ActorState {
    pub(super) owner: Option<OwnerCaptureClient>,
    pub(super) lease: LeaseKnowledge,
    pub(super) desired_bindings: Option<wire::Bindings>,
    pub(super) desired_enabled: bool,
    pub(super) desired_session_mode: wire::SessionMode,
    pub(super) runtime_rollback: bool,
    pub(super) next_connect: Instant,
    pub(super) reconnect_backoff: Duration,
    pub(super) reconcile_budget: Option<ReconcileBudget>,
    pub(super) ever_connected: bool,
    #[cfg(feature = "windows-installed-acceptance")]
    pub(super) acceptance_safe_disabled: bool,
}

pub(super) struct CommandEnvelope {
    pub(super) deadline: Instant,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) command: ActorCommand,
    pub(super) reply: Sender<Result<ActorValue, PlatformError>>,
}

pub(super) enum ActorCommand {
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

pub(super) enum ActorValue {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn owner_actor_loop(
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
        {
            if state.reconcile_budget.as_ref().is_some_and(|budget| {
                budget.cancelled.load(Ordering::Acquire) || Instant::now() >= budget.deadline
            }) {
                // An expired caller must never authorize capture on a later connection.
                // Reconnect disabled with a new deadline so a dead owner cannot strand
                // the gateway after the last foreground request has completed.
                fail_closed_actor(&mut state, &published, PlatformError::OwnerUnavailable);
            }
            let budget = state
                .reconcile_budget
                .clone()
                .unwrap_or_else(|| ReconcileBudget {
                    deadline: Instant::now() + GATEWAY_COMMAND_TIMEOUT,
                    cancelled: Arc::clone(&shutdown_requested),
                });
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

fn internal_service_envelope() -> CommandEnvelope {
    let scheduled_at = Instant::now();
    let (reply, _receiver) = bounded(1);
    CommandEnvelope {
        deadline: scheduled_at + client::OWNER_CALL_TIMEOUT,
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
