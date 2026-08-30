//! Platform-neutral executor boundary for the fake owner protocol server.
//!
//! No implementation in this module installs a hook/tap, injects input,
//! launches a process, persists production state, or opens an OS endpoint.

use std::collections::VecDeque;
use std::fmt;

use talking_quill_keyboard_core::NativeTargetToken;
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::schema::{
    FrontAppMetadataResult, FrontAppResult, ObservabilityResult, PermissionState, PermissionsResult,
};

use crate::state::{
    CapabilityId, NativeActionFailure, NativeOwnership, NativeReadiness, PasteAuthorization,
    RequiredAction, RequiredActionKind,
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PasteExecutorRequest {
    pub authorization: PasteAuthorization,
    pub target_token: Option<NativeTargetToken>,
    pub fallback_text_sha256: Bytes32,
}

impl fmt::Debug for PasteExecutorRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasteExecutorRequest(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ExecutorCommand {
    State(RequiredAction),
    AdmitPaste {
        action: RequiredAction,
        request: PasteExecutorRequest,
    },
}

impl ExecutorCommand {
    #[must_use]
    pub const fn action(self) -> RequiredAction {
        match self {
            Self::State(action) | Self::AdmitPaste { action, .. } => action,
        }
    }
}

impl fmt::Debug for ExecutorCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExecutorCommand(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutorResult {
    Applied,
    AdmissionClosed {
        through_event: Option<crate::adapter::AdapterEventId>,
    },
    PasteWaiting,
    PasteRefused(crate::adapter::PasteRefusal),
    CandidateCancelled(NativeOwnership),
    Failed(NativeActionFailure),
    /// Adapter returned a completion shape that cannot correspond to the
    /// dispatched effect. This is fail-closed and distinct from a reported
    /// native operation failure.
    ContractViolation,
}

/// Synchronous B2 executor/query boundary. C1's `NativeAdapterExecutor` maps
/// this exact command/completion vocabulary to the owner-local adapter trait;
/// B2's recording implementation remains entirely in-memory and deterministic.
pub trait OwnerExecutor {
    fn seed_startup_physical_snapshot(&mut self) -> bool;
    fn readiness(&self) -> NativeReadiness;
    fn allocate_capability_id(&mut self) -> Option<CapabilityId>;
    fn execute(&mut self, command: ExecutorCommand) -> ExecutorResult;
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

    fn try_next_adapter_event(&mut self) -> Option<(crate::adapter::AdapterEvent, bool)> {
        None
    }

    fn acknowledge_adapter_event(
        &mut self,
        _id: crate::adapter::AdapterEventId,
        _disposition: crate::adapter::AdapterEventDisposition,
    ) {
    }
}

/// Scriptable safe-disabled executor for integration/property/race tests.
pub struct RecordingFakeExecutor {
    readiness: NativeReadiness,
    next_capability: u64,
    scripted: VecDeque<ExecutorResult>,
    actions: Vec<RequiredActionKind>,
    permissions: PermissionsResult,
    front_app: FrontAppResult,
    observability: ObservabilityResult,
}

impl fmt::Debug for RecordingFakeExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecordingFakeExecutor(<redacted>)")
    }
}

impl Default for RecordingFakeExecutor {
    fn default() -> Self {
        Self::safe_disabled()
    }
}

impl RecordingFakeExecutor {
    #[must_use]
    pub fn safe_disabled() -> Self {
        Self {
            readiness: NativeReadiness {
                keyboard_build_eligible: false,
                paste_ready: false,
                permissions_eligible: false,
                hook_healthy: false,
            },
            next_capability: 1,
            scripted: VecDeque::new(),
            actions: Vec::new(),
            permissions: PermissionsResult {
                accessibility: PermissionState::NotRequired,
                input_monitoring: PermissionState::NotRequired,
                event_post: PermissionState::NotRequired,
            },
            front_app: FrontAppResult {
                available: false,
                application_token: None,
            },
            observability: ObservabilityResult::default(),
        }
    }

    /// Test-only readiness model. Setting this true cannot enable physical
    /// input because the executor has no native adapter or process entry point.
    #[must_use]
    pub fn with_readiness(mut self, readiness: NativeReadiness) -> Self {
        self.readiness = readiness;
        self
    }

    pub fn script(&mut self, result: ExecutorResult) {
        self.scripted.push_back(result);
    }

    #[must_use]
    pub fn actions(&self) -> &[RequiredActionKind] {
        &self.actions
    }

    pub fn clear_actions(&mut self) {
        self.actions.clear();
    }
}

impl OwnerExecutor for RecordingFakeExecutor {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }

    fn readiness(&self) -> NativeReadiness {
        self.readiness
    }

    fn allocate_capability_id(&mut self) -> Option<CapabilityId> {
        let value = self.next_capability;
        self.next_capability = value.checked_add(1)?;
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&value.to_be_bytes());
        CapabilityId::new(bytes)
    }

    fn execute(&mut self, command: ExecutorCommand) -> ExecutorResult {
        let action = command.action();
        self.actions.push(action.kind());
        if let Some(scripted) = self.scripted.pop_front() {
            return scripted;
        }
        match action.kind() {
            RequiredActionKind::OpenFreshAdmission if !self.readiness.keyboard_eligible() => {
                ExecutorResult::Failed(NativeActionFailure::FailedNotApplied)
            }
            RequiredActionKind::AdmitPaste if !self.readiness.paste_eligible() => {
                ExecutorResult::PasteRefused(crate::adapter::PasteRefusal::NativeUnavailable)
            }
            RequiredActionKind::AdmitPaste => ExecutorResult::PasteWaiting,
            RequiredActionKind::CancelCandidate => {
                ExecutorResult::Failed(NativeActionFailure::FailedNotApplied)
            }
            _ => ExecutorResult::Applied,
        }
    }

    fn permissions(&self) -> PermissionsResult {
        self.permissions.clone()
    }

    fn front_app(&self) -> FrontAppResult {
        self.front_app.clone()
    }

    fn observability(&self) -> ObservabilityResult {
        self.observability.clone()
    }
}
