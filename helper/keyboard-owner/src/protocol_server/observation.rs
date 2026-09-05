//! Apply native ownership, paste completion, and capability deadlines.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    #[cfg(talking_quill_unoptimized_test_support)]
    pub(super) fn observe_native_ownership_inner(
        &mut self,
        ownership: NativeOwnership,
    ) -> Result<(), ServerError> {
        self.observe_native_observation_inner(NativeOwnershipObservation {
            candidate: ownership.candidate(),
            activation_drain_keys: u16::from(ownership.activation_drain_keys()),
            session_drain_keys: u16::from(ownership.session_drain_keys()),
            replay_cleanup_edges: u16::from(ownership.replay_cleanup_edges()),
            paste: ownership.paste(),
            conservative_native_work: ownership.conservative_native_work(),
            admitted_effects: u16::from(ownership.admitted_effects()),
        })
    }

    /// Applies a raw aggregate adapter observation without masking max+1
    /// values. Broker-admitted effects remain owner-authoritative; a mismatch
    /// closes fresh admission before the event is rejected.
    pub(super) fn observe_native_observation_inner(
        &mut self,
        observation: NativeOwnershipObservation,
    ) -> Result<(), ServerError> {
        if usize::from(observation.admitted_effects) != self.queued_events.len()
            || observation.admitted_effects != u16::from(self.state.ownership().admitted_effects())
        {
            let transition = self.state.adapter_stream_desynchronized();
            self.drive_external_transition(transition)?;
            return Err(ServerError::ExecutorContract);
        }
        let transition = self.state.observe_native_observation(observation);
        self.drive_external_transition(transition)?;
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    pub(super) fn observe_native_readiness_inner(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<(), ServerError> {
        let transition = self.state.observe_native_readiness(readiness);
        self.drive_external_transition(transition)
    }

    pub(super) fn observe_recoverable_native_fault_inner(&mut self) -> Result<(), ServerError> {
        let transition = self.state.recoverable_native_fault();
        self.drive_external_transition(transition)?;
        if self.current_capture().is_ok() {
            self.publish_terminal_degraded_inner()?;
        }
        Ok(())
    }

    pub(super) fn confirm_paste_claimed_inner(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
        {
            return Err(ServerError::EventScope);
        }
        let transition = self.state.confirm_paste_claimed(authorization)?;
        self.drive_transition(transition, None).map(|_| ())
    }

    pub(super) fn publish_paste_committed_inner(
        &mut self,
        authorization: PasteAuthorization,
        indeterminate: bool,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
        {
            return Err(ServerError::EventScope);
        }
        let notification_route = self
            .current_capture()
            .ok()
            .filter(|(_, authority)| authority.epoch() == authorization.capture_epoch());
        let transition = if indeterminate {
            self.state.confirm_paste_indeterminate(authorization)?
        } else {
            self.state.confirm_paste_completed(authorization)?
        };
        self.drive_transition(transition, None)?;
        if !indeterminate {
            self.active_paste = None;
        }
        // Native completion is authoritative even when capture authority was
        // already revoked or its notification route fails. Never ask the
        // adapter to retry a claimed one-shot paste.
        if let Some((connection, authority)) = notification_route {
            let _ = self.send_direct_event(
                connection,
                &Event::PasteCommitted(PasteCommittedEvent {
                    capture_lease_epoch: wire_u64(authority.epoch().get()),
                    operation_id: Bytes32::new(*authorization.operation().as_bytes()),
                    state: if indeterminate {
                        PasteCommitState::Indeterminate
                    } else {
                        PasteCommitState::Committed
                    },
                }),
            );
        }
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    pub(super) fn confirm_paste_completed_inner(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        if self
            .active_paste
            .is_none_or(|active| active.authorization != authorization)
            || self.state.ownership().paste() != PasteOwnership::Indeterminate
        {
            return Err(ServerError::EventScope);
        }
        let transition = self.state.confirm_paste_completed(authorization)?;
        self.drive_transition(transition, None)?;
        self.active_paste = None;
        self.advance_predecessor_terminal()?;
        Ok(())
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_ownership(
        &mut self,
        ownership: NativeOwnership,
    ) -> Result<(), ServerError> {
        self.observe_native_ownership_inner(ownership)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_observation(
        &mut self,
        observation: NativeOwnershipObservation,
    ) -> Result<(), ServerError> {
        self.observe_native_observation_inner(observation)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_native_readiness(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<(), ServerError> {
        self.observe_native_readiness_inner(readiness)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn observe_recoverable_native_fault(&mut self) -> Result<(), ServerError> {
        self.observe_recoverable_native_fault_inner()
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn confirm_paste_claimed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        self.confirm_paste_claimed_inner(authorization)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn publish_paste_committed(
        &mut self,
        authorization: PasteAuthorization,
        indeterminate: bool,
    ) -> Result<(), ServerError> {
        self.publish_paste_committed_inner(authorization, indeterminate)
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn confirm_paste_completed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<(), ServerError> {
        self.confirm_paste_completed_inner(authorization)
    }

    pub fn expire_connection(&mut self, connection: ConnectionId) -> Result<(), ServerError> {
        self.lease_expired = self.lease_expired.saturating_add(1);
        record_starvation_evidence("lease_expired");
        let result = self.teardown_connection(connection, ControllerLossReason::HeartbeatExpired);
        record_starvation_evidence(if result.is_ok() {
            "lease_expiry_transport_close"
        } else {
            "lease_expiry_close_failed"
        });
        result
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn set_heartbeat_timeout_for_test(&mut self, timeout: Duration) {
        self.heartbeat_timeout = timeout;
    }

    pub(super) fn renew_capability_deadline(
        &mut self,
        connection: ConnectionId,
    ) -> Result<(), DispatchError> {
        record_starvation_evidence("lease_deadline_renewed");
        let deadline = Instant::now()
            .checked_add(self.heartbeat_timeout)
            .ok_or(DispatchError::Fatal)?;
        self.connections
            .get_mut(&connection)
            .ok_or(DispatchError::Fatal)?
            .capability_deadline = Some(deadline);
        Ok(())
    }
}
