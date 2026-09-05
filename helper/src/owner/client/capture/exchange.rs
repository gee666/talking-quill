//! Capture request correlation, event pumping, and deadline enforcement.

use super::*;

impl OwnerCaptureClient {
    /// Pumps unsolicited events and renews under the actor's single internal
    /// command envelope.
    pub fn service_until(
        &mut self,
        now: Instant,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if now >= self.next_renewal {
            self.renew_until(deadline, cancelled)?;
        }
        let started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            self.check_budget(deadline, cancelled)?;
            if frames >= EVENT_PUMP_FRAME_BUDGET
                || self.clock.now().saturating_duration_since(started) >= EVENT_PUMP_TIME_BUDGET
            {
                if self.clock.now() >= self.next_renewal {
                    self.renew_until(deadline, cancelled)?;
                }
                self.last_failure = None;
                return Ok(());
            }
            match self.client.poll() {
                Ok(ClientPoll::Empty) => {
                    self.last_failure = None;
                    return Ok(());
                }
                Ok(ClientPoll::PeerClosed) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "disconnected",
                        operation: "service.poll",
                        correlation_status: "none",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                    self.last_failure = Some(OwnerClientDiagnostic {
                        category: "protocol",
                        operation: "service.poll",
                        correlation_status: "unexpected_response",
                        transport_status: "open",
                    });
                    self.client.abort();
                    return Err(OwnerClientError::Protocol);
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, "service.poll")?;
                    if self.clock.now() >= self.next_renewal {
                        self.renew_until(deadline, cancelled)?;
                    }
                }
                Err(error) => {
                    self.last_failure =
                        Some(client_failure_diagnostic("service.poll", "none", &error));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub(super) fn handle_unsolicited(
        &mut self,
        message: GatewayMessage,
        operation: &'static str,
    ) -> Result<(), OwnerClientError> {
        if let GatewayMessage::Event(talking_quill_owner_protocol::schema::Event::HealthChanged(
            health,
        )) = &message
        {
            self.health = health.clone();
        }
        if (self.event_handler)(message) == OwnerEventDisposition::Terminal {
            self.last_failure = Some(OwnerClientDiagnostic {
                category: "protocol",
                operation,
                correlation_status: "none",
                transport_status: "open",
            });
            self.client.abort();
            Err(OwnerClientError::Protocol)
        } else {
            Ok(())
        }
    }

    pub(super) fn call_until(
        &mut self,
        request: Request,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<SuccessResult, OwnerClientError> {
        let operation = request.method().as_str();
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        let correlation = self.client.send_request(&request).map_err(|error| {
            self.last_failure = Some(client_failure_diagnostic(
                operation,
                "not_established",
                &error,
            ));
            self.client.abort();
            OwnerClientError::Transport
        })?;
        let mut budget_started = self.clock.now();
        let mut frames = 0_usize;
        loop {
            if let Err(error) = self.check_budget(deadline, cancelled) {
                self.last_failure = Some(OwnerClientDiagnostic {
                    category: "uncertain",
                    operation,
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
                        operation,
                        correlation_status: "pending",
                        transport_status: "eof",
                    });
                    return Err(OwnerClientError::Disconnected);
                }
                Ok(ClientPoll::Message(GatewayMessage::Response {
                    correlation_sequence,
                    response,
                })) => {
                    if correlation_sequence != correlation {
                        self.last_failure = Some(OwnerClientDiagnostic {
                            category: "protocol",
                            operation,
                            correlation_status: "mismatched",
                            transport_status: "open",
                        });
                        self.client.abort();
                        return Err(OwnerClientError::Protocol);
                    }
                    return match response {
                        Response::Success(value) => {
                            self.last_failure = None;
                            Ok(value)
                        }
                        Response::Error(error) => {
                            self.last_failure = Some(OwnerClientDiagnostic {
                                category: "rejected",
                                operation,
                                correlation_status: "matched",
                                transport_status: "open",
                            });
                            Err(OwnerClientError::Rejected(error.code()))
                        }
                    };
                }
                Ok(ClientPoll::Message(message)) => {
                    frames += 1;
                    self.handle_unsolicited(message, operation)?;
                }
                Err(error) => {
                    self.last_failure =
                        Some(client_failure_diagnostic(operation, "pending", &error));
                    self.client.abort();
                    return Err(OwnerClientError::Transport);
                }
            }
        }
    }

    pub(super) fn operation_deadline(&self) -> Instant {
        self.clock.now() + OWNER_CALL_TIMEOUT
    }

    pub(super) fn check_budget(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        if cancelled.load(Ordering::Acquire) {
            self.client.abort();
            Err(OwnerClientError::Cancelled)
        } else if self.clock.now() >= deadline {
            self.client.abort();
            Err(OwnerClientError::Uncertain)
        } else {
            Ok(())
        }
    }

    #[doc(hidden)]
    pub fn replace_clock(&mut self, clock: Box<dyn OwnerClock>) {
        self.clock = clock;
    }
}
