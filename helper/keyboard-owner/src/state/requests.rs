//! Paste and maintenance requests, action tokens, transitions, and typed errors.
use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PasteAuthorization {
    pub(super) operation: PasteOperationId,
    pub(super) owner_instance: OwnerInstanceId,
    pub(super) capture_epoch: CapabilityEpoch,
    pub(super) activation_generation: OwnerActivationGeneration,
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
    pub(super) transaction: MaintenanceTransactionId,
    pub(super) operation: MaintenanceOperation,
    pub(super) source_build: BuildDigest,
    pub(super) target_build: Option<BuildDigest>,
    pub(super) target_owner: Option<BuildDigest>,
    pub(super) owner_handoff: MaintenanceHandoff,
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
pub struct ResponseCorrelation(pub(super) NonZeroU64);

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
pub(super) enum ActionScope {
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
    pub(super) owner_instance: OwnerInstanceId,
    pub(super) id: NonZeroU64,
    pub(super) scope: ActionScope,
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
pub struct RequiredActions(pub(super) Vec<RequiredAction>);

impl RequiredActions {
    #[must_use]
    pub fn as_slice(&self) -> &[RequiredAction] {
        &self.0
    }

    #[must_use]
    pub fn contains_kind(&self, kind: RequiredActionKind) -> bool {
        self.0.iter().any(|action| action.kind() == kind)
    }

    pub(super) fn push(&mut self, action: RequiredAction) {
        assert!(
            self.0.len() < OWNER_ADMITTED_EFFECT_CAPACITY,
            "transition action bound"
        );
        self.0.push(action);
    }

    pub(super) fn append(&mut self, other: &mut Self) {
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
    pub(super) const fn is_final(self) -> bool {
        matches!(self, Self::LeaseNeutral | Self::LeaseUnavailable(_))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct PredecessorRoute {
    pub(super) owner_instance: OwnerInstanceId,
    pub(super) connection: ConnectionId,
    pub(super) authority: CapabilityRef,
}

impl fmt::Debug for PredecessorRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorRoute(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PredecessorTerminalOffer {
    pub(super) route: PredecessorRoute,
    pub(super) sequence: NonZeroU64,
    pub(super) event: PredecessorTerminalEvent,
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
    pub(super) actions: RequiredActions,
    pub(super) lease_disposition: Option<LeaseDisposition>,
    pub(super) terminal_offer: Option<PredecessorTerminalOffer>,
    pub(super) response_stage: Option<ResponseStage>,
}

impl Transition {
    pub(super) fn empty() -> Self {
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

    pub(super) fn merge(&mut self, mut other: Self) {
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
    pub(super) kind: TransitionErrorKind,
    pub(super) actions: RequiredActions,
    pub(super) terminal_offer: Option<Box<PredecessorTerminalOffer>>,
}

impl TransitionError {
    pub(super) fn new(kind: TransitionErrorKind) -> Self {
        Self {
            kind,
            actions: RequiredActions::default(),
            terminal_offer: None,
        }
    }

    pub(super) fn with_transition(kind: TransitionErrorKind, transition: Transition) -> Self {
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
