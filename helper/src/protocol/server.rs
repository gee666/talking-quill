use std::{sync::Arc, time::Duration};

use crossbeam_channel::{Sender, bounded};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{PROTOCOL_VERSION, messages::*};
use crate::{
    CriticalDelivery,
    gateway::{
        ActivationCaptureGate, CallbackGate, ClipboardTextHash, EffectOutcomeCounters,
        GatewayBackend, MAX_OBSERVABILITY_COUNTER, PasteFailure, PasteResult, PlatformError,
        TerminalReason, TerminalSignal, TransactionCounters,
    },
};
use talking_quill_keyboard_core::{
    ActivationBindings, ActivationContext, ActivationGeneration, NativeTargetToken,
    SessionCaptureMode,
};

/// `initialize` params schema: `{ "protocolVersion": 10 }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: u16,
}

/// Protocol-v10 `activation.configure` params schema.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigureActivationParams {
    enabled: bool,
    bindings: ActivationBindings,
}

/// `session.set_capture` params schema: `{ "mode": "off" | "recording" | "cancel-only" }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetCaptureParams {
    mode: SessionCaptureMode,
}

/// Protocol-v10 `paste.inject` params schema. The opaque target token is validated
/// and forwarded without interpretation or diagnostic exposure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PasteInjectParams {
    activation_generation: u64,
    target_token: Value,
    expected_clipboard_sha256: String,
}

impl PasteInjectParams {
    fn into_context(self) -> Option<(ActivationContext, ClipboardTextHash)> {
        let generation = ActivationGeneration::new(self.activation_generation)?;
        let expected_hash = ClipboardTextHash::from_lower_hex(&self.expected_clipboard_sha256)?;
        let context = ActivationContext::target_unavailable(generation);
        let context = match self.target_token {
            Value::Null => context,
            Value::String(token) => context.with_target_token(NativeTargetToken::new(&token).ok()?),
            _ => return None,
        };
        Some((context, expected_hash))
    }
}

/// Params schema for parameterless methods. Params are still required and must
/// be exactly `{}` so misspelled or future fields fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PrepareMaintenanceParams {
    operation: MaintenanceOperation,
    transaction_id: String,
    source_build_id: String,
    #[serde(default)]
    target_build_id: Option<String>,
    #[serde(default)]
    target_owner_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

impl PrepareMaintenanceParams {
    fn valid(&self) -> bool {
        #[cfg(not(target_os = "macos"))]
        let bounded = |value: &str| !value.is_empty() && value.len() <= 128;
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        };
        let canonical_id = |value: &str| {
            #[cfg(target_os = "macos")]
            {
                hash(value)
            }
            #[cfg(not(target_os = "macos"))]
            {
                bounded(value)
            }
        };
        canonical_id(&self.transaction_id)
            && canonical_id(&self.source_build_id)
            && match self.operation {
                MaintenanceOperation::Uninstall => {
                    self.target_build_id.is_none() && self.target_owner_sha256.is_none()
                }
                MaintenanceOperation::Update | MaintenanceOperation::Rollback => {
                    self.target_build_id.as_deref().is_some_and(canonical_id)
                        && self.target_owner_sha256.as_deref().is_some_and(hash)
                }
            }
    }
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
struct SetCaptureResult {
    mode: SessionCaptureMode,
}

#[derive(Debug, Serialize)]
struct PingResult {
    ok: bool,
    #[serde(rename = "hookStatus")]
    hook_status: crate::gateway::HookStatus,
    #[serde(rename = "keyboardOwner")]
    keyboard_owner: crate::gateway::KeyboardOwnerSnapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownResult {
    owner_disposition: crate::gateway::ShutdownOwnerDisposition,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardCaptureCounters {
    runtime_rollback_active: bool,
    development_disabled: bool,
    activation_enable_requests_blocked: u64,
    session_capture_requests_blocked: u64,
    shutdown_ownership_deadlines: u64,
    terminal_disablements: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteFailureCounters {
    permission_denied: u64,
    secure_input: u64,
    conflicting_modifiers: u64,
    os_rejected: u64,
    unavailable: u64,
    indeterminate: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteCounters {
    attempted: u64,
    submitted: u64,
    target_validation_fallback: u64,
    native_wait_duration_ms_total: u64,
    native_wait_duration_ms_max: u64,
    modifier_timeouts: u64,
    failures: PasteFailureCounters,
}

impl PasteCounters {
    fn record(&mut self, result: PasteResult) {
        increment_counter(&mut self.attempted);
        if result.submitted {
            increment_counter(&mut self.submitted);
            return;
        }
        match result.reason.unwrap_or(PasteFailure::Unavailable) {
            PasteFailure::PermissionDenied => {
                increment_counter(&mut self.failures.permission_denied)
            }
            PasteFailure::SecureInput => increment_counter(&mut self.failures.secure_input),
            PasteFailure::ConflictingModifiers => {
                increment_counter(&mut self.failures.conflicting_modifiers)
            }
            PasteFailure::OsRejected => increment_counter(&mut self.failures.os_rejected),
            PasteFailure::Unavailable => increment_counter(&mut self.failures.unavailable),
            PasteFailure::Indeterminate => increment_counter(&mut self.failures.indeterminate),
        }
    }

    fn merge_native(
        &mut self,
        target_fallbacks: u64,
        modifier_wait_duration_ms_total: u64,
        modifier_wait_duration_ms_max: u64,
        modifier_timeouts: u64,
    ) {
        self.target_validation_fallback = self
            .target_validation_fallback
            .saturating_add(target_fallbacks)
            .min(MAX_OBSERVABILITY_COUNTER);
        self.native_wait_duration_ms_total = self
            .native_wait_duration_ms_total
            .saturating_add(modifier_wait_duration_ms_total)
            .min(MAX_OBSERVABILITY_COUNTER);
        self.native_wait_duration_ms_max = self
            .native_wait_duration_ms_max
            .max(modifier_wait_duration_ms_max);
        self.modifier_timeouts = self
            .modifier_timeouts
            .saturating_add(modifier_timeouts)
            .min(MAX_OBSERVABILITY_COUNTER);
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeObservabilityResult {
    keyboard_capture: KeyboardCaptureCounters,
    keyboard_owner: crate::gateway::KeyboardOwnerSnapshot,
    owner: crate::gateway::OwnerObservabilitySnapshot,
    registered_input: crate::gateway::RegisteredInputCounters,
    transactions: TransactionCounters,
    replay: EffectOutcomeCounters,
    dummy: EffectOutcomeCounters,
    paste: PasteCounters,
}

fn decode_sha256(value: &str) -> Result<[u8; 32], ()> {
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let nibble = |value| match value {
            b'0'..=b'9' => Ok(value - b'0'),
            b'a'..=b'f' => Ok(value - b'a' + 10),
            _ => Err(()),
        };
        bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn increment_counter(counter: &mut u64) {
    *counter = counter.saturating_add(1).min(MAX_OBSERVABILITY_COUNTER);
}

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
const CALLBACK_QUIESCENCE_TIMEOUT: Duration = Duration::from_millis(250);

pub struct Server<P: GatewayBackend> {
    platform: P,
    outbound: Sender<Outbound>,
    critical_outbound: Sender<CriticalDelivery>,
    final_outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    activation_capture_gate: ActivationCaptureGate,
    initialized: bool,
    platform_stopped: bool,
    final_response_sent: bool,
    activation_enable_requests_blocked: u64,
    session_capture_requests_blocked: u64,
    paste_counters: PasteCounters,
    terminal_observability_collected: bool,
    terminal_observability_authoritative: bool,
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

    fn runtime_observability_result(&self) -> RuntimeObservabilityResult {
        let owner_snapshot = self.platform.runtime_owner_observability();
        let native = owner_snapshot.native;
        let mut paste = self.paste_counters;
        paste.merge_native(
            native.native_paste.target_validation_fallbacks,
            native.native_paste.modifier_wait_duration_ms_total,
            native.native_paste.modifier_wait_duration_ms_max,
            native.native_paste.modifier_timeouts,
        );
        RuntimeObservabilityResult {
            keyboard_owner: self.platform.keyboard_owner(),
            owner: owner_snapshot.owner,
            registered_input: native.registered_input,
            keyboard_capture: KeyboardCaptureCounters {
                runtime_rollback_active: self.activation_capture_gate.runtime_rollback_active(),
                development_disabled: self.activation_capture_gate.development_disabled(),
                activation_enable_requests_blocked: self.activation_enable_requests_blocked,
                session_capture_requests_blocked: self.session_capture_requests_blocked,
                shutdown_ownership_deadlines: native.native_paste.shutdown_ownership_deadlines,
                terminal_disablements: u64::from(self.terminal.is_triggered()),
            },
            transactions: native.transactions,
            replay: native.replay,
            dummy: native.dummy,
            paste,
        }
    }

    /// Collects one fixed aggregate after native teardown and every admitted
    /// callback delivery is quiescent. If native owner completion could not be
    /// proved, no final snapshot is returned because its counters could still
    /// change on a detached thread.
    #[doc(hidden)]
    pub fn take_terminal_observability(&mut self) -> Option<Value> {
        self.quiesce_platform();
        if self.terminal_observability_collected || !self.terminal_observability_authoritative {
            return None;
        }
        self.terminal_observability_collected = true;
        serde_json::to_value(self.runtime_observability_result()).ok()
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

    fn quiesce_platform(&mut self) {
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

    fn send_platform_error(&self, id: RequestId, error: PlatformError) -> bool {
        let error = match error {
            PlatformError::OwnerUnavailable | PlatformError::NativeFailure => {
                RpcError::native_unavailable()
            }
            PlatformError::OwnerAuthentication => RpcError::owner_authentication(),
            PlatformError::OwnerIncompatible => RpcError::owner_incompatible(),
            PlatformError::OwnerBusy => RpcError::owner_busy(),
            PlatformError::OwnerSingletonCollision => RpcError::owner_singleton_collision(),
            PlatformError::OwnerDraining => RpcError::owner_draining(),
            PlatformError::OwnerRollback => RpcError::owner_rollback(),
            PlatformError::OwnerSecurityFault => RpcError::owner_security_fault(),
            PlatformError::Indeterminate => RpcError::indeterminate(),
        };
        self.send_error(id, error)
    }

    fn send_error(&self, id: RequestId, error: RpcError) -> bool {
        self.send(Outbound::Response(RpcResponse::error(Some(id), error)))
    }

    fn reserve_critical_delivery(&self) -> Option<Sender<Vec<Outbound>>> {
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

    fn complete_critical_delivery(
        &self,
        delivery: Sender<Vec<Outbound>>,
        batch: Vec<Outbound>,
    ) -> bool {
        if delivery.send(batch).is_ok() {
            true
        } else {
            self.terminal.trigger(TerminalReason::StdoutDisconnected);
            false
        }
    }

    fn send(&self, outbound: Outbound) -> bool {
        self.send_to(&self.outbound, outbound)
    }

    fn send_final(&self, outbound: Outbound) -> bool {
        self.send_to(&self.final_outbound, outbound)
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
    fn paste_wait_uses_native_platform_measurements_and_saturates() {
        let mut counters = PasteCounters::default();
        counters.record(PasteResult {
            submitted: true,
            reason: None,
        });
        counters.record(PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        });
        counters.merge_native(0, 35, 25, 0);
        assert_eq!(counters.attempted, 2);
        assert_eq!(counters.submitted, 1);
        assert_eq!(counters.native_wait_duration_ms_total, 35);
        assert_eq!(counters.native_wait_duration_ms_max, 25);

        counters.merge_native(0, MAX_OBSERVABILITY_COUNTER, MAX_OBSERVABILITY_COUNTER, 0);
        assert_eq!(
            counters.native_wait_duration_ms_total,
            MAX_OBSERVABILITY_COUNTER
        );
        assert_eq!(
            counters.native_wait_duration_ms_max,
            MAX_OBSERVABILITY_COUNTER
        );
    }

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
