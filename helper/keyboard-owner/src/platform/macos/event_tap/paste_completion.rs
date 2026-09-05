//! Publish exact paste results and retain ambiguous native insertion.

use super::*;

pub(super) fn publish_pending_paste_result(
    command: &PendingPaste,
    result: PasteResult,
) -> PasteResult {
    let authoritative = command.result.publish(result);
    let _ = command.acknowledgement.try_send(());
    authoritative
}

pub(super) fn finish_pending_paste(pending: &mut Option<PendingPaste>, result: PasteResult) {
    let Some(command) = pending.as_ref() else {
        return;
    };
    let _ = publish_pending_paste_result(command, result);
    // Release publication precedes Applied so no caller can observe completion
    // without the fixed authoritative result slot.
    command
        .state
        .store(PasteCommandState::Applied as u8, Ordering::Release);
    let _ = pending.take();
}

pub(super) fn cancel_pending_pastes(context: &CallbackContext) {
    let Ok(mut pending) = context.pending_paste.try_lock() else {
        return;
    };
    let Some(command) = pending.as_mut() else {
        return;
    };
    let barrier_unobserved =
        command.neutral_barrier_token.is_some() && command.neutral_barrier_state < 2;
    let insertion_status = command
        .insertion_request
        .as_ref()
        .map(InsertionRequest::status);
    match insertion_status {
        Some(InsertionStatus::Claimed | InsertionStatus::Ambiguous) => {
            close_ambiguous_insertion_admission(context);
            return;
        }
        Some(InsertionStatus::Pending) if Instant::now() < command.deadline => {
            arm_maintenance_timer(context);
            return;
        }
        Some(InsertionStatus::Pending) => {
            if command
                .insertion_request
                .as_ref()
                .is_some_and(InsertionRequest::cancel)
            {
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
            close_ambiguous_insertion_admission(context);
            return;
        }
        Some(InsertionStatus::Succeeded) => {
            command
                .state
                .store(PasteCommandState::Committed as u8, Ordering::Release);
            finish_pending_paste(
                &mut pending,
                PasteResult {
                    submitted: true,
                    reason: None,
                },
            );
            return;
        }
        Some(InsertionStatus::Failed(reason)) => {
            finish_pending_paste(&mut pending, failed_paste(reason));
            return;
        }
        Some(InsertionStatus::TargetInvalid) => {
            context.observability.record_target_validation_fallback();
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        }
        None => {}
    }
    if barrier_unobserved && Instant::now() < command.deadline {
        let _ = cancel_paste_command(&command.state);
        arm_maintenance_timer(context);
        return;
    }
    if barrier_unobserved {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
}
