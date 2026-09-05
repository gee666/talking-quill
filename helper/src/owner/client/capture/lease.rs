//! Capture lease renewal, release, and command sequence allocation.

use super::*;

impl OwnerCaptureClient {
    pub fn renew_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        let params = self.capture_params_without_renewal()?;
        match self.call_until(Request::LeaseRenew(params), deadline, cancelled)? {
            SuccessResult::Renew(value) if value.renewed => {
                self.next_renewal = self.clock.now() + LEASE_RENEW_INTERVAL;
                self.last_failure = None;
                Ok(())
            }
            _ => self.protocol_failure("lease.renew", "matched"),
        }
    }

    #[cfg(feature = "windows-installed-acceptance")]
    pub fn force_capture_safe_disabled_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        if self.enabled {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.set_session_mode_until(SessionMode::Off, deadline, cancelled)
    }

    pub fn runtime_rollback(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.runtime_rollback_until(deadline, &cancelled)
    }

    pub fn runtime_rollback_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.last_failure = None;
        let params = self.capture_params_until(deadline, cancelled)?;
        match self.call_until(Request::RuntimeRollback(params), deadline, cancelled)? {
            SuccessResult::Rollback(value) if value.latched => {
                self.enabled = false;
                Ok(value.disposition)
            }
            _ => self.protocol_failure("runtime.rollback", "matched"),
        }
    }

    pub fn release(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.release_with_exit_until(false, deadline, &cancelled)
    }

    pub fn release_and_exit_when_neutral(&mut self) -> Result<LeaseDisposition, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.release_and_exit_when_neutral_until(deadline, &cancelled)
    }

    pub fn release_and_exit_when_neutral_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.release_with_exit_until(true, deadline, cancelled)
    }

    fn release_with_exit_until(
        &mut self,
        exit_when_neutral: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        self.last_failure = None;
        if self.enabled && !exit_when_neutral {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.check_budget(deadline, cancelled)?;
        let params = self.capture_params_until(deadline, cancelled)?;
        let request = if exit_when_neutral {
            Request::OwnerExitWhenNeutral(params)
        } else {
            Request::LeaseRelease(params)
        };
        match self.call_until(request, deadline, cancelled)? {
            SuccessResult::Release(value)
                if exit_when_neutral && value.disposition == LeaseDisposition::Draining =>
            {
                self.wait_for_planned_neutral(deadline, cancelled)
            }
            SuccessResult::Release(value) => Ok(value.disposition),
            _ => self.protocol_failure("lease.release", "matched"),
        }
    }

    fn wait_for_planned_neutral(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<LeaseDisposition, OwnerClientError> {
        loop {
            self.check_budget(deadline, cancelled)?;
            match self.client.poll() {
                Ok(ClientPoll::Empty) => self.clock.sleep(Duration::from_millis(1)),
                Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(
                    talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseNeutral {
                        ..
                    },
                ))) => return Ok(LeaseDisposition::Neutral),
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(
                    talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseDraining {
                        ..
                    },
                ))) => {}
                Ok(ClientPoll::Message(GatewayMessage::PredecessorTerminal(_))) => {
                    self.client.abort();
                    return Err(OwnerClientError::Uncertain);
                }
                Ok(ClientPoll::Message(message)) => {
                    self.handle_unsolicited(message, "owner.exit_when_neutral")?;
                }
                Err(_) => {
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub(super) fn capture_params_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<CaptureCommandParams, OwnerClientError> {
        self.check_budget(deadline, cancelled)?;
        self.renew_if_due_until(deadline, cancelled)?;
        self.check_budget(deadline, cancelled)?;
        self.capture_params_without_renewal()
    }

    pub(super) fn renew_if_due_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.check_budget(deadline, cancelled)?;
        if self.clock.now() >= self.next_renewal {
            self.renew_until(deadline, cancelled)
        } else {
            Ok(())
        }
    }

    pub(super) fn capture_params_without_renewal(
        &mut self,
    ) -> Result<CaptureCommandParams, OwnerClientError> {
        let sequence = self.next_command_sequence;
        self.next_command_sequence = match sequence.checked_add(1) {
            Some(next) => next,
            None => return self.sequence_failure("command.sequence"),
        };
        let command_sequence = match U64String::try_from(sequence) {
            Ok(sequence) => sequence,
            Err(_) => return self.protocol_failure("command.sequence", "not_established"),
        };
        Ok(CaptureCommandParams {
            capture_lease_id: self.lease.capture_lease_id,
            capture_lease_epoch: self.lease.capture_lease_epoch,
            command_sequence,
        })
    }
}
