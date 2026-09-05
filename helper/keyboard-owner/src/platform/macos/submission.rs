//! Coordinator-side bounded command submission.
use super::*;

impl NativePlatform {
    pub(super) fn submit_owner_mutation(
        &self,
        mutation: OwnerMutation,
        terminal_reason: TerminalReason,
    ) -> Result<(), PlatformError> {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return Err(PlatformError::ThreadStopped);
        }

        let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        if self
            .owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .is_err()
        {
            self.mark_owner_failure(terminal_reason);
            return Err(PlatformError::NativeFailure);
        }

        if !self.signal_owner() {
            let state = cancel_owner_command(&command_state);
            if state == OwnerCommandState::Cancelled {
                self.mark_owner_failure(terminal_reason);
                return Err(PlatformError::NativeFailure);
            }
        }

        if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
            return result;
        }

        match cancel_owner_command(&command_state) {
            OwnerCommandState::Applied => Ok(()),
            OwnerCommandState::Applying => {
                if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
                    return result;
                }
                if owner_command_state(&command_state) == OwnerCommandState::Applied {
                    Ok(())
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    Err(PlatformError::NativeFailure)
                }
            }
            OwnerCommandState::Pending | OwnerCommandState::Cancelled => {
                self.mark_owner_failure(terminal_reason);
                Err(PlatformError::NativeFailure)
            }
        }
    }

    pub(super) fn submit_paste(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        let deadline = Instant::now() + PASTE_COMMAND_TIMEOUT;
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return failed_paste(PasteFailure::Unavailable);
        }
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, response) = bounded(1);
        if self
            .paste_commands
            .send_timeout(
                PasteCommand {
                    context,
                    expected_clipboard_sha256,
                    state: Arc::clone(&state),
                    result: Arc::clone(&result),
                    acknowledgement,
                    deadline,
                },
                deadline.saturating_duration_since(Instant::now()),
            )
            .is_err()
        {
            let _ = cancel_paste_command(&state);
            return result.publish(failed_paste(PasteFailure::Unavailable));
        }
        if !self.signal_owner() {
            match cancel_paste_command(&state) {
                PasteCommandState::Pending
                | PasteCommandState::Waiting
                | PasteCommandState::Cancelled => {
                    return result.publish(failed_paste(PasteFailure::Unavailable));
                }
                PasteCommandState::Injecting
                | PasteCommandState::Committed
                | PasteCommandState::Applied => {}
            }
        }
        if let Some(observed) = await_paste_result(&result, &response, deadline) {
            return observed;
        }
        match cancel_paste_command(&state) {
            PasteCommandState::Pending
            | PasteCommandState::Waiting
            | PasteCommandState::Cancelled => {
                result.publish(failed_paste(PasteFailure::Unavailable))
            }
            PasteCommandState::Injecting
            | PasteCommandState::Committed
            | PasteCommandState::Applied => {
                // Native acceptance may have occurred. Do not poison the fixed
                // result slot with an ordinary failure which a later AX success
                // could contradict; close admission and let the server suppress
                // this RPC response as an explicit terminal platform failure.
                self.mark_owner_failure(TerminalReason::InputInjectionUnavailable);
                failed_paste(PasteFailure::Indeterminate)
            }
        }
    }

    pub(super) fn signal_owner(&self) -> bool {
        signal_owner_endpoint(&self.state) == OwnerSignalOutcome::Signalled
    }

    pub(super) fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }

    pub(super) fn suspend_native_input(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::suspend_native_input(),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }
}
