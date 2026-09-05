//! Connection identity, capabilities, native ownership, and configuration types.
use super::*;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct ConnectionId(pub(super) NonZeroU64);

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
pub struct CapabilityEpoch(pub(super) NonZeroU64);

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
pub struct CommandSequence(pub(super) NonZeroU64);

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
    pub(super) id: CapabilityId,
    pub(super) epoch: CapabilityEpoch,
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
pub(super) struct CapabilityState {
    pub(super) authority: CapabilityRef,
    pub(super) last_command_sequence: u64,
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
pub(super) enum ControllerInternal {
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
    pub(super) const fn public(self) -> ControllerState {
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

    pub(super) const fn connection(self) -> Option<ConnectionId> {
        match self {
            Self::NoController => None,
            Self::AuthenticatedObserver { connection }
            | Self::CaptureLeaseDisabled { connection, .. }
            | Self::CaptureLeaseEnabled { connection, .. }
            | Self::MaintenanceExclusive { connection, .. } => Some(connection),
        }
    }

    pub(super) const fn capture(self) -> Option<(ConnectionId, CapabilityState, bool)> {
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
    pub(super) candidate: CandidateOwnership,
    pub(super) activation_drain_keys: u8,
    pub(super) session_drain_keys: u8,
    pub(super) replay_cleanup_edges: u8,
    pub(super) paste: PasteOwnership,
    pub(super) conservative_native_work: bool,
    pub(super) admitted_effects: u8,
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

    pub(super) fn legal_while_keyboard_active(self, previous: Self) -> bool {
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

    pub(super) fn legal_after_keyboard_closed(self, previous: Self) -> bool {
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
pub(super) enum ProcessHealth {
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
pub struct ConfigurationRevision(pub(super) NonZeroU64);

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
    pub(super) capture_epoch: CapabilityEpoch,
    pub(super) revision: ConfigurationRevision,
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
    pub(super) identity: ConfigurationIdentity,
    pub(super) bindings: ActivationBindings,
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
pub struct OwnerActivationGeneration(pub(super) NonZeroU64);

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
