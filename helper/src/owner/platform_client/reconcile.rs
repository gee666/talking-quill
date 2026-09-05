use super::{
    actor::{ActorState, LeaseKnowledge, PublishedState},
    client::{OwnerCaptureClient, OwnerClientDiagnostic, OwnerClientError, OwnerConnector},
    conversions::{permissions_from_wire, snapshot_from_owner},
    errors::{client_error_category, map_client_error},
    events::{GatewayEventCounters, map_event},
};
use crate::{
    gateway::{
        CallbackGate, KeyboardOwnerSnapshot, KeyboardOwnerState, Permissions, PlatformError,
        TerminalSignal,
    },
    protocol::Outbound,
};
use crossbeam_channel::Sender;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::schema as wire;

pub(super) const RECONNECT_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(5);

#[allow(clippy::too_many_arguments)]
pub(super) fn connect_and_reconcile_until(
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

pub(super) fn report_actor_error(
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

pub(super) fn publish_owner(owner: &OwnerCaptureClient, published: &Arc<Mutex<PublishedState>>) {
    if let Ok(mut value) = published.lock() {
        value.snapshot = snapshot_from_owner(owner);
        value.permissions = permissions_from_wire(owner.permissions());
        value.last_error = PlatformError::OwnerUnavailable;
    }
}

pub(super) fn disconnect_actor(
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

pub(super) fn fail_closed_actor(
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
