use super::{
    actor::{ActorCommand, ActorState, ActorValue, LeaseKnowledge, PublishedState},
    client::{OwnerClientError, OwnerConnector, OwnerMaintenanceClient},
    errors::map_client_error,
    reconcile::{fail_closed_actor, publish_owner},
};
#[cfg(feature = "windows-installed-acceptance")]
use super::{
    client::acceptance_observability_without_lease, observability::owner_observability_from_wire,
};
use crate::gateway::{PlatformError, PlatformShutdown, ShutdownOwnerDisposition};
#[cfg(feature = "windows-installed-acceptance")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Instant,
};
use talking_quill_owner_protocol::schema as wire;

#[cfg(feature = "windows-installed-acceptance")]
const ACCEPTANCE_LEASE_RENEWAL_PAUSE: Duration = Duration::from_millis(6_500);

pub(super) fn execute_actor_command(
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

#[cfg(feature = "windows-installed-acceptance")]
fn unix_timestamp_ms() -> Result<u64, PlatformError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| PlatformError::OwnerSecurityFault)?
        .as_millis();
    u64::try_from(millis).map_err(|_| PlatformError::OwnerSecurityFault)
}
