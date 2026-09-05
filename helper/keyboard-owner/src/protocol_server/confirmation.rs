//! Translate native action completions into protocol responses.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    pub(super) fn confirm_executor_result(
        &mut self,
        action: RequiredAction,
        result: ExecutorResult,
        summary: &mut DriveSummary,
    ) -> Result<Option<Transition>, ServerError> {
        let transition = match (action, result) {
            (RequiredAction::CloseFreshAdmission { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_admission_closed(token)?)
            }
            (RequiredAction::EmergencyCloseFreshAdmission, ExecutorResult::Applied) => {
                Some(self.state.confirm_emergency_admission_closed()?)
            }
            (RequiredAction::OpenFreshAdmission { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_admission_opened(token)?)
            }
            (RequiredAction::ApplySessionMode { token, mode }, ExecutorResult::Applied) => {
                Some(self.state.confirm_session_mode_applied(token, mode)?)
            }
            (
                RequiredAction::ApplyConfiguration { token, request, .. },
                ExecutorResult::Applied,
            ) => Some(self.state.confirm_configuration_applied(token, request)?),
            (
                RequiredAction::AdmitPaste {
                    token,
                    authorization,
                },
                ExecutorResult::PasteWaiting,
            ) => {
                summary.paste_waiting = true;
                Some(self.state.confirm_paste_waiting(token, authorization)?)
            }
            (
                RequiredAction::AdmitPaste {
                    token,
                    authorization,
                },
                ExecutorResult::PasteRefused(reason),
            ) => {
                summary.paste_refusal = Some(reason);
                Some(self.state.confirm_paste_refused(token, authorization)?)
            }
            (
                RequiredAction::CancelCandidate { token },
                ExecutorResult::CandidateCancelled(ownership),
            ) => Some(self.state.confirm_candidate_cancelled(token, ownership)?),
            (
                RequiredAction::CancelWaitingPaste {
                    token,
                    authorization,
                },
                ExecutorResult::Applied,
            ) => Some(
                self.state
                    .confirm_waiting_paste_cancelled(token, authorization)?,
            ),
            (
                RequiredAction::PersistMaintenanceRecord { token, request },
                ExecutorResult::Applied,
            ) => Some(self.state.confirm_maintenance_persisted(token, request)?),
            (RequiredAction::StopNativeAdapter { token }, ExecutorResult::Applied) => {
                Some(self.state.confirm_native_stopped(token)?)
            }
            (RequiredAction::ContinueNativeDrain, ExecutorResult::Applied) => None,
            (action, ExecutorResult::AdmissionClosed { .. }) => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_adapter_contract(action.token())?)
            }
            (action, ExecutorResult::ContractViolation) => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_adapter_contract(action.token())?)
            }
            (action, ExecutorResult::Failed(failure)) if action.token().is_some() => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(failure);
                }
                summary.native_failure = Some(failure);
                Some(
                    self.state
                        .fail_native_action(action.token().expect("checked token"), failure)?,
                )
            }
            (action, _) if action.token().is_some() => {
                if action.kind() == RequiredActionKind::AdmitPaste {
                    summary.paste_failure = Some(NativeActionFailure::Indeterminate);
                }
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.fail_native_action(
                    action.token().expect("checked token"),
                    NativeActionFailure::Indeterminate,
                )?)
            }
            _ => {
                summary.native_failure = Some(NativeActionFailure::Indeterminate);
                Some(self.state.recoverable_native_fault()?)
            }
        };
        Ok(transition)
    }

    pub(super) fn validate_capability_request(
        &mut self,
        connection: ConnectionId,
        request: &Request,
    ) -> Result<(), ServerError> {
        let active = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?;
        let validator = match request {
            Request::MaintenanceRenew(_) | Request::MaintenancePrepare(_) => {
                active.maintenance_sequence.as_mut()
            }
            _ => active.capture_sequence.as_mut(),
        }
        .ok_or(ServerError::CapabilityProtocol)?;
        validator
            .accept(request)
            .map_err(|_| ServerError::CapabilityProtocol)
    }
}
