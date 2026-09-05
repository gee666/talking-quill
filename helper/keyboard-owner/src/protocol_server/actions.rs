//! Drive state transitions and ordered native executor actions.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    /// Closes every attached authority route before signal/session shutdown.
    /// Native ownership remains in this server and continues through the
    /// ordinary orphan cancellation/drain path.
    pub fn detach_all_for_shutdown(&mut self) -> Result<(), ServerError> {
        let connections = self.connections.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for connection in connections {
            if self.connections.contains_key(&connection)
                && let Err(error) = self.teardown_connection(connection, ControllerLossReason::Eof)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Applies the process rollback latch through the same ordered native close
    /// path used by a protocol rollback command.
    pub fn latch_runtime_rollback(&mut self) -> Result<(), ServerError> {
        let transition = self.state.latch_runtime_rollback();
        self.drive_external_transition(transition)
    }

    /// Attempts a neutral, quiescent idle exit. Busy/draining are returned to
    /// the outer runtime so it can keep pumping rather than invent neutrality.
    pub fn request_idle_exit(&mut self) -> Result<(), ServerError> {
        let transition = self.state.request_idle_exit();
        self.drive_external_transition(transition)
    }

    /// Explicit containment for an outer-loop/provider failure. This sacrifices
    /// future availability, closes fresh admission, and preserves drain work.
    pub fn recover_fatal_runtime_fault(&mut self) -> Result<(), ServerError> {
        let transition = self.state.recoverable_native_fault();
        self.drive_external_transition(transition)
    }

    pub(super) fn apply_capture(
        &mut self,
        connection: ConnectionId,
        sequence: u64,
        command: crate::state::CaptureCommand,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        let authority = self
            .capture_authority(connection)
            .ok_or(DispatchError::Fatal)?;
        let sequence = CommandSequence::new(sequence).ok_or(DispatchError::Fatal)?;
        let transition = self
            .state
            .apply_capture_command(connection, authority, sequence, command);
        self.apply_state(transition, paste)
    }

    pub(super) fn apply_maintenance(
        &mut self,
        connection: ConnectionId,
        sequence: u64,
        command: MaintenanceCommand,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        let authority = self
            .maintenance_authority(connection)
            .ok_or(DispatchError::Fatal)?;
        let sequence = CommandSequence::new(sequence).ok_or(DispatchError::Fatal)?;
        let transition = self
            .state
            .apply_maintenance_command(connection, authority, sequence, command);
        self.apply_state(transition, paste)
    }

    pub(super) fn apply_broker_event(
        &mut self,
        event: BrokerEvent,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        match event {
            BrokerEvent::Keyboard(event) => {
                self.admit_keyboard_event_inner(event, before_close_barrier)
            }
            BrokerEvent::RegisteredObservation { generation } => {
                self.admit_registered_observation_inner(generation, before_close_barrier)
            }
            BrokerEvent::AudioInputDevicesChanged => {
                self.admit_audio_devices_changed_inner(before_close_barrier)
            }
            BrokerEvent::OwnershipChanged(observation) => {
                self.observe_native_observation_inner(observation)
            }
            BrokerEvent::ReadinessChanged(readiness) => {
                self.observe_native_readiness_inner(readiness)
            }
            BrokerEvent::PasteClaimed(authorization) => {
                self.confirm_paste_claimed_inner(authorization)
            }
            BrokerEvent::PasteFinished {
                authorization,
                outcome,
            } => self.publish_paste_committed_inner(
                authorization,
                outcome == PasteCommitOutcome::Indeterminate,
            ),
            BrokerEvent::PasteIndeterminateResolved(authorization) => {
                self.confirm_paste_completed_inner(authorization)
            }
            BrokerEvent::RecoverableNativeFault => self.observe_recoverable_native_fault_inner(),
        }
    }

    pub(super) fn latch_adapter_desynchronization(&mut self) {
        let transition = self.state.adapter_stream_desynchronized();
        let _ = self.drive_external_transition(transition);
    }

    pub(super) fn pump_executor_adapter_event(
        &mut self,
        before_close_barrier: bool,
    ) -> Option<AdapterEventDisposition> {
        let (envelope, sequence_valid) = self.executor.try_next_adapter_event()?;
        let event = envelope.event();
        let nested = self.adapter_event_in_flight.is_some();
        if !nested {
            self.adapter_event_in_flight = Some(envelope.id());
        }
        let server_sequence_valid = self
            .adapter_event_high_water
            .checked_add(1)
            .is_some_and(|expected| expected == envelope.id().get());
        let disposition = if sequence_valid && server_sequence_valid {
            // Advance before semantic application so a reentrant close barrier
            // recognizes the current event. Its acknowledgement remains
            // ordered after application and before any nested acknowledgements.
            self.adapter_event_high_water = envelope.id().get();
            let preflush_error = if matches!(
                event,
                BrokerEvent::Keyboard(_)
                    | BrokerEvent::RegisteredObservation { .. }
                    | BrokerEvent::AudioInputDevicesChanged
            ) {
                None
            } else {
                self.flush_admitted_events().err()
            };
            // A later authoritative fact is applied even when delivery of an
            // earlier semantic notification failed and tore down its route.
            let application = self.apply_broker_event(event, before_close_barrier);
            if let Err(error) = &application
                && authoritative_control_event(event)
            {
                eprintln!("keyboard-owner adapter event failed: {event:?}: {error:?}");
            }
            match application {
                Ok(()) => AdapterEventDisposition::Accepted,
                Err(_error) if authoritative_control_event(event) => {
                    AdapterEventDisposition::Accepted
                }
                Err(error) => AdapterEventDisposition::Rejected(adapter_rejection(
                    event,
                    preflush_error.as_ref().unwrap_or(&error),
                )),
            }
        } else {
            self.latch_adapter_desynchronization();
            AdapterEventDisposition::Rejected(AdapterEventRejection::InvalidTransition)
        };
        if nested {
            self.deferred_adapter_acknowledgements
                .push_back((envelope.id(), disposition));
        } else {
            self.executor
                .acknowledge_adapter_event(envelope.id(), disposition);
            self.adapter_event_in_flight = None;
            while let Some((id, deferred)) = self.deferred_adapter_acknowledgements.pop_front() {
                self.executor.acknowledge_adapter_event(id, deferred);
            }
            self.finalize_acknowledged_close_confirmation();
            self.service_deferred_controller_losses();
        }
        Some(disposition)
    }

    pub(super) fn service_deferred_controller_losses(&mut self) {
        while let Some((connection, reason)) = self.deferred_controller_losses.pop_front() {
            let _ = self.handle_connection_loss(connection, reason);
            self.reap_closed_connection(connection);
        }
    }

    pub(super) fn finalize_acknowledged_close_confirmation(&mut self) {
        let Some(action) = self.pending_close_confirmation.take() else {
            return;
        };
        let mut summary = DriveSummary::default();
        match self.confirm_executor_result(action, ExecutorResult::Applied, &mut summary) {
            Ok(Some(transition)) => {
                if self.state.admission() == AdmissionState::Closed {
                    self.closing_event_scope = None;
                }
                let _ = self.drive_transition(transition, None);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = self.drive_confirmation_error(&error, None);
            }
        }
    }

    pub(super) fn drain_adapter_events_through(
        &mut self,
        through_event: Option<crate::adapter::AdapterEventId>,
    ) {
        let target = through_event.map_or(0, crate::adapter::AdapterEventId::get);
        if target < self.adapter_event_high_water {
            self.latch_adapter_desynchronization();
        }
        while self.adapter_event_high_water < target {
            let before = self.adapter_event_high_water;
            if self.pump_executor_adapter_event(true).is_none()
                || self.adapter_event_high_water == before
            {
                self.latch_adapter_desynchronization();
                break;
            }
        }
        // Delivery failure retires the semantic notification and tears down
        // its controller route; it is not an adapter event-sequence fault.
        let _ = self.flush_admitted_events();
    }

    pub(super) fn drive_external_transition(
        &mut self,
        transition: Result<Transition, TransitionError>,
    ) -> Result<(), ServerError> {
        match transition {
            Ok(transition) => {
                self.drive_transition(transition, None)?;
                Ok(())
            }
            Err(error) => {
                let actions = error.actions().as_slice().to_vec();
                let offer = error.terminal_offer();
                self.drive_actions_and_offer(&actions, offer, None)?;
                Err(ServerError::State(error))
            }
        }
    }

    pub(super) fn apply_state(
        &mut self,
        transition: Result<Transition, TransitionError>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, DispatchError> {
        match transition {
            Ok(transition) => match self.drive_transition(transition, paste) {
                Ok(summary) => Ok(summary),
                Err(ServerError::CloseRetryExhausted) => Err(DispatchError::Fatal),
                Err(_) => Err(DispatchError::Semantic(ErrorCode::NativeFailure)),
            },
            Err(error) => Err(self.handle_transition_error(error, paste)),
        }
    }

    pub(super) fn handle_transition_error(
        &mut self,
        error: TransitionError,
        paste: Option<PasteExecutorRequest>,
    ) -> DispatchError {
        let actions = error.actions().as_slice().to_vec();
        let offer = error.terminal_offer();
        if self
            .drive_actions_and_offer(&actions, offer, paste)
            .is_err()
        {
            return DispatchError::Fatal;
        }
        if matches!(
            error.kind(),
            TransitionErrorKind::ProtocolFault | TransitionErrorKind::WrongController
        ) {
            DispatchError::Fatal
        } else {
            DispatchError::Semantic(error_code(error.kind()))
        }
    }

    pub(super) fn drive_transition(
        &mut self,
        transition: Transition,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        let actions = transition.actions().as_slice().to_vec();
        let offer = transition.terminal_offer();
        let mut summary = DriveSummary {
            lease_disposition: transition.lease_disposition(),
            response_stage: transition.response_stage(),
            ..DriveSummary::default()
        };
        summary.merge(self.drive_actions_and_offer(&actions, offer, paste)?);
        Ok(summary)
    }

    pub(super) fn drive_actions_and_offer(
        &mut self,
        actions: &[RequiredAction],
        offer: Option<PredecessorTerminalOffer>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        let mut summary = DriveSummary::default();
        let mut pending_offer = offer;
        if self.state.admission() == AdmissionState::Closed
            && let Some(offer) = pending_offer.take()
        {
            self.emit_terminal_offer(offer)?;
        }
        for action in actions.iter().copied() {
            if matches!(
                action,
                RequiredAction::CloseFreshAdmission { .. }
                    | RequiredAction::EmergencyCloseFreshAdmission
            ) {
                summary.merge(self.drive_close_iteratively(action, &mut pending_offer, paste)?);
                continue;
            }
            if action == RequiredAction::ExitOwner {
                // W1 has already authorized exit after final flush. This is an
                // outer-loop directive, not a fallible native confirmation.
                let _ = self.executor.execute(ExecutorCommand::State(action));
                self.exit_requested = true;
                continue;
            }
            let command = if matches!(action, RequiredAction::AdmitPaste { .. }) {
                ExecutorCommand::AdmitPaste {
                    action,
                    request: paste.ok_or(ServerError::ExecutorContract)?,
                }
            } else {
                ExecutorCommand::State(action)
            };
            let result = self.executor.execute(command);
            let transition = match self.confirm_executor_result(action, result, &mut summary) {
                Ok(transition) => transition,
                Err(error) => {
                    let recovery = self.drive_confirmation_error(&error, paste);
                    self.fail_pending_terminal_offer(&mut pending_offer);
                    recovery?;
                    return Err(error);
                }
            };
            if let Some(transition) = transition {
                summary.merge(self.drive_transition(transition, paste)?);
            }
        }
        if pending_offer.is_some() {
            // Closure could not be established, so do not publish revocation
            // ahead of the native boundary. Retire the best-effort route.
            self.fail_pending_terminal_offer(&mut pending_offer);
        }
        self.advance_predecessor_terminal()?;
        Ok(summary)
    }

    pub(super) fn drive_close_iteratively(
        &mut self,
        initial_action: RequiredAction,
        pending_offer: &mut Option<PredecessorTerminalOffer>,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<DriveSummary, ServerError> {
        self.ensure_closing_event_scope(*pending_offer);
        let mut summary = DriveSummary::default();
        let mut action = initial_action;
        for _ in 0..MAX_SYNCHRONOUS_CLOSE_ATTEMPTS {
            let mut result = self.executor.execute(ExecutorCommand::State(action));
            if let ExecutorResult::AdmissionClosed { through_event } = result {
                self.drain_adapter_events_through(through_event);
                if self.adapter_event_in_flight.is_some() {
                    // The native barrier is authoritative, but reducer close
                    // and dependent effects must follow FIFO acknowledgements
                    // for the event that triggered this close.
                    self.pending_close_confirmation = Some(action);
                    return Ok(summary);
                }
                result = ExecutorResult::Applied;
            }
            let transition = match self.confirm_executor_result(action, result, &mut summary) {
                Ok(Some(transition)) => transition,
                Ok(None) => {
                    self.fail_pending_terminal_offer(pending_offer);
                    return Err(ServerError::ExecutorContract);
                }
                Err(error) => {
                    let recovery = self.drive_confirmation_error(&error, paste);
                    self.fail_pending_terminal_offer(pending_offer);
                    recovery?;
                    return Err(ServerError::ExecutorContract);
                }
            };
            summary.lease_disposition =
                transition.lease_disposition().or(summary.lease_disposition);
            summary.response_stage = transition.response_stage().or(summary.response_stage);
            if self.state.admission() == AdmissionState::Closed {
                self.closing_event_scope = None;
                if let Some(offer) = pending_offer.take() {
                    // Close is confirmed before revocation, and revocation is
                    // emitted before the transition's dependent work.
                    self.emit_terminal_offer(offer)?;
                }
                summary.merge(self.drive_transition(transition, paste)?);
                return Ok(summary);
            }
            let retry_actions = transition.actions().as_slice();
            if transition.terminal_offer().is_some()
                || retry_actions.len() != 1
                || !matches!(
                    retry_actions[0],
                    RequiredAction::CloseFreshAdmission { .. }
                        | RequiredAction::EmergencyCloseFreshAdmission
                )
            {
                self.fail_pending_terminal_offer(pending_offer);
                return Err(ServerError::ExecutorContract);
            }
            action = retry_actions[0];
        }
        self.fail_pending_terminal_offer(pending_offer);
        self.deferred_close = Some(action);
        Err(ServerError::CloseRetryExhausted)
    }

    pub(super) fn ensure_closing_event_scope(&mut self, offer: Option<PredecessorTerminalOffer>) {
        if self.closing_event_scope.is_some() {
            return;
        }
        let route = self
            .current_capture()
            .ok()
            .or_else(|| offer.map(|offer| (offer.connection(), offer.authority())));
        if let Some((connection, authority)) = route {
            self.closing_event_scope = Some(ClosingEventScope {
                connection,
                authority,
                bindings: self.active_bindings,
                session_mode: self.active_session_mode,
            });
        }
    }

    pub(super) fn drive_confirmation_error(
        &mut self,
        error: &ServerError,
        paste: Option<PasteExecutorRequest>,
    ) -> Result<(), ServerError> {
        let ServerError::State(error) = error else {
            return Ok(());
        };
        let mut actions = error.actions().as_slice().to_vec();
        if let Some(index) = actions.iter().position(|action| {
            matches!(
                action,
                RequiredAction::CloseFreshAdmission { .. }
                    | RequiredAction::EmergencyCloseFreshAdmission
            )
        }) {
            // Retain one close directive for the outer iterative service loop;
            // never recursively start a fresh close retry budget from a
            // confirmation error.
            self.deferred_close.get_or_insert(actions.remove(index));
        }
        let offer = error.terminal_offer();
        if actions.is_empty() && offer.is_none() {
            return Ok(());
        }
        self.drive_actions_and_offer(&actions, offer, paste)?;
        Ok(())
    }

    pub(super) fn fail_pending_terminal_offer(
        &mut self,
        pending_offer: &mut Option<PredecessorTerminalOffer>,
    ) {
        if let Some(offer) = pending_offer.take() {
            let _ = self.state.fail_predecessor_terminal_write(offer);
        }
    }
}
