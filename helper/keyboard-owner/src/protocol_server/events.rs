//! Admit keyboard, observation, audio, and health events.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    /// Admits a core keyboard event after binding, owner-instance, generation,
    /// and current capture scope have been derived and validated locally.
    pub(super) fn admit_keyboard_event_inner(
        &mut self,
        event: KeyboardEvent,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        let (connection, epoch) = self.current_capture_for_event(before_close_barrier)?;
        let registered_activation = matches!(
            event,
            KeyboardEvent::Activation { .. } | KeyboardEvent::ActivationComplete { .. }
        );
        let bindings = if before_close_barrier {
            self.closing_event_scope
                .and_then(|scope| scope.bindings)
                .ok_or(ServerError::EventScope)?
        } else {
            self.active_bindings.ok_or(ServerError::EventScope)?
        };
        let wire = match event {
            KeyboardEvent::Activation {
                binding,
                context,
                phase,
            } => {
                if !bindings.iter().any(|configured| configured == binding) {
                    return Err(ServerError::EventScope);
                }
                let generation = context.activation_generation().get();
                match phase {
                    EventPhase::Down
                        if self.active_activation.is_none()
                            && generation > self.activation_generation_high_water => {}
                    EventPhase::Up if self.active_activation == Some((binding, context)) => {}
                    _ => return Err(ServerError::EventScope),
                }
                Event::Activation(wire_activation_event(
                    self.state.owner_instance(),
                    epoch.get(),
                    binding,
                    context.target_token(),
                    generation,
                    phase,
                    None,
                )?)
            }
            KeyboardEvent::ActivationComplete {
                binding,
                context,
                held_ms,
            } => {
                let generation = context.activation_generation().get();
                if !bindings.iter().any(|configured| configured == binding)
                    || self.active_activation.is_some()
                    || generation <= self.activation_generation_high_water
                {
                    return Err(ServerError::EventScope);
                }
                Event::Activation(wire_activation_event(
                    self.state.owner_instance(),
                    epoch.get(),
                    binding,
                    context.target_token(),
                    generation,
                    EventPhase::Up,
                    Some(held_ms),
                )?)
            }
            KeyboardEvent::SessionKey { key, phase } => {
                let bit = match key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                let phase_valid = match phase {
                    EventPhase::Down => {
                        self.active_session_keys & bit == 0
                            && if before_close_barrier {
                                self.closing_event_scope
                                    .and_then(|scope| scope.session_mode)
                                    .is_some_and(|mode| mode.allows(key))
                            } else {
                                self.state
                                    .applied_session_mode()
                                    .is_some_and(|mode| mode.allows(key))
                            }
                    }
                    EventPhase::Up => self.active_session_keys & bit != 0,
                };
                if !phase_valid {
                    return Err(ServerError::EventScope);
                }
                Event::SessionKey(SessionKeyEvent {
                    capture_lease_epoch: wire_u64(epoch.get()),
                    key: match key {
                        SessionKey::Escape => WireSessionKey::Escape,
                        SessionKey::Enter => WireSessionKey::Enter,
                    },
                    phase: wire_phase(phase),
                })
            }
        };
        if let Err(error) = self.admit_capture_event(connection, wire) {
            if registered_activation {
                self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            }
            return Err(error);
        }
        if registered_activation {
            self.registered_owner_admitted = self.registered_owner_admitted.saturating_add(1);
        }
        match event {
            KeyboardEvent::Activation {
                binding,
                context,
                phase,
            } => {
                let generation = context.activation_generation().get();
                if phase == EventPhase::Down {
                    self.active_activation = Some((binding, context));
                    self.activation_generation_high_water = generation;
                } else {
                    self.active_activation = None;
                }
            }
            KeyboardEvent::ActivationComplete { context, .. } => {
                self.activation_generation_high_water = context.activation_generation().get();
            }
            KeyboardEvent::SessionKey { key, phase } => {
                let bit = match key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                match phase {
                    EventPhase::Down => self.active_session_keys |= bit,
                    EventPhase::Up => self.active_session_keys &= !bit,
                }
            }
        }
        Ok(())
    }

    pub(super) fn admit_registered_observation_inner(
        &mut self,
        generation: u64,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        // The shortcut tester deliberately closes suppression while retaining
        // its authenticated lease. Observations carry no input ownership and
        // must not require the capture admission gate to be open.
        if before_close_barrier {
            return Err(ServerError::EventScope);
        }
        let (connection, authority) = self.current_capture()?;
        let epoch = authority.epoch();
        let negotiated = self.connections.get(&connection).is_some_and(|active| {
            active
                .codec
                .supports_feature(talking_quill_owner_protocol::REGISTERED_INPUT_OBSERVABILITY_V1)
        });
        if !negotiated || generation == 0 || generation <= self.observation_generation_high_water {
            self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            return Err(ServerError::EventScope);
        }
        let wire = Event::RegisteredObservation(RegisteredObservationEvent {
            capture_lease_epoch: wire_u64(epoch.get()),
            generation: wire_u64(generation),
        });
        let frame = self
            .connections
            .get_mut(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .encode_event(&wire)?;
        if let Err(error) =
            self.enqueue_transport_frame(connection, frame, FlushCompletion::RegisteredObservation)
        {
            self.registered_owner_rejected = self.registered_owner_rejected.saturating_add(1);
            return Err(error);
        }
        self.observation_generation_high_water = generation;
        self.registered_owner_admitted = self.registered_owner_admitted.saturating_add(1);
        Ok(())
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn admit_keyboard_event(&mut self, event: KeyboardEvent) -> Result<(), ServerError> {
        self.admit_keyboard_event_inner(event, false)
    }

    pub(super) fn admit_audio_devices_changed_inner(
        &mut self,
        before_close_barrier: bool,
    ) -> Result<(), ServerError> {
        let (connection, epoch) = self.current_capture_for_event(before_close_barrier)?;
        self.admit_capture_event(
            connection,
            Event::AudioDevicesChanged(AudioDevicesChangedEvent {
                capture_lease_epoch: wire_u64(epoch.get()),
            }),
        )
    }

    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    pub fn admit_audio_devices_changed(&mut self) -> Result<(), ServerError> {
        self.admit_audio_devices_changed_inner(false)
    }

    pub(super) fn publish_terminal_degraded_inner(&mut self) -> Result<(), ServerError> {
        let (connection, _) = self.current_capture()?;
        let status = self.state.status();
        let reason = if status.native_state_unknown {
            TerminalDegradedReason::OwnershipUnknown
        } else if status.process_state == ProcessState::Degraded {
            TerminalDegradedReason::NativeFault
        } else {
            return Err(ServerError::EventScope);
        };
        self.send_direct_event(
            connection,
            &Event::TerminalDegraded(TerminalDegradedEvent { reason }),
        )
    }

    /// Constructs health from the authoritative W1 state rather than accepting
    /// caller-provided wire status.
    pub fn publish_health_changed(&mut self, connection: ConnectionId) -> Result<(), ServerError> {
        let purpose = self
            .connections
            .get(&connection)
            .ok_or(ServerError::UnknownConnection)?
            .codec
            .purpose();
        let current_route = match purpose {
            Purpose::Observe => true,
            Purpose::Capture => self.capture_authority(connection).is_some(),
            Purpose::Maintenance => self.maintenance_authority(connection).is_some(),
        };
        if !current_route || self.terminal_draining_sent.contains_key(&connection) {
            return Err(ServerError::EventScope);
        }
        self.send_direct_event(connection, &Event::HealthChanged(self.health()))
    }

    pub(super) fn admit_capture_event(
        &mut self,
        connection: ConnectionId,
        event: Event,
    ) -> Result<(), ServerError> {
        if self.queued_events.len() >= OWNER_ADMITTED_EFFECT_CAPACITY {
            // The ninth item is never admitted. Existing accepted effects cross
            // the writer boundary before fail-closed degradation proceeds.
            self.flush_admitted_events()?;
            let transition = self.state.recoverable_native_fault()?;
            self.drive_transition(transition, None)?;
            return Err(ServerError::EventCapacity);
        }
        let next = self
            .state
            .ownership()
            .admitted_effects()
            .checked_add(1)
            .ok_or(ServerError::EventCapacity)?;
        let transition = self.state.set_broker_admitted_effects(next)?;
        // Queue insertion and the authoritative admitted-effect count form one
        // local commit. Preserve the queue entry even if fail-closed follow-up
        // work fails so count and queue can never diverge.
        self.queued_events
            .push_back(QueuedEvent { connection, event });
        self.drive_transition(transition, None)?;
        self.ensure_effect_queue_consistent()?;
        Ok(())
    }

    pub fn flush_admitted_events(&mut self) -> Result<(), ServerError> {
        while let Some((connection, event)) = self
            .queued_events
            .front()
            .map(|queued| (queued.connection, queued.event.clone()))
        {
            if self
                .connections
                .get(&connection)
                .is_some_and(|active| !active.pending_flushes.is_empty())
            {
                return Ok(());
            }
            let send = self
                .connections
                .get_mut(&connection)
                .ok_or(ServerError::UnknownConnection)
                .and_then(|active| active.codec.encode_event(&event).map_err(Into::into))
                .and_then(|frame| {
                    self.enqueue_transport_frame(connection, frame, FlushCompletion::AdmittedEvent)
                });
            if send.is_err() {
                self.queued_events.clear();
                let _ = self.retire_all_admitted_effects();
                if self.adapter_event_in_flight.is_some() {
                    self.abort_connection(connection);
                    if !self
                        .deferred_controller_losses
                        .iter()
                        .any(|(active, _)| *active == connection)
                    {
                        self.deferred_controller_losses
                            .push_back((connection, ControllerLossReason::Eof));
                    }
                } else {
                    let _ = self.teardown_connection(connection, ControllerLossReason::Eof);
                }
                self.ensure_effect_queue_consistent()?;
                return Err(ServerError::EventDelivery);
            }
            if send? == TransportProgress::Pending {
                return Ok(());
            }
            self.ensure_effect_queue_consistent()?;
        }
        Ok(())
    }
}
