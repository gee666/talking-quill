use super::{
    OwnerGatewayBackend, ProductionOwnerConnector, SystemOwnerWorkerSpawner,
    actor::{ActorCommand, ActorValue, CommandEnvelope},
    client::CaptureRevocation,
    conversions::{decode_opaque32, map_paste_error, permissions_from_wire, wire_bindings},
    maintenance_acquire_params, maintenance_digest,
    observability::owner_observability_from_wire,
};
use crate::{
    gateway::{
        ActivationCaptureGate, CallbackGate, ClipboardTextHash, FrontApp, GatewayBackend,
        HookStatus, KeyboardOwnerSnapshot, PasteFailure, PasteResult, Permissions, PlatformError,
        PlatformShutdown, ShutdownOwnerDisposition, TerminalReason, TerminalSignal,
        TransactionObservabilitySnapshot, WindowBounds,
    },
    protocol::Outbound,
};
use crossbeam_channel::{Sender, bounded};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use talking_quill_keyboard_core::{ActivationBindings, ActivationContext, SessionCaptureMode};
use talking_quill_owner_protocol::{Bytes32, U64String, schema as wire};

const GATEWAY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(feature = "windows-installed-acceptance")]
const ACCEPTANCE_LEASE_RENEWAL_TIMEOUT: Duration = Duration::from_secs(10);

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
            Ok(ActorValue::Observability(value)) => self.native_observability(&value),
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
                    native: self.native_observability(&value),
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
