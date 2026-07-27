use std::{sync::Arc, time::Duration};

use crossbeam_channel::{Sender, bounded};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{PROTOCOL_VERSION, messages::*};
use crate::{
    CriticalDelivery,
    keyboard::ActivationBindings,
    platform::{CallbackGate, Platform, PlatformError, TerminalReason, TerminalSignal},
};

/// `initialize` params schema: `{ "protocolVersion": 5 }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: u16,
}

/// Protocol-v5 `activation.configure` params schema.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigureActivationParams {
    enabled: bool,
    bindings: ActivationBindings,
}

/// `session.set_capture` params schema: `{ "active": boolean }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCaptureParams {
    active: bool,
}

/// Params schema for parameterless methods. Params are still required and must
/// be exactly `{}` so misspelled or future fields fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: u16,
    helper_version: &'static str,
    platform: &'static str,
    architecture: &'static str,
    hook_status: crate::platform::HookStatus,
    permissions: crate::platform::Permissions,
}

#[derive(Debug, Serialize)]
struct SetCaptureResult {
    active: bool,
}

#[derive(Debug, Serialize)]
struct PingResult {
    ok: bool,
    #[serde(rename = "hookStatus")]
    hook_status: crate::platform::HookStatus,
}

#[derive(Debug, Serialize)]
struct EmptyResult {}

pub(crate) enum HandleOutcome {
    Continue,
    Shutdown(RequestId),
    Stop,
}

impl HandleOutcome {
    const fn from_keep_running(keep_running: bool) -> Self {
        if keep_running {
            Self::Continue
        } else {
            Self::Stop
        }
    }
}

const CRITICAL_ACQUISITION_TIMEOUT: Duration = Duration::from_millis(250);

pub struct Server<P: Platform> {
    platform: P,
    outbound: Sender<Outbound>,
    critical_outbound: Sender<CriticalDelivery>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    initialized: bool,
    platform_stopped: bool,
}

impl<P: Platform> Server<P> {
    /// Creates a server whose critical receiver must accept complete paste
    /// batches and serialize each batch without interleaving ordinary output.
    #[doc(hidden)]
    pub fn new(
        platform: P,
        outbound: Sender<Outbound>,
        critical_outbound: Sender<CriticalDelivery>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Self {
        Self {
            platform,
            outbound,
            critical_outbound,
            gate,
            terminal,
            initialized: false,
            platform_stopped: false,
        }
    }

    /// Handles one already-bounded frame. Returns false after an accepted,
    /// fully quiesced shutdown or when the stdout queue has disconnected.
    pub fn handle_payload(&mut self, payload: &[u8]) -> bool {
        match self.handle_payload_deferred(payload) {
            HandleOutcome::Continue => true,
            HandleOutcome::Shutdown(id) => {
                let _ = self.complete_shutdown(id);
                false
            }
            HandleOutcome::Stop => false,
        }
    }

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
                match self
                    .platform
                    .configure_activation(params.enabled, params.bindings)
                {
                    Ok(()) => self.send_success(request.id, params),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "session.set_capture" => {
                let params = match self.params::<SetCaptureParams>(&request) {
                    Ok(params) => params,
                    Err(keep_running) => return HandleOutcome::from_keep_running(keep_running),
                };
                match self.platform.set_session_capture(params.active) {
                    Ok(()) => self.send_success(
                        request.id,
                        SetCaptureResult {
                            active: params.active,
                        },
                    ),
                    Err(error) => self.send_platform_error(request.id, error),
                }
            }
            "paste.inject" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                // Reserve the complete writer delivery before crossing the native boundary.
                // Queue pressure can reject a paste, but can never accept and then discard it.
                let reservation = match self.reserve_paste_delivery() {
                    Some(reservation) => reservation,
                    None => return HandleOutcome::Stop,
                };
                let result = self.platform.inject_paste();
                if result.submitted {
                    // Stop session-key capture before the success response can reach Electron so
                    // restoration-phase Esc/Enter pass through.
                    let _ = self.platform.set_session_capture(false);
                }
                let mut batch = Vec::with_capacity(2);
                if result.submitted {
                    batch.push(Outbound::PasteCommitted(request.id.clone()));
                }
                batch.push(success_outbound(request.id, result));
                self.complete_paste_delivery(reservation, batch)
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
            "ping" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                self.send_success(
                    request.id,
                    PingResult {
                        ok: true,
                        hook_status: self.platform.hook_status(),
                    },
                )
            }
            "shutdown" => {
                if let Err(keep_running) = self.empty_params(&request) {
                    return HandleOutcome::from_keep_running(keep_running);
                }
                // Stop new callback work immediately. The coordinator then
                // joins the native hook before the response is enqueued.
                self.gate.close();
                return HandleOutcome::Shutdown(request.id);
            }
            "initialize" => unreachable!("initialize handled above"),
            _ => unreachable!("method allowlist checked above"),
        };
        HandleOutcome::from_keep_running(keep_running)
    }

    pub(crate) fn complete_shutdown(&mut self, id: RequestId) -> bool {
        self.gate.close();
        let _ = self.platform.set_session_capture(false);
        self.stop_platform();
        if self.terminal.is_triggered() {
            return false;
        }
        self.send_success(id, EmptyResult {})
    }

    pub fn shutdown(&mut self) {
        self.gate.close();
        self.stop_platform();
    }

    fn stop_platform(&mut self) {
        if !self.platform_stopped {
            if let Some(reason) = self.platform.shutdown() {
                self.terminal.trigger(reason);
            }
            self.platform_stopped = true;
        }
    }

    fn handle_initialize(&mut self, request: Request) -> bool {
        if self.initialized {
            return self.send_error(request.id, RpcError::invalid_state());
        }
        let params = match self.params::<InitializeParams>(&request) {
            Ok(params) => params,
            Err(keep_running) => return keep_running,
        };
        if params.protocol_version != PROTOCOL_VERSION {
            return self.send_error(request.id, RpcError::incompatible_protocol());
        }

        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            helper_version: env!("CARGO_PKG_VERSION"),
            platform: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            hook_status: self.platform.hook_status(),
            permissions: self.platform.permissions(),
        };
        if !self.send_success(request.id, result) {
            return false;
        }
        self.initialized = true;
        self.gate.open();
        if self.terminal.is_triggered() {
            self.gate.close();
            false
        } else {
            true
        }
    }

    fn params<T: DeserializeOwned>(&self, request: &Request) -> Result<T, bool> {
        match serde_json::from_str(request.params.get()) {
            Ok(params) => Ok(params),
            Err(_) => Err(self.send_error(request.id.clone(), RpcError::invalid_params())),
        }
    }

    fn empty_params(&self, request: &Request) -> Result<(), bool> {
        self.params::<EmptyParams>(request).map(|_| ())
    }

    fn send_success<T: Serialize>(&self, id: RequestId, value: T) -> bool {
        self.send(success_outbound(id, value))
    }

    fn send_platform_error(&self, id: RequestId, _error: PlatformError) -> bool {
        self.send_error(id, RpcError::native_unavailable())
    }

    fn send_error(&self, id: RequestId, error: RpcError) -> bool {
        self.send(Outbound::Response(RpcResponse::error(Some(id), error)))
    }

    fn reserve_paste_delivery(&self) -> Option<Sender<Vec<Outbound>>> {
        let (acquired_tx, acquired_rx) = bounded(1);
        let (completion_tx, completion_rx) = bounded(1);
        if self
            .critical_outbound
            .try_send(CriticalDelivery::new(acquired_tx, completion_rx))
            .is_err()
            || acquired_rx
                .recv_timeout(CRITICAL_ACQUISITION_TIMEOUT)
                .is_err()
        {
            self.terminal
                .trigger(TerminalReason::OutboundQueueUnavailable);
            return None;
        }
        if self.terminal.is_triggered() {
            return None;
        }
        Some(completion_tx)
    }

    fn complete_paste_delivery(
        &self,
        completion: Sender<Vec<Outbound>>,
        batch: Vec<Outbound>,
    ) -> bool {
        if completion.send(batch).is_ok() {
            true
        } else {
            self.terminal.trigger(TerminalReason::StdoutDisconnected);
            false
        }
    }

    fn send(&self, outbound: Outbound) -> bool {
        self.send_to(&self.outbound, outbound)
    }

    fn send_to(&self, sender: &Sender<Outbound>, outbound: Outbound) -> bool {
        // Final guard for every server-produced message. Oversized success
        // results have already been replaced; never recurse if even a fallback
        // or future message violates the bound.
        if encode_outbound(&outbound).is_err() {
            self.terminal
                .trigger(TerminalReason::OutboundEncodingUnavailable);
            return false;
        }
        if sender.try_send(outbound).is_ok() {
            true
        } else {
            self.terminal
                .trigger(TerminalReason::OutboundQueueUnavailable);
            false
        }
    }
}

fn success_outbound<T: Serialize>(id: RequestId, value: T) -> Outbound {
    let response_id = id.clone();
    match RpcResponse::success(id, value) {
        Ok(response) => {
            let outbound = Outbound::Response(response);
            match encode_outbound(&outbound) {
                Ok(_) => outbound,
                Err(OutboundEncodingError::FrameTooLarge(_)) => Outbound::Response(
                    RpcResponse::error(Some(response_id), RpcError::response_too_large()),
                ),
                Err(OutboundEncodingError::Serialization(_)) => {
                    Outbound::Response(RpcResponse::error(None, RpcError::internal_error()))
                }
            }
        }
        Err(_) => Outbound::Response(RpcResponse::error(None, RpcError::internal_error())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::MAX_FRAME_BYTES;

    #[test]
    fn oversized_success_becomes_stable_bounded_error_without_recursion() {
        let outbound = success_outbound(RequestId::for_test(7), "\u{0001}".repeat(MAX_FRAME_BYTES));
        let payload = encode_outbound(&outbound).unwrap();
        assert!(payload.len() < MAX_FRAME_BYTES);
        let response: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32_004);
        assert_eq!(response["error"]["message"], "Response too large");
        assert!(response.get("result").is_none());
    }
}
