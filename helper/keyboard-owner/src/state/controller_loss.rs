//! Controller disconnects, native observations, and predecessor terminal events.
use super::*;

impl KeyboardOwnerState {
    pub fn controller_lost(
        &mut self,
        connection: ConnectionId,
        reason: ControllerLossReason,
    ) -> Result<Transition, TransitionError> {
        if self.controller.connection() != Some(connection) {
            match self.maintenance {
                MaintenanceState::Sealing {
                    request,
                    requester: Some(requester),
                    reserved_capability,
                } if requester == connection => {
                    self.maintenance = MaintenanceState::Sealing {
                        request,
                        requester: None,
                        reserved_capability,
                    };
                    return Ok(Transition::empty());
                }
                MaintenanceState::Persisting {
                    request,
                    requester: Some(requester),
                    reserved_capability,
                } if requester == connection => {
                    self.maintenance = MaintenanceState::Persisting {
                        request,
                        requester: None,
                        reserved_capability,
                    };
                    return Ok(Transition::empty());
                }
                _ => return Err(TransitionError::new(TransitionErrorKind::WrongController)),
            }
        }
        let terminal_reason = match reason {
            ControllerLossReason::Eof => TerminalReason::Eof,
            ControllerLossReason::HeartbeatExpired => TerminalReason::Heartbeat,
            ControllerLossReason::MacFault | ControllerLossReason::ProtocolFault => {
                TerminalReason::Protocol
            }
        };
        self.lose_active_controller(terminal_reason)
    }

    pub fn controller_disconnected(
        &mut self,
        connection: ConnectionId,
    ) -> Result<Transition, TransitionError> {
        self.controller_lost(connection, ControllerLossReason::Eof)
    }

    /// Owner-local semantic writer accounting. Native adapters cannot mutate
    /// this count. Increments are legal only while callback admission is open
    /// or while a closing barrier is active (including uncertain close retry);
    /// decrements never create native drain work.
    pub(crate) fn set_broker_admitted_effects(
        &mut self,
        admitted_effects: u8,
    ) -> Result<Transition, TransitionError> {
        if usize::from(admitted_effects) > OWNER_ADMITTED_EFFECT_CAPACITY
            || (admitted_effects > self.ownership.admitted_effects
                && !matches!(
                    self.admission,
                    AdmissionState::Open | AdmissionState::Closing | AdmissionState::Unknown
                ))
        {
            return Err(self.native_confirmation_fault());
        }
        self.ownership.admitted_effects = admitted_effects;
        let mut transition = Transition::empty();
        if admitted_effects == 0 {
            self.maybe_begin_degraded_exit(&mut transition.actions)?;
        }
        Ok(transition)
    }

    pub fn observe_native_observation(
        &mut self,
        observation: NativeOwnershipObservation,
    ) -> Result<Transition, TransitionError> {
        let bounded = u8::try_from(observation.activation_drain_keys)
            .ok()
            .zip(u8::try_from(observation.session_drain_keys).ok())
            .zip(u8::try_from(observation.replay_cleanup_edges).ok())
            .zip(u8::try_from(observation.admitted_effects).ok())
            .and_then(|(((activation, session), cleanup), effects)| {
                NativeOwnership::new(
                    observation.candidate,
                    activation,
                    session,
                    cleanup,
                    observation.paste,
                    effects,
                )
                .map(|ownership| {
                    ownership.with_conservative_native_work(observation.conservative_native_work)
                })
                .ok()
            });
        if let Some(ownership) = bounded {
            return self.observe_native_ownership(ownership);
        }

        self.impossible_native_observation = Some(observation);
        self.ownership = NativeOwnership {
            candidate: observation.candidate,
            activation_drain_keys: observation
                .activation_drain_keys
                .min(ACTIVATION_KEY_CAPACITY as u16) as u8,
            session_drain_keys: observation
                .session_drain_keys
                .min(SESSION_KEY_CAPACITY as u16) as u8,
            replay_cleanup_edges: observation
                .replay_cleanup_edges
                .min(REPLAY_CLEANUP_EDGE_CAPACITY as u16) as u8,
            paste: observation.paste,
            conservative_native_work: observation.conservative_native_work,
            admitted_effects: observation
                .admitted_effects
                .min(OWNER_ADMITTED_EFFECT_CAPACITY as u16) as u8,
        };
        self.degrade_unknown();
        let transition = self.request_close(ClosePlan::default())?;
        Err(TransitionError::with_transition(
            TransitionErrorKind::InvalidOwnershipTransition,
            transition,
        ))
    }

    pub fn observe_native_ownership(
        &mut self,
        ownership: NativeOwnership,
    ) -> Result<Transition, TransitionError> {
        let keyboard_active = matches!(
            self.admission,
            AdmissionState::Open | AdmissionState::Closing | AdmissionState::Unknown
        );
        let paste_matches = ownership.paste == self.ownership.paste;
        let legal = paste_matches
            && if keyboard_active {
                ownership.legal_while_keyboard_active(self.ownership)
            } else {
                ownership.legal_after_keyboard_closed(self.ownership)
            };
        if !legal {
            self.ownership = ownership;
            self.degrade_unknown();
            let transition = self.request_close(ClosePlan::default())?;
            return Err(TransitionError::with_transition(
                TransitionErrorKind::InvalidOwnershipTransition,
                transition,
            ));
        }
        self.ownership = ownership;
        let mut transition = Transition::empty();
        if !ownership.is_native_neutral()
            && !matches!(
                self.admission,
                AdmissionState::Closing | AdmissionState::Unknown
            )
        {
            transition.actions.push(RequiredAction::ContinueNativeDrain);
        }
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn latch_runtime_rollback(&mut self) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        self.apply_priority_rollback(Some(TerminalReason::Rollback))
    }

    /// The adapter event stream or broker-owned effect accounting lost
    /// contiguity. Native ownership can no longer be proved from aggregates,
    /// so neutral/exit remain forbidden even if the last bounded snapshot was
    /// zero.
    pub fn adapter_stream_desynchronized(&mut self) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        self.degrade_unknown();
        self.request_close(ClosePlan::default())
    }

    pub fn recoverable_native_fault(&mut self) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        self.process_health = ProcessHealth::Degraded;
        if !self.native_state_unknown {
            self.terminal_unavailable_reason = Some(TerminalUnavailableReason::NativeFault);
        }
        self.cancel_unclaimed_paste();
        let mut transition = self.request_close(ClosePlan::default())?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn request_idle_exit(&mut self) -> Result<Transition, TransitionError> {
        if !matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(TransitionErrorKind::MaintenanceSealed));
        }
        if !matches!(self.controller, ControllerInternal::NoController) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        if !self.quiescent_for_neutral() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        self.ensure_process_healthy()?;
        self.begin_native_stop(ExitPurpose::Idle)
    }

    pub fn maintenance_guard_lost(&mut self) -> Result<Transition, TransitionError> {
        if matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(
                TransitionErrorKind::MaintenanceTransactionMismatch,
            ));
        }
        if matches!(
            self.controller,
            ControllerInternal::MaintenanceExclusive { .. }
        ) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        if !self.quiescent_for_neutral() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        if matches!(self.exit, ExitState::NativeStoppedSealed) {
            self.exit = ExitState::Exiting;
            let mut transition = Transition::empty();
            transition.actions.push(RequiredAction::ExitOwner);
            return Ok(transition);
        }
        self.begin_native_stop(ExitPurpose::MaintenanceGuardLost)
    }

    pub fn offer_predecessor_terminal(
        &mut self,
        event: PredecessorTerminalEvent,
    ) -> Option<PredecessorTerminalOffer> {
        let event_is_truthful = match event {
            PredecessorTerminalEvent::LeaseRevoked(_) => true,
            PredecessorTerminalEvent::LeaseUnavailable(reason) => {
                self.terminal_unavailable_reason == Some(reason)
            }
            PredecessorTerminalEvent::LeaseDraining(ownership) => {
                self.terminal_unavailable_reason.is_none()
                    && self.terminal_ownership() == Some(ownership)
            }
            PredecessorTerminalEvent::LeaseNeutral => {
                self.terminal_unavailable_reason.is_none()
                    && self.native_quiescent_for_disposition()
            }
        };
        if !event_is_truthful {
            return None;
        }
        let predecessor = self.predecessor.as_mut()?;
        if predecessor.in_flight.is_some() || predecessor.final_offered {
            return None;
        }
        if matches!(event, PredecessorTerminalEvent::LeaseRevoked(_)) {
            if predecessor.revoked_offered {
                return None;
            }
            predecessor.revoked_offered = true;
        } else if !predecessor.revoked_offered {
            return None;
        }
        let sequence = predecessor
            .high_water
            .checked_add(1)
            .and_then(NonZeroU64::new)?;
        let offer = PredecessorTerminalOffer {
            route: predecessor.route,
            sequence,
            event,
        };
        predecessor.final_offered |= event.is_final();
        predecessor.in_flight = Some(offer);
        Some(offer)
    }

    pub fn confirm_predecessor_terminal_written(
        &mut self,
        offer: PredecessorTerminalOffer,
    ) -> Result<Transition, TransitionError> {
        let Some(predecessor) = &mut self.predecessor else {
            return Err(TransitionError::new(
                TransitionErrorKind::NativeConfirmationMismatch,
            ));
        };
        if predecessor.in_flight != Some(offer) {
            return Err(TransitionError::new(
                TransitionErrorKind::NativeConfirmationMismatch,
            ));
        }
        predecessor.high_water = offer.sequence.get();
        predecessor.in_flight = None;
        if offer.event.is_final() {
            self.predecessor = None;
        }
        Ok(Transition::empty())
    }

    pub fn fail_predecessor_terminal_write(
        &mut self,
        offer: PredecessorTerminalOffer,
    ) -> Result<Transition, TransitionError> {
        if self
            .predecessor
            .is_none_or(|predecessor| predecessor.in_flight != Some(offer))
        {
            return Err(TransitionError::new(
                TransitionErrorKind::NativeConfirmationMismatch,
            ));
        }
        self.predecessor = None;
        Ok(Transition::empty())
    }
}
