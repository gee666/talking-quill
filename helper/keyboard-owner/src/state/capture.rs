//! Startup, observer authentication, capture leases, and capture commands.
use super::*;

impl KeyboardOwnerState {
    pub fn confirm_startup_snapshot_seeded(&mut self) -> Result<Transition, TransitionError> {
        if !matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        self.startup_snapshot_seeded = true;
        Ok(Transition::empty())
    }

    pub fn startup_completed(&mut self) -> Result<Transition, TransitionError> {
        if !matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        if !self.startup_snapshot_seeded {
            return Err(TransitionError::new(
                TransitionErrorKind::StartupSnapshotRequired,
            ));
        }
        if !self.ownership.is_native_neutral() || self.admission != AdmissionState::Closed {
            return Err(TransitionError::new(
                TransitionErrorKind::InvalidOwnershipTransition,
            ));
        }
        self.process_health = ProcessHealth::Healthy;
        Ok(Transition::empty())
    }

    pub fn observe_native_readiness(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<Transition, TransitionError> {
        let keyboard_lost = (self.readiness.keyboard_build_eligible
            && !readiness.keyboard_build_eligible)
            || (self.readiness.permissions_eligible && !readiness.permissions_eligible)
            || (self.readiness.hook_healthy && !readiness.hook_healthy);
        let paste_lost = self.readiness.paste_ready && !readiness.paste_ready;
        self.readiness = readiness;
        if keyboard_lost {
            self.invalidate_reconciliation();
        }
        if paste_lost {
            self.cancel_unclaimed_paste();
        }
        if keyboard_lost && !matches!(self.admission, AdmissionState::Closed) {
            self.request_close(ClosePlan::default())
        } else {
            let mut transition = Transition::empty();
            self.issue_next_dependent(&mut transition.actions)?;
            Ok(transition)
        }
    }

    pub fn authenticate_observer(
        &mut self,
        connection: ConnectionId,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if !matches!(self.controller, ControllerInternal::NoController) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        self.controller = ControllerInternal::AuthenticatedObserver { connection };
        Ok(Transition::empty())
    }

    pub fn acquire_capture_lease(
        &mut self,
        connection: ConnectionId,
        capability_id: CapabilityId,
    ) -> Result<Transition, TransitionError> {
        self.ensure_process_healthy()?;
        if self.rollback_latched {
            return Err(TransitionError::new(TransitionErrorKind::RollbackLatched));
        }
        if !matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(TransitionErrorKind::MaintenanceSealed));
        }
        if !self.quiescent_for_neutral() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        match self.controller {
            ControllerInternal::AuthenticatedObserver {
                connection: observer,
            } if observer == connection => {}
            ControllerInternal::AuthenticatedObserver { .. }
            | ControllerInternal::CaptureLeaseDisabled { .. }
            | ControllerInternal::CaptureLeaseEnabled { .. }
            | ControllerInternal::MaintenanceExclusive { .. } => {
                return Err(TransitionError::new(TransitionErrorKind::Busy));
            }
            ControllerInternal::NoController => {
                return Err(TransitionError::new(TransitionErrorKind::WrongController));
            }
        }
        let epoch = Self::next_epoch(&mut self.last_capture_epoch)?;
        self.controller = ControllerInternal::CaptureLeaseDisabled {
            connection,
            capability: CapabilityState {
                authority: CapabilityRef {
                    id: capability_id,
                    epoch,
                },
                last_command_sequence: 0,
            },
        };
        self.configuration_high_water = None;
        self.invalidate_reconciliation();
        Ok(Transition::empty())
    }

    pub fn apply_capture_command(
        &mut self,
        connection: ConnectionId,
        authority: CapabilityRef,
        sequence: CommandSequence,
        command: CaptureCommand,
    ) -> Result<Transition, TransitionError> {
        let (enabled, mut capability) = match self.controller.capture() {
            Some((active, capability, enabled)) if active == connection => (enabled, capability),
            _ => return Err(self.command_for_wrong_controller(connection)),
        };
        if capability.authority != authority
            || capability
                .last_command_sequence
                .checked_add(1)
                .is_none_or(|expected| expected != sequence.get())
        {
            let transition = self.lose_active_controller(TerminalReason::Protocol)?;
            return Err(TransitionError::with_transition(
                TransitionErrorKind::ProtocolFault,
                transition,
            ));
        }
        capability.last_command_sequence = sequence.get();
        self.controller = if enabled {
            ControllerInternal::CaptureLeaseEnabled {
                connection,
                capability,
            }
        } else {
            ControllerInternal::CaptureLeaseDisabled {
                connection,
                capability,
            }
        };

        if matches!(command, CaptureCommand::RuntimeRollback) {
            return self.apply_priority_rollback(Some(TerminalReason::Rollback));
        }
        self.ensure_capture_command_allowed(command)?;
        if !matches!(
            command,
            CaptureCommand::Renew | CaptureCommand::Disable | CaptureCommand::Release
        ) && !self.pending_actions_empty()
        {
            return Err(TransitionError::new(
                TransitionErrorKind::AdmissionTransitionPending,
            ));
        }

        match command {
            CaptureCommand::Renew => Ok(Transition::empty()),
            CaptureCommand::ReconcileSessionOff => {
                self.request_session_mode(authority.epoch, SessionCaptureMode::Off)
            }
            CaptureCommand::SetSessionMode(mode) => {
                if mode != SessionCaptureMode::Off
                    && (!enabled || self.admission != AdmissionState::Open)
                {
                    return Err(TransitionError::new(
                        TransitionErrorKind::LeaseMustBeDisabled,
                    ));
                }
                self.request_session_mode(authority.epoch, mode)
            }
            CaptureCommand::ReplaceConfiguration { revision, bindings } => {
                if self
                    .configuration_high_water
                    .is_some_and(|current| revision <= current)
                {
                    return Err(TransitionError::new(
                        TransitionErrorKind::InvalidConfigurationRevision,
                    ));
                }
                let request = ConfigurationRequest {
                    identity: ConfigurationIdentity {
                        capture_epoch: authority.epoch,
                        revision,
                    },
                    bindings,
                };
                // Reserve before any allocation/native dispatch. Nothing may
                // lower this high-water within the capture epoch.
                self.configuration_high_water = Some(revision);
                self.requested_configuration = Some(request);
                self.applied_configuration = None;
                self.request_close(ClosePlan {
                    apply_configuration: Some(request),
                    ..ClosePlan::default()
                })
            }
            CaptureCommand::Enable => {
                self.ensure_enable_allowed()?;
                let mut transition = Transition::empty();
                self.issue_action(
                    PendingActionKind::OpenAdmission,
                    ActionScope::Capture(authority.epoch),
                    &mut transition.actions,
                )?;
                self.admission = AdmissionState::Opening;
                Ok(transition)
            }
            CaptureCommand::Disable => self.request_close(ClosePlan::default()),
            CaptureCommand::BeginPaste(authorization) => self.begin_paste(authority, authorization),
            CaptureCommand::Release => {
                let mut transition = self.snapshot_predecessor(TerminalReason::Release);
                self.cancel_unclaimed_paste();
                self.controller = ControllerInternal::NoController;
                self.invalidate_reconciliation();
                let close = self.request_close(ClosePlan {
                    release_response: true,
                    ..ClosePlan::default()
                });
                match close {
                    Ok(close) => {
                        transition.merge(close);
                        Ok(transition)
                    }
                    Err(mut error) => {
                        if error.terminal_offer.is_none() {
                            error.terminal_offer = transition.terminal_offer.map(Box::new);
                        }
                        Err(error)
                    }
                }
            }
            CaptureCommand::RuntimeRollback => unreachable!("priority rollback handled above"),
        }
    }
}
