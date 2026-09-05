//! Confirm native actions and contain executor failures.
use super::*;

impl KeyboardOwnerState {
    pub fn confirm_session_mode_applied(
        &mut self,
        token: NativeActionToken,
        mode: SessionCaptureMode,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::SessionMode(mode))?;
        if !pending.superseded {
            self.applied_session_mode = Some(mode);
        }
        Ok(Transition::empty())
    }

    pub fn confirm_configuration_applied(
        &mut self,
        token: NativeActionToken,
        request: ConfigurationRequest,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::Configuration(request))?;
        if self.admission != AdmissionState::Closed {
            return Err(self.native_confirmation_fault());
        }
        if !pending.superseded && self.requested_configuration == Some(request) {
            self.applied_configuration = Some(request.identity);
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_admission_opened(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::OpenAdmission)?;
        if pending.superseded || self.admission == AdmissionState::Closing {
            return Ok(Transition::empty());
        }
        if self.admission != AdmissionState::Opening || !self.enable_prerequisites_hold() {
            return Err(self.native_confirmation_fault());
        }
        let ControllerInternal::CaptureLeaseDisabled {
            connection,
            capability,
        } = self.controller
        else {
            return Err(self.native_confirmation_fault());
        };
        self.admission = AdmissionState::Open;
        self.controller = ControllerInternal::CaptureLeaseEnabled {
            connection,
            capability,
        };
        Ok(Transition::empty())
    }

    pub fn confirm_emergency_admission_closed(&mut self) -> Result<Transition, TransitionError> {
        if !self.emergency_close_pending {
            return Err(self.native_confirmation_fault());
        }
        if !matches!(
            self.admission,
            AdmissionState::Closing | AdmissionState::Unknown
        ) || self.ownership.admitted_effects != 0
        {
            return Err(self.native_confirmation_fault());
        }
        self.emergency_close_pending = false;
        self.admission = AdmissionState::Closed;
        let plan = self.close_plan.take().unwrap_or_default();
        self.after_admission_closed(plan)
    }

    pub fn confirm_admission_closed(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CloseAdmission)?;
        if !matches!(
            self.admission,
            AdmissionState::Closing | AdmissionState::Unknown
        ) || self.ownership.admitted_effects != 0
        {
            return Err(self.native_confirmation_fault());
        }
        self.admission = AdmissionState::Closed;
        let plan = self.close_plan.take().unwrap_or_default();
        self.after_admission_closed(plan)
    }

    pub fn confirm_paste_waiting(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        let pending =
            self.take_pending_action(token, PendingActionKind::AdmitPaste(authorization))?;
        let PastePhase::Admitting {
            authorization: expected,
            cancel_after_admit,
        } = self.paste_phase
        else {
            return Err(self.native_confirmation_fault());
        };
        if expected != authorization {
            return Err(self.native_confirmation_fault());
        }
        self.ownership.paste = PasteOwnership::Waiting;
        if cancel_after_admit || pending.superseded {
            self.paste_phase = PastePhase::Cancelling(authorization);
            self.ownership.paste = PasteOwnership::Cancelling;
            self.dependent_plan.cancel_paste = true;
        } else {
            self.paste_phase = PastePhase::Waiting(authorization);
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_paste_refused(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::AdmitPaste(authorization))?;
        if self.paste_phase.authorization() != Some(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_paste_claimed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if self.paste_phase != PastePhase::Waiting(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::Claimed(authorization);
        self.ownership.paste = PasteOwnership::Claimed;
        Ok(Transition::empty())
    }

    pub fn confirm_paste_indeterminate(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if self.paste_phase != PastePhase::Claimed(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::Indeterminate(authorization);
        self.ownership.paste = PasteOwnership::Indeterminate;
        Ok(Transition::empty())
    }

    pub fn confirm_paste_completed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if !matches!(
            self.paste_phase,
            PastePhase::Claimed(current) | PastePhase::Indeterminate(current)
                if current == authorization
        ) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_candidate_cancelled(
        &mut self,
        token: NativeActionToken,
        resulting_ownership: NativeOwnership,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CancelCandidate)?;
        if resulting_ownership.candidate == CandidateOwnership::Active
            || resulting_ownership.paste != self.ownership.paste
            || !resulting_ownership.legal_after_keyboard_closed(self.ownership)
        {
            self.dependent_plan = DependentPlan::default();
            self.dependent_work_poisoned = true;
            return Err(self.native_confirmation_fault());
        }
        self.ownership = resulting_ownership;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_waiting_paste_cancelled(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CancelPaste(authorization))?;
        if self.paste_phase != PastePhase::Cancelling(authorization) {
            self.dependent_plan = DependentPlan::default();
            self.dependent_work_poisoned = true;
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn fail_native_action(
        &mut self,
        token: NativeActionToken,
        failure: NativeActionFailure,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_by_token(token)?;
        let mut transition = Transition::empty();
        match (pending.kind, failure) {
            (PendingActionKind::CloseAdmission, NativeActionFailure::FailedNotApplied) => {
                self.issue_action(
                    PendingActionKind::CloseAdmission,
                    token.scope,
                    &mut transition.actions,
                )?;
            }
            (PendingActionKind::CloseAdmission, NativeActionFailure::Indeterminate)
            | (PendingActionKind::OpenAdmission, NativeActionFailure::Indeterminate) => {
                self.degrade_unknown();
                self.admission = AdmissionState::Unknown;
                self.close_plan.get_or_insert_with(ClosePlan::default);
                if !self.has_pending_kind(PendingActionKind::CloseAdmission) {
                    self.issue_action(
                        PendingActionKind::CloseAdmission,
                        token.scope,
                        &mut transition.actions,
                    )?;
                }
            }
            (PendingActionKind::OpenAdmission, NativeActionFailure::FailedNotApplied) => {
                if self.admission == AdmissionState::Opening {
                    self.admission = AdmissionState::Closed;
                }
                if self.close_plan.is_some() {
                    transition.merge(self.request_close(ClosePlan::default())?);
                }
            }
            (PendingActionKind::SessionMode(_), _) => {
                self.applied_session_mode = None;
            }
            (PendingActionKind::Configuration(_), _) => {
                self.applied_configuration = None;
                self.issue_next_dependent(&mut transition.actions)?;
            }
            (
                PendingActionKind::AdmitPaste(authorization),
                NativeActionFailure::FailedNotApplied,
            ) => {
                if self.paste_phase.authorization() == Some(authorization) {
                    self.paste_phase = PastePhase::None;
                    self.ownership.paste = PasteOwnership::None;
                }
                self.issue_next_dependent(&mut transition.actions)?;
            }
            (PendingActionKind::AdmitPaste(authorization), NativeActionFailure::Indeterminate) => {
                self.paste_phase = PastePhase::Indeterminate(authorization);
                self.ownership.paste = PasteOwnership::Indeterminate;
                self.degrade_unknown();
                transition.merge(self.request_close(ClosePlan::default())?);
            }
            (PendingActionKind::CancelCandidate | PendingActionKind::CancelPaste(_), _) => {
                self.degrade_unknown();
                // No later dependent step may be dispatched after uncertain
                // cancellation. Retained physical facts remain in ownership.
                self.dependent_plan = DependentPlan::default();
                self.dependent_work_poisoned = true;
            }
            (PendingActionKind::PersistMaintenance(request), _) => {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.process_health = ProcessHealth::Degraded;
                if !self.native_state_unknown {
                    self.terminal_unavailable_reason = Some(TerminalUnavailableReason::NativeFault);
                }
                self.dependent_plan.persist_maintenance = false;
            }
            (PendingActionKind::StopNative, _) => {
                self.exit = ExitState::Running;
                self.process_health = ProcessHealth::Degraded;
                if !self.native_state_unknown {
                    self.terminal_unavailable_reason = Some(TerminalUnavailableReason::NativeFault);
                }
            }
        }
        Ok(transition)
    }

    /// Consumes the exact pending token, applies the conservative
    /// indeterminate failure semantics for that action, then latches a known
    /// adapter-contract fault so no later enablement is possible.
    pub fn fail_native_adapter_contract(
        &mut self,
        token: Option<NativeActionToken>,
    ) -> Result<Transition, TransitionError> {
        let mut transition = if let Some(token) = token {
            self.fail_native_action(token, NativeActionFailure::Indeterminate)?
        } else {
            Transition::empty()
        };
        transition.merge(self.adapter_stream_desynchronized()?);
        Ok(transition)
    }
}
