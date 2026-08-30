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

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ConnectionId(NonZeroU64);

impl ConnectionId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for ConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConnectionId(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CapabilityEpoch(NonZeroU64);

impl CapabilityEpoch {
    #[cfg(any(test, talking_quill_unoptimized_test_support))]
    #[doc(hidden)]
    #[must_use]
    pub const fn new_for_test(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for CapabilityEpoch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityEpoch(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CommandSequence(NonZeroU64);

impl CommandSequence {
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for CommandSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommandSequence(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CapabilityRef {
    id: CapabilityId,
    epoch: CapabilityEpoch,
}

impl CapabilityRef {
    #[must_use]
    pub const fn id(self) -> CapabilityId {
        self.id
    }

    #[must_use]
    pub const fn epoch(self) -> CapabilityEpoch {
        self.epoch
    }
}

impl fmt::Debug for CapabilityRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityRef(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct CapabilityState {
    authority: CapabilityRef,
    last_command_sequence: u64,
}

impl fmt::Debug for CapabilityState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilityState(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ControllerState {
    NoController,
    AuthenticatedObserver {
        connection: ConnectionId,
    },
    CaptureLeaseDisabled {
        connection: ConnectionId,
        authority: CapabilityRef,
    },
    CaptureLeaseEnabled {
        connection: ConnectionId,
        authority: CapabilityRef,
    },
    MaintenanceExclusive {
        connection: ConnectionId,
        authority: CapabilityRef,
    },
}

impl fmt::Debug for ControllerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::NoController => "NoController",
            Self::AuthenticatedObserver { .. } => "AuthenticatedObserver(<redacted>)",
            Self::CaptureLeaseDisabled { .. } => "CaptureLeaseDisabled(<redacted>)",
            Self::CaptureLeaseEnabled { .. } => "CaptureLeaseEnabled(<redacted>)",
            Self::MaintenanceExclusive { .. } => "MaintenanceExclusive(<redacted>)",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ControllerInternal {
    NoController,
    AuthenticatedObserver {
        connection: ConnectionId,
    },
    CaptureLeaseDisabled {
        connection: ConnectionId,
        capability: CapabilityState,
    },
    CaptureLeaseEnabled {
        connection: ConnectionId,
        capability: CapabilityState,
    },
    MaintenanceExclusive {
        connection: ConnectionId,
        capability: CapabilityState,
    },
}

impl ControllerInternal {
    const fn public(self) -> ControllerState {
        match self {
            Self::NoController => ControllerState::NoController,
            Self::AuthenticatedObserver { connection } => {
                ControllerState::AuthenticatedObserver { connection }
            }
            Self::CaptureLeaseDisabled {
                connection,
                capability,
            } => ControllerState::CaptureLeaseDisabled {
                connection,
                authority: capability.authority,
            },
            Self::CaptureLeaseEnabled {
                connection,
                capability,
            } => ControllerState::CaptureLeaseEnabled {
                connection,
                authority: capability.authority,
            },
            Self::MaintenanceExclusive {
                connection,
                capability,
            } => ControllerState::MaintenanceExclusive {
                connection,
                authority: capability.authority,
            },
        }
    }

    const fn connection(self) -> Option<ConnectionId> {
        match self {
            Self::NoController => None,
            Self::AuthenticatedObserver { connection }
            | Self::CaptureLeaseDisabled { connection, .. }
            | Self::CaptureLeaseEnabled { connection, .. }
            | Self::MaintenanceExclusive { connection, .. } => Some(connection),
        }
    }

    const fn capture(self) -> Option<(ConnectionId, CapabilityState, bool)> {
        match self {
            Self::CaptureLeaseDisabled {
                connection,
                capability,
            } => Some((connection, capability, false)),
            Self::CaptureLeaseEnabled {
                connection,
                capability,
            } => Some((connection, capability, true)),
            _ => None,
        }
    }
}

impl fmt::Debug for ControllerInternal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.public().fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CandidateOwnership {
    #[default]
    None,
    Active,
    Cancelling,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PasteOwnership {
    #[default]
    None,
    Waiting,
    Cancelling,
    Claimed,
    Indeterminate,
}

/// Bounded aggregate native ownership. It deliberately contains no key
/// identities, replay records, target evidence, or paste content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeOwnership {
    candidate: CandidateOwnership,
    activation_drain_keys: u8,
    session_drain_keys: u8,
    replay_cleanup_edges: u8,
    paste: PasteOwnership,
    conservative_native_work: bool,
    admitted_effects: u8,
}

/// Raw aggregate observation accepted from a native adapter before bounds are
/// trusted. Invalid counts are retained by the owner state for diagnostics-free
/// fail-closed handling rather than being masked or discarded.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct NativeOwnershipObservation {
    pub candidate: CandidateOwnership,
    pub activation_drain_keys: u16,
    pub session_drain_keys: u16,
    pub replay_cleanup_edges: u16,
    pub paste: PasteOwnership,
    pub conservative_native_work: bool,
    pub admitted_effects: u16,
}

impl fmt::Debug for NativeOwnershipObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeOwnershipObservation(<redacted>)")
    }
}

impl NativeOwnership {
    pub const NEUTRAL: Self = Self {
        candidate: CandidateOwnership::None,
        activation_drain_keys: 0,
        session_drain_keys: 0,
        replay_cleanup_edges: 0,
        paste: PasteOwnership::None,
        conservative_native_work: false,
        admitted_effects: 0,
    };

    pub fn new(
        candidate: CandidateOwnership,
        activation_drain_keys: u8,
        session_drain_keys: u8,
        replay_cleanup_edges: u8,
        paste: PasteOwnership,
        admitted_effects: u8,
    ) -> Result<Self, OwnershipError> {
        if usize::from(activation_drain_keys) > ACTIVATION_KEY_CAPACITY {
            return Err(OwnershipError::TooManyActivationKeys);
        }
        if usize::from(session_drain_keys) > SESSION_KEY_CAPACITY {
            return Err(OwnershipError::TooManySessionKeys);
        }
        if usize::from(activation_drain_keys) + usize::from(session_drain_keys)
            > COMBINED_PHYSICAL_DRAIN_CAPACITY
        {
            return Err(OwnershipError::TooManyCombinedKeys);
        }
        if usize::from(replay_cleanup_edges) > REPLAY_CLEANUP_EDGE_CAPACITY {
            return Err(OwnershipError::TooManyReplayCleanupEdges);
        }
        if usize::from(admitted_effects) > OWNER_ADMITTED_EFFECT_CAPACITY {
            return Err(OwnershipError::TooManyAdmittedEffects);
        }
        Ok(Self {
            candidate,
            activation_drain_keys,
            session_drain_keys,
            replay_cleanup_edges,
            paste,
            conservative_native_work: false,
            admitted_effects,
        })
    }

    pub(crate) fn with_conservative_native_work(mut self, pending: bool) -> Self {
        self.conservative_native_work = pending;
        self
    }

    #[must_use]
    pub const fn candidate(self) -> CandidateOwnership {
        self.candidate
    }

    #[must_use]
    pub const fn activation_drain_keys(self) -> u8 {
        self.activation_drain_keys
    }

    #[must_use]
    pub const fn session_drain_keys(self) -> u8 {
        self.session_drain_keys
    }

    #[must_use]
    pub const fn replay_cleanup_edges(self) -> u8 {
        self.replay_cleanup_edges
    }

    #[must_use]
    pub const fn paste(self) -> PasteOwnership {
        self.paste
    }

    #[must_use]
    pub const fn conservative_native_work(self) -> bool {
        self.conservative_native_work
    }

    #[must_use]
    pub const fn admitted_effects(self) -> u8 {
        self.admitted_effects
    }

    #[must_use]
    pub const fn is_native_neutral(self) -> bool {
        matches!(self.candidate, CandidateOwnership::None)
            && self.activation_drain_keys == 0
            && self.session_drain_keys == 0
            && self.replay_cleanup_edges == 0
            && matches!(self.paste, PasteOwnership::None)
            && !self.conservative_native_work
            && self.admitted_effects == 0
    }

    fn legal_while_keyboard_active(self, previous: Self) -> bool {
        let candidate = match previous.candidate {
            CandidateOwnership::None => matches!(
                self.candidate,
                CandidateOwnership::None | CandidateOwnership::Active
            ),
            CandidateOwnership::Active => true,
            CandidateOwnership::Cancelling => matches!(
                self.candidate,
                CandidateOwnership::Cancelling | CandidateOwnership::None
            ),
        };
        let cleanup = self.replay_cleanup_edges <= previous.replay_cleanup_edges
            || matches!(
                previous.candidate,
                CandidateOwnership::Active | CandidateOwnership::Cancelling
            );
        candidate && cleanup
    }

    fn legal_after_keyboard_closed(self, previous: Self) -> bool {
        if self.candidate == CandidateOwnership::Active
            || self.activation_drain_keys > previous.activation_drain_keys
            || self.session_drain_keys > previous.session_drain_keys
            || (self.conservative_native_work && !previous.conservative_native_work)
            || self.admitted_effects > previous.admitted_effects
        {
            return false;
        }
        if self.replay_cleanup_edges > previous.replay_cleanup_edges
            && previous.candidate != CandidateOwnership::Cancelling
        {
            return false;
        }
        match previous.candidate {
            CandidateOwnership::None => self.candidate == CandidateOwnership::None,
            CandidateOwnership::Active => false,
            CandidateOwnership::Cancelling => matches!(
                self.candidate,
                CandidateOwnership::Cancelling | CandidateOwnership::None
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum OwnershipError {
    #[error("activation ownership exceeds 26 keys")]
    TooManyActivationKeys,
    #[error("session ownership exceeds Escape and Enter")]
    TooManySessionKeys,
    #[error("combined physical ownership exceeds 28 keys")]
    TooManyCombinedKeys,
    #[error("replay cleanup exceeds 26 balancing releases")]
    TooManyReplayCleanupEdges,
    #[error("admitted effects exceed the fixed owner queue")]
    TooManyAdmittedEffects,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionState {
    Closed,
    Opening,
    Open,
    Closing,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Starting,
    Healthy,
    RollbackLatched,
    Degraded,
    StoppingNative,
    FlushingResponse,
    Exiting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessHealth {
    Starting,
    Healthy,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitPhase {
    Running,
    NativeStopPending,
    NativeStoppedSealed,
    FinalResponseReady,
    FlushingResponse,
    Exiting,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeReadiness {
    pub keyboard_build_eligible: bool,
    pub paste_ready: bool,
    pub permissions_eligible: bool,
    pub hook_healthy: bool,
}

impl NativeReadiness {
    #[must_use]
    pub const fn keyboard_eligible(self) -> bool {
        self.keyboard_build_eligible && self.permissions_eligible && self.hook_healthy
    }

    #[must_use]
    pub const fn paste_eligible(self) -> bool {
        // `paste_ready` is the adapter's independent aggregate fact. It must
        // not inherit keyboard build/hook readiness in feature-free test builds.
        self.paste_ready
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportedState {
    Starting,
    IdleNeutral,
    LeaseDisabled,
    LeaseEnabled,
    LeaseDraining,
    OrphanCancelling,
    OrphanDraining,
    MaintenanceDraining,
    DegradedDraining,
    MaintenanceReady,
    Stopping,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerStatus {
    pub reported_state: ReportedState,
    pub process_state: ProcessState,
    pub rollback_latched: bool,
    pub native_state_unknown: bool,
    pub maintenance_sealed: bool,
    pub keyboard_build_eligible: bool,
    pub paste_ready: bool,
    pub permissions_eligible: bool,
    pub hook_healthy: bool,
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConfigurationRevision(NonZeroU64);

impl ConfigurationRevision {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for ConfigurationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfigurationRevision(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ConfigurationIdentity {
    capture_epoch: CapabilityEpoch,
    revision: ConfigurationRevision,
}

impl ConfigurationIdentity {
    #[must_use]
    pub const fn capture_epoch(self) -> CapabilityEpoch {
        self.capture_epoch
    }

    #[must_use]
    pub const fn revision(self) -> ConfigurationRevision {
        self.revision
    }

    #[must_use]
    pub fn core_identity(self) -> CoreConfigIdentity {
        CoreConfigIdentity::scoped(self.capture_epoch.get(), self.revision.get())
            .expect("owner configuration identity is positive")
    }
}

impl fmt::Debug for ConfigurationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfigurationIdentity(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ConfigurationRequest {
    identity: ConfigurationIdentity,
    bindings: ActivationBindings,
}

impl ConfigurationRequest {
    #[must_use]
    pub const fn identity(self) -> ConfigurationIdentity {
        self.identity
    }

    #[must_use]
    pub const fn bindings(self) -> ActivationBindings {
        self.bindings
    }
}

impl fmt::Debug for ConfigurationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfigurationRequest(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnerActivationGeneration(NonZeroU64);

impl OwnerActivationGeneration {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for OwnerActivationGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerActivationGeneration(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PasteAuthorization {
    operation: PasteOperationId,
    owner_instance: OwnerInstanceId,
    capture_epoch: CapabilityEpoch,
    activation_generation: OwnerActivationGeneration,
}

impl PasteAuthorization {
    #[must_use]
    pub const fn new(
        operation: PasteOperationId,
        owner_instance: OwnerInstanceId,
        capture_epoch: CapabilityEpoch,
        activation_generation: OwnerActivationGeneration,
    ) -> Self {
        Self {
            operation,
            owner_instance,
            capture_epoch,
            activation_generation,
        }
    }

    #[must_use]
    pub const fn operation(self) -> PasteOperationId {
        self.operation
    }

    #[must_use]
    pub const fn owner_instance(self) -> OwnerInstanceId {
        self.owner_instance
    }

    #[must_use]
    pub const fn capture_epoch(self) -> CapabilityEpoch {
        self.capture_epoch
    }

    #[must_use]
    pub const fn activation_generation(self) -> OwnerActivationGeneration {
        self.activation_generation
    }
}

impl fmt::Debug for PasteAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasteAuthorization(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MaintenanceRequest {
    transaction: MaintenanceTransactionId,
    operation: MaintenanceOperation,
    source_build: BuildDigest,
    target_build: Option<BuildDigest>,
    target_owner: Option<BuildDigest>,
    owner_handoff: MaintenanceHandoff,
}

impl MaintenanceRequest {
    #[must_use]
    pub const fn new(
        transaction: MaintenanceTransactionId,
        operation: MaintenanceOperation,
        source_build: BuildDigest,
        target_build: Option<BuildDigest>,
        target_owner: Option<BuildDigest>,
        owner_handoff: MaintenanceHandoff,
    ) -> Option<Self> {
        let target_shape_valid = match operation {
            MaintenanceOperation::Uninstall => target_build.is_none() && target_owner.is_none(),
            MaintenanceOperation::Update | MaintenanceOperation::Rollback => {
                target_build.is_some() && target_owner.is_some()
            }
        };
        if target_shape_valid {
            Some(Self {
                transaction,
                operation,
                source_build,
                target_build,
                target_owner,
                owner_handoff,
            })
        } else {
            None
        }
    }

    #[must_use]
    pub const fn transaction(self) -> MaintenanceTransactionId {
        self.transaction
    }

    #[must_use]
    pub const fn operation(self) -> MaintenanceOperation {
        self.operation
    }

    #[must_use]
    pub const fn source_build(self) -> BuildDigest {
        self.source_build
    }

    #[must_use]
    pub const fn target_build(self) -> Option<BuildDigest> {
        self.target_build
    }

    #[must_use]
    pub const fn target_owner(self) -> Option<BuildDigest> {
        self.target_owner
    }

    #[must_use]
    pub const fn owner_handoff(self) -> MaintenanceHandoff {
        self.owner_handoff
    }
}

impl fmt::Debug for MaintenanceRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MaintenanceRequest(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenancePhase {
    None,
    Sealing,
    Persisting,
    Exclusive,
    Sealed,
    SealedFailed,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ResponseCorrelation(NonZeroU64);

impl ResponseCorrelation {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl fmt::Debug for ResponseCorrelation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseCorrelation(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum ActionScope {
    Process,
    Capture(CapabilityEpoch),
    Maintenance(CapabilityEpoch),
}

impl fmt::Debug for ActionScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActionScope(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct NativeActionToken {
    owner_instance: OwnerInstanceId,
    id: NonZeroU64,
    scope: ActionScope,
}

impl fmt::Debug for NativeActionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeActionToken(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredActionKind {
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
    ExitOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreHeldKeyFence {
    FenceCurrentPhysical,
}

// Configuration dispatch deliberately carries the complete immutable snapshot;
// indirection would weaken the reference model's atomic snapshot contract.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum RequiredAction {
    CloseFreshAdmission {
        token: NativeActionToken,
    },
    /// Non-tokenized fail-closed directive used only when the non-wrapping
    /// action-ID domain itself is exhausted.
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
        authorization: PasteAuthorization,
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
    ExitOwner,
}

impl RequiredAction {
    #[must_use]
    pub const fn kind(self) -> RequiredActionKind {
        match self {
            Self::CloseFreshAdmission { .. } => RequiredActionKind::CloseFreshAdmission,
            Self::EmergencyCloseFreshAdmission => RequiredActionKind::EmergencyCloseFreshAdmission,
            Self::OpenFreshAdmission { .. } => RequiredActionKind::OpenFreshAdmission,
            Self::ApplySessionMode { .. } => RequiredActionKind::ApplySessionMode,
            Self::ApplyConfiguration { .. } => RequiredActionKind::ApplyConfiguration,
            Self::AdmitPaste { .. } => RequiredActionKind::AdmitPaste,
            Self::CancelCandidate { .. } => RequiredActionKind::CancelCandidate,
            Self::CancelWaitingPaste { .. } => RequiredActionKind::CancelWaitingPaste,
            Self::PersistMaintenanceRecord { .. } => RequiredActionKind::PersistMaintenanceRecord,
            Self::ContinueNativeDrain => RequiredActionKind::ContinueNativeDrain,
            Self::StopNativeAdapter { .. } => RequiredActionKind::StopNativeAdapter,
            Self::ExitOwner => RequiredActionKind::ExitOwner,
        }
    }

    #[must_use]
    pub const fn token(self) -> Option<NativeActionToken> {
        match self {
            Self::CloseFreshAdmission { token }
            | Self::OpenFreshAdmission { token }
            | Self::ApplySessionMode { token, .. }
            | Self::ApplyConfiguration { token, .. }
            | Self::AdmitPaste { token, .. }
            | Self::CancelCandidate { token }
            | Self::CancelWaitingPaste { token, .. }
            | Self::PersistMaintenanceRecord { token, .. }
            | Self::StopNativeAdapter { token } => Some(token),
            Self::EmergencyCloseFreshAdmission | Self::ContinueNativeDrain | Self::ExitOwner => {
                None
            }
        }
    }
}

impl fmt::Debug for RequiredAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind() {
            RequiredActionKind::CloseFreshAdmission => {
                "RequiredAction::CloseFreshAdmission(<redacted>)"
            }
            RequiredActionKind::EmergencyCloseFreshAdmission => {
                "RequiredAction::EmergencyCloseFreshAdmission"
            }
            RequiredActionKind::OpenFreshAdmission => {
                "RequiredAction::OpenFreshAdmission(<redacted>)"
            }
            RequiredActionKind::ApplySessionMode => "RequiredAction::ApplySessionMode(<redacted>)",
            RequiredActionKind::ApplyConfiguration => {
                "RequiredAction::ApplyConfiguration(<redacted>)"
            }
            RequiredActionKind::AdmitPaste => "RequiredAction::AdmitPaste(<redacted>)",
            RequiredActionKind::CancelCandidate => "RequiredAction::CancelCandidate(<redacted>)",
            RequiredActionKind::CancelWaitingPaste => {
                "RequiredAction::CancelWaitingPaste(<redacted>)"
            }
            RequiredActionKind::PersistMaintenanceRecord => {
                "RequiredAction::PersistMaintenanceRecord(<redacted>)"
            }
            RequiredActionKind::ContinueNativeDrain => "RequiredAction::ContinueNativeDrain",
            RequiredActionKind::StopNativeAdapter => {
                "RequiredAction::StopNativeAdapter(<redacted>)"
            }
            RequiredActionKind::ExitOwner => "RequiredAction::ExitOwner",
        })
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct RequiredActions(Vec<RequiredAction>);

impl RequiredActions {
    #[must_use]
    pub fn as_slice(&self) -> &[RequiredAction] {
        &self.0
    }

    #[must_use]
    pub fn contains_kind(&self, kind: RequiredActionKind) -> bool {
        self.0.iter().any(|action| action.kind() == kind)
    }

    fn push(&mut self, action: RequiredAction) {
        assert!(
            self.0.len() < OWNER_ADMITTED_EFFECT_CAPACITY,
            "transition action bound"
        );
        self.0.push(action);
    }

    fn append(&mut self, other: &mut Self) {
        while let Some(action) = other.0.first().copied() {
            self.push(action);
            other.0.remove(0);
        }
    }
}

impl fmt::Debug for RequiredActions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|action| action.kind()))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseDisposition {
    Neutral,
    Draining,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseStage {
    MaintenanceAcquireReady,
    FinalResponseReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalReason {
    Eof,
    Heartbeat,
    Maintenance,
    Release,
    Protocol,
    Rollback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOwnership {
    Candidate,
    Activation,
    Session,
    ReplayCleanup,
    Paste,
    Multiple,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalUnavailableReason {
    NativeFault,
    OwnershipUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PredecessorTerminalEvent {
    LeaseRevoked(TerminalReason),
    LeaseDraining(TerminalOwnership),
    LeaseNeutral,
    LeaseUnavailable(TerminalUnavailableReason),
}

impl PredecessorTerminalEvent {
    const fn is_final(self) -> bool {
        matches!(self, Self::LeaseNeutral | Self::LeaseUnavailable(_))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct PredecessorRoute {
    owner_instance: OwnerInstanceId,
    connection: ConnectionId,
    authority: CapabilityRef,
}

impl fmt::Debug for PredecessorRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorRoute(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PredecessorTerminalOffer {
    route: PredecessorRoute,
    sequence: NonZeroU64,
    event: PredecessorTerminalEvent,
}

impl PredecessorTerminalOffer {
    #[must_use]
    pub const fn event(self) -> PredecessorTerminalEvent {
        self.event
    }

    #[must_use]
    pub const fn connection(self) -> ConnectionId {
        self.route.connection
    }

    #[must_use]
    pub const fn authority(self) -> CapabilityRef {
        self.route.authority
    }

    #[must_use]
    pub const fn owner_instance(self) -> OwnerInstanceId {
        self.route.owner_instance
    }

    #[must_use]
    pub const fn terminal_sequence(self) -> u64 {
        self.sequence.get()
    }

    #[must_use]
    pub fn same_route_as(self, other: Self) -> bool {
        self.route.owner_instance == other.route.owner_instance
            && self.route.connection == other.route.connection
            && self.route.authority == other.route.authority
    }
}

impl fmt::Debug for PredecessorTerminalOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorTerminalOffer(<redacted>)")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct Transition {
    actions: RequiredActions,
    lease_disposition: Option<LeaseDisposition>,
    terminal_offer: Option<PredecessorTerminalOffer>,
    response_stage: Option<ResponseStage>,
}

impl Transition {
    fn empty() -> Self {
        Self {
            actions: RequiredActions::default(),
            lease_disposition: None,
            terminal_offer: None,
            response_stage: None,
        }
    }

    #[must_use]
    pub const fn actions(&self) -> &RequiredActions {
        &self.actions
    }

    #[must_use]
    pub const fn lease_disposition(&self) -> Option<LeaseDisposition> {
        self.lease_disposition
    }

    #[must_use]
    pub const fn terminal_offer(&self) -> Option<PredecessorTerminalOffer> {
        self.terminal_offer
    }

    #[must_use]
    pub const fn response_stage(&self) -> Option<ResponseStage> {
        self.response_stage
    }

    fn merge(&mut self, mut other: Self) {
        self.actions.append(&mut other.actions);
        if self.lease_disposition.is_none() {
            self.lease_disposition = other.lease_disposition;
        }
        if self.terminal_offer.is_none() {
            self.terminal_offer = other.terminal_offer;
        }
        if self.response_stage.is_none() {
            self.response_stage = other.response_stage;
        }
    }
}

impl fmt::Debug for Transition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Transition(<redacted>)")
    }
}

// Replacement is intentionally a complete by-value snapshot in this pure
// reference model rather than an externally mutable handle.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CaptureCommand {
    Renew,
    ReconcileSessionOff,
    SetSessionMode(SessionCaptureMode),
    ReplaceConfiguration {
        revision: ConfigurationRevision,
        bindings: ActivationBindings,
    },
    Enable,
    Disable,
    BeginPaste(PasteAuthorization),
    Release,
    RuntimeRollback,
}

impl fmt::Debug for CaptureCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Renew => "CaptureCommand::Renew",
            Self::ReconcileSessionOff => "CaptureCommand::ReconcileSessionOff",
            Self::SetSessionMode(_) => "CaptureCommand::SetSessionMode(<redacted>)",
            Self::ReplaceConfiguration { .. } => "CaptureCommand::ReplaceConfiguration(<redacted>)",
            Self::Enable => "CaptureCommand::Enable",
            Self::Disable => "CaptureCommand::Disable",
            Self::BeginPaste(_) => "CaptureCommand::BeginPaste(<redacted>)",
            Self::Release => "CaptureCommand::Release",
            Self::RuntimeRollback => "CaptureCommand::RuntimeRollback",
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum MaintenanceCommand {
    Renew,
    /// Consumes an authenticated current-capability sequence for a request
    /// rejected by the protocol mapper without extending lease liveness or
    /// dispatching native work.
    ConsumeSemanticRejection,
    Prepare {
        operation: MaintenanceOperation,
        response_correlation: ResponseCorrelation,
    },
}

impl fmt::Debug for MaintenanceCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Renew => "MaintenanceCommand::Renew",
            Self::ConsumeSemanticRejection => "MaintenanceCommand::ConsumeSemanticRejection",
            Self::Prepare { .. } => "MaintenanceCommand::Prepare(<redacted>)",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerLossReason {
    Eof,
    HeartbeatExpired,
    MacFault,
    ProtocolFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeActionFailure {
    FailedNotApplied,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum TransitionErrorKind {
    #[error("owner startup has not completed")]
    Starting,
    #[error("the startup physical snapshot has not been seeded")]
    StartupSnapshotRequired,
    #[error("owner is stopping or exiting")]
    Stopping,
    #[error("another controller is active")]
    Busy,
    #[error("native ownership has not drained")]
    Draining,
    #[error("capture is sealed for maintenance")]
    MaintenanceSealed,
    #[error("runtime rollback is latched")]
    RollbackLatched,
    #[error("owner health is degraded")]
    Degraded,
    #[error("the command came from the wrong controller")]
    WrongController,
    #[error("capability, epoch, or command sequence is invalid")]
    ProtocolFault,
    #[error("the capture lease must be disabled")]
    LeaseMustBeDisabled,
    #[error("a native transition is still pending")]
    AdmissionTransitionPending,
    #[error("a full activation configuration is required")]
    ConfigurationRequired,
    #[error("session capture must first be reconciled off")]
    SessionOffReconciliationRequired,
    #[error("native readiness is not eligible")]
    NativeReadinessRequired,
    #[error("configuration revision is not newer")]
    InvalidConfigurationRevision,
    #[error("paste scope does not match the current owner and capture capability")]
    PasteScopeMismatch,
    #[error("paste is unavailable or another paste conflicts")]
    PasteUnavailable,
    #[error("maintenance transaction does not match the sealed transaction")]
    MaintenanceTransactionMismatch,
    #[error("maintenance operation does not match the sealed transaction")]
    MaintenanceOperationMismatch,
    #[error("a native confirmation did not match the pending request")]
    NativeConfirmationMismatch,
    #[error("the native ownership transition is impossible")]
    InvalidOwnershipTransition,
    #[error("a capability epoch exhausted its non-wrapping range")]
    EpochExhausted,
    #[error("the owner-native action sequence exhausted its non-wrapping range")]
    ActionIdExhausted,
    #[error("the final response correlation or phase is invalid")]
    ResponseFlushMismatch,
}

#[derive(Clone, Eq, PartialEq)]
pub struct TransitionError {
    kind: TransitionErrorKind,
    actions: RequiredActions,
    terminal_offer: Option<Box<PredecessorTerminalOffer>>,
}

impl TransitionError {
    fn new(kind: TransitionErrorKind) -> Self {
        Self {
            kind,
            actions: RequiredActions::default(),
            terminal_offer: None,
        }
    }

    fn with_transition(kind: TransitionErrorKind, transition: Transition) -> Self {
        Self {
            kind,
            actions: transition.actions,
            terminal_offer: transition.terminal_offer.map(Box::new),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> TransitionErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn actions(&self) -> &RequiredActions {
        &self.actions
    }

    #[must_use]
    pub fn terminal_offer(&self) -> Option<PredecessorTerminalOffer> {
        self.terminal_offer.as_deref().copied()
    }
}

impl fmt::Debug for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TransitionError(<redacted>)")
    }
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)
    }
}

impl std::error::Error for TransitionError {}

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

    pub fn confirm_startup_snapshot_seeded(&mut self) -> Result<Transition, TransitionError> {
        if !matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        self.startup_snapshot_seeded = true;
        Ok(Transition::empty())
    }

    pub fn startup_completed(&mut self) -> Result<Transition, TransitionError> {
        if !matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        if !self.startup_snapshot_seeded {
            return Err(TransitionError::new(
                TransitionErrorKind::StartupSnapshotRequired,
            ));
        }
        if !self.ownership.is_native_neutral() || self.admission != AdmissionState::Closed {
            return Err(TransitionError::new(
                TransitionErrorKind::InvalidOwnershipTransition,
            ));
        }
        self.process_health = ProcessHealth::Healthy;
        Ok(Transition::empty())
    }

    pub fn observe_native_readiness(
        &mut self,
        readiness: NativeReadiness,
    ) -> Result<Transition, TransitionError> {
        let keyboard_lost = (self.readiness.keyboard_build_eligible
            && !readiness.keyboard_build_eligible)
            || (self.readiness.permissions_eligible && !readiness.permissions_eligible)
            || (self.readiness.hook_healthy && !readiness.hook_healthy);
        let paste_lost = self.readiness.paste_ready && !readiness.paste_ready;
        self.readiness = readiness;
        if keyboard_lost {
            self.invalidate_reconciliation();
        }
        if paste_lost {
            self.cancel_unclaimed_paste();
        }
        if keyboard_lost && !matches!(self.admission, AdmissionState::Closed) {
            self.request_close(ClosePlan::default())
        } else {
            let mut transition = Transition::empty();
            self.issue_next_dependent(&mut transition.actions)?;
            Ok(transition)
        }
    }

    pub fn authenticate_observer(
        &mut self,
        connection: ConnectionId,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if !matches!(self.controller, ControllerInternal::NoController) {
            return Err(TransitionError::new(TransitionErrorKind::Busy));
        }
        self.controller = ControllerInternal::AuthenticatedObserver { connection };
        Ok(Transition::empty())
    }

    pub fn acquire_capture_lease(
        &mut self,
        connection: ConnectionId,
        capability_id: CapabilityId,
    ) -> Result<Transition, TransitionError> {
        self.ensure_process_healthy()?;
        if self.rollback_latched {
            return Err(TransitionError::new(TransitionErrorKind::RollbackLatched));
        }
        if !matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(TransitionErrorKind::MaintenanceSealed));
        }
        if !self.quiescent_for_neutral() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        match self.controller {
            ControllerInternal::AuthenticatedObserver {
                connection: observer,
            } if observer == connection => {}
            ControllerInternal::AuthenticatedObserver { .. }
            | ControllerInternal::CaptureLeaseDisabled { .. }
            | ControllerInternal::CaptureLeaseEnabled { .. }
            | ControllerInternal::MaintenanceExclusive { .. } => {
                return Err(TransitionError::new(TransitionErrorKind::Busy));
            }
            ControllerInternal::NoController => {
                return Err(TransitionError::new(TransitionErrorKind::WrongController));
            }
        }
        let epoch = Self::next_epoch(&mut self.last_capture_epoch)?;
        self.controller = ControllerInternal::CaptureLeaseDisabled {
            connection,
            capability: CapabilityState {
                authority: CapabilityRef {
                    id: capability_id,
                    epoch,
                },
                last_command_sequence: 0,
            },
        };
        self.configuration_high_water = None;
        self.invalidate_reconciliation();
        Ok(Transition::empty())
    }

    pub fn apply_capture_command(
        &mut self,
        connection: ConnectionId,
        authority: CapabilityRef,
        sequence: CommandSequence,
        command: CaptureCommand,
    ) -> Result<Transition, TransitionError> {
        let (enabled, mut capability) = match self.controller.capture() {
            Some((active, capability, enabled)) if active == connection => (enabled, capability),
            _ => return Err(self.command_for_wrong_controller(connection)),
        };
        if capability.authority != authority
            || capability
                .last_command_sequence
                .checked_add(1)
                .is_none_or(|expected| expected != sequence.get())
        {
            let transition = self.lose_active_controller(TerminalReason::Protocol)?;
            return Err(TransitionError::with_transition(
                TransitionErrorKind::ProtocolFault,
                transition,
            ));
        }
        capability.last_command_sequence = sequence.get();
        self.controller = if enabled {
            ControllerInternal::CaptureLeaseEnabled {
                connection,
                capability,
            }
        } else {
            ControllerInternal::CaptureLeaseDisabled {
                connection,
                capability,
            }
        };

        if matches!(command, CaptureCommand::RuntimeRollback) {
            return self.apply_priority_rollback(Some(TerminalReason::Rollback));
        }
        self.ensure_capture_command_allowed(command)?;
        if !matches!(
            command,
            CaptureCommand::Renew | CaptureCommand::Disable | CaptureCommand::Release
        ) && !self.pending_actions_empty()
        {
            return Err(TransitionError::new(
                TransitionErrorKind::AdmissionTransitionPending,
            ));
        }

        match command {
            CaptureCommand::Renew => Ok(Transition::empty()),
            CaptureCommand::ReconcileSessionOff => {
                self.request_session_mode(authority.epoch, SessionCaptureMode::Off)
            }
            CaptureCommand::SetSessionMode(mode) => {
                if mode != SessionCaptureMode::Off
                    && (!enabled || self.admission != AdmissionState::Open)
                {
                    return Err(TransitionError::new(
                        TransitionErrorKind::LeaseMustBeDisabled,
                    ));
                }
                self.request_session_mode(authority.epoch, mode)
            }
            CaptureCommand::ReplaceConfiguration { revision, bindings } => {
                if self
                    .configuration_high_water
                    .is_some_and(|current| revision <= current)
                {
                    return Err(TransitionError::new(
                        TransitionErrorKind::InvalidConfigurationRevision,
                    ));
                }
                let request = ConfigurationRequest {
                    identity: ConfigurationIdentity {
                        capture_epoch: authority.epoch,
                        revision,
                    },
                    bindings,
                };
                // Reserve before any allocation/native dispatch. Nothing may
                // lower this high-water within the capture epoch.
                self.configuration_high_water = Some(revision);
                self.requested_configuration = Some(request);
                self.applied_configuration = None;
                self.request_close(ClosePlan {
                    apply_configuration: Some(request),
                    ..ClosePlan::default()
                })
            }
            CaptureCommand::Enable => {
                self.ensure_enable_allowed()?;
                let mut transition = Transition::empty();
                self.issue_action(
                    PendingActionKind::OpenAdmission,
                    ActionScope::Capture(authority.epoch),
                    &mut transition.actions,
                )?;
                self.admission = AdmissionState::Opening;
                Ok(transition)
            }
            CaptureCommand::Disable => self.request_close(ClosePlan::default()),
            CaptureCommand::BeginPaste(authorization) => self.begin_paste(authority, authorization),
            CaptureCommand::Release => {
                let mut transition = self.snapshot_predecessor(TerminalReason::Release);
                self.cancel_unclaimed_paste();
                self.controller = ControllerInternal::NoController;
                self.invalidate_reconciliation();
                let close = self.request_close(ClosePlan {
                    release_response: true,
                    ..ClosePlan::default()
                });
                match close {
                    Ok(close) => {
                        transition.merge(close);
                        Ok(transition)
                    }
                    Err(mut error) => {
                        if error.terminal_offer.is_none() {
                            error.terminal_offer = transition.terminal_offer.map(Box::new);
                        }
                        Err(error)
                    }
                }
            }
            CaptureCommand::RuntimeRollback => unreachable!("priority rollback handled above"),
        }
    }

    pub fn confirm_session_mode_applied(
        &mut self,
        token: NativeActionToken,
        mode: SessionCaptureMode,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::SessionMode(mode))?;
        if !pending.superseded {
            self.applied_session_mode = Some(mode);
        }
        Ok(Transition::empty())
    }

    pub fn confirm_configuration_applied(
        &mut self,
        token: NativeActionToken,
        request: ConfigurationRequest,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::Configuration(request))?;
        if self.admission != AdmissionState::Closed {
            return Err(self.native_confirmation_fault());
        }
        if !pending.superseded && self.requested_configuration == Some(request) {
            self.applied_configuration = Some(request.identity);
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_admission_opened(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_action(token, PendingActionKind::OpenAdmission)?;
        if pending.superseded || self.admission == AdmissionState::Closing {
            return Ok(Transition::empty());
        }
        if self.admission != AdmissionState::Opening || !self.enable_prerequisites_hold() {
            return Err(self.native_confirmation_fault());
        }
        let ControllerInternal::CaptureLeaseDisabled {
            connection,
            capability,
        } = self.controller
        else {
            return Err(self.native_confirmation_fault());
        };
        self.admission = AdmissionState::Open;
        self.controller = ControllerInternal::CaptureLeaseEnabled {
            connection,
            capability,
        };
        Ok(Transition::empty())
    }

    pub fn confirm_emergency_admission_closed(&mut self) -> Result<Transition, TransitionError> {
        if !self.emergency_close_pending {
            return Err(self.native_confirmation_fault());
        }
        if !matches!(
            self.admission,
            AdmissionState::Closing | AdmissionState::Unknown
        ) || self.ownership.admitted_effects != 0
        {
            return Err(self.native_confirmation_fault());
        }
        self.emergency_close_pending = false;
        self.admission = AdmissionState::Closed;
        let plan = self.close_plan.take().unwrap_or_default();
        self.after_admission_closed(plan)
    }

    pub fn confirm_admission_closed(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CloseAdmission)?;
        if !matches!(
            self.admission,
            AdmissionState::Closing | AdmissionState::Unknown
        ) || self.ownership.admitted_effects != 0
        {
            return Err(self.native_confirmation_fault());
        }
        self.admission = AdmissionState::Closed;
        let plan = self.close_plan.take().unwrap_or_default();
        self.after_admission_closed(plan)
    }

    pub fn confirm_paste_waiting(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        let pending =
            self.take_pending_action(token, PendingActionKind::AdmitPaste(authorization))?;
        let PastePhase::Admitting {
            authorization: expected,
            cancel_after_admit,
        } = self.paste_phase
        else {
            return Err(self.native_confirmation_fault());
        };
        if expected != authorization {
            return Err(self.native_confirmation_fault());
        }
        self.ownership.paste = PasteOwnership::Waiting;
        if cancel_after_admit || pending.superseded {
            self.paste_phase = PastePhase::Cancelling(authorization);
            self.ownership.paste = PasteOwnership::Cancelling;
            self.dependent_plan.cancel_paste = true;
        } else {
            self.paste_phase = PastePhase::Waiting(authorization);
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_paste_refused(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::AdmitPaste(authorization))?;
        if self.paste_phase.authorization() != Some(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_paste_claimed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if self.paste_phase != PastePhase::Waiting(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::Claimed(authorization);
        self.ownership.paste = PasteOwnership::Claimed;
        Ok(Transition::empty())
    }

    pub fn confirm_paste_indeterminate(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if self.paste_phase != PastePhase::Claimed(authorization) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::Indeterminate(authorization);
        self.ownership.paste = PasteOwnership::Indeterminate;
        Ok(Transition::empty())
    }

    pub fn confirm_paste_completed(
        &mut self,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if !matches!(
            self.paste_phase,
            PastePhase::Claimed(current) | PastePhase::Indeterminate(current)
                if current == authorization
        ) {
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_candidate_cancelled(
        &mut self,
        token: NativeActionToken,
        resulting_ownership: NativeOwnership,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CancelCandidate)?;
        if resulting_ownership.candidate == CandidateOwnership::Active
            || resulting_ownership.paste != self.ownership.paste
            || !resulting_ownership.legal_after_keyboard_closed(self.ownership)
        {
            self.dependent_plan = DependentPlan::default();
            self.dependent_work_poisoned = true;
            return Err(self.native_confirmation_fault());
        }
        self.ownership = resulting_ownership;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn confirm_waiting_paste_cancelled(
        &mut self,
        token: NativeActionToken,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::CancelPaste(authorization))?;
        if self.paste_phase != PastePhase::Cancelling(authorization) {
            self.dependent_plan = DependentPlan::default();
            self.dependent_work_poisoned = true;
            return Err(self.native_confirmation_fault());
        }
        self.paste_phase = PastePhase::None;
        self.ownership.paste = PasteOwnership::None;
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    pub fn fail_native_action(
        &mut self,
        token: NativeActionToken,
        failure: NativeActionFailure,
    ) -> Result<Transition, TransitionError> {
        let pending = self.take_pending_by_token(token)?;
        let mut transition = Transition::empty();
        match (pending.kind, failure) {
            (PendingActionKind::CloseAdmission, NativeActionFailure::FailedNotApplied) => {
                self.issue_action(
                    PendingActionKind::CloseAdmission,
                    token.scope,
                    &mut transition.actions,
                )?;
            }
            (PendingActionKind::CloseAdmission, NativeActionFailure::Indeterminate)
            | (PendingActionKind::OpenAdmission, NativeActionFailure::Indeterminate) => {
                self.degrade_unknown();
                self.admission = AdmissionState::Unknown;
                self.close_plan.get_or_insert_with(ClosePlan::default);
                if !self.has_pending_kind(PendingActionKind::CloseAdmission) {
                    self.issue_action(
                        PendingActionKind::CloseAdmission,
                        token.scope,
                        &mut transition.actions,
                    )?;
                }
            }
            (PendingActionKind::OpenAdmission, NativeActionFailure::FailedNotApplied) => {
                if self.admission == AdmissionState::Opening {
                    self.admission = AdmissionState::Closed;
                }
                if self.close_plan.is_some() {
                    transition.merge(self.request_close(ClosePlan::default())?);
                }
            }
            (PendingActionKind::SessionMode(_), _) => {
                self.applied_session_mode = None;
            }
            (PendingActionKind::Configuration(_), _) => {
                self.applied_configuration = None;
                self.issue_next_dependent(&mut transition.actions)?;
            }
            (
                PendingActionKind::AdmitPaste(authorization),
                NativeActionFailure::FailedNotApplied,
            ) => {
                if self.paste_phase.authorization() == Some(authorization) {
                    self.paste_phase = PastePhase::None;
                    self.ownership.paste = PasteOwnership::None;
                }
                self.issue_next_dependent(&mut transition.actions)?;
            }
            (PendingActionKind::AdmitPaste(authorization), NativeActionFailure::Indeterminate) => {
                self.paste_phase = PastePhase::Indeterminate(authorization);
                self.ownership.paste = PasteOwnership::Indeterminate;
                self.degrade_unknown();
                transition.merge(self.request_close(ClosePlan::default())?);
            }
            (PendingActionKind::CancelCandidate | PendingActionKind::CancelPaste(_), _) => {
                self.degrade_unknown();
                // No later dependent step may be dispatched after uncertain
                // cancellation. Retained physical facts remain in ownership.
                self.dependent_plan = DependentPlan::default();
                self.dependent_work_poisoned = true;
            }
            (PendingActionKind::PersistMaintenance(request), _) => {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.process_health = ProcessHealth::Degraded;
                if !self.native_state_unknown {
                    self.terminal_unavailable_reason = Some(TerminalUnavailableReason::NativeFault);
                }
                self.dependent_plan.persist_maintenance = false;
            }
            (PendingActionKind::StopNative, _) => {
                self.exit = ExitState::Running;
                self.process_health = ProcessHealth::Degraded;
                if !self.native_state_unknown {
                    self.terminal_unavailable_reason = Some(TerminalUnavailableReason::NativeFault);
                }
            }
        }
        Ok(transition)
    }

    /// Consumes the exact pending token, applies the conservative
    /// indeterminate failure semantics for that action, then latches a known
    /// adapter-contract fault so no later enablement is possible.
    pub fn fail_native_adapter_contract(
        &mut self,
        token: Option<NativeActionToken>,
    ) -> Result<Transition, TransitionError> {
        let mut transition = if let Some(token) = token {
            self.fail_native_action(token, NativeActionFailure::Indeterminate)?
        } else {
            Transition::empty()
        };
        transition.merge(self.adapter_stream_desynchronized()?);
        Ok(transition)
    }

    pub fn acquire_maintenance(
        &mut self,
        connection: ConnectionId,
        capability_id: CapabilityId,
        request: MaintenanceRequest,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if let Some(existing) = self.maintenance.request() {
            if existing != request {
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
            if matches!(self.maintenance, MaintenanceState::SealedFailed { .. }) {
                return Err(TransitionError::new(TransitionErrorKind::Degraded));
            }
            if !matches!(self.maintenance, MaintenanceState::Sealed { .. })
                || !self.quiescent_for_neutral()
            {
                return Err(TransitionError::new(TransitionErrorKind::Draining));
            }
            let epoch = Self::next_epoch(&mut self.last_maintenance_epoch)?;
            self.controller = ControllerInternal::MaintenanceExclusive {
                connection,
                capability: CapabilityState {
                    authority: CapabilityRef {
                        id: capability_id,
                        epoch,
                    },
                    last_command_sequence: 0,
                },
            };
            self.maintenance = MaintenanceState::Exclusive { request };
            let mut transition = Transition::empty();
            transition.response_stage = Some(ResponseStage::MaintenanceAcquireReady);
            return Ok(transition);
        }

        let epoch = Self::next_epoch(&mut self.last_maintenance_epoch)?;
        let reserved_capability = CapabilityState {
            authority: CapabilityRef {
                id: capability_id,
                epoch,
            },
            last_command_sequence: 0,
        };
        let mut transition = self.snapshot_predecessor(TerminalReason::Maintenance);
        self.cancel_unclaimed_paste();
        self.maintenance = MaintenanceState::Sealing {
            request,
            requester: Some(connection),
            reserved_capability,
        };
        self.controller = ControllerInternal::NoController;
        self.invalidate_reconciliation();
        match self.request_close(ClosePlan {
            persist_maintenance: true,
            ..ClosePlan::default()
        }) {
            Ok(close) => {
                transition.merge(close);
                Ok(transition)
            }
            Err(mut error) => {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.degrade_unknown();
                if error.terminal_offer.is_none() {
                    error.terminal_offer = transition.terminal_offer.map(Box::new);
                }
                Err(error)
            }
        }
    }

    pub fn confirm_maintenance_persisted(
        &mut self,
        token: NativeActionToken,
        request: MaintenanceRequest,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::PersistMaintenance(request))?;
        let MaintenanceState::Persisting {
            request: current,
            requester,
            reserved_capability,
        } = self.maintenance
        else {
            return Err(self.native_confirmation_fault());
        };
        if current != request || self.admission != AdmissionState::Closed {
            return Err(self.native_confirmation_fault());
        }
        let mut transition = Transition::empty();
        if let Some(connection) = requester {
            self.controller = ControllerInternal::MaintenanceExclusive {
                connection,
                capability: reserved_capability,
            };
            self.maintenance = MaintenanceState::Exclusive { request };
            transition.response_stage = Some(ResponseStage::MaintenanceAcquireReady);
        } else {
            self.controller = ControllerInternal::NoController;
            self.maintenance = MaintenanceState::Sealed { request };
        }
        Ok(transition)
    }

    pub fn apply_maintenance_command(
        &mut self,
        connection: ConnectionId,
        authority: CapabilityRef,
        sequence: CommandSequence,
        command: MaintenanceCommand,
    ) -> Result<Transition, TransitionError> {
        if !matches!(self.exit, ExitState::Running)
            || matches!(self.process_health, ProcessHealth::Degraded)
        {
            return Err(TransitionError::new(
                if matches!(self.exit, ExitState::Running) {
                    TransitionErrorKind::Degraded
                } else {
                    TransitionErrorKind::Stopping
                },
            ));
        }
        let mut capability = match self.controller {
            ControllerInternal::MaintenanceExclusive {
                connection: active,
                capability,
            } if active == connection => capability,
            _ => return Err(self.command_for_wrong_controller(connection)),
        };
        if capability.authority != authority
            || capability
                .last_command_sequence
                .checked_add(1)
                .is_none_or(|expected| expected != sequence.get())
        {
            let transition = self.lose_active_controller(TerminalReason::Protocol)?;
            return Err(TransitionError::with_transition(
                TransitionErrorKind::ProtocolFault,
                transition,
            ));
        }
        capability.last_command_sequence = sequence.get();
        self.controller = ControllerInternal::MaintenanceExclusive {
            connection,
            capability,
        };
        match command {
            MaintenanceCommand::Renew | MaintenanceCommand::ConsumeSemanticRejection => {
                Ok(Transition::empty())
            }
            MaintenanceCommand::Prepare {
                operation,
                response_correlation,
            } => {
                let Some(request) = self.maintenance.request() else {
                    return Err(TransitionError::new(
                        TransitionErrorKind::MaintenanceTransactionMismatch,
                    ));
                };
                if request.operation != operation {
                    return Err(TransitionError::new(
                        TransitionErrorKind::MaintenanceOperationMismatch,
                    ));
                }
                if !self.quiescent_for_neutral() {
                    return Err(TransitionError::new(TransitionErrorKind::Draining));
                }
                self.begin_native_stop(ExitPurpose::MaintenancePrepare(response_correlation))
            }
        }
    }

    pub fn confirm_native_stopped(
        &mut self,
        token: NativeActionToken,
    ) -> Result<Transition, TransitionError> {
        self.take_pending_action(token, PendingActionKind::StopNative)?;
        let ExitState::NativeStopPending(purpose) = self.exit else {
            return Err(self.native_confirmation_fault());
        };
        let mut transition = Transition::empty();
        match purpose {
            ExitPurpose::MaintenancePrepare(correlation) => {
                self.exit = ExitState::FinalResponseReady(correlation);
                transition.response_stage = Some(ResponseStage::FinalResponseReady);
            }
            ExitPurpose::Idle | ExitPurpose::Degraded | ExitPurpose::MaintenanceGuardLost => {
                self.exit = ExitState::Exiting;
                transition.actions.push(RequiredAction::ExitOwner);
            }
            ExitPurpose::AbandonedMaintenancePrepare => {
                self.exit = ExitState::NativeStoppedSealed;
            }
        }
        Ok(transition)
    }

    pub fn begin_final_response_flush(
        &mut self,
        correlation: ResponseCorrelation,
    ) -> Result<Transition, TransitionError> {
        if self.exit != ExitState::FinalResponseReady(correlation) {
            return Err(TransitionError::new(
                TransitionErrorKind::ResponseFlushMismatch,
            ));
        }
        self.exit = ExitState::FlushingResponse(correlation);
        Ok(Transition::empty())
    }

    pub fn confirm_final_response_flushed(
        &mut self,
        correlation: ResponseCorrelation,
    ) -> Result<Transition, TransitionError> {
        if self.exit != ExitState::FlushingResponse(correlation) {
            return Err(TransitionError::new(
                TransitionErrorKind::ResponseFlushMismatch,
            ));
        }
        self.exit = ExitState::Exiting;
        let mut transition = Transition::empty();
        transition.actions.push(RequiredAction::ExitOwner);
        Ok(transition)
    }

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

    fn ensure_not_starting_or_stopping(&self) -> Result<(), TransitionError> {
        if matches!(self.process_health, ProcessHealth::Starting) {
            return Err(TransitionError::new(TransitionErrorKind::Starting));
        }
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        Ok(())
    }

    fn ensure_process_healthy(&self) -> Result<(), TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        if matches!(self.process_health, ProcessHealth::Degraded) {
            return Err(TransitionError::new(TransitionErrorKind::Degraded));
        }
        Ok(())
    }

    fn ensure_capture_command_allowed(
        &self,
        command: CaptureCommand,
    ) -> Result<(), TransitionError> {
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        if self.rollback_latched
            && !matches!(command, CaptureCommand::Disable | CaptureCommand::Release)
        {
            return Err(TransitionError::new(TransitionErrorKind::RollbackLatched));
        }
        if matches!(self.process_health, ProcessHealth::Degraded)
            && !matches!(command, CaptureCommand::Disable | CaptureCommand::Release)
        {
            return Err(TransitionError::new(TransitionErrorKind::Degraded));
        }
        if !matches!(self.maintenance, MaintenanceState::None) {
            return Err(TransitionError::new(TransitionErrorKind::MaintenanceSealed));
        }
        Ok(())
    }

    fn enable_prerequisites_hold(&self) -> bool {
        matches!(self.process_health, ProcessHealth::Healthy)
            && matches!(self.exit, ExitState::Running)
            && !self.rollback_latched
            && matches!(self.maintenance, MaintenanceState::None)
            && matches!(
                self.controller,
                ControllerInternal::CaptureLeaseDisabled { .. }
            )
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.native_state_unknown
            && self.predecessor.is_none()
            && self.applied_configuration.is_some()
            && self
                .requested_configuration
                .map(ConfigurationRequest::identity)
                == self.applied_configuration
            && self.applied_session_mode == Some(SessionCaptureMode::Off)
            && self.readiness.keyboard_eligible()
    }

    fn ensure_enable_allowed(&self) -> Result<(), TransitionError> {
        if self.admission != AdmissionState::Closed {
            return Err(TransitionError::new(
                TransitionErrorKind::AdmissionTransitionPending,
            ));
        }
        if self.applied_configuration.is_none()
            || self
                .requested_configuration
                .map(ConfigurationRequest::identity)
                != self.applied_configuration
        {
            return Err(TransitionError::new(
                TransitionErrorKind::ConfigurationRequired,
            ));
        }
        if self.applied_session_mode != Some(SessionCaptureMode::Off) {
            return Err(TransitionError::new(
                TransitionErrorKind::SessionOffReconciliationRequired,
            ));
        }
        if !self.readiness.keyboard_eligible() {
            return Err(TransitionError::new(
                TransitionErrorKind::NativeReadinessRequired,
            ));
        }
        if !self.can_open_keyboard() {
            return Err(TransitionError::new(TransitionErrorKind::Draining));
        }
        Ok(())
    }

    fn request_session_mode(
        &mut self,
        epoch: CapabilityEpoch,
        mode: SessionCaptureMode,
    ) -> Result<Transition, TransitionError> {
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::SessionMode(mode),
            ActionScope::Capture(epoch),
            &mut transition.actions,
        )?;
        self.applied_session_mode = None;
        Ok(transition)
    }

    fn begin_paste(
        &mut self,
        authority: CapabilityRef,
        authorization: PasteAuthorization,
    ) -> Result<Transition, TransitionError> {
        if authorization.owner_instance != self.owner_instance
            || authorization.capture_epoch != authority.epoch
        {
            return Err(TransitionError::new(
                TransitionErrorKind::PasteScopeMismatch,
            ));
        }
        if !self.can_begin_paste() {
            return Err(TransitionError::new(TransitionErrorKind::PasteUnavailable));
        }
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::AdmitPaste(authorization),
            ActionScope::Capture(authority.epoch),
            &mut transition.actions,
        )?;
        self.paste_phase = PastePhase::Admitting {
            authorization,
            cancel_after_admit: false,
        };
        Ok(transition)
    }

    fn request_close(&mut self, plan: ClosePlan) -> Result<Transition, TransitionError> {
        self.demote_capture_controller();
        if self.admission == AdmissionState::Closed {
            return self.after_admission_closed(plan);
        }
        match &mut self.close_plan {
            Some(current) => current.merge(plan),
            None => self.close_plan = Some(plan),
        }
        self.mark_pending_superseded(PendingActionKind::OpenAdmission);
        if self.has_pending_kind(PendingActionKind::CloseAdmission) {
            self.admission = AdmissionState::Closing;
            return Ok(Transition::empty());
        }
        if self.emergency_close_pending {
            self.admission = AdmissionState::Unknown;
            return Ok(Transition::empty());
        }
        let scope = self
            .current_capture_epoch()
            .map(ActionScope::Capture)
            .unwrap_or_else(|| self.dependent_scope());
        let mut transition = Transition::empty();
        if let Err(error) = self.issue_action(
            PendingActionKind::CloseAdmission,
            scope,
            &mut transition.actions,
        ) {
            self.degrade_unknown();
            self.admission = AdmissionState::Unknown;
            return Err(error);
        }
        self.admission = AdmissionState::Closing;
        Ok(transition)
    }

    fn after_admission_closed(&mut self, plan: ClosePlan) -> Result<Transition, TransitionError> {
        if !self.native_state_unknown {
            self.dependent_plan.cancel_candidate |=
                self.ownership.candidate == CandidateOwnership::Active;
            self.dependent_plan.cancel_paste |= matches!(self.paste_phase, PastePhase::Waiting(_));
            if plan.release_response {
                self.dependent_plan.apply_configuration = None;
            } else if plan.apply_configuration.is_some() {
                self.dependent_plan.apply_configuration = plan.apply_configuration;
            }
            self.dependent_plan.persist_maintenance |= plan.persist_maintenance;
            self.dependent_plan.continue_drain |= !self.ownership.is_native_neutral();
        } else {
            if plan.release_response {
                self.dependent_plan.apply_configuration = None;
            }
            // Known retained aggregate obligations still need the native owner
            // loop after close uncertainty. Never resurrect work after an
            // indeterminate dependent cancellation poisoned ordering.
            if !self.dependent_work_poisoned {
                self.dependent_plan.continue_drain |= !self.ownership.is_native_neutral();
            }
        }
        let mut transition = Transition::empty();
        self.issue_next_dependent(&mut transition.actions)?;
        if plan.release_response {
            transition.lease_disposition = Some(if self.native_quiescent_for_disposition() {
                LeaseDisposition::Neutral
            } else {
                LeaseDisposition::Draining
            });
        }
        self.maybe_begin_degraded_exit(&mut transition.actions)?;
        Ok(transition)
    }

    fn issue_next_dependent(
        &mut self,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        if self.dependent_work_poisoned {
            self.dependent_plan = DependentPlan::default();
            return Ok(());
        }
        if self
            .pending_actions
            .iter()
            .flatten()
            .any(|pending| pending.kind.is_dependent())
            || matches!(self.paste_phase, PastePhase::Admitting { .. })
        {
            return Ok(());
        }
        if self.dependent_plan.cancel_candidate {
            self.dependent_plan.cancel_candidate = false;
            self.ownership.candidate = CandidateOwnership::Cancelling;
            return self.issue_action(
                PendingActionKind::CancelCandidate,
                self.dependent_scope(),
                actions,
            );
        }
        if self.dependent_plan.cancel_paste {
            self.dependent_plan.cancel_paste = false;
            let Some(authorization) = self.paste_phase.authorization() else {
                return Err(self.native_confirmation_fault());
            };
            self.paste_phase = PastePhase::Cancelling(authorization);
            self.ownership.paste = PasteOwnership::Cancelling;
            return self.issue_action(
                PendingActionKind::CancelPaste(authorization),
                self.dependent_scope(),
                actions,
            );
        }
        if let Some(request) = self.dependent_plan.apply_configuration.take() {
            return self.issue_action(
                PendingActionKind::Configuration(request),
                ActionScope::Capture(request.identity.capture_epoch),
                actions,
            );
        }
        if self.dependent_plan.continue_drain {
            self.dependent_plan.continue_drain = false;
            if !self.ownership.is_native_neutral() {
                actions.push(RequiredAction::ContinueNativeDrain);
            }
        }
        if self.dependent_plan.persist_maintenance {
            self.dependent_plan.persist_maintenance = false;
            let MaintenanceState::Sealing {
                request,
                requester,
                reserved_capability,
            } = self.maintenance
            else {
                return Err(self.native_confirmation_fault());
            };
            self.maintenance = MaintenanceState::Persisting {
                request,
                requester,
                reserved_capability,
            };
            if let Err(mut error) = self.issue_action(
                PendingActionKind::PersistMaintenance(request),
                ActionScope::Maintenance(reserved_capability.authority.epoch),
                actions,
            ) {
                self.maintenance = MaintenanceState::SealedFailed { request };
                self.degrade_unknown();
                let mut mandatory = std::mem::take(actions);
                mandatory.append(&mut error.actions);
                error.actions = mandatory;
                return Err(error);
            }
            return Ok(());
        }
        Ok(())
    }

    fn dependent_scope(&self) -> ActionScope {
        self.predecessor
            .map(|predecessor| ActionScope::Capture(predecessor.route.authority.epoch))
            .or_else(|| self.current_capture_epoch().map(ActionScope::Capture))
            .unwrap_or(ActionScope::Process)
    }

    fn apply_priority_rollback(
        &mut self,
        reason: Option<TerminalReason>,
    ) -> Result<Transition, TransitionError> {
        self.ensure_not_starting_or_stopping()?;
        // Latch before all allocation and close work. Failure may degrade, but
        // it can never erase the rollback fact.
        self.rollback_latched = true;
        let mut transition = reason.map_or_else(Transition::empty, |reason| {
            self.snapshot_predecessor(reason)
        });
        self.controller = ControllerInternal::NoController;
        self.invalidate_reconciliation();
        self.dependent_plan.apply_configuration = None;
        self.mark_semantic_work_superseded();
        self.cancel_unclaimed_paste();
        match self.request_close(ClosePlan::default()) {
            Ok(close) => {
                transition.merge(close);
                transition.lease_disposition = Some(if self.native_quiescent_for_disposition() {
                    LeaseDisposition::Neutral
                } else {
                    LeaseDisposition::Draining
                });
                Ok(transition)
            }
            Err(mut error) => {
                // request_close already degraded/latched unknown. Preserve the
                // terminal route and priority fact in the error transition.
                if error.terminal_offer.is_none() {
                    error.terminal_offer = transition.terminal_offer.map(Box::new);
                }
                Err(error)
            }
        }
    }

    fn cancel_unclaimed_paste(&mut self) {
        if self.native_state_unknown || self.dependent_work_poisoned {
            return;
        }
        match self.paste_phase {
            PastePhase::Admitting { authorization, .. } => {
                self.paste_phase = PastePhase::Admitting {
                    authorization,
                    cancel_after_admit: true,
                };
                self.mark_pending_superseded(PendingActionKind::AdmitPaste(authorization));
            }
            PastePhase::Waiting(authorization) => {
                self.paste_phase = PastePhase::Cancelling(authorization);
                self.ownership.paste = PasteOwnership::Cancelling;
                self.dependent_plan.cancel_paste = true;
            }
            PastePhase::None
            | PastePhase::Cancelling(_)
            | PastePhase::Claimed(_)
            | PastePhase::Indeterminate(_) => {}
        }
    }

    fn lose_active_controller(
        &mut self,
        reason: TerminalReason,
    ) -> Result<Transition, TransitionError> {
        let was_capture = self.controller.capture().is_some();
        let mut transition = if was_capture {
            self.snapshot_predecessor(reason)
        } else {
            Transition::empty()
        };
        match self.exit {
            ExitState::NativeStopPending(ExitPurpose::MaintenancePrepare(_)) => {
                self.exit = ExitState::NativeStopPending(ExitPurpose::AbandonedMaintenancePrepare);
            }
            ExitState::FinalResponseReady(_) | ExitState::FlushingResponse(_) => {
                self.exit = ExitState::NativeStoppedSealed;
            }
            _ => {}
        }
        match self.maintenance {
            MaintenanceState::Sealing {
                request,
                reserved_capability,
                ..
            } => {
                self.maintenance = MaintenanceState::Sealing {
                    request,
                    requester: None,
                    reserved_capability,
                };
            }
            MaintenanceState::Persisting {
                request,
                reserved_capability,
                ..
            } => {
                self.maintenance = MaintenanceState::Persisting {
                    request,
                    requester: None,
                    reserved_capability,
                };
            }
            MaintenanceState::Exclusive { request } => {
                self.maintenance = MaintenanceState::Sealed { request };
            }
            _ => {}
        }
        self.controller = ControllerInternal::NoController;
        if was_capture {
            self.invalidate_reconciliation();
            self.cancel_unclaimed_paste();
            match self.request_close(ClosePlan::default()) {
                Ok(close) => transition.merge(close),
                Err(mut error) => {
                    if error.terminal_offer.is_none() {
                        error.terminal_offer = transition.terminal_offer.map(Box::new);
                    }
                    return Err(error);
                }
            }
        }
        Ok(transition)
    }

    fn command_for_wrong_controller(&mut self, connection: ConnectionId) -> TransitionError {
        if self.controller.connection() == Some(connection) {
            match self.lose_active_controller(TerminalReason::Protocol) {
                Ok(transition) => {
                    TransitionError::with_transition(TransitionErrorKind::ProtocolFault, transition)
                }
                Err(error) => error,
            }
        } else {
            TransitionError::new(TransitionErrorKind::WrongController)
        }
    }

    fn snapshot_predecessor(&mut self, reason: TerminalReason) -> Transition {
        if self.predecessor.is_none()
            && let Some((connection, capability, _)) = self.controller.capture()
        {
            self.predecessor = Some(PredecessorState {
                route: PredecessorRoute {
                    owner_instance: self.owner_instance,
                    connection,
                    authority: capability.authority,
                },
                high_water: 0,
                revoked_offered: false,
                final_offered: false,
                in_flight: None,
            });
        }
        let mut transition = Transition::empty();
        transition.terminal_offer =
            self.offer_predecessor_terminal(PredecessorTerminalEvent::LeaseRevoked(reason));
        transition
    }

    fn begin_native_stop(&mut self, purpose: ExitPurpose) -> Result<Transition, TransitionError> {
        if !matches!(self.exit, ExitState::Running) {
            return Err(TransitionError::new(TransitionErrorKind::Stopping));
        }
        let mut transition = Transition::empty();
        self.issue_action(
            PendingActionKind::StopNative,
            ActionScope::Process,
            &mut transition.actions,
        )?;
        self.exit = ExitState::NativeStopPending(purpose);
        Ok(transition)
    }

    fn maybe_begin_degraded_exit(
        &mut self,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        if matches!(self.process_health, ProcessHealth::Degraded)
            && matches!(self.maintenance, MaintenanceState::None)
            && matches!(self.exit, ExitState::Running)
            && self.controller.capture().is_none()
            && self.quiescent_for_neutral()
        {
            let mut transition = self.begin_native_stop(ExitPurpose::Degraded)?;
            actions.append(&mut transition.actions);
        }
        Ok(())
    }

    fn current_capture_epoch(&self) -> Option<CapabilityEpoch> {
        self.controller
            .capture()
            .map(|(_, capability, _)| capability.authority.epoch)
    }

    fn demote_capture_controller(&mut self) {
        if let ControllerInternal::CaptureLeaseEnabled {
            connection,
            capability,
        } = self.controller
        {
            self.controller = ControllerInternal::CaptureLeaseDisabled {
                connection,
                capability,
            };
        }
    }

    fn invalidate_reconciliation(&mut self) {
        self.requested_configuration = None;
        self.applied_configuration = None;
        self.applied_session_mode = None;
    }

    fn degrade_unknown(&mut self) {
        if !self.native_state_unknown {
            self.cancel_unclaimed_paste();
        }
        self.process_health = ProcessHealth::Degraded;
        self.native_state_unknown = true;
        self.terminal_unavailable_reason = Some(TerminalUnavailableReason::OwnershipUnknown);
        self.maintenance = match self.maintenance {
            MaintenanceState::Sealing { request, .. }
            | MaintenanceState::Persisting { request, .. } => {
                MaintenanceState::SealedFailed { request }
            }
            other => other,
        };
        self.demote_capture_controller();
        if self.admission != AdmissionState::Closed {
            self.admission = AdmissionState::Unknown;
        }
    }

    fn mark_pending_superseded(&mut self, kind: PendingActionKind) {
        for pending in self.pending_actions.iter_mut().flatten() {
            if pending.kind == kind {
                pending.superseded = true;
            }
        }
    }

    fn mark_semantic_work_superseded(&mut self) {
        for pending in self.pending_actions.iter_mut().flatten() {
            if matches!(
                pending.kind,
                PendingActionKind::OpenAdmission
                    | PendingActionKind::SessionMode(_)
                    | PendingActionKind::Configuration(_)
                    | PendingActionKind::AdmitPaste(_)
            ) {
                pending.superseded = true;
            }
        }
    }

    fn issue_action(
        &mut self,
        kind: PendingActionKind,
        scope: ActionScope,
        actions: &mut RequiredActions,
    ) -> Result<(), TransitionError> {
        let Some(id) = self.last_action_id.checked_add(1).and_then(NonZeroU64::new) else {
            return Err(self.action_allocation_error());
        };
        let Some(slot_index) = self.pending_actions.iter().position(Option::is_none) else {
            return Err(self.action_allocation_error());
        };
        self.last_action_id = id.get();
        let token = NativeActionToken {
            owner_instance: self.owner_instance,
            id,
            scope,
        };
        self.pending_actions[slot_index] = Some(PendingAction {
            token,
            kind,
            superseded: false,
        });
        actions.push(match kind {
            PendingActionKind::CloseAdmission => RequiredAction::CloseFreshAdmission { token },
            PendingActionKind::OpenAdmission => RequiredAction::OpenFreshAdmission { token },
            PendingActionKind::SessionMode(mode) => {
                RequiredAction::ApplySessionMode { token, mode }
            }
            PendingActionKind::Configuration(request) => RequiredAction::ApplyConfiguration {
                token,
                request,
                fence: PreHeldKeyFence::FenceCurrentPhysical,
            },
            PendingActionKind::AdmitPaste(authorization) => RequiredAction::AdmitPaste {
                token,
                authorization,
            },
            PendingActionKind::CancelCandidate => RequiredAction::CancelCandidate { token },
            PendingActionKind::CancelPaste(authorization) => RequiredAction::CancelWaitingPaste {
                token,
                authorization,
            },
            PendingActionKind::PersistMaintenance(request) => {
                RequiredAction::PersistMaintenanceRecord { token, request }
            }
            PendingActionKind::StopNative => RequiredAction::StopNativeAdapter { token },
        });
        Ok(())
    }

    fn action_allocation_error(&mut self) -> TransitionError {
        let needs_emergency_close = self.admission != AdmissionState::Closed
            && !self.has_pending_kind(PendingActionKind::CloseAdmission)
            && !self.emergency_close_pending;
        self.degrade_unknown();
        let mut error = TransitionError::new(TransitionErrorKind::ActionIdExhausted);
        if needs_emergency_close {
            self.emergency_close_pending = true;
            error
                .actions
                .push(RequiredAction::EmergencyCloseFreshAdmission);
        }
        error
    }

    fn take_pending_action(
        &mut self,
        token: NativeActionToken,
        expected: PendingActionKind,
    ) -> Result<PendingAction, TransitionError> {
        let Some(actual) = self
            .pending_actions
            .iter()
            .flatten()
            .find(|pending| pending.token == token)
            .map(|pending| pending.kind)
        else {
            return Err(self.native_confirmation_fault());
        };
        if actual != expected {
            return Err(self.native_confirmation_fault());
        }
        self.take_pending_by_token(token)
    }

    fn take_pending_by_token(
        &mut self,
        token: NativeActionToken,
    ) -> Result<PendingAction, TransitionError> {
        let Some(slot) = self
            .pending_actions
            .iter_mut()
            .find(|slot| slot.is_some_and(|pending| pending.token == token))
        else {
            return Err(self.native_confirmation_fault());
        };
        Ok(slot.take().expect("matched pending action"))
    }

    fn native_confirmation_fault(&mut self) -> TransitionError {
        self.degrade_unknown();
        let transition = match self.request_close(ClosePlan::default()) {
            Ok(transition) => transition,
            Err(error) => Transition {
                actions: error.actions,
                lease_disposition: None,
                terminal_offer: error.terminal_offer.map(|offer| *offer),
                response_stage: None,
            },
        };
        TransitionError::with_transition(
            TransitionErrorKind::NativeConfirmationMismatch,
            transition,
        )
    }

    fn pending_actions_empty(&self) -> bool {
        self.pending_actions.iter().all(Option::is_none)
    }

    fn has_pending_kind(&self, kind: PendingActionKind) -> bool {
        self.pending_actions
            .iter()
            .flatten()
            .any(|pending| pending.kind == kind)
    }

    fn terminal_ownership(&self) -> Option<TerminalOwnership> {
        let mut count = 0_u8;
        let mut ownership = None;
        for (present, kind) in [
            (
                self.ownership.candidate != CandidateOwnership::None,
                TerminalOwnership::Candidate,
            ),
            (
                self.ownership.activation_drain_keys != 0,
                TerminalOwnership::Activation,
            ),
            (
                self.ownership.session_drain_keys != 0,
                TerminalOwnership::Session,
            ),
            (
                self.ownership.replay_cleanup_edges != 0,
                TerminalOwnership::ReplayCleanup,
            ),
            (
                self.ownership.paste != PasteOwnership::None,
                TerminalOwnership::Paste,
            ),
        ] {
            if present {
                count += 1;
                ownership = Some(kind);
            }
        }
        if count > 1 {
            Some(TerminalOwnership::Multiple)
        } else {
            ownership
        }
    }

    fn native_quiescent_for_disposition(&self) -> bool {
        self.admission == AdmissionState::Closed
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.emergency_close_pending
            && self.close_plan.is_none()
            && self.dependent_plan == DependentPlan::default()
            && !self.native_state_unknown
    }

    fn quiescent_for_neutral(&self) -> bool {
        self.admission == AdmissionState::Closed
            && self.ownership.is_native_neutral()
            && self.pending_actions_empty()
            && !self.emergency_close_pending
            && self.close_plan.is_none()
            && self.dependent_plan == DependentPlan::default()
            && !self.native_state_unknown
            && self.predecessor.is_none()
    }

    fn next_epoch(high_water: &mut u64) -> Result<CapabilityEpoch, TransitionError> {
        let next = high_water
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| TransitionError::new(TransitionErrorKind::EpochExhausted))?;
        *high_water = next.get();
        Ok(CapabilityEpoch(next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(value: u8) -> OwnerInstanceId {
        OwnerInstanceId::new([value; AUTHORITY_BYTES]).unwrap()
    }

    fn capability(value: u8) -> CapabilityId {
        CapabilityId::new([value; AUTHORITY_BYTES]).unwrap()
    }

    fn connection(value: u64) -> ConnectionId {
        ConnectionId::new(value).unwrap()
    }

    fn capture_state(enabled: bool) -> (KeyboardOwnerState, ConnectionId, CapabilityRef) {
        let mut state = KeyboardOwnerState::new(owner(1));
        state.process_health = ProcessHealth::Healthy;
        state.startup_snapshot_seeded = true;
        state.readiness = NativeReadiness {
            keyboard_build_eligible: true,
            paste_ready: true,
            permissions_eligible: true,
            hook_healthy: true,
        };
        let connection = connection(1);
        let authority = CapabilityRef {
            id: capability(1),
            epoch: CapabilityEpoch(NonZeroU64::MIN),
        };
        let capability = CapabilityState {
            authority,
            last_command_sequence: 0,
        };
        state.controller = if enabled {
            state.admission = AdmissionState::Open;
            ControllerInternal::CaptureLeaseEnabled {
                connection,
                capability,
            }
        } else {
            ControllerInternal::CaptureLeaseDisabled {
                connection,
                capability,
            }
        };
        (state, connection, authority)
    }

    #[test]
    fn action_exhaustion_reserves_configuration_high_water_and_degrades() {
        let (mut state, connection, authority) = capture_state(false);
        state.last_action_id = u64::MAX;
        let error = state
            .apply_capture_command(
                connection,
                authority,
                CommandSequence::FIRST,
                CaptureCommand::ReplaceConfiguration {
                    revision: ConfigurationRevision::new(8).unwrap(),
                    bindings: ActivationBindings::default(),
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
        assert_eq!(
            state.configuration_high_water,
            ConfigurationRevision::new(8)
        );
        assert_eq!(state.process(), ProcessState::Degraded);
        assert!(state.native_state_unknown);
    }

    #[test]
    fn rollback_action_exhaustion_preserves_priority_latch() {
        let (mut state, _, _) = capture_state(true);
        state.last_action_id = u64::MAX;
        let error = state.latch_runtime_rollback().unwrap_err();
        assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
        assert!(
            error
                .actions()
                .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
        );
        assert!(state.rollback_latched);
        assert_eq!(state.process(), ProcessState::Degraded);
        assert_ne!(state.admission, AdmissionState::Open);
        assert!(matches!(state.controller, ControllerInternal::NoController));
    }

    #[test]
    fn maintenance_post_seal_action_exhaustion_never_restores_capture() {
        let (mut state, _, _) = capture_state(true);
        state.last_action_id = u64::MAX;
        let request = MaintenanceRequest::new(
            MaintenanceTransactionId::new([3; AUTHORITY_BYTES]).unwrap(),
            MaintenanceOperation::Update,
            BuildDigest::new([4; AUTHORITY_BYTES]).unwrap(),
            Some(BuildDigest::new([5; AUTHORITY_BYTES]).unwrap()),
            Some(BuildDigest::new([6; AUTHORITY_BYTES]).unwrap()),
            MaintenanceHandoff::new([7; AUTHORITY_BYTES]).unwrap(),
        )
        .unwrap();
        let error = state
            .acquire_maintenance(connection(2), capability(2), request)
            .unwrap_err();
        assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
        assert!(
            error
                .actions()
                .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
        );
        assert_eq!(state.maintenance_phase(), MaintenancePhase::SealedFailed);
        assert_eq!(state.process(), ProcessState::Degraded);
        assert!(!matches!(
            state.controller,
            ControllerInternal::CaptureLeaseEnabled { .. }
        ));
    }

    #[test]
    fn mismatch_exhaustion_preserves_and_confirms_emergency_close() {
        let (mut state, _, _) = capture_state(true);
        state.last_action_id = u64::MAX;
        let stale = NativeActionToken {
            owner_instance: state.owner_instance,
            id: NonZeroU64::MIN,
            scope: ActionScope::Process,
        };
        let error = state.confirm_admission_opened(stale).unwrap_err();
        assert_eq!(
            error.kind(),
            TransitionErrorKind::NativeConfirmationMismatch
        );
        assert!(
            error
                .actions()
                .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
        );
        assert!(state.emergency_close_pending);
        state.confirm_emergency_admission_closed().unwrap();
        assert_eq!(state.admission, AdmissionState::Closed);
        assert!(!state.emergency_close_pending);
    }

    #[test]
    fn controller_loss_exhaustion_returns_retrievable_predecessor_offer() {
        let (mut state, connection, _) = capture_state(true);
        state.last_action_id = u64::MAX;
        let error = state.controller_disconnected(connection).unwrap_err();
        assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
        assert!(
            error
                .actions()
                .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
        );
        let offer = error.terminal_offer().expect("predecessor offer retained");
        state.fail_predecessor_terminal_write(offer).unwrap();
        state.confirm_emergency_admission_closed().unwrap();
    }

    #[test]
    fn persistence_allocation_exhaustion_returns_prior_drain_directive() {
        let (mut state, _, _) = capture_state(true);
        state.ownership =
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap();
        state.last_action_id = u64::MAX - 1;
        let request = MaintenanceRequest::new(
            MaintenanceTransactionId::new([9; AUTHORITY_BYTES]).unwrap(),
            MaintenanceOperation::Update,
            BuildDigest::new([10; AUTHORITY_BYTES]).unwrap(),
            Some(BuildDigest::new([11; AUTHORITY_BYTES]).unwrap()),
            Some(BuildDigest::new([12; AUTHORITY_BYTES]).unwrap()),
            MaintenanceHandoff::new([13; AUTHORITY_BYTES]).unwrap(),
        )
        .unwrap();
        let acquire = state
            .acquire_maintenance(connection(2), capability(2), request)
            .unwrap();
        let error = state
            .confirm_admission_closed(
                acquire
                    .actions()
                    .as_slice()
                    .iter()
                    .find_map(|action| action.token())
                    .unwrap(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
        assert!(
            error
                .actions()
                .contains_kind(RequiredActionKind::ContinueNativeDrain)
        );
        assert_eq!(state.maintenance_phase(), MaintenancePhase::SealedFailed);
    }

    #[test]
    fn capture_and_maintenance_epoch_exhaustion_never_wraps() {
        let mut capture = KeyboardOwnerState::new(owner(1));
        capture.process_health = ProcessHealth::Healthy;
        capture.controller = ControllerInternal::AuthenticatedObserver {
            connection: connection(1),
        };
        capture.last_capture_epoch = u64::MAX;
        assert_eq!(
            capture
                .acquire_capture_lease(connection(1), capability(1))
                .unwrap_err()
                .kind(),
            TransitionErrorKind::EpochExhausted
        );

        let mut maintenance = KeyboardOwnerState::new(owner(2));
        maintenance.process_health = ProcessHealth::Healthy;
        maintenance.last_maintenance_epoch = u64::MAX;
        let request = MaintenanceRequest::new(
            MaintenanceTransactionId::new([7; AUTHORITY_BYTES]).unwrap(),
            MaintenanceOperation::Uninstall,
            BuildDigest::new([8; AUTHORITY_BYTES]).unwrap(),
            None,
            None,
            MaintenanceHandoff::new([9; AUTHORITY_BYTES]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            maintenance
                .acquire_maintenance(connection(2), capability(2), request)
                .unwrap_err()
                .kind(),
            TransitionErrorKind::EpochExhausted
        );
        assert_eq!(maintenance.maintenance_phase(), MaintenancePhase::None);
    }
}
