//! Owner-side paste progress from validation to authenticated neutral barrier.

use super::*;

pub(super) fn poll_pending_paste(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    let mut pending = match context.pending_paste.try_lock() {
        Ok(pending) => pending,
        Err(_) => return,
    };
    let Some(command) = pending.as_mut() else {
        return;
    };
    if paste_command_state(&command.state) == PasteCommandState::Injecting
        && command.insertion_request.is_some()
    {
        let status = command
            .insertion_request
            .as_ref()
            .map(InsertionRequest::status)
            .unwrap_or(InsertionStatus::Ambiguous);
        match insertion_owner_action(status, Instant::now() >= command.deadline) {
            InsertionOwnerAction::CompleteSuccess => {
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
            }
            InsertionOwnerAction::CompleteFailure(reason) => {
                finish_pending_paste(&mut pending, failed_paste(reason));
            }
            InsertionOwnerAction::CompleteTargetFallback => {
                context.observability.record_target_validation_fallback();
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            }
            InsertionOwnerAction::TerminalAmbiguous => {
                close_ambiguous_insertion_admission(context);
            }
            InsertionOwnerAction::CancelPending => {
                let cancelled = command
                    .insertion_request
                    .as_ref()
                    .is_some_and(InsertionRequest::cancel);
                if cancelled {
                    finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                } else {
                    // A failed cancellation can only mean that the worker won
                    // claim/completion. Retain the request and let the next
                    // owner turn consume exact completion or go terminal.
                    arm_maintenance_timer(context);
                }
            }
            InsertionOwnerAction::Wait => arm_maintenance_timer(context),
        }
        return;
    }
    if command.neutral_modifier_epoch.is_some() && command.neutral_barrier_state < 2 {
        if Instant::now() >= command.deadline {
            // Modifiers were already neutral when the barrier was posted. A
            // missing observation is an event-post/tap failure, not a modifier
            // wait timeout.
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
        }
        return;
    }
    let checks = current_paste_checks(context, command, true);
    if checks.modifiers_neutral {
        command.modifier_wait.finish();
    }
    if let Some(reason) = paste_check_failure(&command.state, checks) {
        if reason == PasteFailure::ConflictingModifiers
            && paste_before_deadline(command.injection_cutoff, Instant::now())
        {
            // Physical modifiers may still be unwinding from the activation
            // chord. Wait under the same absolute deadline; do not post a
            // barrier or paste until both logical and HID state are neutral.
            command.modifier_wait.start();
            return;
        }
        if modifier_wait_timed_out(
            reason,
            paste_before_deadline(command.injection_cutoff, Instant::now()),
        ) {
            context.observability.record_modifier_timeout();
        }
        finish_pending_paste(&mut pending, failed_paste(reason));
        return;
    }

    // AX validation is the last asynchronous operation. Its exact workspace
    // and event-boundary epochs are frozen before the authenticated modifier
    // barrier is posted; no AX request is allowed after that barrier.
    if command.validated_target_epoch.is_none() {
        match poll_worker_target_validation(context, command) {
            WorkerValidation::Pending => return,
            WorkerValidation::Invalid => {
                context.observability.record_target_validation_fallback();
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
            WorkerValidation::Valid {
                epoch,
                boundary_epoch,
                selected_range_epoch,
            } => {
                command.validated_target_epoch = Some(epoch);
                command.validated_target_boundary_epoch = Some(boundary_epoch);
                command.validated_selected_range_epoch = Some(selected_range_epoch);
            }
        }
    }

    if command.neutral_barrier_token.is_none() {
        let checks = current_paste_checks(context, command, true);
        if checks.modifiers_neutral {
            command.modifier_wait.finish();
        }
        if let Some(reason) = paste_check_failure(&command.state, checks) {
            if reason == PasteFailure::ConflictingModifiers
                && paste_before_deadline(command.injection_cutoff, Instant::now())
            {
                command.modifier_wait.start();
                return;
            }
            if modifier_wait_timed_out(
                reason,
                paste_before_deadline(command.injection_cutoff, Instant::now()),
            ) {
                context.observability.record_modifier_timeout();
            }
            finish_pending_paste(&mut pending, failed_paste(reason));
            return;
        }
        let Some(cache) = context.target_cache.as_ref() else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        };
        let (Some(epoch), Some(boundary_epoch), Some(selected_range_epoch)) = (
            command.validated_target_epoch,
            command.validated_target_boundary_epoch,
            command.validated_selected_range_epoch,
        ) else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        };
        command.insertion_request = cache.prepare_insertion(
            command.evidence,
            epoch,
            boundary_epoch,
            selected_range_epoch,
            command.expected_clipboard_sha256,
            command.injection_cutoff,
        );
        if command.insertion_request.is_none() {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        }
        let mut native_events = match context.native_events.try_lock() {
            Ok(pool) if pool.is_some() => pool,
            _ => {
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
        };
        let pool = native_events
            .as_mut()
            .expect("native event pool checked above");
        let modifier_epoch = context
            .keyboard
            .try_lock()
            .ok()
            .filter(|keyboard| keyboard.modifiers.sides().bits() == 0)
            .map(|keyboard| keyboard.modifier_epoch);
        let Some(modifier_epoch) = modifier_epoch else {
            finish_pending_paste(
                &mut pending,
                failed_paste(PasteFailure::ConflictingModifiers),
            );
            return;
        };
        let Some(barrier) = injection::prepare_paste_barrier(pool) else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::OsRejected));
            return;
        };
        command.neutral_modifier_epoch = Some(modifier_epoch);
        command.neutral_barrier_state = 0;
        command.neutral_barrier_token = Some(barrier.token());
        // Install the exact token before CGEventPost and release the callback
        // lock so the down cannot be lost to owner-side contention.
        drop(pending);
        #[cfg(feature = "transactional-shortcuts-dev")]
        let split_barrier = context.test_paste_barrier_paused.is_some()
            && context.test_paste_barrier_release.is_some();
        #[cfg(feature = "transactional-shortcuts-dev")]
        if split_barrier {
            context
                .test_paste_barrier_down_observed
                .store(false, Ordering::Release);
            context
                .test_paste_barrier_pause_announced
                .store(false, Ordering::Release);
            context
                .test_paste_barrier_split_active
                .store(true, Ordering::Release);
            barrier.post_down(pool);
        } else {
            barrier.post(pool);
        }
        #[cfg(not(feature = "transactional-shortcuts-dev"))]
        barrier.post(pool);
        drop(native_events);
        // Require the ordered neutral barrier and another independently
        // refreshed target tuple before posting.
        return;
    }

    // The barrier callback submits only the preallocated target-specific AX
    // work. Owner polling waits for that worker result; no global Command+V is
    // posted for this path.
    arm_maintenance_timer(context);
}
