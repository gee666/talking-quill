use super::{
    HandleOutcome, MaintenanceOperation, PrepareMaintenanceParams, Server, decode_sha256,
    delivery::success_outbound, hex, messages::*, observability::increment_counter, params::*,
};
use crate::gateway::GatewayBackend;
#[cfg(feature = "windows-installed-acceptance")]
use crate::gateway::PlatformError;
use talking_quill_keyboard_core::SessionCaptureMode;

impl<P: GatewayBackend> Server<P> {
    pub(crate) fn handle_payload_deferred(&mut self, payload: &[u8]) -> HandleOutcome {
        let request = match parse_request(payload) {
            ParseRequest::Request(request) => request,
            ParseRequest::IgnoreNotification => return HandleOutcome::Continue,
            ParseRequest::Error(response) => {
                return HandleOutcome::from_keep_running(self.send(Outbound::Response(response)));
            }
        };

        if !INBOUND_METHODS.contains(&request.method.as_str()) {
            return HandleOutcome::from_keep_running(
                self.send_error(request.id, RpcError::method_not_found()),
            );
        }

        if request.method == "initialize" {
            return HandleOutcome::from_keep_running(self.handle_initialize(request));
        }

        if !self.initialized {
            return HandleOutcome::from_keep_running(
                self.send_error(request.id, RpcError::invalid_state()),
            );
        }

        let keep_running = match request.method.as_str() {
            "activation.configure" => {
                let params = match self.params::<ConfigureActivationParams>(&request) {
                    Ok(params) => params,
                    Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                };
                if params.enabled && params.bindings.is_empty() {
                    return HandleOutcome::from_keep_running(
                        self.send_error(request.id, RpcError::invalid_params()),
                    );
                }
                let effective = ConfigureActivationParams {
                    enabled: self.activation_capture_gate.filter_enabled(params.enabled),
                    bindings: params.bindings,
                };
                if params.enabled && !effective.enabled {
                    increment_counter(&mut self.activation_enable_requests_blocked);
                }
                match self
                    .platform
                    .configure_activation(effective.enabled, effective.bindings)
                {
                    Ok(()) => self.send_success(request.id, effective),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "session.set_capture" => {
                let params = match self.params::<SetCaptureParams>(&request) {
                    Ok(params) => params,
                    Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                };
                let effective = self
                    .activation_capture_gate
                    .filter_session_mode(params.mode);
                if params.mode != SessionCaptureMode::Off && effective == SessionCaptureMode::Off {
                    increment_counter(&mut self.session_capture_requests_blocked);
                }
                match self.platform.set_session_capture(effective) {
                    Ok(()) => self.send_success(request.id, SetCaptureResult { mode: effective }),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "paste.inject" => {
                let (context, expected_clipboard_sha256) =
                    match self.params::<PasteInjectParams>(&request) {
                        Ok(params) => match params.into_context() {
                            Some(context) => context,
                            None => {
                                return HandleOutcome::from_keep_running(
                                    self.send_error(request.id, RpcError::invalid_params()),
                                );
                            }
                        },
                        Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                    };
                // Reserve the complete writer delivery before crossing the native boundary.
                // Queue pressure can reject a paste, but can never accept and then discard it.
                let reservation = match self.reserve_critical_delivery() {
                    Some(reservation) => reservation,
                    None => return HandleOutcome::Stop,
                };
                let result = self
                    .platform
                    .inject_paste_for_activation_with_clipboard_hash(
                        context,
                        expected_clipboard_sha256,
                    );
                self.paste_counters.record(result);
                // Before native submission, terminal state invalidates the
                // reservation. After submission, the reserved commit/success
                // batch is authoritative and must survive later cleanup
                // terminalization so the host cannot duplicate insertion.
                if self.terminal.is_triggered() && !result.submitted {
                    drop(reservation);
                    return HandleOutcome::Stop;
                }
                if result.submitted {
                    // Native submission is authoritative even if output became
                    // terminal concurrently. Always close session-key capture;
                    // otherwise restoration-phase Esc/Enter could remain owned
                    // after Electron can no longer issue a cleanup request.
                    let _ = self.platform.set_session_capture(SessionCaptureMode::Off);
                }
                let mut batch = Vec::with_capacity(2);
                if result.submitted {
                    batch.push(Outbound::PasteCommitted(request.id.clone()));
                }
                batch.push(success_outbound(request.id, result));
                self.complete_critical_delivery(reservation, batch)
            }
            "front_app.get" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                match self.platform.front_app() {
                    Ok(front_app) => self.send_success(request.id, front_app.bounded()),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "permissions.get" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                self.send_success(request.id, self.platform.permissions())
            }
            "runtime.observability" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                self.send_success(request.id, self.runtime_observability_result())
            }
            #[cfg(feature = "windows-installed-acceptance")]
            "acceptance.endpoint_observability" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                match self.platform.acceptance_endpoint_observability() {
                    Some(observability) => self.send_success(request.id, observability),
                    None => self.send_platform_error(request.id, PlatformError::OwnerUnavailable),
                }
            }
            #[cfg(feature = "windows-installed-acceptance")]
            "acceptance.pause_lease_renewal" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                match self.platform.acceptance_pause_lease_renewal() {
                    Ok(observability) => self.send_success(request.id, observability),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "ping" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                self.send_success(
                    request.id,
                    PingResult {
                        ok: true,
                        hook_status: self.platform.hook_status(),
                        keyboard_owner: self.platform.keyboard_owner(),
                    },
                )
            }
            "diagnostic.ack" => {
                let params =
                    match self.params::<crate::diagnostic_transport::DiagnosticAck>(&request) {
                        Ok(params) => params,
                        Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                    };
                self.send_success(
                    request.id,
                    serde_json::json!({
                        "acknowledged": crate::diagnostic_transport::acknowledge(&params).is_ok(),
                    }),
                )
            }
            "owner.prepare_maintenance" => {
                let params = match self.params::<PrepareMaintenanceParams>(&request) {
                    Ok(params) if params.valid() => params,
                    Ok(_) => {
                        return HandleOutcome::from_keep_running(
                            self.send_error(request.id, RpcError::invalid_params()),
                        );
                    }
                    Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                };
                let target_owner_sha256 = params
                    .target_owner_sha256
                    .as_deref()
                    .map(decode_sha256)
                    .transpose()
                    .expect("validated hash");
                let operation = match params.operation {
                    MaintenanceOperation::Update => crate::gateway::MaintenanceOperation::Update,
                    MaintenanceOperation::Uninstall => {
                        crate::gateway::MaintenanceOperation::Uninstall
                    }
                    MaintenanceOperation::Rollback => {
                        crate::gateway::MaintenanceOperation::Rollback
                    }
                };
                let maintenance = crate::gateway::MaintenanceRequest {
                    operation,
                    transaction_id: params.transaction_id,
                    source_build_id: params.source_build_id,
                    target_build_id: params.target_build_id,
                    target_owner_sha256,
                };
                match self.platform.prepare_maintenance(maintenance) {
                    Ok(owner_handoff) => self.send_success(
                        request.id,
                        serde_json::json!({
                            "maintenanceReady": true,
                            "ownerHandoff": hex(&owner_handoff),
                        }),
                    ),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "shutdown" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                // Closing the lease gate precedes native stop. The final
                // response is enqueued only after the platform and every
                // already-admitted callback delivery are quiescent.
                self.gate.close();
                return HandleOutcome::Shutdown(request.id);
            }
            "initialize" => unreachable!("initialize handled above"),
            _ => unreachable!("method allowlist checked above"),
        };
        HandleOutcome::from_keep_running(keep_running)
    }
}
