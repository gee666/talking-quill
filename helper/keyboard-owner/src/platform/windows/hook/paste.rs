//! Validate paste targets, wait for neutral modifiers, and submit clipboard input.
use super::*;

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
mod test_seams;
#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
pub(super) use test_seams::*;

mod admission;
pub(super) use admission::*;

pub(super) fn native_paste_modifiers_neutral() -> bool {
    ModifierTracker::from_state(key_is_down).is_neutral() && !key_is_down(VK_V)
}

pub(super) const fn paste_deadline_reason(modifiers_neutral: bool) -> PasteFailure {
    if modifiers_neutral {
        PasteFailure::Unavailable
    } else {
        PasteFailure::ConflictingModifiers
    }
}

pub(super) fn paste_deadline_failure(
    context: &CallbackContext,
    command: &mut PendingPaste,
) -> PasteResult {
    if native_paste_modifiers_neutral() {
        command.modifier_wait.finish();
        failed_paste(paste_deadline_reason(true))
    } else {
        command.modifier_wait.start();
        context.observability.record_modifier_timeout();
        failed_paste(paste_deadline_reason(false))
    }
}

pub(super) fn poll_pending_paste(context: &CallbackContext, pending: &mut Option<PendingPaste>) {
    let Some(command) = pending.as_mut() else {
        return;
    };
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
    let timed_out = Instant::now() >= command.deadline;
    let modifiers_neutral = native_paste_modifiers_neutral();
    let unavailable = paste_command_state(&command.state) == PasteCommandState::Cancelled
        || context.state.stopping.load(Ordering::Acquire)
        || context.terminal.is_triggered()
        || !context.gate.is_open();
    if unavailable || timed_out {
        eprintln!(
            "keyboard-owner paste unavailable: gate_or_deadline unavailable={unavailable} timed_out={timed_out}"
        );
        let result = if timed_out {
            paste_deadline_failure(context, command)
        } else {
            failed_paste(PasteFailure::Unavailable)
        };
        finish_pending_paste(pending, result);
        return;
    }
    if !revalidate_target(command.evidence) {
        eprintln!("keyboard-owner paste unavailable: target changed");
        context.observability.record_target_validation_fallback();
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    }
    if !modifiers_neutral {
        command.modifier_wait.start();
        return;
    }
    command.modifier_wait.finish();

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    // A previous accepted-prefix cleanup is retired while this command remains
    // Waiting and cancellable. No new injection authority exists yet.
    retry_pending_paste_cleanup(&mut keyboard);
    if !keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    drop(keyboard);

    // Clipboard Open/Get, canonical UTF-16 hashing, and stable sequence
    // sampling all remain on Waiting authority and share the immutable
    // submission-time deadline.
    let clipboard_sequence = match clipboard::matching_text_sequence(
        command.expected_clipboard_sha256,
        command.deadline,
    ) {
        Ok(sequence) => sequence,
        // Windows clipboard listeners can hold the clipboard briefly after a write.
        // Stay cancellable and retry on the existing timer, without extending the
        // original deadline. Every attempt revalidates the target and exact text.
        Err(clipboard::ClipboardReadError::Busy) => return,
        Err(clipboard::ClipboardReadError::Invalid) => {
            eprintln!("keyboard-owner paste unavailable: clipboard validation failed");
            let result = if Instant::now() >= command.deadline {
                paste_deadline_failure(context, command)
            } else {
                failed_paste(PasteFailure::Unavailable)
            };
            finish_pending_paste(pending, result);
            return;
        }
    };
    if Instant::now() >= command.deadline {
        let result = paste_deadline_failure(context, command);
        finish_pending_paste(pending, result);
        return;
    }

    #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
    pause_after_valid_clipboard_sample();

    if Instant::now() >= command.deadline {
        let result = paste_deadline_failure(context, command);
        finish_pending_paste(pending, result);
        return;
    }
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    if !keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    let target_valid = revalidate_target(command.evidence);
    if !target_valid {
        context.observability.record_target_validation_fallback();
    }
    let final_safe = paste_command_state(&command.state) == PasteCommandState::Waiting
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.terminal.is_triggered()
        && context.gate.is_open()
        && target_valid
        && ModifierTracker::from_state(key_is_down).is_neutral()
        && !key_is_down(VK_V);
    if !final_safe {
        drop(keyboard);
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    }

    let Some(outcome) = injection::inject_paste_initial_if(context.injection_markers, || {
        // These scalar checks and the single CAS are the complete final
        // closure. If cancellation wins Waiting->Cancelled, or the immutable
        // deadline/sequence changed, SendInput is never called. After the CAS
        // wins there is no blocking work before SendInput.
        Instant::now() < command.deadline
            && clipboard::sequence_is_current(clipboard_sequence)
            && Instant::now() < command.deadline
            && claim_paste_injection(&command.state)
    }) else {
        drop(keyboard);
        let result = if Instant::now() >= command.deadline {
            paste_deadline_failure(context, command)
        } else {
            failed_paste(PasteFailure::Unavailable)
        };
        finish_pending_paste(pending, result);
        return;
    };
    let result = publish_initial_paste_acceptance(
        &command.state,
        &command.result,
        &mut keyboard,
        outcome,
        || {},
    );
    // Wake a caller whose timeout lost the final CAS. The slot was published
    // first, so it consumes the authoritative SendInput result without waiting
    // for cleanup or receiving a contradictory ordinary failure.
    let _ = command.acknowledgement.try_send(());
    retry_pending_paste_cleanup(&mut keyboard);
    let degraded = outcome.initial_accepted != 4 || !keyboard.pending_paste_cleanup.is_empty();
    drop(keyboard);
    if degraded {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    finish_pending_paste(pending, result);
    if context
        .state
        .post_claim_timeout
        .swap(false, Ordering::AcqRel)
    {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
}

pub(super) fn publish_initial_paste_acceptance(
    state: &AtomicU8,
    result_slot: &PasteResultSlot,
    keyboard: &mut CallbackKeyboard,
    outcome: injection::PasteInjectionOutcome,
    after_commit: impl FnOnce(),
) -> PasteResult {
    keyboard.pending_paste_cleanup = outcome.pending_cleanup;
    // These atomics are the first operations after retaining the exact cleanup
    // obligation. No cleanup SendInput or blocking work can precede them.
    let result = result_slot.publish(outcome.result);
    state.store(
        if result.submitted {
            PasteCommandState::Committed as u8
        } else {
            PasteCommandState::ResultReady as u8
        },
        Ordering::Release,
    );
    after_commit();
    result
}

pub(super) fn finish_pending_paste(pending: &mut Option<PendingPaste>, result: PasteResult) {
    let Some(command) = pending.take() else {
        return;
    };
    // SAFETY: this timer ID was returned by SetTimer for the current thread.
    unsafe { KillTimer(null_mut(), command.timer_id) };
    let _ = command.result.publish(result);
    command
        .state
        .store(PasteCommandState::Applied as u8, Ordering::Release);
    let _ = command.acknowledgement.try_send(());
}
