//! Rollback, authority retirement, and native stop sequencing.
use super::*;

impl KeyboardOwnerState {
    pub(super) fn apply_priority_rollback(
        &mut self,
        reason: Option<TerminalReason>,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        // Latch before all allocation and close work. Failure may degrade, but
        // it can never erase the rollback fact.
        self.rollback_latched = true;
        let mut transition = reason.map_or_else(Transition::empty, |reason| {
            self.snapshot_predecessor(reason)
        });
        self.controller = ControllerInternal::NoController;
        self.invalidate_reconciliation();
        self.dependent_plan.apply_configuration = None;
        self.mark_semantic_work_superseded();
        self.cancel_unclaimed_paste();
        match self.request_close(ClosePlan::default()) {
            Ok(close) => {
                transition.merge(close);
                transition.lease_disposition = Some(if self.native_quiescent_for_disposition() {
                    LeaseDisposition::Neutral
                } else {
                    LeaseDisposition::Draining
                });
                Ok(transition)
            }
            Err(mut error) => {
                // request_close already degraded/latched unknown. Preserve the
                // terminal route and priority fact in the error transition.
                if error.terminal_offer.is_none() {
                    error.terminal_offer = transition.terminal_offer.map(Box::new);
                }
                Err(error)
            }
        }
    }

    pub(super) fn cancel_unclaimed_paste(&mut self) {
        if self.native_state_unknown || self.dependent_work_poisoned {
            return;
        }
        match self.paste_phase {
            PastePhase::Admitting { authorization, .. } => {
                self.paste_phase = PastePhase::Admitting {
                    authorization,
                    cancel_after_admit: true,
                };
                self.mark_pending_superseded(PendingActionKind::AdmitPaste(authorization));
            }
            PastePhase::Waiting(authorization) => {
                self.paste_phase = PastePhase::Cancelling(authorization);
                self.ownership.paste = PasteOwnership::Cancelling;
                self.dependent_plan.cancel_paste = true;
            }
            PastePhase::None
            | PastePhase::Cancelling(_)
            | PastePhase::Claimed(_)
            | PastePhase::Indeterminate(_) => {}
        }
    }

    pub(super) fn lose_active_controller(
        &mut self,
        reason: TerminalReason,
    ) -> Result<Transition, TransitionError> {
        let was_capture = self.controller.capture().is_some();
        let mut transition = if was_capture {
            self.snapshot_predecessor(reason)
        } else {
            Transition::empty()
        };
        match self.exit {
            ExitState::NativeStopPending(ExitPurpose::MaintenancePrepare(_)) => {
                self.exit = ExitState::NativeStopPending(ExitPurpose::AbandonedMaintenancePrepare);
            }
            ExitState::FinalResponseReady(_) | ExitState::FlushingResponse(_) => {
                self.exit = ExitState::NativeStoppedSealed;
            }
            _ => {}
        }
        match self.maintenance {
            MaintenanceState::Sealing {
                request,
                reserved_capability,
                ..
            } => {
                self.maintenance = MaintenanceState::Sealing {
                    request,
                    requester: None,
                    reserved_capability,
                };
            }
            MaintenanceState::Persisting {
                request,
                reserved_capability,
                ..
            } => {
                self.maintenance = MaintenanceState::Persisting {
                    request,
                    requester: None,
                    reserved_capability,
                };
            }
            MaintenanceState::Exclusive { request } => {
                self.maintenance = MaintenanceState::Sealed { request };
            }
            _ => {}
        }
        self.controller = ControllerInternal::NoController;
        if was_capture {
            self.invalidate_reconciliation();
            self.cancel_unclaimed_paste();
            match self.request_close(ClosePlan::default()) {
                Ok(close) => transition.merge(close),
                Err(mut error) => {
                    if error.terminal_offer.is_none() {
                        error.terminal_offer = transition.terminal_offer.map(Box::new);
                    }
                    return Err(error);
                }
            }
        }
        Ok(transition)
    }

    pub(super) fn command_for_wrong_controller(
        &mut self,
        connection: ConnectionId,
    ) -> TransitionError {
        if self.controller.connection() == Some(connection) {
            match self.lose_active_controller(TerminalReason::Protocol) {
                Ok(transition) => {
                    TransitionError::with_transition(TransitionErrorKind::ProtocolFault, transition)
                }
                Err(error) => error,
            }
        } else {
            TransitionError::new(TransitionErrorKind::WrongController)
        }
    }

    pub(super) fn snapshot_predecessor(&mut self, reason: TerminalReason) -> Transition {
        if self.predecessor.is_none()
            && let Some((connection, capability, _)) = self.controller.capture()
        {
            self.predecessor = Some(PredecessorState {
                route: PredecessorRoute {
                    owner_instance: self.owner_instance,
                    connection,
                    authority: capability.authority,
                },
                high_water: 0,
                revoked_offered: false,
                final_offered: false,
                in_flight: None,
            });
        }
        let mut transition = Transition::empty();
        transition.terminal_offer =
            self.offer_predecessor_terminal(PredecessorTerminalEvent::LeaseRevoked(reason));
        transition
    }

    pub(super) fn begin_native_stop(
        &mut self,
        purpose: ExitPurpose,
    ) -> Result<Transition, TransitionError> {
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::StopNative,
            ActionScope::Process,
            &mut transition.actions,
        )?;
        self.exit = ExitState::NativeStopPending(purpose);
        Ok(transition)
    }

    pub(super) fn maybe_begin_degraded_exit(
        &mut self,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        if matches!(self.process_health, ProcessHealth::Degraded)
            && matches!(self.maintenance, MaintenanceState::None)
            && matches!(self.exit, ExitState::Running)
            && self.controller.capture().is_none()
            && self.quiescent_for_neutral()
        {
            let mut transition = self.begin_native_stop(ExitPurpose::Degraded)?;
            actions.append(&mut transition.actions);
        }
        Ok(())
    }

    pub(super) fn current_capture_epoch(&self) -> Option<CapabilityEpoch> {
        self.controller
            .capture()
            .map(|(_, capability, _)| capability.authority.epoch)
    }

    pub(super) fn demote_capture_controller(&mut self) {
        if let ControllerInternal::CaptureLeaseEnabled {
            connection,
            capability,
        } = self.controller
        {
            self.controller = ControllerInternal::CaptureLeaseDisabled {
                connection,
                capability,
            };
        }
    }

    pub(super) fn invalidate_reconciliation(&mut self) {
        self.requested_configuration = None;
        self.applied_configuration = None;
        self.applied_session_mode = None;
    }

    pub(super) fn degrade_unknown(&mut self) {
        if !self.native_state_unknown {
            self.cancel_unclaimed_paste();
        }
        self.process_health = ProcessHealth::Degraded;
        self.native_state_unknown = true;
        self.terminal_unavailable_reason = Some(TerminalUnavailableReason::OwnershipUnknown);
        self.maintenance = match self.maintenance {
            MaintenanceState::Sealing { request, .. }
            | MaintenanceState::Persisting { request, .. } => {
                MaintenanceState::SealedFailed { request }
            }
            other => other,
        };
        self.demote_capture_controller();
        if self.admission != AdmissionState::Closed {
            self.admission = AdmissionState::Unknown;
        }
    }

    pub(super) fn mark_pending_superseded(&mut self, kind: PendingActionKind) {
        for pending in self.pending_actions.iter_mut().flatten() {
            if pending.kind == kind {
                pending.superseded = true;
            }
        }
    }

    pub(super) fn mark_semantic_work_superseded(&mut self) {
        for pending in self.pending_actions.iter_mut().flatten() {
            if matches!(
                pending.kind,
                PendingActionKind::OpenAdmission
                    | PendingActionKind::SessionMode(_)
                    | PendingActionKind::Configuration(_)
                    | PendingActionKind::AdmitPaste(_)
            ) {
                pending.superseded = true;
            }
        }
    }
}
