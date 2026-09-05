//! Conservative native-work lifetime and bounded shutdown reporting.

use super::*;

pub(super) fn keyboard_has_pending_native_work(keyboard: &CallbackKeyboard) -> bool {
    keyboard.strict_drain_recovery_needed()
}

/// Single conservative authority for owner-loop lifetime. A busy state lock is
/// pending work: teardown is forbidden until an owner turn can prove otherwise.
pub(super) fn pending_native_work(context: &CallbackContext) -> bool {
    let keyboard_pending = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| keyboard_has_pending_native_work(&keyboard));
    let activation_pending = context
        .pending_activation
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    let paste_pending = context
        .pending_paste
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    let deferred_pending = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.has_pending());
    let pending = context.state.recovery_pending.load(Ordering::Acquire)
        || context.state.recovery_deferred_mode.load(Ordering::Acquire)
        || keyboard_pending
        || activation_pending
        || paste_pending
        || deferred_pending
        || !context.owner_commands.is_empty()
        || !context.paste_commands.is_empty();
    context
        .state
        .pending_native_work
        .store(pending, Ordering::Release);
    pending
}

pub(super) fn stop_owner_run_loop_if_drained(context: &CallbackContext) -> bool {
    if pending_native_work(context) {
        arm_maintenance_timer(context);
        false
    } else {
        // SAFETY: this is called only on the owner run loop. The authoritative
        // predicate proved that no retained/submitted native work can outlive
        // the tap or startup-allocated event pool.
        #[cfg(feature = "transactional-shortcuts-dev")]
        crate::platform::macos::mark_test_semantic_drain_complete();
        unsafe { ffi::CFRunLoopStop(ffi::CFRunLoopGetCurrent()) };
        true
    }
}

pub(super) fn begin_owner_shutdown(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    // Keep the AX target worker alive through candidate replay and terminal
    // drain. Owner teardown stops it only after native authority is drained.
    let _ = resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
    cancel_pending_pastes(context);
    while let Ok(command) = context.owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    while let Ok(command) = context.paste_commands.try_recv() {
        let _ = cancel_paste_command(&command.state);
        let _ = command
            .result
            .publish(failed_paste(PasteFailure::Unavailable));
        let _ = command.acknowledgement.try_send(());
    }
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            poisoned.into_inner()
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            arm_maintenance_timer(context);
            return;
        }
    };
    if !keyboard.shutdown_requested {
        // Install the one-shot control authority before executing a turn. If
        // native submission or continuation recovery unwinds, the poisoned
        // authoritative state prevents a second Shutdown control/post.
        keyboard.shutdown_requested = true;
        keyboard.shutdown_deadline = context
            .state
            .shutdown_deadline
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |installed| *installed)
            .or_else(|| Some(Instant::now() + SHUTDOWN_DRAIN_TIMEOUT));
        keyboard.shutdown_deadline_reported = false;
        #[cfg(test)]
        TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.set(count.get() + 1));
        let _ = begin_transaction_control(context, &mut keyboard, Control::Shutdown);
    }
    drop(keyboard);
    let _ = stop_owner_run_loop_if_drained(context);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShutdownDrainAction {
    Continue,
    ReportUnresponsive,
    Stop,
}

pub(super) fn shutdown_drain_action(
    has_ownership: bool,
    deadline: Option<Instant>,
    deadline_reported: bool,
    now: Instant,
) -> ShutdownDrainAction {
    if !has_ownership {
        ShutdownDrainAction::Stop
    } else if !deadline_reported && deadline.is_some_and(|deadline| now >= deadline) {
        ShutdownDrainAction::ReportUnresponsive
    } else {
        ShutdownDrainAction::Continue
    }
}

pub(super) fn terminal_shutdown_incomplete(context: &CallbackContext) {
    context.observability.record_shutdown_ownership_deadline();
    // An in-process owner cannot hand off a still-held suppressed down within a
    // bounded deadline without exposing the down or a later unmatched up.
    // Terminal teardown therefore posts no keyboard event and retires no
    // ownership. Production cannot reach this state because its process gate
    // prevents every native suppression facility from opening.
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if !tap.is_null() {
        unsafe { ffi::CGEventTapEnable(tap, false) };
    }
    if let Ok(mut pending) = context.pending_paste.try_lock()
        && let Some(command) = pending.as_ref()
    {
        let acceptance_ambiguous = command.insertion_request.as_ref().is_some_and(|request| {
            matches!(
                request.status(),
                InsertionStatus::Claimed | InsertionStatus::Ambiguous
            )
        });
        if !acceptance_ambiguous {
            let _ = publish_pending_paste_result(command, failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
        }
        // Terminal teardown may release the RPC waiter, but an
        // acceptance-ambiguous operation deliberately has no ordinary result.
        let _ = pending.take();
    }
    // Do not repost deferred keyboard or mouse input here. The absolute
    // deadline is an explicit incomplete semantic drain, not authority to
    // inject into whichever target is now foreground or to clear ownership.
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::OwnerThreadUnresponsive);
    unsafe { ffi::CFRunLoopStop(ffi::CFRunLoopGetCurrent()) };
}

pub(super) fn poll_shutdown_drain(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    if !context.state.stopping.load(Ordering::Acquire) {
        return;
    }
    let keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            poisoned.into_inner()
        }
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if !keyboard.shutdown_requested {
        drop(keyboard);
        begin_owner_shutdown(context);
        return;
    }
    let now = Instant::now();
    let deadline = keyboard.shutdown_deadline;
    let deadline_reported = keyboard.shutdown_deadline_reported;
    drop(keyboard);
    let has_ownership = pending_native_work(context);
    match shutdown_drain_action(has_ownership, deadline, deadline_reported, now) {
        ShutdownDrainAction::Stop => {
            let _ = stop_owner_run_loop_if_drained(context);
        }
        ShutdownDrainAction::ReportUnresponsive => {
            if let Ok(mut keyboard) = context.keyboard.try_lock() {
                keyboard.shutdown_deadline_reported = true;
            }
            terminal_shutdown_incomplete(context);
        }
        ShutdownDrainAction::Continue => {}
    }
}
