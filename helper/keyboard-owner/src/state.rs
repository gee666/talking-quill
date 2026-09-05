//! Pure lifecycle model shared with feature-free structural tests for the out-of-process owner.
//!
//! The model owns no transport, timers, entropy, persistence, native callback,
//! process launch, or suppression authority. Outer layers allocate opaque IDs,
//! execute returned actions, and feed exact authoritative confirmations back.

use std::{fmt, num::NonZeroU64};

use talking_quill_keyboard_core::{
    ACTIVATION_KEY_CAPACITY, ActivationBindings, COMBINED_PHYSICAL_DRAIN_CAPACITY,
    OWNER_ADMITTED_EFFECT_CAPACITY, REPLAY_CLEANUP_EDGE_CAPACITY, SESSION_KEY_CAPACITY,
    SessionCaptureMode, transactional::ConfigIdentity as CoreConfigIdentity,
};
use thiserror::Error;

const AUTHORITY_BYTES: usize = 32;
const MAX_PENDING_ACTIONS: usize = OWNER_ADMITTED_EFFECT_CAPACITY;

macro_rules! opaque_id {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Copy, Eq, Hash, PartialEq)]
        pub struct $name([u8; AUTHORITY_BYTES]);

        impl $name {
            #[must_use]
            pub fn new(bytes: [u8; AUTHORITY_BYTES]) -> Option<Self> {
                bytes.iter().any(|byte| *byte != 0).then_some(Self(bytes))
            }

            /// Exact bytes for an authenticated executor/transport boundary.
            /// Debug formatting remains redacted.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; AUTHORITY_BYTES] {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!($label, "(<redacted>)"))
            }
        }
    };
}

opaque_id!(OwnerInstanceId, "OwnerInstanceId");
opaque_id!(CapabilityId, "CapabilityId");
opaque_id!(MaintenanceTransactionId, "MaintenanceTransactionId");
opaque_id!(MaintenanceHandoff, "MaintenanceHandoff");
opaque_id!(PasteOperationId, "PasteOperationId");
opaque_id!(BuildDigest, "BuildDigest");

mod identity;
pub use identity::*;

mod requests;
pub use requests::*;

// Pending configuration retains the same complete snapshot that was emitted;
// equality checks therefore reject swapped revision/snapshot confirmations.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum PendingActionKind {
    CloseAdmission,
    OpenAdmission,
    SessionMode(SessionCaptureMode),
    Configuration(ConfigurationRequest),
    AdmitPaste(PasteAuthorization),
    CancelCandidate,
    CancelPaste(PasteAuthorization),
    PersistMaintenance(MaintenanceRequest),
    StopNative,
}

impl PendingActionKind {
    const fn is_dependent(self) -> bool {
        matches!(
            self,
            Self::CancelCandidate
                | Self::CancelPaste(_)
                | Self::Configuration(_)
                | Self::PersistMaintenance(_)
        )
    }
}

impl fmt::Debug for PendingActionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PendingActionKind(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct PendingAction {
    token: NativeActionToken,
    kind: PendingActionKind,
    superseded: bool,
}

impl fmt::Debug for PendingAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PendingAction(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClosePlan {
    release_response: bool,
    apply_configuration: Option<ConfigurationRequest>,
    persist_maintenance: bool,
}

impl ClosePlan {
    fn merge(&mut self, other: Self) {
        self.release_response |= other.release_response;
        self.persist_maintenance |= other.persist_maintenance;
        if self.release_response {
            self.apply_configuration = None;
        } else if other.apply_configuration.is_some() {
            self.apply_configuration = other.apply_configuration;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DependentPlan {
    cancel_candidate: bool,
    cancel_paste: bool,
    apply_configuration: Option<ConfigurationRequest>,
    persist_maintenance: bool,
    continue_drain: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PastePhase {
    None,
    Admitting {
        authorization: PasteAuthorization,
        cancel_after_admit: bool,
    },
    Waiting(PasteAuthorization),
    Cancelling(PasteAuthorization),
    Claimed(PasteAuthorization),
    Indeterminate(PasteAuthorization),
}

impl PastePhase {
    const fn authorization(self) -> Option<PasteAuthorization> {
        match self {
            Self::None => None,
            Self::Admitting { authorization, .. }
            | Self::Waiting(authorization)
            | Self::Cancelling(authorization)
            | Self::Claimed(authorization)
            | Self::Indeterminate(authorization) => Some(authorization),
        }
    }
}

impl fmt::Debug for PastePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PastePhase(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MaintenanceState {
    None,
    Sealing {
        request: MaintenanceRequest,
        requester: Option<ConnectionId>,
        reserved_capability: CapabilityState,
    },
    Persisting {
        request: MaintenanceRequest,
        requester: Option<ConnectionId>,
        reserved_capability: CapabilityState,
    },
    Exclusive {
        request: MaintenanceRequest,
    },
    Sealed {
        request: MaintenanceRequest,
    },
    SealedFailed {
        request: MaintenanceRequest,
    },
}

impl MaintenanceState {
    const fn request(self) -> Option<MaintenanceRequest> {
        match self {
            Self::None => None,
            Self::Sealing { request, .. }
            | Self::Persisting { request, .. }
            | Self::Exclusive { request }
            | Self::Sealed { request }
            | Self::SealedFailed { request } => Some(request),
        }
    }

    const fn phase(self) -> MaintenancePhase {
        match self {
            Self::None => MaintenancePhase::None,
            Self::Sealing { .. } => MaintenancePhase::Sealing,
            Self::Persisting { .. } => MaintenancePhase::Persisting,
            Self::Exclusive { .. } => MaintenancePhase::Exclusive,
            Self::Sealed { .. } => MaintenancePhase::Sealed,
            Self::SealedFailed { .. } => MaintenancePhase::SealedFailed,
        }
    }
}

impl fmt::Debug for MaintenanceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MaintenanceState(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct PredecessorState {
    route: PredecessorRoute,
    high_water: u64,
    revoked_offered: bool,
    final_offered: bool,
    in_flight: Option<PredecessorTerminalOffer>,
}

impl fmt::Debug for PredecessorState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorState(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExitPurpose {
    Idle,
    Degraded,
    MaintenanceGuardLost,
    MaintenancePrepare(ResponseCorrelation),
    AbandonedMaintenancePrepare,
}

impl fmt::Debug for ExitPurpose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExitPurpose(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExitState {
    Running,
    NativeStopPending(ExitPurpose),
    NativeStoppedSealed,
    FinalResponseReady(ResponseCorrelation),
    FlushingResponse(ResponseCorrelation),
    Exiting,
}

impl ExitState {
    const fn public(self) -> ExitPhase {
        match self {
            Self::Running => ExitPhase::Running,
            Self::NativeStopPending(_) => ExitPhase::NativeStopPending,
            Self::NativeStoppedSealed => ExitPhase::NativeStoppedSealed,
            Self::FinalResponseReady(_) => ExitPhase::FinalResponseReady,
            Self::FlushingResponse(_) => ExitPhase::FlushingResponse,
            Self::Exiting => ExitPhase::Exiting,
        }
    }
}

impl fmt::Debug for ExitState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExitState(<redacted>)")
    }
}

/// Orthogonal controller, admission, ownership, process, rollback, and
/// maintenance state. Debug output is intentionally completely redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct KeyboardOwnerState {
    owner_instance: OwnerInstanceId,
    controller: ControllerInternal,
    ownership: NativeOwnership,
    paste_phase: PastePhase,
    process_health: ProcessHealth,
    exit: ExitState,
    rollback_latched: bool,
    admission: AdmissionState,
    readiness: NativeReadiness,
    startup_snapshot_seeded: bool,
    last_capture_epoch: u64,
    last_maintenance_epoch: u64,
    last_action_id: u64,
    pending_actions: [Option<PendingAction>; MAX_PENDING_ACTIONS],
    emergency_close_pending: bool,
    close_plan: Option<ClosePlan>,
    dependent_plan: DependentPlan,
    dependent_work_poisoned: bool,
    native_state_unknown: bool,
    terminal_unavailable_reason: Option<TerminalUnavailableReason>,
    impossible_native_observation: Option<NativeOwnershipObservation>,
    configuration_high_water: Option<ConfigurationRevision>,
    requested_configuration: Option<ConfigurationRequest>,
    applied_configuration: Option<ConfigurationIdentity>,
    applied_session_mode: Option<SessionCaptureMode>,
    maintenance: MaintenanceState,
    predecessor: Option<PredecessorState>,
}

impl fmt::Debug for KeyboardOwnerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyboardOwnerState(<redacted>)")
    }
}

impl KeyboardOwnerState {
    #[must_use]
    pub const fn new(owner_instance: OwnerInstanceId) -> Self {
        Self {
            owner_instance,
            controller: ControllerInternal::NoController,
            ownership: NativeOwnership::NEUTRAL,
            paste_phase: PastePhase::None,
            process_health: ProcessHealth::Starting,
            exit: ExitState::Running,
            rollback_latched: false,
            admission: AdmissionState::Closed,
            readiness: NativeReadiness {
                keyboard_build_eligible: false,
                paste_ready: false,
                permissions_eligible: false,
                hook_healthy: false,
            },
            startup_snapshot_seeded: false,
            last_capture_epoch: 0,
            last_maintenance_epoch: 0,
            last_action_id: 0,
            pending_actions: [None; MAX_PENDING_ACTIONS],
            emergency_close_pending: false,
            close_plan: None,
            dependent_plan: DependentPlan {
                cancel_candidate: false,
                cancel_paste: false,
                apply_configuration: None,
                persist_maintenance: false,
                continue_drain: false,
            },
            dependent_work_poisoned: false,
            native_state_unknown: false,
            terminal_unavailable_reason: None,
            impossible_native_observation: None,
            configuration_high_water: None,
            requested_configuration: None,
            applied_configuration: None,
            applied_session_mode: None,
            maintenance: MaintenanceState::None,
            predecessor: None,
        }
    }

    #[must_use]
    pub const fn owner_instance(&self) -> OwnerInstanceId {
        self.owner_instance
    }

    #[must_use]
    pub const fn controller(&self) -> ControllerState {
        self.controller.public()
    }

    #[must_use]
    pub const fn ownership(&self) -> NativeOwnership {
        self.ownership
    }

    #[must_use]
    pub const fn admission(&self) -> AdmissionState {
        self.admission
    }

    #[must_use]
    pub const fn readiness(&self) -> NativeReadiness {
        self.readiness
    }

    #[must_use]
    pub const fn exit_phase(&self) -> ExitPhase {
        self.exit.public()
    }

    #[must_use]
    pub const fn maintenance_phase(&self) -> MaintenancePhase {
        self.maintenance.phase()
    }

    #[must_use]
    pub const fn has_impossible_native_observation(&self) -> bool {
        self.impossible_native_observation.is_some()
    }

    #[must_use]
    pub const fn configuration_high_water(&self) -> Option<ConfigurationRevision> {
        self.configuration_high_water
    }

    #[must_use]
    pub const fn applied_configuration(&self) -> Option<ConfigurationIdentity> {
        self.applied_configuration
    }

    #[must_use]
    pub const fn applied_session_mode(&self) -> Option<SessionCaptureMode> {
        self.applied_session_mode
    }

    #[must_use]
    pub fn process(&self) -> ProcessState {
        match self.exit {
            ExitState::NativeStopPending(_)
            | ExitState::NativeStoppedSealed
            | ExitState::FinalResponseReady(_) => ProcessState::StoppingNative,
            ExitState::FlushingResponse(_) => ProcessState::FlushingResponse,
            ExitState::Exiting => ProcessState::Exiting,
            ExitState::Running => match self.process_health {
                ProcessHealth::Starting => ProcessState::Starting,
                ProcessHealth::Healthy if self.rollback_latched => ProcessState::RollbackLatched,
                ProcessHealth::Healthy => ProcessState::Healthy,
                ProcessHealth::Degraded => ProcessState::Degraded,
            },
        }
    }

    #[must_use]
    pub fn status(&self) -> OwnerStatus {
        OwnerStatus {
            reported_state: self.reported_state(),
            process_state: self.process(),
            rollback_latched: self.rollback_latched,
            native_state_unknown: self.native_state_unknown,
            maintenance_sealed: !matches!(self.maintenance, MaintenanceState::None),
            keyboard_build_eligible: self.readiness.keyboard_build_eligible,
            paste_ready: self.readiness.paste_ready,
            permissions_eligible: self.readiness.permissions_eligible,
            hook_healthy: self.readiness.hook_healthy,
        }
    }

    #[must_use]
    pub fn reported_state(&self) -> ReportedState {
        if matches!(self.process_health, ProcessHealth::Starting) {
            return ReportedState::Starting;
        }
        if !matches!(self.exit, ExitState::Running) {
            return ReportedState::Stopping;
        }
        if matches!(self.process_health, ProcessHealth::Degraded) {
            return ReportedState::DegradedDraining;
        }
        if !matches!(self.maintenance, MaintenanceState::None) {
            return if self.quiescent_for_neutral() {
                ReportedState::MaintenanceReady
            } else {
                ReportedState::MaintenanceDraining
            };
        }
        match self.controller {
            ControllerInternal::CaptureLeaseEnabled { .. } => ReportedState::LeaseEnabled,
            ControllerInternal::CaptureLeaseDisabled { .. } => {
                if self.quiescent_for_neutral() {
                    ReportedState::LeaseDisabled
                } else {
                    ReportedState::LeaseDraining
                }
            }
            ControllerInternal::NoController | ControllerInternal::AuthenticatedObserver { .. } => {
                if self.ownership.candidate == CandidateOwnership::Cancelling {
                    ReportedState::OrphanCancelling
                } else if self.quiescent_for_neutral() {
                    ReportedState::IdleNeutral
                } else {
                    ReportedState::OrphanDraining
                }
            }
            ControllerInternal::MaintenanceExclusive { .. } => ReportedState::MaintenanceDraining,
        }
    }

    #[must_use]
    pub fn can_open_keyboard(&self) -> bool {
        self.admission == AdmissionState::Closed
            && self.close_plan.is_none()
            && self.dependent_plan == DependentPlan::default()
            && self.enable_prerequisites_hold()
    }

    #[must_use]
    pub fn can_begin_paste(&self) -> bool {
        matches!(self.process_health, ProcessHealth::Healthy)
            && matches!(self.exit, ExitState::Running)
            && !self.rollback_latched
            && matches!(self.maintenance, MaintenanceState::None)
            && self.controller.capture().is_some()
            && matches!(self.paste_phase, PastePhase::None)
            && !self.native_state_unknown
            && self.readiness.paste_eligible()
    }
}

#[cfg(test)]
mod tests;

mod admission;
mod capture;
mod confirmation;
mod controller_loss;
mod maintenance;
mod pending_actions;
mod retirement;
