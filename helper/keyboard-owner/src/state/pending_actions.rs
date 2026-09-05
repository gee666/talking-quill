//! Bounded action tokens, native confirmations, and neutral-state checks.
use super::*;

impl KeyboardOwnerState {
    pub(super) fn issue_action(
        &mut self,
        kind: PendingActionKind,
        scope: ActionScope,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        let Some(id) = self.last_action_id.checked_add(1).and_then(NonZeroU64::new) else {
            return Err(self.action_allocation_error());
        };
        let Some(slot_index) = self.pending_actions.iter().position(Option::is_none) else {
            return Err(self.action_allocation_error());
        };
        self.last_action_id = id.get();
        let token = NativeActionToken {
            owner_instance: self.owner_instance,
            id,
            scope,
        };
        self.pending_actions[slot_index] = Some(PendingAction {
            token,
            kind,
            superseded: false,
        });
        actions.push(match kind {
            PendingActionKind::CloseAdmission => RequiredAction::CloseFreshAdmission { token },
            PendingActionKind::OpenAdmission => RequiredAction::OpenFreshAdmission { token },
            PendingActionKind::SessionMode(mode) => {
                RequiredAction::ApplySessionMode { token, mode }
            }
            PendingActionKind::Configuration(request) => RequiredAction::ApplyConfiguration {
                token,
                request,
                fence: PreHeldKeyFence::FenceCurrentPhysical,
            },
            PendingActionKind::AdmitPaste(authorization) => RequiredAction::AdmitPaste {
                token,
                authorization,
            },
            PendingActionKind::CancelCandidate => RequiredAction::CancelCandidate { token },
            PendingActionKind::CancelPaste(authorization) => RequiredAction::CancelWaitingPaste {
                token,
                authorization,
            },
            PendingActionKind::PersistMaintenance(request) => {
                RequiredAction::PersistMaintenanceRecord { token, request }
            }
            PendingActionKind::StopNative => RequiredAction::StopNativeAdapter { token },
        });
        Ok(())
    }

    pub(super) fn action_allocation_error(&mut self) -> TransitionError {
        let needs_emergency_close = self.admission != AdmissionState::Closed
            && !self.has_pending_kind(PendingActionKind::CloseAdmission)
            && !self.emergency_close_pending;
        self.degrade_unknown();
        let mut error = TransitionError::new(TransitionErrorKind::ActionIdExhausted);
        if needs_emergency_close {
            self.emergency_close_pending = true;
            error
                .actions
                .push(RequiredAction::EmergencyCloseFreshAdmission);
        }
        error
    }

    pub(super) fn take_pending_action(
        &mut self,
        token: NativeActionToken,
        expected: PendingActionKind,
    ) -> Result<PendingAction, TransitionError> {
        let Some(actual) = self
            .pending_actions
            .iter()
            .flatten()
            .find(|pending| pending.token == token)
            .map(|pending| pending.kind)
        else {
            return Err(self.native_confirmation_fault());
        };
        if actual != expected {
            return Err(self.native_confirmation_fault());
        }
        self.take_pending_by_token(token)
    }

    pub(super) fn take_pending_by_token(
        &mut self,
        token: NativeActionToken,
    ) -> Result<PendingAction, TransitionError> {
        let Some(slot) = self
            .pending_actions
            .iter_mut()
            .find(|slot| slot.is_some_and(|pending| pending.token == token))
        else {
            return Err(self.native_confirmation_fault());
        };
        Ok(slot.take().expect("matched pending action"))
    }

    pub(super) fn native_confirmation_fault(&mut self) -> TransitionError {
        self.degrade_unknown();
        let transition = match self.request_close(ClosePlan::default()) {
            Ok(transition) => transition,
            Err(error) => Transition {
                actions: error.actions,
                lease_disposition: None,
                terminal_offer: error.terminal_offer.map(|offer| *offer),
                response_stage: None,
            },
        };
        TransitionError::with_transition(
            TransitionErrorKind::NativeConfirmationMismatch,
            transition,
        )
    }

    pub(super) fn pending_actions_empty(&self) -> bool {
        self.pending_actions.iter().all(Option::is_none)
    }

    pub(super) fn has_pending_kind(&self, kind: PendingActionKind) -> bool {
        self.pending_actions
            .iter()
            .flatten()
            .any(|pending| pending.kind == kind)
    }

    pub(super) fn terminal_ownership(&self) -> Option<TerminalOwnership> {
        let mut count = 0_u8;
        let mut ownership = None;
        for (present, kind) in [
            (
                self.ownership.candidate != CandidateOwnership::None,
                TerminalOwnership::Candidate,
            ),
            (
                self.ownership.activation_drain_keys != 0,
                TerminalOwnership::Activation,
            ),
            (
                self.ownership.session_drain_keys != 0,
                TerminalOwnership::Session,
            ),
            (
                self.ownership.replay_cleanup_edges != 0,
                TerminalOwnership::ReplayCleanup,
            ),
            (
                self.ownership.paste != PasteOwnership::None,
                TerminalOwnership::Paste,
            ),
        ] {
            if present {
                count += 1;
                ownership = Some(kind);
            }
        }
        if count > 1 {
            Some(TerminalOwnership::Multiple)
        } else {
            ownership
        }
    }

    pub(super) fn native_quiescent_for_disposition(&self) -> bool {
        self.admission == AdmissionState::Closed
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.emergency_close_pending
            && self.close_plan.is_none()
            && self.dependent_plan == DependentPlan::default()
            && !self.native_state_unknown
    }

    pub(super) fn quiescent_for_neutral(&self) -> bool {
        self.admission == AdmissionState::Closed
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.emergency_close_pending
            && self.close_plan.is_none()
            && self.dependent_plan == DependentPlan::default()
            && !self.native_state_unknown
            && self.predecessor.is_none()
    }

    pub(super) fn next_epoch(high_water: &mut u64) -> Result<CapabilityEpoch, TransitionError> {
        let next = high_water
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| TransitionError::new(TransitionErrorKind::EpochExhausted))?;
        *high_water = next.get();
        Ok(CapabilityEpoch(next))
    }
}
