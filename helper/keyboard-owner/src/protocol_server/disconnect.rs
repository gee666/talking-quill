//! Retire failed connections and revoke their capture authority.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    pub(super) fn handle_connection_loss(
        &mut self,
        connection: ConnectionId,
        reason: ControllerLossReason,
    ) -> Result<(), ServerError> {
        match self.state.controller() {
            ControllerState::AuthenticatedObserver { connection: active }
            | ControllerState::CaptureLeaseDisabled {
                connection: active, ..
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active, ..
            }
            | ControllerState::MaintenanceExclusive {
                connection: active, ..
            } if active == connection => match self.state.controller_lost(connection, reason) {
                Ok(transition) => {
                    self.drive_transition(transition, None)?;
                }
                Err(error) => {
                    let _ = self.handle_transition_error(error, None);
                }
            },
            _ => self.retire_disconnected_predecessor(connection)?,
        }
        if self.capture_authority(connection).is_none() {
            self.active_activation = None;
            self.active_session_keys = 0;
            self.active_session_mode = None;
        }
        if self.state.ownership().paste() == PasteOwnership::None {
            self.active_paste = None;
        }
        Ok(())
    }

    pub(super) fn retire_disconnected_predecessor(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        if !self.terminal_draining_sent.contains_key(&connection) {
            return Ok(());
        }
        let status = self.state.status();
        let event = if status.native_state_unknown {
            PredecessorTerminalEvent::LeaseUnavailable(TerminalUnavailableReason::OwnershipUnknown)
        } else if status.process_state == ProcessState::Degraded {
            PredecessorTerminalEvent::LeaseUnavailable(TerminalUnavailableReason::NativeFault)
        } else if self.state.ownership().is_native_neutral() {
            PredecessorTerminalEvent::LeaseNeutral
        } else if let Some(ownership) = terminal_ownership(self.state.ownership()) {
            PredecessorTerminalEvent::LeaseDraining(ownership)
        } else {
            return Ok(());
        };
        if let Some(offer) = self.state.offer_predecessor_terminal(event) {
            self.state.fail_predecessor_terminal_write(offer)?;
            self.terminal_draining_sent.remove(&connection);
        }
        Ok(())
    }

    pub(super) fn abort_connection(&mut self, connection: ConnectionId) {
        if let Some(active) = self.connections.get_mut(&connection) {
            active.endpoint.abort();
            active.closed = true;
        }
    }

    pub(super) fn teardown_connection(
        &mut self,
        connection: ConnectionId,
        reason: ControllerLossReason,
    ) -> Result<(), ServerError> {
        eprintln!("keyboard-owner connection ended: {reason:?}");
        let held_capture_lease = matches!(
            self.state.controller(),
            ControllerState::CaptureLeaseDisabled { connection: active, .. }
                | ControllerState::CaptureLeaseEnabled { connection: active, .. }
                if active == connection
        );
        if held_capture_lease && reason != ControllerLossReason::HeartbeatExpired {
            self.lease_disconnected = self.lease_disconnected.saturating_add(1);
        }
        let pending_failure = self.fail_pending_transport_writes(connection);
        self.abort_connection(connection);
        let retirement = self.retire_queued_events_for_connection(connection);
        let controller_loss = self.handle_connection_loss(connection, reason);
        self.reap_closed_connection(connection);
        match (pending_failure, retirement, controller_loss) {
            (Err(error), _, _) | (Ok(()), Err(error), _) | (Ok(()), Ok(()), Err(error)) => {
                Err(error)
            }
            (Ok(()), Ok(()), Ok(())) => Ok(()),
        }
    }

    pub(super) fn fail_pending_transport_writes(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), ServerError> {
        let pending = self
            .connections
            .get_mut(&connection)
            .map(|active| {
                active.pending_close = None;
                active.pending_flushes.drain(..).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for write in pending {
            if let FlushCompletion::PredecessorTerminal(offer) = write.completion {
                self.state.fail_predecessor_terminal_write(offer)?;
                self.terminal_draining_sent.remove(&connection);
            }
        }
        Ok(())
    }

    pub(super) fn reap_closed_connection(&mut self, connection: ConnectionId) {
        let is_controller = match self.state.controller() {
            ControllerState::AuthenticatedObserver { connection: active }
            | ControllerState::CaptureLeaseDisabled {
                connection: active, ..
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active, ..
            }
            | ControllerState::MaintenanceExclusive {
                connection: active, ..
            } => active == connection,
            ControllerState::NoController => false,
        };
        let removable = self
            .connections
            .get(&connection)
            .is_some_and(|active| active.closed)
            && !is_controller
            && !self.terminal_draining_sent.contains_key(&connection)
            && !self
                .queued_events
                .iter()
                .any(|queued| queued.connection == connection)
            && self
                .closing_event_scope
                .is_none_or(|scope| scope.connection != connection);
        if removable {
            self.connections.remove(&connection);
        }
    }

    pub(super) fn sweep_closed_connections(&mut self) {
        let candidates = self
            .connections
            .iter()
            .filter_map(|(connection, active)| active.closed.then_some(*connection))
            .collect::<Vec<_>>();
        for connection in candidates {
            self.reap_closed_connection(connection);
        }
    }

    pub(super) fn capture_authority(&self, connection: ConnectionId) -> Option<CapabilityRef> {
        match self.state.controller() {
            ControllerState::CaptureLeaseDisabled {
                connection: active,
                authority,
            }
            | ControllerState::CaptureLeaseEnabled {
                connection: active,
                authority,
            } if active == connection => Some(authority),
            _ => None,
        }
    }

    pub(super) fn maintenance_authority(&self, connection: ConnectionId) -> Option<CapabilityRef> {
        match self.state.controller() {
            ControllerState::MaintenanceExclusive {
                connection: active,
                authority,
            } if active == connection => Some(authority),
            _ => None,
        }
    }

    pub(super) fn current_capture(&self) -> Result<(ConnectionId, CapabilityRef), ServerError> {
        match self.state.controller() {
            ControllerState::CaptureLeaseDisabled {
                connection,
                authority,
            }
            | ControllerState::CaptureLeaseEnabled {
                connection,
                authority,
            } => Ok((connection, authority)),
            _ => Err(ServerError::EventScope),
        }
    }

    pub(super) fn current_capture_for_event(
        &self,
        before_close_barrier: bool,
    ) -> Result<(ConnectionId, crate::state::CapabilityEpoch), ServerError> {
        if before_close_barrier {
            let scope = self.closing_event_scope.ok_or(ServerError::EventScope)?;
            if !matches!(
                self.state.admission(),
                AdmissionState::Closing | AdmissionState::Unknown
            ) {
                return Err(ServerError::EventScope);
            }
            return Ok((scope.connection, scope.authority.epoch()));
        }
        let (connection, authority) = self.current_capture()?;
        if self.state.admission() != AdmissionState::Open {
            return Err(ServerError::EventScope);
        }
        Ok((connection, authority.epoch()))
    }
}
