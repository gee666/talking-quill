use super::{
    HandleOutcome, PROTOCOL_VERSION, Server, delivery::success_outbound, messages::*,
    observability::PasteCounters,
};
use crate::{
    CriticalDelivery,
    gateway::{
        ActivationCaptureGate, CallbackGate, GatewayBackend, TerminalReason, TerminalSignal,
    },
};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const CALLBACK_QUIESCENCE_TIMEOUT: Duration = Duration::from_millis(250);

/// `initialize` params schema: `{ "protocolVersion": 10 }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: u16,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: u16,
    helper_version: &'static str,
    platform: &'static str,
    architecture: &'static str,
    hook_status: crate::gateway::HookStatus,
    permissions: crate::gateway::Permissions,
    keyboard_capture: KeyboardCaptureBuildState,
    keyboard_owner: crate::gateway::KeyboardOwnerSnapshot,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardCaptureBuildState {
    activation_available: bool,
    session_key_capture_available: bool,
    runtime_rollback_active: bool,
    build_disabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownResult {
    owner_disposition: crate::gateway::ShutdownOwnerDisposition,
}

impl<P: GatewayBackend> Server<P> {
    /// Creates an inert feature-free server whose critical receiver must accept
    /// complete paste batches and serialize each batch without interleaving
    /// ordinary output. Opening activation capture requires the explicit
    /// process/test-gate constructor.
    #[doc(hidden)]
    pub fn new(
        platform: P,
        outbound: Sender<Outbound>,
        critical_outbound: Sender<CriticalDelivery>,
        final_outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Self {
        Self::new_with_activation_capture_gate(
            platform,
            outbound,
            critical_outbound,
            final_outbound,
            gate,
            terminal,
            ActivationCaptureGate::default(),
        )
    }

    /// Creates a server with a process-lifetime activation rollback gate.
    /// A closed gate filters every enable request before it reaches native code.
    #[doc(hidden)]
    pub fn new_with_activation_capture_gate(
        platform: P,
        outbound: Sender<Outbound>,
        critical_outbound: Sender<CriticalDelivery>,
        final_outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        activation_capture_gate: ActivationCaptureGate,
    ) -> Self {
        Self {
            platform,
            outbound,
            critical_outbound,
            final_outbound,
            gate,
            terminal,
            activation_capture_gate,
            initialized: false,
            platform_stopped: false,
            final_response_sent: false,
            activation_enable_requests_blocked: 0,
            session_capture_requests_blocked: 0,
            paste_counters: PasteCounters::default(),
            terminal_observability_collected: false,
            terminal_observability_authoritative: true,
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

    pub(crate) fn complete_shutdown(&mut self, id: RequestId) -> bool {
        if self.final_response_sent {
            return false;
        }
        self.quiesce_platform();
        if self.terminal.is_triggered() {
            return false;
        }
        // The dedicated one-slot path is unavailable to ordinary producers.
        // Its writer-side contract drains the complete accepted ordinary
        // prefix before writing this response exactly once and closing stdout.
        self.final_response_sent = true;
        self.send_final(success_outbound(
            id,
            ShutdownResult {
                owner_disposition: self.platform.shutdown_owner_disposition(),
            },
        ))
    }

    pub fn shutdown(&mut self) {
        self.quiesce_platform();
    }

    pub(super) fn quiesce_platform(&mut self) {
        self.gate.close();
        self.stop_platform();
        if !self
            .gate
            .wait_for_delivery_quiescence(CALLBACK_QUIESCENCE_TIMEOUT)
        {
            self.terminal_observability_authoritative = false;
            if !self.terminal.is_triggered() {
                self.terminal
                    .trigger(TerminalReason::OwnerThreadUnresponsive);
            }
        }
    }

    fn stop_platform(&mut self) {
        if !self.platform_stopped {
            let shutdown = self.platform.shutdown();
            self.terminal_observability_authoritative &= shutdown.observability_quiescent;
            if let Some(reason) = shutdown.terminal_reason {
                self.terminal.trigger(reason);
            }
            self.platform_stopped = true;
        }
    }

    pub(super) fn handle_initialize(&mut self, request: Request) -> bool {
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
            keyboard_capture: KeyboardCaptureBuildState {
                activation_available: self.activation_capture_gate.is_open()
                    && self.platform.keyboard_capture_available(),
                session_key_capture_available: self.activation_capture_gate.is_open()
                    && self.platform.keyboard_capture_available(),
                runtime_rollback_active: self.activation_capture_gate.runtime_rollback_active(),
                build_disabled: self.activation_capture_gate.development_disabled(),
            },
            keyboard_owner: self.platform.keyboard_owner(),
        };
        if !self.send_success(request.id, result) {
            return false;
        }
        self.initialized = true;
        self.gate.open();
        // GatewayBackend workers may retain pre-initialize state but must not publish
        // it before the initialize response has been queued.
        self.platform.protocol_initialized();
        if self.terminal.is_triggered() {
            self.gate.close();
            false
        } else {
            true
        }
    }
}
