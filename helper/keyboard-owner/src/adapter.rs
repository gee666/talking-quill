//! Platform-neutral broker semantic boundary.
//!
//! This module maps the completed owner executor vocabulary to a deliberately
//! small native-adapter contract. It contains no adapter implementation, OS
//! endpoint, process launcher, singleton, or production enable path.

use std::{fmt, num::NonZeroU64};

use talking_quill_keyboard_core::{KeyboardEvent, SessionCaptureMode};
use talking_quill_owner_protocol::schema::{
    FrontAppMetadataResult, FrontAppResult, ObservabilityResult, PermissionsResult,
};

use crate::{
    ActivationCaptureGate,
    executor::{ExecutorCommand, ExecutorResult, OwnerExecutor, PasteExecutorRequest},
    state::{
        CapabilityId, ConfigurationRequest, MaintenanceRequest, NativeActionFailure,
        NativeActionToken, NativeOwnership, NativeOwnershipObservation, NativeReadiness,
        PasteAuthorization, PreHeldKeyFence, RequiredAction,
    },
};

/// Opaque, adapter-local correlation for one semantic event. It is never put on
/// the owner wire and its debug representation never reveals the value.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct AdapterEventId(NonZeroU64);

impl AdapterEventId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for AdapterEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdapterEventId(<redacted>)")
    }
}

/// One-shot paste completion after the adapter has already reported claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasteCommitOutcome {
    Committed,
    Indeterminate,
}

/// Coarse, content-free semantic event emitted by an adapter. Raw physical
/// events, replay journals, target evidence, clipboard text, and credentials
/// are deliberately unrepresentable here.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum BrokerEvent {
    Keyboard(KeyboardEvent),
    RegisteredObservation {
        generation: u64,
    },
    AudioInputDevicesChanged,
    OwnershipChanged(NativeOwnershipObservation),
    ReadinessChanged(NativeReadiness),
    PasteClaimed(PasteAuthorization),
    PasteFinished {
        authorization: PasteAuthorization,
        outcome: PasteCommitOutcome,
    },
    /// Authoritative late completion of a previously indeterminate paste.
    PasteIndeterminateResolved(PasteAuthorization),
    RecoverableNativeFault,
}

impl fmt::Debug for BrokerEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Keyboard(_) => "BrokerEvent::Keyboard(<redacted>)",
            Self::RegisteredObservation { .. } => "BrokerEvent::RegisteredObservation(<redacted>)",
            Self::AudioInputDevicesChanged => "BrokerEvent::AudioInputDevicesChanged",
            Self::OwnershipChanged(_) => "BrokerEvent::OwnershipChanged(<redacted>)",
            Self::ReadinessChanged(_) => "BrokerEvent::ReadinessChanged(<redacted>)",
            Self::PasteClaimed(_) => "BrokerEvent::PasteClaimed(<redacted>)",
            Self::PasteFinished { .. } => "BrokerEvent::PasteFinished(<redacted>)",
            Self::PasteIndeterminateResolved(_) => {
                "BrokerEvent::PasteIndeterminateResolved(<redacted>)"
            }
            Self::RecoverableNativeFault => "BrokerEvent::RecoverableNativeFault",
        })
    }
}

/// One event plus an adapter-local completion correlation. The owner must call
/// `NativeAdapter::acknowledge_event` exactly once for every polled envelope.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AdapterEvent {
    id: AdapterEventId,
    event: BrokerEvent,
}

impl AdapterEvent {
    #[must_use]
    pub const fn new(id: AdapterEventId, event: BrokerEvent) -> Self {
        Self { id, event }
    }

    #[must_use]
    pub const fn id(self) -> AdapterEventId {
        self.id
    }

    #[must_use]
    pub const fn event(self) -> BrokerEvent {
        self.event
    }
}

impl fmt::Debug for AdapterEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdapterEvent(<redacted>)")
    }
}

/// Stable reasons returned to the local adapter when an event was not admitted.
/// They contain no wire error, key, target, capability, or process metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterEventRejection {
    AdmissionClosed,
    StaleScope,
    Capacity,
    DeliveryFailed,
    InvalidTransition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterEventDisposition {
    Accepted,
    Rejected(AdapterEventRejection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeAdapterPump {
    Empty,
    ClosePending,
    Processed(AdapterEventDisposition),
}

/// Privacy-safe categories for a paste refusal that occurred before native
/// injection authority was claimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasteRefusal {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    TargetUnavailable,
    ClipboardChanged,
    NativeUnavailable,
    NativeRejected,
}

impl PasteRefusal {
    /// Exact owner-protocol mapping. No pre-claim refusal category is
    /// collapsed before reaching the gateway.
    #[must_use]
    pub const fn wire_reason(self) -> talking_quill_owner_protocol::schema::PasteRefusalReason {
        use talking_quill_owner_protocol::schema::PasteRefusalReason as Wire;
        match self {
            Self::PermissionDenied => Wire::PermissionDenied,
            Self::ConflictingModifiers => Wire::ConflictingModifiers,
            Self::SecureInput => Wire::SecureInput,
            Self::TargetUnavailable => Wire::TargetUnavailable,
            Self::ClipboardChanged => Wire::ClipboardChanged,
            Self::NativeUnavailable => Wire::NativeUnavailable,
            Self::NativeRejected => Wire::NativeRejected,
        }
    }
}

/// Data-free discriminator used by tests and observability. Full effect debug
/// formatting is always redacted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeEffectKind {
    CloseFreshAdmission,
    EmergencyCloseFreshAdmission,
    OpenFreshAdmission,
    ApplySessionMode,
    ApplyConfiguration,
    AdmitPaste,
    CancelCandidate,
    CancelWaitingPaste,
    PersistMaintenanceRecord,
    ContinueNativeDrain,
    StopNativeAdapter,
}

/// Exact effect vocabulary accepted by a future native owner-loop adapter.
/// Every tokenized B2 action preserves its original token and complete immutable
/// payload. `ExitOwner` is intentionally absent: process exit remains an outer
/// owner-loop directive after native stop and response-flush confirmation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NativeEffect {
    CloseFreshAdmission {
        token: NativeActionToken,
    },
    EmergencyCloseFreshAdmission,
    OpenFreshAdmission {
        token: NativeActionToken,
    },
    ApplySessionMode {
        token: NativeActionToken,
        mode: SessionCaptureMode,
    },
    ApplyConfiguration {
        token: NativeActionToken,
        request: ConfigurationRequest,
        fence: PreHeldKeyFence,
    },
    AdmitPaste {
        token: NativeActionToken,
        request: PasteExecutorRequest,
    },
    CancelCandidate {
        token: NativeActionToken,
    },
    CancelWaitingPaste {
        token: NativeActionToken,
        authorization: PasteAuthorization,
    },
    PersistMaintenanceRecord {
        token: NativeActionToken,
        request: MaintenanceRequest,
    },
    ContinueNativeDrain,
    StopNativeAdapter {
        token: NativeActionToken,
    },
}

impl NativeEffect {
    #[must_use]
    pub const fn kind(self) -> NativeEffectKind {
        match self {
            Self::CloseFreshAdmission { .. } => NativeEffectKind::CloseFreshAdmission,
            Self::EmergencyCloseFreshAdmission => NativeEffectKind::EmergencyCloseFreshAdmission,
            Self::OpenFreshAdmission { .. } => NativeEffectKind::OpenFreshAdmission,
            Self::ApplySessionMode { .. } => NativeEffectKind::ApplySessionMode,
            Self::ApplyConfiguration { .. } => NativeEffectKind::ApplyConfiguration,
            Self::AdmitPaste { .. } => NativeEffectKind::AdmitPaste,
            Self::CancelCandidate { .. } => NativeEffectKind::CancelCandidate,
            Self::CancelWaitingPaste { .. } => NativeEffectKind::CancelWaitingPaste,
            Self::PersistMaintenanceRecord { .. } => NativeEffectKind::PersistMaintenanceRecord,
            Self::ContinueNativeDrain => NativeEffectKind::ContinueNativeDrain,
            Self::StopNativeAdapter { .. } => NativeEffectKind::StopNativeAdapter,
        }
    }
}

impl fmt::Debug for NativeEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind() {
            NativeEffectKind::CloseFreshAdmission => {
                "NativeEffect::CloseFreshAdmission(<redacted>)"
            }
            NativeEffectKind::EmergencyCloseFreshAdmission => {
                "NativeEffect::EmergencyCloseFreshAdmission"
            }
            NativeEffectKind::OpenFreshAdmission => "NativeEffect::OpenFreshAdmission(<redacted>)",
            NativeEffectKind::ApplySessionMode => "NativeEffect::ApplySessionMode(<redacted>)",
            NativeEffectKind::ApplyConfiguration => "NativeEffect::ApplyConfiguration(<redacted>)",
            NativeEffectKind::AdmitPaste => "NativeEffect::AdmitPaste(<redacted>)",
            NativeEffectKind::CancelCandidate => "NativeEffect::CancelCandidate(<redacted>)",
            NativeEffectKind::CancelWaitingPaste => "NativeEffect::CancelWaitingPaste(<redacted>)",
            NativeEffectKind::PersistMaintenanceRecord => {
                "NativeEffect::PersistMaintenanceRecord(<redacted>)"
            }
            NativeEffectKind::ContinueNativeDrain => "NativeEffect::ContinueNativeDrain",
            NativeEffectKind::StopNativeAdapter => "NativeEffect::StopNativeAdapter(<redacted>)",
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NativeEffectResult {
    Applied,
    /// Adapter atomically closed fresh callback admission. Every adapter event
    /// through this inclusive watermark was emitted before the close barrier;
    /// `None` means no event has ever been emitted.
    AdmissionClosed {
        through_event: Option<AdapterEventId>,
    },
    PasteWaiting,
    PasteRefused(PasteRefusal),
    CandidateCancelled(NativeOwnership),
    Failed(NativeActionFailure),
}

impl fmt::Debug for NativeEffectResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeEffectResult(<redacted>)")
    }
}

/// Source for cryptographically random capability IDs. C1 intentionally ships
/// no production source; transport/runtime work must inject one later.
pub trait CapabilityIdSource {
    fn next_capability_id(&mut self) -> Option<CapabilityId>;
}

/// Native owner-loop abstraction. C1 supplies no Windows/macOS implementation.
/// Tests provide fakes; future native integration must implement this trait in
/// the owner package without exposing it to the gateway package.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedShutdownOutcome {
    Quiescent,
    TerminalIncomplete,
    Failed,
}

/// Platform process-retirement policy after controller authority is gone.
/// Windows must keep observing owned physical ups until semantic neutrality.
/// macOS keeps its existing bounded endpoint/run-loop shutdown policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrphanRetirementPolicy {
    ObserveUntilNeutral,
    BoundedNativeShutdown,
}

pub trait NativeAdapter {
    fn seed_startup_physical_snapshot(&mut self) -> bool;
    fn readiness(&self) -> NativeReadiness;
    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult;
    fn try_next_event(&mut self) -> Option<AdapterEvent>;
    fn acknowledge_event(&mut self, id: AdapterEventId, disposition: AdapterEventDisposition);
    fn permissions(&self) -> PermissionsResult;
    fn front_app(&self) -> FrontAppResult;
    fn front_app_metadata(&self) -> FrontAppMetadataResult {
        FrontAppMetadataResult {
            available: false,
            process_name: None,
            window_title: None,
            window_bounds: None,
        }
    }
    fn observability(&self) -> ObservabilityResult;
    fn orphan_retirement_policy(&self) -> OrphanRetirementPolicy {
        OrphanRetirementPolicy::BoundedNativeShutdown
    }
    /// Stops native observation at its platform deadline. This is used only
    /// after controller authority is gone; it must never synthesize balancing
    /// physical input.
    fn bounded_orphan_shutdown(&mut self) -> BoundedShutdownOutcome {
        BoundedShutdownOutcome::Failed
    }
}

/// Exact bridge from the B2 executor contract to a native adapter. The normal
/// constructor always applies the compile-time/process rollback gate. An open
/// gate can be supplied only to debug/test builds.
pub struct NativeAdapterExecutor<A, C> {
    adapter: A,
    capabilities: C,
    capture_gate: ActivationCaptureGate,
    last_adapter_event_id: u64,
    adapter_event_sequence_fault: bool,
}

impl<A, C> fmt::Debug for NativeAdapterExecutor<A, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeAdapterExecutor(<redacted>)")
    }
}

impl<A: NativeAdapter, C: CapabilityIdSource> NativeAdapterExecutor<A, C> {
    #[must_use]
    pub fn new(adapter: A, capabilities: C) -> Self {
        Self {
            adapter,
            capabilities,
            capture_gate: ActivationCaptureGate::for_process(),
            last_adapter_event_id: 0,
            adapter_event_sequence_fault: false,
        }
    }

    /// Explicit non-production seam for fake adapter integration tests.
    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    #[must_use]
    pub const fn new_for_test(
        adapter: A,
        capabilities: C,
        capture_gate: ActivationCaptureGate,
    ) -> Self {
        Self {
            adapter,
            capabilities,
            capture_gate,
            last_adapter_event_id: 0,
            adapter_event_sequence_fault: false,
        }
    }

    pub(crate) fn try_next_event(&mut self) -> Option<(AdapterEvent, bool)> {
        self.adapter.try_next_event().map(|envelope| {
            let sequence_valid = !self.adapter_event_sequence_fault
                && self
                    .last_adapter_event_id
                    .checked_add(1)
                    .is_some_and(|expected| expected == envelope.id().get());
            if sequence_valid {
                self.last_adapter_event_id = envelope.id().get();
            } else {
                self.adapter_event_sequence_fault = true;
            }
            let event = match envelope.event() {
                BrokerEvent::ReadinessChanged(readiness) => {
                    BrokerEvent::ReadinessChanged(self.gated_readiness(readiness))
                }
                event => event,
            };
            (AdapterEvent::new(envelope.id(), event), sequence_valid)
        })
    }

    pub(crate) fn acknowledge_event(
        &mut self,
        id: AdapterEventId,
        disposition: AdapterEventDisposition,
    ) {
        self.adapter.acknowledge_event(id, disposition);
    }

    pub(crate) fn orphan_retirement_policy(&self) -> OrphanRetirementPolicy {
        self.adapter.orphan_retirement_policy()
    }

    pub(crate) fn bounded_orphan_shutdown(&mut self) -> BoundedShutdownOutcome {
        self.adapter.bounded_orphan_shutdown()
    }

    const fn gated_readiness(&self, mut readiness: NativeReadiness) -> NativeReadiness {
        readiness.keyboard_build_eligible =
            readiness.keyboard_build_eligible && self.capture_gate.is_open();
        readiness
    }

    fn map_command(
        &self,
        command: ExecutorCommand,
    ) -> Result<Option<NativeEffect>, ExecutorResult> {
        let effect = match command {
            ExecutorCommand::State(RequiredAction::CloseFreshAdmission { token }) => {
                NativeEffect::CloseFreshAdmission { token }
            }
            ExecutorCommand::State(RequiredAction::EmergencyCloseFreshAdmission) => {
                NativeEffect::EmergencyCloseFreshAdmission
            }
            ExecutorCommand::State(RequiredAction::OpenFreshAdmission { token }) => {
                if !self.capture_gate.is_open() {
                    return Err(ExecutorResult::Failed(
                        NativeActionFailure::FailedNotApplied,
                    ));
                }
                NativeEffect::OpenFreshAdmission { token }
            }
            ExecutorCommand::State(RequiredAction::ApplySessionMode { token, mode }) => {
                if mode != SessionCaptureMode::Off && !self.capture_gate.is_open() {
                    return Err(ExecutorResult::Failed(
                        NativeActionFailure::FailedNotApplied,
                    ));
                }
                NativeEffect::ApplySessionMode { token, mode }
            }
            ExecutorCommand::State(RequiredAction::ApplyConfiguration {
                token,
                request,
                fence,
            }) => NativeEffect::ApplyConfiguration {
                token,
                request,
                fence,
            },
            ExecutorCommand::AdmitPaste {
                action:
                    RequiredAction::AdmitPaste {
                        token,
                        authorization,
                    },
                request,
            } if request.authorization == authorization => {
                NativeEffect::AdmitPaste { token, request }
            }
            ExecutorCommand::State(RequiredAction::CancelCandidate { token }) => {
                NativeEffect::CancelCandidate { token }
            }
            ExecutorCommand::State(RequiredAction::CancelWaitingPaste {
                token,
                authorization,
            }) => NativeEffect::CancelWaitingPaste {
                token,
                authorization,
            },
            ExecutorCommand::State(RequiredAction::PersistMaintenanceRecord { token, request }) => {
                NativeEffect::PersistMaintenanceRecord { token, request }
            }
            ExecutorCommand::State(RequiredAction::ContinueNativeDrain) => {
                NativeEffect::ContinueNativeDrain
            }
            ExecutorCommand::State(RequiredAction::StopNativeAdapter { token }) => {
                NativeEffect::StopNativeAdapter { token }
            }
            ExecutorCommand::State(RequiredAction::ExitOwner) => return Ok(None),
            ExecutorCommand::State(RequiredAction::AdmitPaste { .. })
            | ExecutorCommand::AdmitPaste { .. } => {
                return Err(ExecutorResult::ContractViolation);
            }
        };
        Ok(Some(effect))
    }

    fn map_result(effect: NativeEffect, result: NativeEffectResult) -> ExecutorResult {
        match (effect.kind(), result) {
            (
                NativeEffectKind::CloseFreshAdmission
                | NativeEffectKind::EmergencyCloseFreshAdmission,
                NativeEffectResult::AdmissionClosed { through_event },
            ) => ExecutorResult::AdmissionClosed { through_event },
            (NativeEffectKind::AdmitPaste, NativeEffectResult::PasteWaiting) => {
                ExecutorResult::PasteWaiting
            }
            (NativeEffectKind::AdmitPaste, NativeEffectResult::PasteRefused(reason)) => {
                ExecutorResult::PasteRefused(reason)
            }
            (
                NativeEffectKind::CancelCandidate,
                NativeEffectResult::CandidateCancelled(ownership),
            ) => ExecutorResult::CandidateCancelled(ownership),
            (_, NativeEffectResult::Failed(failure)) => ExecutorResult::Failed(failure),
            (
                NativeEffectKind::AdmitPaste
                | NativeEffectKind::CancelCandidate
                | NativeEffectKind::CloseFreshAdmission
                | NativeEffectKind::EmergencyCloseFreshAdmission,
                NativeEffectResult::Applied,
            )
            | (_, NativeEffectResult::AdmissionClosed { .. })
            | (_, NativeEffectResult::PasteWaiting | NativeEffectResult::PasteRefused(_))
            | (_, NativeEffectResult::CandidateCancelled(_)) => ExecutorResult::ContractViolation,
            (_, NativeEffectResult::Applied) => ExecutorResult::Applied,
        }
    }
}

impl<A: NativeAdapter, C: CapabilityIdSource> OwnerExecutor for NativeAdapterExecutor<A, C> {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        self.adapter.seed_startup_physical_snapshot()
    }

    fn readiness(&self) -> NativeReadiness {
        self.gated_readiness(self.adapter.readiness())
    }

    fn allocate_capability_id(&mut self) -> Option<CapabilityId> {
        self.capabilities.next_capability_id()
    }

    fn execute(&mut self, command: ExecutorCommand) -> ExecutorResult {
        match self.map_command(command) {
            Ok(Some(effect)) => {
                let result = self.adapter.execute(effect);
                Self::map_result(effect, result)
            }
            Ok(None) => ExecutorResult::Applied,
            Err(result) => result,
        }
    }

    fn permissions(&self) -> PermissionsResult {
        self.adapter.permissions()
    }

    fn front_app(&self) -> FrontAppResult {
        self.adapter.front_app()
    }

    fn front_app_metadata(&self) -> FrontAppMetadataResult {
        self.adapter.front_app_metadata()
    }

    fn observability(&self) -> ObservabilityResult {
        self.adapter.observability()
    }

    fn try_next_adapter_event(&mut self) -> Option<(AdapterEvent, bool)> {
        self.try_next_event()
    }

    fn acknowledge_adapter_event(
        &mut self,
        id: AdapterEventId,
        disposition: AdapterEventDisposition,
    ) {
        self.acknowledge_event(id, disposition);
    }
}
