//! Worker target validation and insertion completion decisions.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkerValidation {
    Pending,
    Valid {
        epoch: u64,
        boundary_epoch: u64,
        selected_range_epoch: u64,
    },
    Invalid,
}

pub(super) fn poll_worker_target_validation(
    context: &CallbackContext,
    command: &mut PendingPaste,
) -> WorkerValidation {
    let Some(cache) = context.target_cache.as_ref() else {
        return WorkerValidation::Invalid;
    };
    if command.validation_request.is_none() {
        let Some(request) = cache.request_target_validation(command.evidence) else {
            return WorkerValidation::Invalid;
        };
        command.validation_request = Some(request);
        return WorkerValidation::Pending;
    }
    let request = command
        .validation_request
        .as_mut()
        .expect("validation request installed above");
    let ticket = request.ticket();
    let Some(response) = request.try_response() else {
        return WorkerValidation::Pending;
    };
    command.validation_request = None;
    let Some((current, epoch, boundary_epoch, selected_range_epoch, _worker_confirmed)) = response
        .into_current_handle(
            ticket,
            cache.current_epoch(),
            cache.current_boundary_epoch(),
            cache.current_selected_range_epoch(),
        )
    else {
        return WorkerValidation::Invalid;
    };
    if command.evidence == current {
        WorkerValidation::Valid {
            epoch,
            boundary_epoch,
            selected_range_epoch,
        }
    } else {
        WorkerValidation::Invalid
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InsertionOwnerAction {
    Wait,
    CompleteSuccess,
    CompleteFailure(PasteFailure),
    CompleteTargetFallback,
    CancelPending,
    TerminalAmbiguous,
}

pub(super) fn insertion_owner_action(
    status: InsertionStatus,
    deadline_reached: bool,
) -> InsertionOwnerAction {
    match status {
        InsertionStatus::Succeeded => InsertionOwnerAction::CompleteSuccess,
        InsertionStatus::Failed(reason) => InsertionOwnerAction::CompleteFailure(reason),
        InsertionStatus::TargetInvalid => InsertionOwnerAction::CompleteTargetFallback,
        InsertionStatus::Ambiguous => InsertionOwnerAction::TerminalAmbiguous,
        InsertionStatus::Claimed if deadline_reached => InsertionOwnerAction::TerminalAmbiguous,
        InsertionStatus::Pending if deadline_reached => InsertionOwnerAction::CancelPending,
        InsertionStatus::Pending | InsertionStatus::Claimed => InsertionOwnerAction::Wait,
    }
}

pub(super) fn close_ambiguous_insertion_admission(context: &CallbackContext) {
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::InputInjectionUnavailable);
    arm_maintenance_timer(context);
}
