//! Paste command admission and final mutable safety checks.

use super::*;

pub(super) fn process_paste_commands(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    while let Ok(command) = context.paste_commands.try_recv() {
        if !paste_before_deadline(command.deadline, Instant::now())
            || !paste_admission_open(context)
        {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        if command
            .state
            .compare_exchange(
                PasteCommandState::Pending as u8,
                PasteCommandState::Waiting as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        if !paste_admission_open(context) {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        let evidence = if let Ok(mut keyboard) = context.keyboard.try_lock() {
            let evidence = keyboard.dispatcher.targets.take(command.context);
            if evidence.is_none() {
                context.observability.record_target_validation_fallback();
            }
            evidence
        } else {
            None
        };
        let Some(evidence) = evidence else {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(());
            continue;
        };
        let mut pending = match context.pending_paste.try_lock() {
            Ok(pending) => pending,
            Err(_) => {
                let _ = command
                    .result
                    .publish(failed_paste(PasteFailure::Unavailable));
                command
                    .state
                    .store(PasteCommandState::Applied as u8, Ordering::Release);
                let _ = command.acknowledgement.try_send(());
                continue;
            }
        };
        if pending.is_some() {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        *pending = Some(PendingPaste {
            state: command.state,
            result: command.result,
            acknowledgement: command.acknowledgement,
            evidence,
            expected_clipboard_sha256: command.expected_clipboard_sha256,
            validation_request: None,
            validated_target_epoch: None,
            validated_target_boundary_epoch: None,
            validated_selected_range_epoch: None,
            insertion_request: None,
            neutral_modifier_epoch: None,
            neutral_barrier_state: 0,
            neutral_barrier_token: None,
            deadline: command.deadline,
            injection_cutoff: paste_injection_cutoff(command.deadline),
            modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
        });
        drop(pending);
        poll_pending_paste(context);
        if context
            .pending_paste
            .try_lock()
            .is_ok_and(|pending| pending.is_some())
        {
            arm_maintenance_timer(context);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PastePostChecks {
    pub(super) admission_open: bool,
    pub(super) before_deadline: bool,
    pub(super) before_injection_cutoff: bool,
    pub(super) modifiers_neutral: bool,
    pub(super) permissions_granted: bool,
    pub(super) secure_input_inactive: bool,
    pub(super) target_valid: bool,
}

pub(super) const fn modifier_wait_timed_out(
    reason: PasteFailure,
    before_injection_cutoff: bool,
) -> bool {
    matches!(reason, PasteFailure::ConflictingModifiers) && !before_injection_cutoff
}

pub(super) fn paste_check_failure(
    state: &AtomicU8,
    checks: PastePostChecks,
) -> Option<PasteFailure> {
    if paste_command_state(state) != PasteCommandState::Waiting || !checks.admission_open {
        return Some(PasteFailure::Unavailable);
    }
    if !checks.before_deadline || !checks.before_injection_cutoff {
        return Some(if checks.modifiers_neutral {
            PasteFailure::Unavailable
        } else {
            PasteFailure::ConflictingModifiers
        });
    }
    if !checks.modifiers_neutral {
        return Some(PasteFailure::ConflictingModifiers);
    }
    if !checks.permissions_granted {
        return Some(PasteFailure::PermissionDenied);
    }
    if !checks.secure_input_inactive {
        return Some(PasteFailure::SecureInput);
    }
    if !checks.target_valid {
        return Some(PasteFailure::Unavailable);
    }
    None
}

pub(super) fn claim_paste_injection(
    state: &AtomicU8,
    checks: PastePostChecks,
) -> Result<(), PasteFailure> {
    if let Some(reason) = paste_check_failure(state, checks) {
        return Err(reason);
    }
    state
        .compare_exchange(
            PasteCommandState::Waiting as u8,
            PasteCommandState::Injecting as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map(|_| ())
        .map_err(|_| PasteFailure::Unavailable)
}

#[cfg(any(test, feature = "transactional-shortcuts-dev"))]
pub(super) const fn paste_modifier_barrier_valid(
    expected_epoch: Option<u64>,
    current_epoch: u64,
    logical_neutral: bool,
    hid_neutral: bool,
    barrier_complete: bool,
) -> bool {
    barrier_complete
        && logical_neutral
        && hid_neutral
        && matches!(expected_epoch, Some(expected) if expected == current_epoch)
}

pub(super) fn paste_admission_open(context: &CallbackContext) -> bool {
    context.gate.is_open()
        && !context.terminal.is_triggered()
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.state.quiescing.load(Ordering::Acquire)
}

pub(super) fn current_paste_checks(
    context: &CallbackContext,
    command: &PendingPaste,
    target_valid: bool,
) -> PastePostChecks {
    let now = Instant::now();
    let permissions = permission_snapshot();
    PastePostChecks {
        admission_open: paste_admission_open(context),
        before_deadline: paste_before_deadline(command.deadline, now),
        before_injection_cutoff: paste_before_deadline(command.injection_cutoff, now),
        modifiers_neutral: native_modifiers_neutral()
            && context
                .keyboard
                .try_lock()
                .is_ok_and(|keyboard| keyboard.modifiers.sides().bits() == 0),
        permissions_granted: permissions.accessibility == PermissionState::Granted
            && permissions.input_monitoring == PermissionState::Granted
            && permissions.event_post == PermissionState::Granted,
        secure_input_inactive: !secure_input_active(),
        target_valid,
    }
}
