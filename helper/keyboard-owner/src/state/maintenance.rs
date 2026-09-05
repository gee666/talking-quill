//! Maintenance authority and final response lifecycle.
use super::*;

impl KeyboardOwnerState {
    pub fn acquire_maintenance(
        &mut self,
        connection: ConnectionId,
        capability_id: CapabilityId,
        request: MaintenanceRequest,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if let Some(existing) = self.maintenance.request() {
            if existing != request {
                return Err(TransitionError::new(
                    TransitionErrorKind::MaintenanceTransactionMismatch,
                ));
            }
            if matches!(
                self.controller,
                ControllerInternal::MaintenanceExclusive { .. }
            ) {
                return Err(TransitionError::new(TransitionErrorKind::Busy));
            }
            if matches!(self.maintenance, MaintenanceState::SealedFailed { .. }) {
                return Err(TransitionError::new(TransitionErrorKind::Degraded));
            }
            if !matches!(self.maintenance, MaintenanceState::Sealed { .. })
                || !self.quiescent_for_neutral()
            {
                return Err(TransitionError::new(TransitionErrorKind::Draining));
            }
            let epoch = Self::next_epoch(&mut self.last_maintenance_epoch)?;
            self.controller = ControllerInternal::MaintenanceExclusive {
                connection,
                capability: CapabilityState {
                    authority: CapabilityRef {
                        id: capability_id,
                        epoch,
                    },
                    last_command_sequence: 0,
                },
            };
            self.maintenance = MaintenanceState::Exclusive { request };
            let mut transition = Transition::empty();
            transition.response_stage = Some(ResponseStage::MaintenanceAcquireReady);
            return Ok(transition);
        }

        let epoch = Self::next_epoch(&mut self.last_maintenance_epoch)?;
        let reserved_capability = CapabilityState {
            authority: CapabilityRef {
                id: capability_id,
                epoch,
            },
            last_command_sequence: 0,
        };
        let mut transition = self.snapshot_predecessor(TerminalReason::Maintenance);
        self.cancel_unclaimed_paste();
        self.maintenance = MaintenanceState::Sealing {
            request,
            requester: Some(connection),
            reserved_capability,
        };
        self.controller = ControllerInternal::NoController;
        self.invalidate_reconciliation();
        match self.request_close(ClosePlan {
            persist_maintenance: true,
            ..ClosePlan::default()
        }) {
            Ok(close) => {
                transition.merge(close);
                Ok(transition)
            }
            Err(mut error) => {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.degrade_unknown();
                if error.terminal_offer.is_none() {
                    error.terminal_offer = transition.terminal_offer.map(Box::new);
                }
                Err(error)
            }
        }
    }

    pub fn confirm_maintenance_persisted(
        &mut self,
        token: NativeActionToken,
        request: MaintenanceRequest,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::PersistMaintenance(request))?;
        let MaintenanceState::Persisting {
            request: current,
            requester,
            reserved_capability,
        } = self.maintenance
        else {
            return Err(self.native_confirmation_fault());
        };
        if current != request || self.admission != AdmissionState::Closed {
            return Err(self.native_confirmation_fault());
        }
        let mut transition = Transition::empty();
        if let Some(connection) = requester {
            self.controller = ControllerInternal::MaintenanceExclusive {
                connection,
                capability: reserved_capability,
            };
            self.maintenance = MaintenanceState::Exclusive { request };
            transition.response_stage = Some(ResponseStage::MaintenanceAcquireReady);
        } else {
            self.controller = ControllerInternal::NoController;
            self.maintenance = MaintenanceState::Sealed { request };
        }
        Ok(transition)
    }

    pub fn apply_maintenance_command(
        &mut self,
        connection: ConnectionId,
        authority: CapabilityRef,
        sequence: CommandSequence,
        command: MaintenanceCommand,
    ) -> Result<Transition, TransitionError> {
        if !matches!(self.exit, ExitState::Running)
            || matches!(self.process_health, ProcessHealth::Degraded)
        {
            return Err(TransitionError::new(
                if matches!(self.exit, ExitState::Running) {
                    TransitionErrorKind::Degraded
                } else {
                    TransitionErrorKind::Stopping
                },
            ));
        }
        let mut capability = match self.controller {
            ControllerInternal::MaintenanceExclusive {
                connection: active,
                capability,
            } if active == connection => capability,
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
        self.controller = ControllerInternal::MaintenanceExclusive {
            connection,
            capability,
        };
        match command {
            MaintenanceCommand::Renew | MaintenanceCommand::ConsumeSemanticRejection => {
                Ok(Transition::empty())
            }
            MaintenanceCommand::Prepare {
                operation,
                response_correlation,
            } => {
                let Some(request) = self.maintenance.request() else {
                    return Err(TransitionError::new(
                        TransitionErrorKind::MaintenanceTransactionMismatch,
                    ));
                };
                if request.operation != operation {
                    return Err(TransitionError::new(
                        TransitionErrorKind::MaintenanceOperationMismatch,
                    ));
                }
                if !self.quiescent_for_neutral() {
                    return Err(TransitionError::new(TransitionErrorKind::Draining));
                }
                self.begin_native_stop(ExitPurpose::MaintenancePrepare(response_correlation))
            }
        }
    }

    pub fn confirm_native_stopped(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::StopNative)?;
        let ExitState::NativeStopPending(purpose) = self.exit else {
            return Err(self.native_confirmation_fault());
        };
        let mut transition = Transition::empty();
        match purpose {
            ExitPurpose::MaintenancePrepare(correlation) => {
                self.exit = ExitState::FinalResponseReady(correlation);
                transition.response_stage = Some(ResponseStage::FinalResponseReady);
            }
            ExitPurpose::Idle | ExitPurpose::Degraded | ExitPurpose::MaintenanceGuardLost => {
                self.exit = ExitState::Exiting;
                transition.actions.push(RequiredAction::ExitOwner);
            }
            ExitPurpose::AbandonedMaintenancePrepare => {
                self.exit = ExitState::NativeStoppedSealed;
            }
        }
        Ok(transition)
    }

    pub fn begin_final_response_flush(
        &mut self,
        correlation: ResponseCorrelation,
    ) -> Result<Transition, TransitionError> {
        if self.exit != ExitState::FinalResponseReady(correlation) {
            return Err(TransitionError::new(
                TransitionErrorKind::ResponseFlushMismatch,
            ));
        }
        self.exit = ExitState::FlushingResponse(correlation);
        Ok(Transition::empty())
    }

    pub fn confirm_final_response_flushed(
        &mut self,
        correlation: ResponseCorrelation,
    ) -> Result<Transition, TransitionError> {
        if self.exit != ExitState::FlushingResponse(correlation) {
            return Err(TransitionError::new(
                TransitionErrorKind::ResponseFlushMismatch,
            ));
        }
        self.exit = ExitState::Exiting;
        let mut transition = Transition::empty();
        transition.actions.push(RequiredAction::ExitOwner);
        Ok(transition)
    }
}
