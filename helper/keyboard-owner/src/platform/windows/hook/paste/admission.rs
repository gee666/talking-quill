//! Admit one cancellable paste command and own its deadline timer.
use super::*;

pub(in crate::platform::windows::hook) fn process_paste_commands(
    context: &CallbackContext,
    receiver: &Receiver<PasteCommand>,
    pending: &mut Option<PendingPaste>,
) {
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
    while let Ok(command) = receiver.try_recv() {
        if pending.is_some()
            || command
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
        let evidence = if let Some(mut keyboard) = lock_keyboard_recovering(context) {
            let evidence = keyboard.dispatcher.targets.take(command.context);
            if evidence.is_none() {
                context.observability.record_target_validation_fallback();
            }
            evidence
        } else {
            None
        };
        let Some(evidence) = evidence else {
            eprintln!("keyboard-owner paste unavailable: activation target missing");
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(());
            continue;
        };
        // SAFETY: a null HWND creates a thread timer delivered to this owner's
        // message queue. It is killed on every completion/cancellation path.
        let timer_id = unsafe { SetTimer(null_mut(), 0, PASTE_TIMER_INTERVAL_MS, None) };
        if timer_id == 0 {
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
            expected_clipboard_sha256: command.expected_clipboard_sha256,
            acknowledgement: command.acknowledgement,
            evidence,
            deadline: command.injection_deadline,
            timer_id,
            modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
        });
        #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
        pause_after_paste_admission(pending.as_ref().expect("installed paste").deadline);
    }
}
