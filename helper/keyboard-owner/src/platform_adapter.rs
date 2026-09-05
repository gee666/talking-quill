//! Production bridge between the native platform owner loop and the C1 adapter.
//!
//! The bridge is deliberately owner-local. It does not open a transport, mint a
//! capability, or decide protocol authority. `NativePlatform` remains the sole
//! owner of physical state, suppression, replay, target validation, and input
//! injection; this module only translates already-authorized semantic effects.

use std::{collections::VecDeque, sync::Arc};

use crossbeam_channel::{Receiver, bounded};
use talking_quill_keyboard_core::{
    ActivationBindings, ActivationContext, ActivationGeneration, SessionCaptureMode,
};
use talking_quill_owner_protocol::Counter;
use talking_quill_owner_protocol::schema::{
    CancellationReasons, EffectCounters, FrontAppMetadataResult, FrontAppResult,
    NativePasteCounters, ObservabilityResult, PermissionState as WirePermissionState,
    PermissionsResult, RegisteredInputCounters as WireRegisteredInputCounters, TransactionCounters,
};

use crate::adapter::BoundedShutdownOutcome;
use crate::{
    ActivationCaptureGate, AdapterEvent, AdapterEventDisposition, AdapterEventId, BrokerEvent,
    NativeAdapter, NativeEffect, NativeEffectResult, PasteCommitOutcome, PasteRefusal,
    executor::PasteExecutorRequest,
    platform::{
        CallbackGate, ClipboardTextHash, HookStatus, NativeEvent, NativePlatform, PasteFailure,
        PermissionState, Permissions, Platform, PlatformError, TerminalReason, TerminalSignal,
        TransactionObservabilitySnapshot,
    },
    state::{CandidateOwnership, NativeActionFailure, NativeOwnership, NativeReadiness},
};

const NATIVE_EVENT_CAPACITY: usize = 256;
const GENERATED_EVENT_CAPACITY: usize = 16;

/// Native adapter used by the owner coordinator. Runtime wiring supplies this
/// value to `NativeAdapterExecutor`; no gateway package can construct it.
pub struct PlatformAdapter<P = NativePlatform> {
    platform: P,
    native_events: Receiver<NativeEvent>,
    callback_gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    terminal_events: Receiver<TerminalReason>,
    queued: VecDeque<AdapterEvent>,
    claimed_paste: Option<crate::state::PasteAuthorization>,
    paste_ownership: crate::state::PasteOwnership,
    bindings: ActivationBindings,
    admission_open: bool,
    session_mode: SessionCaptureMode,
    last_native_work_pending: bool,
    terminal_event_emitted: bool,
    next_event_id: u64,
    last_queued_event_id: Option<AdapterEventId>,
    in_flight: Option<AdapterEvent>,
    stopped: bool,
}

impl PlatformAdapter<NativePlatform> {
    /// Starts the production native owner loop with the immutable build/runtime
    /// rollback gate. The callback delivery gate is kept open for the adapter
    /// lifetime; fresh suppression is controlled only by ordered native
    /// configuration commands, so drain events remain observable after close.
    pub fn start() -> Result<Self, PlatformError> {
        let capture_gate = ActivationCaptureGate::for_process();
        let callback_gate = Arc::new(CallbackGate::new());
        let (terminal_sender, terminal_events) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(
            Arc::clone(&callback_gate),
            terminal_sender,
        ));
        let (native_sender, native_events) = bounded(NATIVE_EVENT_CAPACITY);
        let platform = NativePlatform::start(
            native_sender,
            Arc::clone(&callback_gate),
            Arc::clone(&terminal),
            capture_gate,
        )?;
        // Open callback admission before publishing protocol initialization.
        // The Windows callback treats initialized+closed as an irreversible
        // terminal fence; publishing in the opposite order left a startup
        // window where ordinary input could permanently disable the matcher.
        callback_gate.open();
        platform.protocol_initialized();
        Ok(Self::from_parts(
            platform,
            native_events,
            callback_gate,
            terminal,
            terminal_events,
        ))
    }
}

impl<P: Platform> PlatformAdapter<P> {
    fn from_parts(
        platform: P,
        native_events: Receiver<NativeEvent>,
        callback_gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        terminal_events: Receiver<TerminalReason>,
    ) -> Self {
        Self {
            platform,
            native_events,
            callback_gate,
            terminal,
            terminal_events,
            queued: VecDeque::new(),
            claimed_paste: None,
            paste_ownership: crate::state::PasteOwnership::None,
            bindings: ActivationBindings::default(),
            admission_open: false,
            session_mode: SessionCaptureMode::Off,
            last_native_work_pending: false,
            terminal_event_emitted: false,
            next_event_id: 1,
            last_queued_event_id: None,
            in_flight: None,
            stopped: false,
        }
    }

    fn failure(error: PlatformError) -> NativeEffectResult {
        NativeEffectResult::Failed(match error {
            PlatformError::HookUnavailable | PlatformError::PermissionDenied => {
                NativeActionFailure::FailedNotApplied
            }
            PlatformError::NativeFailure | PlatformError::ThreadStopped => {
                NativeActionFailure::Indeterminate
            }
        })
    }

    fn close_fresh_admission(&mut self) -> NativeEffectResult {
        // Close fresh admission without disposing an unresolved candidate.
        // The ordered CancelCandidate action performs replay or exact-up drain.
        if let Err(error) = self.platform.close_activation_admission(self.bindings) {
            self.admission_open = false;
            return Self::failure(error);
        }
        self.admission_open = false;
        self.ingest_all_native_events();
        self.refresh_native_control_events();
        NativeEffectResult::AdmissionClosed {
            through_event: self.last_queued_event_id,
        }
    }

    fn emergency_close(&mut self) -> NativeEffectResult {
        let activation = match self.platform.configure_activation(false, self.bindings) {
            Ok(()) => NativeEffectResult::AdmissionClosed {
                through_event: self.last_queued_event_id,
            },
            Err(error) => Self::failure(error),
        };
        let session = self.platform.set_session_capture(SessionCaptureMode::Off);
        self.session_mode = SessionCaptureMode::Off;
        match (activation, session) {
            (NativeEffectResult::AdmissionClosed { through_event }, Ok(())) => {
                NativeEffectResult::AdmissionClosed { through_event }
            }
            (NativeEffectResult::Failed(failure), _) => NativeEffectResult::Failed(failure),
            (_, Err(error)) => Self::failure(error),
            _ => NativeEffectResult::Failed(NativeActionFailure::Indeterminate),
        }
    }

    fn apply_configuration(&mut self, bindings: ActivationBindings) -> NativeEffectResult {
        // Configuration is always installed closed. ReplaceConfig takes the
        // platform's authoritative current physical snapshot, fences pre-held
        // keys, and completes cancellation/replay before acknowledging.
        match self.platform.configure_activation(false, bindings) {
            Ok(()) => {
                self.bindings = bindings;
                self.admission_open = false;
                NativeEffectResult::Applied
            }
            Err(error) => Self::failure(error),
        }
    }

    fn open_fresh_admission(&mut self) -> NativeEffectResult {
        match self.platform.configure_activation(true, self.bindings) {
            Ok(()) => {
                self.admission_open = true;
                NativeEffectResult::Applied
            }
            Err(error) => {
                self.admission_open = false;
                Self::failure(error)
            }
        }
    }

    fn apply_session_mode(&mut self, mode: SessionCaptureMode) -> NativeEffectResult {
        match self.platform.set_session_capture(mode) {
            Ok(()) => {
                self.session_mode = mode;
                NativeEffectResult::Applied
            }
            Err(error) => Self::failure(error),
        }
    }

    fn admit_paste(&mut self, request: PasteExecutorRequest) -> NativeEffectResult {
        if self.claimed_paste.is_some() {
            return NativeEffectResult::PasteRefused(PasteRefusal::NativeUnavailable);
        }
        let Some(generation) =
            ActivationGeneration::new(request.authorization.activation_generation().get())
        else {
            return NativeEffectResult::PasteRefused(PasteRefusal::NativeRejected);
        };
        let mut context = ActivationContext::target_unavailable(generation);
        if let Some(target) = request.target_token {
            context = context.with_target_token(target);
        }
        let result = self
            .platform
            .inject_paste_for_activation_with_clipboard_hash(
                context,
                ClipboardTextHash::from_bytes(*request.fallback_text_sha256.as_bytes()),
            );
        if result.submitted || result.reason == Some(PasteFailure::Indeterminate) {
            self.claimed_paste = Some(request.authorization);
            self.paste_ownership = if result.submitted {
                crate::state::PasteOwnership::Claimed
            } else {
                crate::state::PasteOwnership::Indeterminate
            };
            self.push_event(BrokerEvent::PasteClaimed(request.authorization));
            self.push_event(BrokerEvent::PasteFinished {
                authorization: request.authorization,
                outcome: if result.submitted {
                    PasteCommitOutcome::Committed
                } else {
                    PasteCommitOutcome::Indeterminate
                },
            });
            NativeEffectResult::PasteWaiting
        } else {
            NativeEffectResult::PasteRefused(paste_refusal(result.reason))
        }
    }

    fn cancel_waiting_paste(
        &mut self,
        authorization: crate::state::PasteAuthorization,
    ) -> NativeEffectResult {
        if self.claimed_paste == Some(authorization) {
            // The synchronous native boundary has already crossed claim. Never
            // let a racing rollback invent a successful pre-claim cancellation.
            NativeEffectResult::Failed(NativeActionFailure::Indeterminate)
        } else {
            NativeEffectResult::Failed(NativeActionFailure::FailedNotApplied)
        }
    }

    fn push_event(&mut self, event: BrokerEvent) {
        if self.queued.len() >= NATIVE_EVENT_CAPACITY + GENERATED_EVENT_CAPACITY {
            self.terminal
                .trigger(crate::platform::TerminalReason::OutboundQueueUnavailable);
            return;
        }
        let Some(id) = AdapterEventId::new(self.next_event_id) else {
            self.terminal
                .trigger(crate::platform::TerminalReason::OutboundQueueUnavailable);
            return;
        };
        let Some(next) = self.next_event_id.checked_add(1) else {
            self.terminal
                .trigger(crate::platform::TerminalReason::OutboundQueueUnavailable);
            return;
        };
        self.next_event_id = next;
        self.last_queued_event_id = Some(id);
        self.queued.push_back(AdapterEvent::new(id, event));
    }

    fn ingest_native_event(&mut self, event: NativeEvent) {
        match event {
            NativeEvent::AudioInputDevicesChanged => {
                self.push_event(BrokerEvent::AudioInputDevicesChanged)
            }
            NativeEvent::RegisteredObservation { generation } => {
                self.platform.record_adapter_dequeued();
                self.push_event(BrokerEvent::RegisteredObservation { generation });
            }
            NativeEvent::Keyboard(event) => {
                if matches!(
                    event,
                    talking_quill_keyboard_core::KeyboardEvent::Activation { .. }
                        | talking_quill_keyboard_core::KeyboardEvent::ActivationComplete { .. }
                ) {
                    self.platform.record_adapter_dequeued();
                }
                self.push_event(BrokerEvent::Keyboard(event));
            }
        }
    }

    fn refresh_native_control_events(&mut self) {
        let pending = self.platform.native_work_pending();
        if pending != self.last_native_work_pending {
            self.last_native_work_pending = pending;
            self.push_event(BrokerEvent::OwnershipChanged(
                crate::state::NativeOwnershipObservation {
                    candidate: CandidateOwnership::None,
                    activation_drain_keys: 0,
                    session_drain_keys: 0,
                    replay_cleanup_edges: 0,
                    paste: self.paste_ownership,
                    conservative_native_work: pending,
                    admitted_effects: 0,
                },
            ));
        }
        if !self.terminal_event_emitted
            && let Ok(reason) = self.terminal_events.try_recv()
        {
            eprintln!("keyboard-owner native failure: {reason:?}");
            eprintln!(
                "keyboard-owner failure counters: {:?}",
                self.platform.transaction_observability()
            );
            self.terminal_event_emitted = true;
            self.push_event(BrokerEvent::RecoverableNativeFault);
        }
    }

    fn ingest_all_native_events(&mut self) {
        while let Ok(event) = self.native_events.try_recv() {
            self.ingest_native_event(event);
        }
    }

    fn next_envelope(&mut self) -> Option<AdapterEvent> {
        if self.in_flight.is_some() || self.stopped {
            return None;
        }
        if self.queued.is_empty() {
            if let Ok(event) = self.native_events.try_recv() {
                self.ingest_native_event(event);
            }
            self.refresh_native_control_events();
        }
        let envelope = self.queued.pop_front()?;
        self.in_flight = Some(envelope);
        Some(envelope)
    }

    fn stop_adapter(&mut self) -> NativeEffectResult {
        self.refresh_native_control_events();
        let in_flight_orders_stop = self.in_flight.is_none_or(|event| {
            matches!(
                event.event(),
                BrokerEvent::OwnershipChanged(observation)
                    if !observation.conservative_native_work
                        && observation.activation_drain_keys == 0
                        && observation.session_drain_keys == 0
                        && observation.replay_cleanup_edges == 0
                        && observation.paste == crate::state::PasteOwnership::None
                        && observation.admitted_effects == 0
            )
        });
        if self.platform.native_work_pending()
            || self.paste_ownership != crate::state::PasteOwnership::None
            || !in_flight_orders_stop
            || !self.queued.is_empty()
        {
            return NativeEffectResult::Failed(NativeActionFailure::FailedNotApplied);
        }
        self.callback_gate.close();
        let shutdown = self.platform.shutdown();
        self.stopped = true;
        if shutdown.terminal_reason.is_none() && shutdown.observability_quiescent {
            NativeEffectResult::Applied
        } else {
            NativeEffectResult::Failed(NativeActionFailure::Indeterminate)
        }
    }

    fn readiness_snapshot(&self) -> NativeReadiness {
        let permissions = self.platform.permissions();
        let permissions_eligible = permissions_eligible(permissions);
        let hook_healthy = matches!(
            self.platform.hook_status(),
            HookStatus::InstalledUnobserved | HookStatus::PhysicalObserved
        ) && !self.terminal.is_triggered()
            && !self.stopped;
        NativeReadiness {
            keyboard_build_eligible: cfg!(feature = "local-unsigned-owner"),
            paste_ready: permissions_eligible && hook_healthy,
            permissions_eligible,
            hook_healthy,
        }
    }
}

impl<P: Platform> NativeAdapter for PlatformAdapter<P> {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        !matches!(
            self.platform.hook_status(),
            HookStatus::Unavailable | HookStatus::Stopped
        ) && !self.terminal.is_triggered()
    }

    fn readiness(&self) -> NativeReadiness {
        self.readiness_snapshot()
    }

    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult {
        if self.stopped {
            return NativeEffectResult::Failed(NativeActionFailure::FailedNotApplied);
        }
        match effect {
            NativeEffect::CloseFreshAdmission { .. } => self.close_fresh_admission(),
            NativeEffect::EmergencyCloseFreshAdmission => self.emergency_close(),
            NativeEffect::OpenFreshAdmission { .. } => self.open_fresh_admission(),
            NativeEffect::ApplySessionMode { mode, .. } => self.apply_session_mode(mode),
            NativeEffect::ApplyConfiguration { request, .. } => {
                self.apply_configuration(request.bindings())
            }
            NativeEffect::AdmitPaste { request, .. } => self.admit_paste(request),
            NativeEffect::CancelCandidate { .. } => {
                match self.platform.cancel_activation_candidate() {
                    Ok(()) => {
                        self.ingest_all_native_events();
                        self.refresh_native_control_events();
                        NativeEffectResult::CandidateCancelled(self.current_ownership())
                    }
                    Err(error) => Self::failure(error),
                }
            }
            NativeEffect::CancelWaitingPaste { authorization, .. } => {
                self.cancel_waiting_paste(authorization)
            }
            NativeEffect::PersistMaintenanceRecord { request, .. } => {
                self.persist_maintenance_record(request)
            }
            NativeEffect::ContinueNativeDrain => {
                self.ingest_all_native_events();
                self.refresh_native_control_events();
                NativeEffectResult::Applied
            }
            NativeEffect::StopNativeAdapter { .. } => self.stop_adapter(),
        }
    }

    fn try_next_event(&mut self) -> Option<AdapterEvent> {
        self.next_envelope()
    }

    fn acknowledge_event(&mut self, id: AdapterEventId, _disposition: AdapterEventDisposition) {
        if self.in_flight.is_some_and(|event| event.id() == id) {
            if let Some(event) = self.in_flight.map(AdapterEvent::event)
                && let BrokerEvent::PasteFinished { outcome, .. } = event
                && outcome == PasteCommitOutcome::Committed
            {
                self.claimed_paste = None;
                self.paste_ownership = crate::state::PasteOwnership::None;
            }
            self.in_flight = None;
        } else {
            self.terminal
                .trigger(crate::platform::TerminalReason::OutboundQueueUnavailable);
        }
    }

    fn permissions(&self) -> PermissionsResult {
        wire_permissions(self.platform.permissions())
    }

    fn front_app(&self) -> FrontAppResult {
        // The legacy strict v1 method remains token-only. Display metadata is
        // available exclusively through the negotiated extension method.
        FrontAppResult {
            available: false,
            application_token: None,
        }
    }

    fn front_app_metadata(&self) -> FrontAppMetadataResult {
        match self.platform.front_app() {
            Ok(value) => FrontAppMetadataResult {
                available: true,
                process_name: Some(bound_front_app_field(value.process_name)),
                window_title: Some(bound_front_app_field(value.window_title)),
                window_bounds: value.window_bounds.map(|bounds| {
                    talking_quill_owner_protocol::schema::FrontAppWindowBounds {
                        x: bounds.x,
                        y: bounds.y,
                        width: bounds.width,
                        height: bounds.height,
                    }
                }),
            },
            Err(_) => FrontAppMetadataResult {
                available: false,
                process_name: None,
                window_title: None,
                window_bounds: None,
            },
        }
    }

    fn observability(&self) -> ObservabilityResult {
        wire_observability(self.platform.transaction_observability())
    }

    fn orphan_retirement_policy(&self) -> crate::OrphanRetirementPolicy {
        // Once no controller remains, drain through the platform's bounded
        // shutdown rather than retaining an unreachable singleton indefinitely.
        crate::OrphanRetirementPolicy::BoundedNativeShutdown
    }

    fn bounded_orphan_shutdown(&mut self) -> BoundedShutdownOutcome {
        self.callback_gate.close();
        let shutdown = self.platform.shutdown();
        self.stopped = true;
        if !shutdown.observability_quiescent {
            BoundedShutdownOutcome::Failed
        } else if shutdown.terminal_incomplete {
            BoundedShutdownOutcome::TerminalIncomplete
        } else {
            BoundedShutdownOutcome::Quiescent
        }
    }
}

impl<P: Platform> PlatformAdapter<P> {
    fn persist_maintenance_record(
        &mut self,
        _request: crate::state::MaintenanceRequest,
    ) -> NativeEffectResult {
        #[cfg(target_os = "macos")]
        {
            use crate::macos::{KeychainStore, NativeKeychainStore};
            use talking_quill_owner_protocol::macos_maintenance::{
                MacosMaintenanceOperation, MacosMaintenanceRecord,
            };
            let operation = match _request.operation() {
                crate::state::MaintenanceOperation::Update => MacosMaintenanceOperation::Update,
                crate::state::MaintenanceOperation::Uninstall => {
                    MacosMaintenanceOperation::Uninstall
                }
                crate::state::MaintenanceOperation::Rollback => MacosMaintenanceOperation::Rollback,
            };
            let handoff =
                talking_quill_owner_protocol::Bytes32::new(*_request.owner_handoff().as_bytes());
            let record = MacosMaintenanceRecord::in_progress(
                operation,
                talking_quill_owner_protocol::Bytes32::new(*_request.transaction().as_bytes()),
                talking_quill_owner_protocol::Bytes32::new(*_request.source_build().as_bytes()),
                _request
                    .target_build()
                    .map(|value| talking_quill_owner_protocol::Bytes32::new(*value.as_bytes())),
                _request
                    .target_owner()
                    .map(|value| talking_quill_owner_protocol::Bytes32::new(*value.as_bytes())),
                handoff,
            )
            .and_then(|record| record.encode())
            .map_err(|_| ());
            if record.is_ok_and(|record| {
                NativeKeychainStore
                    .write_maintenance_latch_without_ui(&record)
                    .is_ok()
            }) {
                return NativeEffectResult::Applied;
            }
        }
        NativeEffectResult::Failed(NativeActionFailure::FailedNotApplied)
    }

    fn current_ownership(&self) -> NativeOwnership {
        NativeOwnership::new(CandidateOwnership::None, 0, 0, 0, self.paste_ownership, 0)
            .map(|ownership| ownership.with_conservative_native_work(self.last_native_work_pending))
            .unwrap_or(NativeOwnership::NEUTRAL)
    }
}

fn bound_front_app_field(mut value: String) -> String {
    const MAX_ESCAPED_BYTES: usize = 320;
    let mut escaped = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let bytes = if character <= '\u{001f}' {
            6
        } else if matches!(character, '"' | '\\') {
            2
        } else {
            character.len_utf8()
        };
        if escaped + bytes > MAX_ESCAPED_BYTES {
            break;
        }
        escaped += bytes;
        end = index + character.len_utf8();
    }
    value.truncate(end);
    value
}

const fn paste_refusal(reason: Option<PasteFailure>) -> PasteRefusal {
    match reason {
        Some(PasteFailure::PermissionDenied) => PasteRefusal::PermissionDenied,
        Some(PasteFailure::ConflictingModifiers) => PasteRefusal::ConflictingModifiers,
        Some(PasteFailure::SecureInput) => PasteRefusal::SecureInput,
        Some(PasteFailure::OsRejected) => PasteRefusal::NativeRejected,
        Some(PasteFailure::Unavailable | PasteFailure::Indeterminate) | None => {
            PasteRefusal::NativeUnavailable
        }
    }
}

const fn permissions_eligible(permissions: Permissions) -> bool {
    permission_eligible(permissions.accessibility)
        && permission_eligible(permissions.input_monitoring)
        && permission_eligible(permissions.event_post)
}

const fn permission_eligible(permission: PermissionState) -> bool {
    matches!(
        permission,
        PermissionState::Granted | PermissionState::NotApplicable
    )
}

const fn wire_permission(permission: PermissionState) -> WirePermissionState {
    match permission {
        PermissionState::Granted => WirePermissionState::Granted,
        PermissionState::Denied => WirePermissionState::Denied,
        PermissionState::Unknown => WirePermissionState::Unknown,
        PermissionState::NotApplicable => WirePermissionState::NotRequired,
    }
}

const fn wire_permissions(permissions: Permissions) -> PermissionsResult {
    PermissionsResult {
        accessibility: wire_permission(permissions.accessibility),
        input_monitoring: wire_permission(permissions.input_monitoring),
        event_post: wire_permission(permissions.event_post),
    }
}

fn counter(value: u64) -> Counter {
    Counter::new(value).expect("native observability is bounded to a JS-safe counter")
}

fn wire_observability(value: TransactionObservabilitySnapshot) -> ObservabilityResult {
    let reasons = value.transactions.cancellation_reasons;
    ObservabilityResult {
        owner: Default::default(),
        registered_input: Some(WireRegisteredInputCounters {
            hook_installed: counter(value.registered_input.hook_installed),
            pump_alive: counter(value.registered_input.pump_alive),
            hc_action_callbacks: counter(value.registered_input.hc_action_callbacks),
            physical_callbacks: counter(value.registered_input.physical_callbacks),
            physical_callbacks_filtered: counter(
                value.registered_input.physical_callbacks_filtered,
            ),
            registered_candidate_callbacks: counter(
                value.registered_input.registered_candidate_callbacks,
            ),
            registered_match_callbacks: counter(value.registered_input.registered_match_callbacks),
            registered_release_callbacks: counter(
                value.registered_input.registered_release_callbacks,
            ),
            callback_channel_accepted: counter(value.registered_input.callback_channel_accepted),
            callback_channel_rejected: counter(value.registered_input.callback_channel_rejected),
            adapter_dequeued: counter(value.registered_input.adapter_dequeued),
            owner_admitted: counter(0),
            owner_flushed: counter(0),
            owner_rejected: counter(0),
        }),
        transactions: TransactionCounters {
            started: counter(value.transactions.started),
            committed: counter(value.transactions.committed),
            replayed: counter(value.transactions.replayed),
            cancelled: counter(value.transactions.cancelled),
            journal_high_water: counter(value.transactions.journal_high_water),
            cancellation_reasons: CancellationReasons {
                invalid_continuation: counter(reasons.invalid_continuation),
                modifier_changed: counter(reasons.modifier_changed),
                alt_gr: counter(reasons.alt_gr),
                journal_overflow: counter(reasons.journal_overflow),
                configuration_replaced: counter(reasons.configuration_replaced),
                revision_mismatch: counter(reasons.revision_mismatch),
                gate_closed: counter(reasons.gate_closed),
                shutdown: counter(reasons.shutdown),
                helper_disconnected: counter(reasons.helper_disconnected),
                secure_desktop: counter(reasons.secure_desktop),
                timeout: counter(reasons.timeout),
                activation_delivery_failed: counter(reasons.activation_delivery_failed),
                neutralization_failed: counter(reasons.neutralization_failed),
                replay_failed: counter(reasons.replay_failed),
                effect_protocol_violation: counter(reasons.effect_protocol_violation),
                physical_state_mismatch: counter(reasons.physical_state_mismatch),
                target_changed: counter(reasons.target_changed),
            },
        },
        replay: EffectCounters {
            attempted: counter(value.replay.attempted),
            succeeded: counter(value.replay.succeeded),
            partial: counter(value.replay.partial),
            failed: counter(value.replay.failed),
        },
        dummy: EffectCounters {
            attempted: counter(value.dummy.attempted),
            succeeded: counter(value.dummy.succeeded),
            partial: counter(value.dummy.partial),
            failed: counter(value.dummy.failed),
        },
        native_paste: NativePasteCounters {
            target_validation_fallbacks: counter(value.native_paste.target_validation_fallbacks),
            modifier_wait_duration_ms_total: counter(
                value.native_paste.modifier_wait_duration_ms_total,
            ),
            modifier_wait_duration_ms_max: counter(
                value.native_paste.modifier_wait_duration_ms_max,
            ),
            modifier_timeouts: counter(value.native_paste.modifier_timeouts),
            shutdown_ownership_deadlines: counter(value.native_paste.shutdown_ownership_deadlines),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use crossbeam_channel::Sender;
    use talking_quill_keyboard_core::{EventPhase, KeyboardEvent, SessionKey};
    use talking_quill_owner_protocol::Bytes32;

    use super::*;
    use crate::{
        platform::{PasteResult, PlatformShutdown},
        state::{
            CapabilityEpoch, OwnerActivationGeneration, OwnerInstanceId, PasteAuthorization,
            PasteOperationId,
        },
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Configure(bool),
        CloseAdmission,
        CancelCandidate,
        Session(SessionCaptureMode),
        Paste,
        Shutdown,
    }

    struct FakePlatform {
        calls: Arc<Mutex<Vec<Call>>>,
        paste: Arc<Mutex<VecDeque<PasteResult>>>,
        status: HookStatus,
        permissions: Permissions,
        shutdown: PlatformShutdown,
        pending_native_work: Arc<AtomicBool>,
    }

    impl Platform for FakePlatform {
        fn start(
            _outbound: Sender<NativeEvent>,
            _gate: Arc<CallbackGate>,
            _terminal: Arc<TerminalSignal>,
            _capture_gate: ActivationCaptureGate,
        ) -> Result<Self, PlatformError> {
            unreachable!("tests construct explicit channels")
        }

        fn hook_status(&self) -> HookStatus {
            self.status
        }

        fn configure_activation(
            &self,
            enabled: bool,
            _bindings: ActivationBindings,
        ) -> Result<(), PlatformError> {
            self.calls.lock().unwrap().push(Call::Configure(enabled));
            Ok(())
        }

        fn close_activation_admission(
            &self,
            _bindings: ActivationBindings,
        ) -> Result<(), PlatformError> {
            self.calls.lock().unwrap().push(Call::CloseAdmission);
            Ok(())
        }

        fn cancel_activation_candidate(&self) -> Result<(), PlatformError> {
            self.calls.lock().unwrap().push(Call::CancelCandidate);
            Ok(())
        }

        fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError> {
            self.calls.lock().unwrap().push(Call::Session(mode));
            Ok(())
        }

        fn inject_paste(&self) -> PasteResult {
            unreachable!("the bridge never uses untargeted paste")
        }

        fn inject_paste_for_activation_with_clipboard_hash(
            &self,
            _context: ActivationContext,
            _expected_clipboard_sha256: ClipboardTextHash,
        ) -> PasteResult {
            self.calls.lock().unwrap().push(Call::Paste);
            self.paste.lock().unwrap().pop_front().unwrap()
        }

        fn front_app(&self) -> Result<crate::platform::FrontApp, PlatformError> {
            Err(PlatformError::NativeFailure)
        }

        fn permissions(&self) -> Permissions {
            self.permissions
        }

        fn native_work_pending(&self) -> bool {
            self.pending_native_work.load(Ordering::Acquire)
        }

        fn shutdown(&mut self) -> PlatformShutdown {
            self.calls.lock().unwrap().push(Call::Shutdown);
            self.shutdown
        }
    }

    fn harness(
        paste: Vec<PasteResult>,
    ) -> (
        PlatformAdapter<FakePlatform>,
        Sender<NativeEvent>,
        Arc<Mutex<Vec<Call>>>,
    ) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (native_sender, native_events) = bounded(NATIVE_EVENT_CAPACITY);
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_sender, terminal_events) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_sender));
        let platform = FakePlatform {
            calls: Arc::clone(&calls),
            paste: Arc::new(Mutex::new(paste.into())),
            status: HookStatus::InstalledUnobserved,
            permissions: Permissions {
                accessibility: PermissionState::Granted,
                input_monitoring: PermissionState::NotApplicable,
                event_post: PermissionState::Granted,
            },
            shutdown: PlatformShutdown::quiescent(None),
            pending_native_work: Arc::new(AtomicBool::new(false)),
        };
        (
            PlatformAdapter::from_parts(platform, native_events, gate, terminal, terminal_events),
            native_sender,
            calls,
        )
    }

    fn authorization() -> PasteAuthorization {
        PasteAuthorization::new(
            PasteOperationId::new([1; 32]).unwrap(),
            OwnerInstanceId::new([2; 32]).unwrap(),
            CapabilityEpoch::new_for_test(3).unwrap(),
            OwnerActivationGeneration::new(4).unwrap(),
        )
    }

    fn paste_request() -> PasteExecutorRequest {
        PasteExecutorRequest {
            authorization: authorization(),
            target_token: None,
            fallback_text_sha256: Bytes32::new([5; 32]),
        }
    }

    #[test]
    fn bridge_orders_configuration_open_session_close_and_emergency_close() {
        let (mut bridge, _sender, calls) = harness(Vec::new());
        assert_eq!(
            bridge.apply_configuration(ActivationBindings::default()),
            NativeEffectResult::Applied
        );
        assert_eq!(bridge.open_fresh_admission(), NativeEffectResult::Applied);
        assert_eq!(
            bridge.apply_session_mode(SessionCaptureMode::Recording),
            NativeEffectResult::Applied
        );
        assert!(matches!(
            bridge.close_fresh_admission(),
            NativeEffectResult::AdmissionClosed { .. }
        ));
        assert!(matches!(
            bridge.emergency_close(),
            NativeEffectResult::AdmissionClosed { .. }
        ));
        assert_eq!(
            *calls.lock().unwrap(),
            [
                Call::Configure(false),
                Call::Configure(true),
                Call::Session(SessionCaptureMode::Recording),
                Call::CloseAdmission,
                Call::Configure(false),
                Call::Session(SessionCaptureMode::Off),
            ]
        );
    }

    #[test]
    fn close_watermark_includes_semantic_event_and_following_ownership_fact() {
        let (mut bridge, sender, _calls) = harness(Vec::new());
        bridge
            .platform
            .pending_native_work
            .store(true, Ordering::Release);
        sender
            .send(NativeEvent::Keyboard(KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            }))
            .unwrap();
        let NativeEffectResult::AdmissionClosed {
            through_event: Some(through),
        } = bridge.close_fresh_admission()
        else {
            panic!("close watermark")
        };
        assert_eq!(through.get(), 2);
        let first = bridge.try_next_event().unwrap();
        assert!(matches!(first.event(), BrokerEvent::Keyboard(_)));
        bridge.acknowledge_event(first.id(), AdapterEventDisposition::Accepted);
        let second = bridge.try_next_event().unwrap();
        let BrokerEvent::OwnershipChanged(observation) = second.event() else {
            panic!("ownership observation")
        };
        assert_eq!(observation.replay_cleanup_edges, 0);
        assert!(observation.conservative_native_work);
        assert_eq!(observation.admitted_effects, 0);
        assert_eq!(second.id(), through);
        bridge.acknowledge_event(second.id(), AdapterEventDisposition::Accepted);
        assert!(bridge.try_next_event().is_none());
    }

    #[test]
    fn close_watermark_preserves_a_full_native_backlog_without_expansion_loss() {
        let (mut bridge, sender, _calls) = harness(Vec::new());
        bridge
            .platform
            .pending_native_work
            .store(true, Ordering::Release);
        for _ in 0..NATIVE_EVENT_CAPACITY {
            sender.send(NativeEvent::AudioInputDevicesChanged).unwrap();
        }
        let NativeEffectResult::AdmissionClosed {
            through_event: Some(through),
        } = bridge.close_fresh_admission()
        else {
            panic!("full close watermark")
        };
        assert_eq!(through.get(), NATIVE_EVENT_CAPACITY as u64 + 1);
        let mut drained = 0;
        while let Some(event) = bridge.try_next_event() {
            drained += 1;
            bridge.acknowledge_event(event.id(), AdapterEventDisposition::Accepted);
        }
        assert_eq!(drained, NATIVE_EVENT_CAPACITY + 1);
    }

    #[test]
    fn paste_bridge_exhausts_preclaim_refusals_and_claimed_outcomes() {
        let cases = [
            (
                PasteFailure::PermissionDenied,
                PasteRefusal::PermissionDenied,
            ),
            (
                PasteFailure::ConflictingModifiers,
                PasteRefusal::ConflictingModifiers,
            ),
            (PasteFailure::SecureInput, PasteRefusal::SecureInput),
            (PasteFailure::OsRejected, PasteRefusal::NativeRejected),
            (PasteFailure::Unavailable, PasteRefusal::NativeUnavailable),
        ];
        for (failure, refusal) in cases {
            let (mut bridge, _sender, _calls) = harness(vec![PasteResult {
                submitted: false,
                reason: Some(failure),
            }]);
            assert_eq!(
                bridge.admit_paste(paste_request()),
                NativeEffectResult::PasteRefused(refusal)
            );
            assert!(bridge.try_next_event().is_none());
        }
        for result in [
            PasteResult {
                submitted: true,
                reason: None,
            },
            PasteResult {
                submitted: false,
                reason: Some(PasteFailure::Indeterminate),
            },
        ] {
            let (mut bridge, _sender, _calls) = harness(vec![result]);
            assert_eq!(
                bridge.admit_paste(paste_request()),
                NativeEffectResult::PasteWaiting
            );
            let claimed = bridge.try_next_event().unwrap();
            assert!(matches!(claimed.event(), BrokerEvent::PasteClaimed(_)));
            bridge.acknowledge_event(claimed.id(), AdapterEventDisposition::Accepted);
            let finished = bridge.try_next_event().unwrap();
            assert!(matches!(
                finished.event(),
                BrokerEvent::PasteFinished { .. }
            ));
            bridge.acknowledge_event(finished.id(), AdapterEventDisposition::Accepted);
        }
    }

    #[test]
    fn terminal_and_native_ownership_changes_are_sequenced_control_events() {
        let (mut bridge, _sender, _calls) = harness(Vec::new());
        bridge
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        assert_eq!(
            bridge.stop_adapter(),
            NativeEffectResult::Failed(NativeActionFailure::FailedNotApplied)
        );
        assert!(!bridge.stopped);
        let fault = bridge.try_next_event().unwrap();
        assert_eq!(fault.event(), BrokerEvent::RecoverableNativeFault);
        bridge.acknowledge_event(fault.id(), AdapterEventDisposition::Accepted);
        assert!(bridge.try_next_event().is_none());
    }

    #[test]
    fn final_neutral_ownership_event_can_reentrantly_order_native_stop() {
        let (mut bridge, _sender, calls) = harness(Vec::new());
        bridge
            .platform
            .pending_native_work
            .store(true, Ordering::Release);
        bridge.refresh_native_control_events();
        let pending = bridge.try_next_event().unwrap();
        bridge.acknowledge_event(pending.id(), AdapterEventDisposition::Accepted);

        bridge
            .platform
            .pending_native_work
            .store(false, Ordering::Release);
        bridge.refresh_native_control_events();
        let neutral = bridge.try_next_event().unwrap();
        assert!(matches!(
            neutral.event(),
            BrokerEvent::OwnershipChanged(observation)
                if !observation.conservative_native_work
        ));
        assert_eq!(bridge.stop_adapter(), NativeEffectResult::Applied);
        bridge.acknowledge_event(neutral.id(), AdapterEventDisposition::Accepted);
        assert_eq!(*calls.lock().unwrap(), [Call::Shutdown]);
    }

    #[test]
    fn indeterminate_paste_remains_authoritative_during_cancellation() {
        let (mut bridge, _sender, _calls) = harness(vec![PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Indeterminate),
        }]);
        assert_eq!(
            bridge.admit_paste(paste_request()),
            NativeEffectResult::PasteWaiting
        );
        assert_eq!(
            bridge.current_ownership().paste(),
            crate::state::PasteOwnership::Indeterminate
        );
        assert_eq!(
            bridge.cancel_waiting_paste(authorization()),
            NativeEffectResult::Failed(NativeActionFailure::Indeterminate)
        );
    }

    #[test]
    fn claimed_paste_cannot_be_cancelled_and_stop_is_ordered_last() {
        let (mut bridge, _sender, calls) = harness(vec![PasteResult {
            submitted: true,
            reason: None,
        }]);
        assert_eq!(
            bridge.admit_paste(paste_request()),
            NativeEffectResult::PasteWaiting
        );
        assert_eq!(
            bridge.cancel_waiting_paste(authorization()),
            NativeEffectResult::Failed(NativeActionFailure::Indeterminate)
        );
        bridge.callback_gate.close();
        let shutdown = bridge.platform.shutdown();
        bridge.stopped = true;
        assert_eq!(shutdown, PlatformShutdown::quiescent(None));
        assert_eq!(*calls.lock().unwrap(), [Call::Paste, Call::Shutdown]);
    }

    #[test]
    fn front_app_metadata_escaped_json_is_bounded_for_every_worst_case_character_class() {
        for input in [
            "\u{0001}".repeat(2_000),
            "\"\\".repeat(2_000),
            "é🙂".repeat(2_000),
        ] {
            let result = FrontAppMetadataResult {
                available: true,
                process_name: Some(bound_front_app_field(input.clone())),
                window_title: Some(bound_front_app_field(input)),
                window_bounds: Some(talking_quill_owner_protocol::schema::FrontAppWindowBounds {
                    x: i32::MIN,
                    y: i32::MAX,
                    width: u32::MAX,
                    height: u32::MAX,
                }),
            };
            let response = talking_quill_owner_protocol::schema::Response::Success(
                talking_quill_owner_protocol::schema::SuccessResult::FrontAppMetadata(result),
            );
            assert!(response.to_json().is_ok());
        }
    }

    #[test]
    fn permission_readiness_queries_and_observability_are_platform_neutral() {
        let (bridge, _sender, _calls) = harness(Vec::new());
        let readiness = bridge.readiness_snapshot();
        assert!(readiness.permissions_eligible);
        assert!(readiness.hook_healthy);
        assert_eq!(
            NativeAdapter::permissions(&bridge),
            PermissionsResult {
                accessibility: WirePermissionState::Granted,
                input_monitoring: WirePermissionState::NotRequired,
                event_post: WirePermissionState::Granted,
            }
        );
        assert_eq!(
            NativeAdapter::front_app(&bridge),
            FrontAppResult {
                available: false,
                application_token: None,
            }
        );
        assert_eq!(
            NativeAdapter::observability(&bridge),
            ObservabilityResult {
                registered_input: Some(WireRegisteredInputCounters::default()),
                ..ObservabilityResult::default()
            }
        );
    }
}
