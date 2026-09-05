//! Validate and dispatch authenticated protocol requests.
use super::*;

impl<'a, E: OwnerExecutor> OwnerProtocolServer<'a, E> {
    pub(super) fn dispatch(
        &mut self,
        connection: ConnectionId,
        received: &ReceivedRequest,
    ) -> Result<DispatchResult, DispatchError> {
        let request = received.request();
        let mut final_flush = None;
        let response = match request {
            Request::LeaseAcquire(_) => {
                let capability = self
                    .executor
                    .allocate_capability_id()
                    .ok_or(DispatchError::Semantic(ErrorCode::Unavailable))?;
                let authenticated = self.state.authenticate_observer(connection);
                self.apply_state(authenticated, None)?;
                if let Err(error) = self.state.acquire_capture_lease(connection, capability) {
                    let _ = self
                        .state
                        .controller_lost(connection, ControllerLossReason::Eof);
                    return Err(self.handle_transition_error(error, None));
                }
                let authority = self
                    .capture_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                self.active_bindings = None;
                self.active_activation = None;
                self.active_session_keys = 0;
                self.active_session_mode = None;
                self.active_paste = None;
                let sequence = CapabilitySequenceValidator::new(
                    CapabilityKind::Capture,
                    Bytes32::new(*authority.id().as_bytes()),
                    authority.epoch().get(),
                )
                .map_err(|_| DispatchError::Fatal)?;
                self.connections
                    .get_mut(&connection)
                    .ok_or(DispatchError::Fatal)?
                    .capture_sequence = Some(sequence);
                self.renew_capability_deadline(connection)?;
                self.lease_acquired = self.lease_acquired.saturating_add(1);
                Response::Success(SuccessResult::LeaseAcquire(LeaseAcquireResult {
                    capture_lease_id: Bytes32::new(*authority.id().as_bytes()),
                    capture_lease_epoch: wire_u64(authority.epoch().get()),
                    state: AcquireState::Disabled,
                }))
            }
            Request::MaintenanceAcquire(params) => {
                let capability = self
                    .executor
                    .allocate_capability_id()
                    .ok_or(DispatchError::Semantic(ErrorCode::Unavailable))?;
                let maintenance = maintenance_request(params).ok_or(DispatchError::Fatal)?;
                if self
                    .maintenance_request
                    .is_some_and(|existing| existing != maintenance)
                {
                    return Err(DispatchError::Semantic(ErrorCode::InvalidState));
                }
                let transition =
                    self.state
                        .acquire_maintenance(connection, capability, maintenance);
                let summary = self.apply_state(transition, None)?;
                if summary.response_stage != Some(ResponseStage::MaintenanceAcquireReady) {
                    return Err(DispatchError::Semantic(
                        if summary.native_failure.is_some() {
                            ErrorCode::NativeFailure
                        } else {
                            ErrorCode::Draining
                        },
                    ));
                }
                self.maintenance_request = Some(maintenance);
                let authority = self
                    .maintenance_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                let sequence = CapabilitySequenceValidator::new(
                    CapabilityKind::Maintenance,
                    Bytes32::new(*authority.id().as_bytes()),
                    authority.epoch().get(),
                )
                .map_err(|_| DispatchError::Fatal)?;
                self.connections
                    .get_mut(&connection)
                    .ok_or(DispatchError::Fatal)?
                    .maintenance_sequence = Some(sequence);
                self.renew_capability_deadline(connection)?;
                Response::Success(SuccessResult::MaintenanceAcquire(
                    MaintenanceAcquireResult {
                        maintenance_capability_id: Bytes32::new(*authority.id().as_bytes()),
                        maintenance_capability_epoch: wire_u64(authority.epoch().get()),
                        state: if self.state.ownership().is_native_neutral() {
                            MaintenanceAcquireState::Sealed
                        } else {
                            MaintenanceAcquireState::Draining
                        },
                    },
                ))
            }
            Request::HealthGet(_) => Response::Success(SuccessResult::Health(self.health())),
            Request::PermissionsGet(_) => {
                Response::Success(SuccessResult::Permissions(self.executor.permissions()))
            }
            Request::ObservabilityGet(_) => {
                let mut observability = self.executor.observability();
                let negotiated = self.connections.get(&connection).is_some_and(|active| {
                    active.codec.supports_feature(
                        talking_quill_owner_protocol::REGISTERED_INPUT_OBSERVABILITY_V1,
                    )
                });
                if !negotiated {
                    observability.registered_input = None;
                }
                observability.owner.lease_acquired = wire_counter(self.lease_acquired);
                observability.owner.lease_renewed = wire_counter(self.lease_renewed);
                observability.owner.lease_expired = wire_counter(self.lease_expired);
                observability.owner.lease_disconnected = wire_counter(self.lease_disconnected);
                observability.owner.lease_released_neutral =
                    wire_counter(self.lease_released_neutral);
                observability.owner.lease_released_draining =
                    wire_counter(self.lease_released_draining);
                if let Some(registered) = observability.registered_input.as_mut() {
                    registered.owner_admitted = wire_counter(self.registered_owner_admitted);
                    registered.owner_flushed = wire_counter(self.registered_owner_flushed);
                    registered.owner_rejected = wire_counter(self.registered_owner_rejected);
                }
                Response::Success(SuccessResult::Observability(Box::new(observability)))
            }
            Request::FrontAppGet(_) => {
                Response::Success(SuccessResult::FrontApp(self.executor.front_app()))
            }
            Request::FrontAppMetadataGet(_) => Response::Success(SuccessResult::FrontAppMetadata(
                self.executor.front_app_metadata(),
            )),
            Request::LeaseRenew(params) => {
                self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Renew,
                    None,
                )?;
                self.renew_capability_deadline(connection)?;
                self.lease_renewed = self.lease_renewed.saturating_add(1);
                Response::Success(SuccessResult::Renew(RenewResult { renewed: true }))
            }
            Request::SessionReconcileOff(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::ReconcileSessionOff,
                    None,
                )?;
                require_native_success(summary)?;
                self.active_session_mode = Some(SessionCaptureMode::Off);
                Response::Success(SuccessResult::SessionMode(SessionModeResult {
                    mode: SessionMode::Off,
                }))
            }
            Request::SessionSetMode(params) => {
                let mode = state_session_mode(params.mode);
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::SetSessionMode(mode),
                    None,
                )?;
                require_native_success(summary)?;
                self.active_session_mode = Some(mode);
                Response::Success(SuccessResult::SessionMode(SessionModeResult {
                    mode: params.mode,
                }))
            }
            Request::CaptureReplaceConfiguration(params) => {
                let bindings = core_bindings(&params.bindings)
                    .map_err(|_| DispatchError::Semantic(ErrorCode::InvalidState))?;
                let revision = ConfigurationRevision::new(params.revision.get())
                    .ok_or(DispatchError::Fatal)?;
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::ReplaceConfiguration { revision, bindings },
                    None,
                )?;
                require_native_success(summary)?;
                self.active_bindings = Some(bindings);
                self.active_activation = None;
                self.active_session_keys = 0;
                Response::Success(SuccessResult::Configuration(ConfigurationResult {
                    revision: params.revision,
                }))
            }
            Request::CaptureSetEnabled(params) => {
                let command = if params.enabled {
                    crate::state::CaptureCommand::Enable
                } else {
                    crate::state::CaptureCommand::Disable
                };
                let summary =
                    self.apply_capture(connection, params.command_sequence.get(), command, None)?;
                require_native_success(summary)?;
                if !params.enabled {
                    self.active_activation = None;
                    self.active_session_keys = 0;
                }
                Response::Success(SuccessResult::Enabled(EnabledResult {
                    enabled: params.enabled,
                }))
            }
            Request::PasteInject(params) => {
                let authority = self
                    .capture_authority(connection)
                    .ok_or(DispatchError::Fatal)?;
                let operation = PasteOperationId::new(*params.operation_id.as_bytes())
                    .ok_or(DispatchError::Fatal)?;
                let owner_instance = OwnerInstanceId::new(*params.owner_instance_id.as_bytes())
                    .ok_or(DispatchError::Fatal)?;
                let generation = OwnerActivationGeneration::new(params.activation_generation.get())
                    .ok_or(DispatchError::Fatal)?;
                let authorization = PasteAuthorization::new(
                    operation,
                    owner_instance,
                    authority.epoch(),
                    generation,
                );
                let target_token = params
                    .target_token
                    .as_ref()
                    .map(|value| NativeTargetToken::new(value.as_str()))
                    .transpose()
                    .map_err(|_| DispatchError::Fatal)?;
                let context = PasteExecutorRequest {
                    authorization,
                    target_token,
                    fallback_text_sha256: params.fallback_text_sha256,
                };
                let summary = match self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::BeginPaste(authorization),
                    Some(context),
                ) {
                    Ok(summary) => summary,
                    Err(error) => {
                        if self.state.ownership().paste() == PasteOwnership::Indeterminate {
                            self.active_paste = Some(context);
                        }
                        return Err(error);
                    }
                };
                let result = if let Some(reason) = summary.paste_refusal {
                    PasteResult::ClipboardOnly {
                        reason: reason.wire_reason(),
                    }
                } else if summary.paste_waiting {
                    self.active_paste = Some(context);
                    PasteResult::Waiting {
                        operation_id: params.operation_id,
                    }
                } else if self.state.ownership().paste() == PasteOwnership::Indeterminate {
                    self.active_paste = Some(context);
                    PasteResult::Indeterminate {
                        operation_id: params.operation_id,
                    }
                } else if summary.paste_failure == Some(NativeActionFailure::FailedNotApplied) {
                    PasteResult::ClipboardOnly {
                        reason: PasteRefusalReason::NativeRejected,
                    }
                } else {
                    return Err(DispatchError::Semantic(ErrorCode::NativeFailure));
                };
                Response::Success(SuccessResult::Paste(result))
            }
            Request::LeaseRelease(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Release,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                match disposition {
                    LeaseDisposition::Neutral => {
                        self.lease_released_neutral = self.lease_released_neutral.saturating_add(1);
                    }
                    LeaseDisposition::Draining => {
                        self.lease_released_draining =
                            self.lease_released_draining.saturating_add(1);
                    }
                }
                self.planned_exit_when_neutral = false;
                Response::Success(SuccessResult::Release(ReleaseResult {
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::OwnerExitWhenNeutral(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::Release,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                match disposition {
                    LeaseDisposition::Neutral => {
                        self.lease_released_neutral = self.lease_released_neutral.saturating_add(1);
                    }
                    LeaseDisposition::Draining => {
                        self.lease_released_draining =
                            self.lease_released_draining.saturating_add(1);
                    }
                }
                self.planned_exit_when_neutral = true;
                self.planned_exit_terminal_pending = disposition == LeaseDisposition::Draining;
                Response::Success(SuccessResult::Release(ReleaseResult {
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::RuntimeRollback(params) => {
                let summary = self.apply_capture(
                    connection,
                    params.command_sequence.get(),
                    crate::state::CaptureCommand::RuntimeRollback,
                    None,
                )?;
                require_native_success(summary)?;
                let disposition = summary
                    .lease_disposition
                    .ok_or(DispatchError::Semantic(ErrorCode::InvalidState))?;
                Response::Success(SuccessResult::Rollback(RollbackResult {
                    latched: true,
                    disposition: wire_disposition(disposition),
                }))
            }
            Request::MaintenanceRenew(params) => {
                self.apply_maintenance(
                    connection,
                    params.command_sequence.get(),
                    MaintenanceCommand::Renew,
                    None,
                )?;
                self.renew_capability_deadline(connection)?;
                Response::Success(SuccessResult::Renew(RenewResult { renewed: true }))
            }
            Request::MaintenancePrepare(params) => {
                let expected = self.maintenance_request.ok_or(DispatchError::Fatal)?;
                if params.transaction_id.as_bytes() != expected.transaction().as_bytes()
                    || maintenance_operation(params.operation) != expected.operation()
                {
                    // B1 already consumed this authenticated capability
                    // sequence. Consume the same sequence in W1 without native
                    // work or lease-liveness renewal before rejecting it.
                    self.apply_maintenance(
                        connection,
                        params.command_sequence.get(),
                        MaintenanceCommand::ConsumeSemanticRejection,
                        None,
                    )?;
                    return Err(DispatchError::Semantic(ErrorCode::InvalidState));
                }
                let correlation = ResponseCorrelation::new(received.transport_sequence())
                    .ok_or(DispatchError::Fatal)?;
                let summary = self.apply_maintenance(
                    connection,
                    params.command_sequence.get(),
                    MaintenanceCommand::Prepare {
                        operation: maintenance_operation(params.operation),
                        response_correlation: correlation,
                    },
                    None,
                )?;
                if summary.response_stage != Some(ResponseStage::FinalResponseReady) {
                    return Err(DispatchError::Semantic(
                        if summary.native_failure.is_some() {
                            ErrorCode::NativeFailure
                        } else {
                            ErrorCode::Draining
                        },
                    ));
                }
                final_flush = Some(correlation);
                Response::Success(SuccessResult::MaintenancePrepare(
                    MaintenancePrepareResult {
                        ready_to_exit: true,
                        owner_handoff: Bytes32::new(*expected.owner_handoff().as_bytes()),
                    },
                ))
            }
        };
        Ok(DispatchResult {
            response,
            final_flush,
            planned_exit: matches!(request, Request::OwnerExitWhenNeutral(_)),
        })
    }
}
