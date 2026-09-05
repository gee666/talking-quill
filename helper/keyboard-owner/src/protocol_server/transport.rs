//! Bounded transport writes, flush completion, and terminal events.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    pub(super) fn enqueue_transport_frame(
        &mut self,
        connection: ConnectionId,
        frame: Vec<u8>,
        completion: FlushCompletion,
    ) -> Result<TransportProgress, ServerError> {
        let (receipt, progress) = {
            let active = self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)?;
            if active.pending_flushes.len() >= MAX_PENDING_TRANSPORT_FLUSHES {
                return Err(ServerError::TransportBackpressure);
            }
            let receipt = active.endpoint.try_send(frame)?;
            let progress = if active.pending_flushes.is_empty() {
                active.endpoint.flush(receipt)?
            } else {
                TransportProgress::Pending
            };
            (receipt, progress)
        };
        match progress {
            TransportProgress::Pending => {
                self.connections
                    .get_mut(&connection)
                    .ok_or(ServerError::UnknownConnection)?
                    .pending_flushes
                    .push_back(PendingFlush {
                        receipt,
                        completion,
                    });
            }
            TransportProgress::Complete => {
                self.complete_transport_flush(connection, completion)?;
            }
        }
        Ok(progress)
    }

    pub(super) fn service_transport_close(
        &mut self,
        connection: ConnectionId,
    ) -> Result<TransportProgress, ServerError> {
        let Some(correlation) = self
            .connections
            .get(&connection)
            .and_then(|active| active.pending_close)
        else {
            return Ok(TransportProgress::Complete);
        };
        let progress = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .endpoint
            .close()?;
        if progress == TransportProgress::Complete {
            self.finish_final_transport_close(connection, correlation)?;
        }
        Ok(progress)
    }

    pub(super) fn service_transport_flush(
        &mut self,
        connection: ConnectionId,
    ) -> Result<TransportProgress, ServerError> {
        let Some(receipt) = self
            .connections
            .get(&connection)
            .and_then(|active| active.pending_flushes.front())
            .map(|pending| pending.receipt)
        else {
            return Ok(TransportProgress::Complete);
        };
        let progress = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .endpoint
            .flush(receipt)?;
        if progress == TransportProgress::Complete {
            let completion = self
                .connections
                .get_mut(&connection)
                .and_then(|active| active.pending_flushes.pop_front())
                .ok_or(ServerError::ExecutorContract)?
                .completion;
            self.complete_transport_flush(connection, completion)?;
        }
        Ok(progress)
    }

    pub(super) fn complete_transport_flush(
        &mut self,
        connection: ConnectionId,
        completion: FlushCompletion,
    ) -> Result<(), ServerError> {
        match completion {
            FlushCompletion::RegisteredObservation => {
                self.registered_owner_flushed = self.registered_owner_flushed.saturating_add(1);
            }
            FlushCompletion::Ordinary => {}
            FlushCompletion::AdmittedEvent => {
                let queued = self
                    .queued_events
                    .pop_front()
                    .ok_or(ServerError::ExecutorContract)?;
                if queued.connection != connection {
                    return Err(ServerError::ExecutorContract);
                }
                let registered_activation = matches!(
                    queued.event,
                    Event::Activation(_) | Event::RegisteredObservation(_)
                );
                self.retire_one_admitted_effect()?;
                if registered_activation {
                    self.registered_owner_flushed = self.registered_owner_flushed.saturating_add(1);
                }
                self.ensure_effect_queue_consistent()?;
            }
            FlushCompletion::PredecessorTerminal(offer) => {
                self.state.confirm_predecessor_terminal_written(offer)?;
                match offer.event() {
                    PredecessorTerminalEvent::LeaseRevoked(_) => {
                        self.terminal_draining_sent.insert(connection, false);
                    }
                    PredecessorTerminalEvent::LeaseDraining(_) => {
                        self.terminal_draining_sent.insert(connection, true);
                    }
                    PredecessorTerminalEvent::LeaseNeutral
                    | PredecessorTerminalEvent::LeaseUnavailable(_) => {
                        if matches!(offer.event(), PredecessorTerminalEvent::LeaseNeutral) {
                            self.planned_exit_terminal_pending = false;
                        }
                        self.terminal_draining_sent.remove(&connection);
                    }
                }
            }
            FlushCompletion::PlannedExitResponse => {
                self.planned_exit_response_flushed = true;
            }
            FlushCompletion::FinalResponse(correlation) => {
                let progress = self
                    .connections
                    .get_mut(&connection)
                    .ok_or(ServerError::UnknownConnection)?
                    .endpoint
                    .close()?;
                if progress == TransportProgress::Pending {
                    self.connections
                        .get_mut(&connection)
                        .ok_or(ServerError::UnknownConnection)?
                        .pending_close = Some(correlation);
                } else {
                    self.finish_final_transport_close(connection, correlation)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish_final_transport_close(
        &mut self,
        connection: ConnectionId,
        correlation: ResponseCorrelation,
    ) -> Result<(), ServerError> {
        let active = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?;
        active.pending_close = None;
        active.closed = true;
        let transition = self.state.confirm_final_response_flushed(correlation)?;
        self.drive_transition(transition, None)?;
        self.reap_closed_connection(connection);
        Ok(())
    }

    pub(super) fn enqueue_and_flush_response(
        &mut self,
        connection: ConnectionId,
        request: &ReceivedRequest,
        response: &Response,
    ) -> Result<TransportProgress, ServerError> {
        let frame = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .encode_response(request, response)?;
        self.enqueue_transport_frame(connection, frame, FlushCompletion::Ordinary)
    }

    pub(super) fn encode_enqueue_flush_event(
        &mut self,
        connection: ConnectionId,
        event: &Event,
    ) -> Result<TransportProgress, ServerError> {
        let frame = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .encode_event(event)?;
        self.enqueue_transport_frame(connection, frame, FlushCompletion::Ordinary)
    }

    pub(super) fn send_direct_event(
        &mut self,
        connection: ConnectionId,
        event: &Event,
    ) -> Result<(), ServerError> {
        if self.encode_enqueue_flush_event(connection, event).is_ok() {
            return Ok(());
        }
        let _ = self.teardown_connection(connection, ControllerLossReason::Eof);
        Err(ServerError::EventDelivery)
    }

    pub(super) fn emit_terminal_offer(
        &mut self,
        offer: PredecessorTerminalOffer,
    ) -> Result<(), ServerError> {
        let event = wire_terminal_event(offer);
        let connection = offer.connection();
        let frame = self
            .connections
            .get_mut(&connection)
            .filter(|active| !active.closed)
            .ok_or(ServerError::TerminalRoute)
            .and_then(|active| {
                active
                    .codec
                    .encode_predecessor_terminal(&event)
                    .map_err(Into::into)
            });
        let write = frame.and_then(|frame| {
            self.enqueue_transport_frame(
                connection,
                frame,
                FlushCompletion::PredecessorTerminal(offer),
            )
        });
        if write.is_err() {
            if let Some(active) = self.connections.get_mut(&connection) {
                active.endpoint.abort();
                active.closed = true;
            }
            self.state.fail_predecessor_terminal_write(offer)?;
            self.terminal_draining_sent.remove(&connection);
            self.reap_closed_connection(connection);
        }
        Ok(())
    }

    pub(super) fn advance_predecessor_terminal(&mut self) -> Result<(), ServerError> {
        let Some((&connection, &draining_sent)) = self.terminal_draining_sent.iter().next() else {
            return Ok(());
        };
        let status = self.state.status();
        let candidate = if status.native_state_unknown {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                    TerminalUnavailableReason::OwnershipUnknown,
                ))
        } else if status.process_state == ProcessState::Degraded {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                    TerminalUnavailableReason::NativeFault,
                ))
        } else if self.state.ownership().is_native_neutral() {
            self.state
                .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
        } else if !draining_sent {
            terminal_ownership(self.state.ownership()).and_then(|ownership| {
                self.state
                    .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseDraining(ownership))
            })
        } else {
            None
        };
        if let Some(offer) = candidate {
            debug_assert_eq!(offer.connection(), connection);
            self.emit_terminal_offer(offer)?;
        }
        Ok(())
    }

    pub(super) fn retire_queued_events_for_connection(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        let before = self.queued_events.len();
        self.queued_events
            .retain(|queued| queued.connection != connection);
        let removed = before - self.queued_events.len();
        if removed == 0 {
            return Ok(());
        }
        let removed = u8::try_from(removed).map_err(|_| ServerError::ExecutorContract)?;
        let current = self.state.ownership().admitted_effects();
        let remaining = current
            .checked_sub(removed)
            .ok_or(ServerError::ExecutorContract)?;
        let transition = self.state.set_broker_admitted_effects(remaining)?;
        self.drive_transition(transition, None)?;
        self.ensure_effect_queue_consistent()
    }
}
