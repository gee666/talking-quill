//! Admission prerequisites and serialized dependent actions.
use super::*;

impl KeyboardOwnerState {
    pub(super) fn ensure_not_starting_or_stopping(&self) -> Result<(), TransitionError> {
        if matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Starting));
        }
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        Ok(())
    }

    pub(super) fn ensure_process_healthy(&self) -> Result<(), TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if matches!(self.process_health, ProcessHealth::Degraded) {
            return Err(TransitionError::new(TransitionErrorKind::Degraded));
        }
        Ok(())
    }

    pub(super) fn ensure_capture_command_allowed(
        &self,
        command: CaptureCommand,
    ) -> Result<(), TransitionError> {
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        if self.rollback_latched
            && !matches!(command, CaptureCommand::Disable | CaptureCommand::Release)
        {
            return Err(TransitionError::new(TransitionErrorKind::RollbackLatched));
        }
        if matches!(self.process_health, ProcessHealth::Degraded)
            && !matches!(command, CaptureCommand::Disable | CaptureCommand::Release)
        {
            return Err(TransitionError::new(TransitionErrorKind::Degraded));
        }
        if !matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(TransitionErrorKind::MaintenanceSealed));
        }
        Ok(())
    }

    pub(super) fn enable_prerequisites_hold(&self) -> bool {
        matches!(self.process_health, ProcessHealth::Healthy)
            && matches!(self.exit, ExitState::Running)
            && !self.rollback_latched
            && matches!(self.maintenance, MaintenanceState::None)
            && matches!(
                self.controller,
                ControllerInternal::CaptureLeaseDisabled { .. }
            )
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.native_state_unknown
            && self.predecessor.is_none()
            && self.applied_configuration.is_some()
            && self
                .requested_configuration
                .map(ConfigurationRequest::identity)
                == self.applied_configuration
            && self.applied_session_mode == Some(SessionCaptureMode::Off)
            && self.readiness.keyboard_eligible()
    }

    pub(super) fn ensure_enable_allowed(&self) -> Result<(), TransitionError> {
        if self.admission != AdmissionState::Closed {
            return Err(TransitionError::new(
                TransitionErrorKind::AdmissionTransitionPending,
            ));
        }
        if self.applied_configuration.is_none()
            || self
                .requested_configuration
                .map(ConfigurationRequest::identity)
                != self.applied_configuration
        {
            return Err(TransitionError::new(
                TransitionErrorKind::ConfigurationRequired,
            ));
        }
        if self.applied_session_mode != Some(SessionCaptureMode::Off) {
            return Err(TransitionError::new(
                TransitionErrorKind::SessionOffReconciliationRequired,
            ));
        }
        if !self.readiness.keyboard_eligible() {
            return Err(TransitionError::new(
                TransitionErrorKind::NativeReadinessRequired,
            ));
        }
        if !self.can_open_keyboard() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        Ok(())
    }

    pub(super) fn request_session_mode(
        &mut self,
        epoch: CapabilityEpoch,
        mode: SessionCaptureMode,
    ) -> Result<Transition, TransitionError> {
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::SessionMode(mode),
            ActionScope::Capture(epoch),
            &mut transition.actions,
        )?;
        self.applied_session_mode = None;
        Ok(transition)
    }

    pub(super) fn begin_paste(
        &mut self,
        authority: CapabilityRef,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if authorization.owner_instance != self.owner_instance
            || authorization.capture_epoch != authority.epoch
        {
            return Err(TransitionError::new(
                TransitionErrorKind::PasteScopeMismatch,
            ));
        }
        if !self.can_begin_paste() {
            return Err(TransitionError::new(TransitionErrorKind::PasteUnavailable));
        }
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::AdmitPaste(authorization),
            ActionScope::Capture(authority.epoch),
            &mut transition.actions,
        )?;
        self.paste_phase = PastePhase::Admitting {
            authorization,
            cancel_after_admit: false,
        };
        Ok(transition)
    }

    pub(super) fn request_close(&mut self, plan: ClosePlan) -> Result<Transition, TransitionError> {
        self.demote_capture_controller();
        if self.admission == AdmissionState::Closed {
            return self.after_admission_closed(plan);
        }
        match &mut self.close_plan {
            Some(current) => current.merge(plan),
            None => self.close_plan = Some(plan),
        }
        self.mark_pending_superseded(PendingActionKind::OpenAdmission);
        if self.has_pending_kind(PendingActionKind::CloseAdmission) {
            self.admission = AdmissionState::Closing;
            return Ok(Transition::empty());
        }
        if self.emergency_close_pending {
            self.admission = AdmissionState::Unknown;
            return Ok(Transition::empty());
        }
        let scope = self
            .current_capture_epoch()
            .map(ActionScope::Capture)
            .unwrap_or_else(|| self.dependent_scope());
        let mut transition = Transition::empty();
        if let Err(error) = self.issue_action(
            PendingActionKind::CloseAdmission,
            scope,
            &mut transition.actions,
        ) {
            self.degrade_unknown();
            self.admission = AdmissionState::Unknown;
            return Err(error);
        }
        self.admission = AdmissionState::Closing;
        Ok(transition)
    }

    pub(super) fn after_admission_closed(
        &mut self,
        plan: ClosePlan,
    ) -> Result<Transition, TransitionError> {
        if !self.native_state_unknown {
            self.dependent_plan.cancel_candidate |=
                self.ownership.candidate == CandidateOwnership::Active;
            self.dependent_plan.cancel_paste |= matches!(self.paste_phase, PastePhase::Waiting(_));
            if plan.release_response {
                self.dependent_plan.apply_configuration = None;
            } else if plan.apply_configuration.is_some() {
                self.dependent_plan.apply_configuration = plan.apply_configuration;
            }
            self.dependent_plan.persist_maintenance |= plan.persist_maintenance;
            self.dependent_plan.continue_drain |= !self.ownership.is_native_neutral();
        } else {
            if plan.release_response {
                self.dependent_plan.apply_configuration = None;
            }
            // Known retained aggregate obligations still need the native owner
            // loop after close uncertainty. Never resurrect work after an
            // indeterminate dependent cancellation poisoned ordering.
            if !self.dependent_work_poisoned {
                self.dependent_plan.continue_drain |= !self.ownership.is_native_neutral();
            }
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        if plan.release_response {
            transition.lease_disposition = Some(if self.native_quiescent_for_disposition() {
                LeaseDisposition::Neutral
            } else {
                LeaseDisposition::Draining
            });
        }
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub(super) fn issue_next_dependent(
        &mut self,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        if self.dependent_work_poisoned {
            self.dependent_plan = DependentPlan::default();
            return Ok(());
        }
        if self
            .pending_actions
            .iter()
            .flatten()
            .any(|pending| pending.kind.is_dependent())
            || matches!(self.paste_phase, PastePhase::Admitting { .. })
        {
            return Ok(());
        }
        if self.dependent_plan.cancel_candidate {
            self.dependent_plan.cancel_candidate = false;
            self.ownership.candidate = CandidateOwnership::Cancelling;
            return self.issue_action(
                PendingActionKind::CancelCandidate,
                self.dependent_scope(),
                actions,
            );
        }
        if self.dependent_plan.cancel_paste {
            self.dependent_plan.cancel_paste = false;
            let Some(authorization) = self.paste_phase.authorization() else {
                return Err(self.native_confirmation_fault());
            };
            self.paste_phase = PastePhase::Cancelling(authorization);
            self.ownership.paste = PasteOwnership::Cancelling;
            return self.issue_action(
                PendingActionKind::CancelPaste(authorization),
                self.dependent_scope(),
                actions,
            );
        }
        if let Some(request) = self.dependent_plan.apply_configuration.take() {
            return self.issue_action(
                PendingActionKind::Configuration(request),
                ActionScope::Capture(request.identity.capture_epoch),
                actions,
            );
        }
        if self.dependent_plan.continue_drain {
            self.dependent_plan.continue_drain = false;
            if !self.ownership.is_native_neutral() {
                actions.push(RequiredAction::ContinueNativeDrain);
            }
        }
        if self.dependent_plan.persist_maintenance {
            self.dependent_plan.persist_maintenance = false;
            let MaintenanceState::Sealing {
                request,
                requester,
                reserved_capability,
            } = self.maintenance
            else {
                return Err(self.native_confirmation_fault());
            };
            self.maintenance = MaintenanceState::Persisting {
                request,
                requester,
                reserved_capability,
            };
            if let Err(mut error) = self.issue_action(
                PendingActionKind::PersistMaintenance(request),
                ActionScope::Maintenance(reserved_capability.authority.epoch),
                actions,
            ) {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.degrade_unknown();
                let mut mandatory = std::mem::take(actions);
                mandatory.append(&mut error.actions);
                error.actions = mandatory;
                return Err(error);
            }
            return Ok(());
        }
        Ok(())
    }

    pub(super) fn dependent_scope(&self) -> ActionScope {
        self.predecessor
            .map(|predecessor| ActionScope::Capture(predecessor.route.authority.epoch))
            .or_else(|| self.current_capture_epoch().map(ActionScope::Capture))
            .unwrap_or(ActionScope::Process)
    }
}
