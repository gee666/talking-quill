//! Resolve worker validation before committing activation delivery.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingActivationResolution {
    Poll,
    ForceTargetless,
    FailDelivery,
}

pub(super) fn resolve_pending_activation(
    context: &CallbackContext,
    resolution: PendingActivationResolution,
) -> bool {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => return false,
    };
    let mut pending_slot = match context.pending_activation.try_lock() {
        Ok(pending) => pending,
        Err(_) => return false,
    };
    let Some(pending) = pending_slot.as_mut() else {
        return true;
    };

    let mut validated_evidence = None;
    let ready = pending.resolved_delivery.is_some()
        || if resolution == PendingActivationResolution::FailDelivery {
            true
        } else if let Some(cache) = context.target_cache.as_ref()
            && let Some(response) = pending.validation_request.try_response()
        {
            let ticket = pending.validation_request.ticket();
            if let Some((after, epoch, boundary_epoch, _range_epoch, worker_confirmed)) = response
                .into_current_handle(
                    ticket,
                    cache.current_epoch(),
                    cache.current_boundary_epoch(),
                    cache.current_selected_range_epoch(),
                )
                && worker_confirmed
                && epoch == cache.current_epoch()
                && boundary_epoch == cache.current_boundary_epoch()
                && cache.reservation_is_current(&pending.reservation)
            {
                validated_evidence = Some(after);
            }
            true
        } else {
            resolution == PendingActivationResolution::ForceTargetless
                || Instant::now() >= pending.deadline
        };
    if !ready {
        return true;
    }

    let delivered = if let Some(delivered) = pending.resolved_delivery {
        delivered
    } else if resolution == PendingActivationResolution::FailDelivery {
        false
    } else {
        // A response can race a notification between channel receipt and this
        // final owner-side check. Such a race keeps activation functional but
        // deliberately strips its paste target.
        let evidence = context
            .target_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.current_epoch() == pending.validation_request.ticket().start_epoch()
                    && cache.current_boundary_epoch()
                        == pending.validation_request.ticket().start_boundary_epoch()
            })
            .then_some(validated_evidence)
            .flatten();
        keyboard.dispatcher.deliver(
            &context.outbound,
            &context.terminal,
            pending.notice,
            evidence,
        )
    };
    pending.resolved_delivery = Some(delivered);
    let continuation = pending.continuation.clone();
    // Transfer continuation authority before resuming it. A failed delivery may
    // install an observed HID replay; the activation slot must never remain a
    // second resumable owner of that same continuation.
    let _resolved = pending_slot
        .take()
        .expect("pending activation remains installed until authority transfer");
    drop(pending_slot);
    let completion = drive_transaction_turn(
        context,
        &mut keyboard,
        continuation.resume(EffectOutcome::ActivationDelivered(delivered)),
        None,
    );
    let DriveCompletion::Complete(Completion::Event(outcome)) = completion else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return false;
    };
    if outcome.terminal && context.gate.is_open() && !context.terminal.is_triggered() {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    true
}
