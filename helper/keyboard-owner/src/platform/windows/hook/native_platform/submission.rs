//! Submit owner mutations and paste commands with bounded acknowledgements.
use super::*;

impl NativePlatform {
    pub(in crate::platform::windows::hook) fn submit_owner_mutation(
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
        self.owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .map_err(|_| PlatformError::NativeFailure)?;

        if !post_owner_message(self.thread_id, WM_OWNER_COMMAND)
            && cancel_owner_command(&command_state) == OwnerCommandState::Cancelled
        {
            self.mark_owner_failure(terminal_reason);
            return Err(PlatformError::NativeFailure);
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

    pub(in crate::platform::windows::hook) fn submit_paste(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
            || !self
                .state
                .target_change_evidence_ready
                .load(Ordering::Acquire)
        {
            return failed_paste(PasteFailure::Unavailable);
        }
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, response) = bounded(1);
        let submitted_at = Instant::now();
        let injection_deadline = submitted_at + PASTE_NEUTRAL_TIMEOUT;
        let response_deadline = submitted_at + PASTE_COMMAND_TIMEOUT;
        if self
            .paste_commands
            .send_timeout(
                PasteCommand {
                    context,
                    expected_clipboard_sha256,
                    injection_deadline,
                    state: Arc::clone(&state),
                    result: Arc::clone(&result),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .is_err()
            || !post_owner_message(self.thread_id, WM_OWNER_PASTE)
        {
            let _ = cancel_paste_command(&state);
            return failed_paste(PasteFailure::Unavailable);
        }
        let _ = response.recv_timeout(response_deadline.saturating_duration_since(Instant::now()));
        if let Some(result) = result.result() {
            return result;
        }
        match cancel_paste_command(&state) {
            PasteCommandState::Injecting => {
                // Once Waiting->Injecting wins, duplicate fallback is unsafe:
                // SendInput has irreversible authority. Wait only for the
                // bounded post-CAS completion budget. If the native call does
                // not return, fail the owner closed and conservatively report
                // submitted so the host cannot issue another insertion.
                let completion = wait_for_claimed_paste_completion(
                    &response,
                    &result,
                    PASTE_POST_CAS_COMPLETION_TIMEOUT,
                );
                if completion.reason == Some(PasteFailure::Indeterminate) {
                    self.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    self.state.post_claim_timeout.store(true, Ordering::Release);
                    self.gate.close();
                }
                completion
            }
            PasteCommandState::Committed
            | PasteCommandState::Applied
            | PasteCommandState::ResultReady => {
                if let Some(result) = result.result() {
                    result
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    failed_paste(PasteFailure::Unavailable)
                }
            }
            PasteCommandState::Pending
            | PasteCommandState::Waiting
            | PasteCommandState::Cancelled => result
                .result()
                .unwrap_or_else(|| failed_paste(PasteFailure::Unavailable)),
        }
    }

    #[cfg(test)]
    pub(in crate::platform::windows::hook) fn inject_test_unmatched_down(
        &self,
    ) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::inject_test_unmatched_down(),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    pub(in crate::platform::windows::hook) fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }
}
