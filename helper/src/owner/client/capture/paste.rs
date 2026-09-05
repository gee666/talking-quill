//! Single-dispatch paste and bounded commit event polling.

use super::*;

impl OwnerCaptureClient {
    pub fn paste(&mut self, params: PasteInjectParams) -> Result<PasteResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.paste_until(params, deadline, &cancelled)
    }

    pub fn paste_until(
        &mut self,
        mut params: PasteInjectParams,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<PasteResult, OwnerClientError> {
        self.last_failure = None;
        let operation_id = params.operation_id;
        let base = self.capture_params_until(deadline, cancelled)?;
        params.capture_lease_id = base.capture_lease_id;
        params.capture_lease_epoch = base.capture_lease_epoch;
        params.command_sequence = base.command_sequence;
        match self.call_until(Request::PasteInject(params), deadline, cancelled)? {
            SuccessResult::Paste(PasteResult::Waiting { .. }) => {
                self.wait_for_paste_completion(operation_id, deadline, cancelled)
            }
            SuccessResult::Paste(value) => Ok(value),
            _ => self.protocol_failure("paste.inject", "matched"),
        }
    }

    fn wait_for_paste_completion(
        &mut self,
        operation_id: Bytes32,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<PasteResult, OwnerClientError> {
        self.last_failure = None;
        let mut budget_started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            if let Err(error) = self.check_budget(deadline, cancelled) {
                self.last_failure = Some(OwnerClientDiagnostic {
                    category: "uncertain",
                    operation: "paste.await_commit",
                    correlation_status: "pending",
                    transport_status: "open",
                });
                return Err(error);
            }
            if frames >= EVENT_PUMP_FRAME_BUDGET
                || self.clock.now().saturating_duration_since(budget_started)
                    >= EVENT_PUMP_TIME_BUDGET
            {
                frames = 0;
                budget_started = self.clock.now();
                std::thread::yield_now();
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => self.clock.sleep(Duration::from_millis(1)),
                Ok(ClientPoll::PeerClosed) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "disconnected",
                        operation: "paste.await_commit",
                        correlation_status: "pending",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Event(
                    talking_quill_owner_protocol::schema::Event::PasteCommitted(event),
                ))) if event.operation_id == operation_id => {
                    self.last_failure = None;
                    return Ok(match event.state {
                        talking_quill_owner_protocol::schema::PasteCommitState::Committed => {
                            PasteResult::Committed { operation_id }
                        }
                        talking_quill_owner_protocol::schema::PasteCommitState::Indeterminate => {
                            PasteResult::Indeterminate { operation_id }
                        }
                    });
                }
                Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "protocol",
                        operation: "paste.await_commit",
                        correlation_status: "unexpected_response",
                        transport_status: "open",
                    });
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, "paste.await_commit")?;
                }
                Err(error) => {
                    self.last_failure = Some(client_failure_diagnostic(
                        "paste.await_commit",
                        "pending",
                        &error,
                    ));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }
}
