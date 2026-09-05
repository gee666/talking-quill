//! Typed method result payloads.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcquireState {
    Disabled,
}
result_struct!(LeaseAcquireResult {
    capture_lease_id: Bytes32,
    capture_lease_epoch: U64String,
    state: AcquireState
});
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintenanceAcquireState {
    Sealed,
    Draining,
}
result_struct!(MaintenanceAcquireResult {
    maintenance_capability_id: Bytes32,
    maintenance_capability_epoch: U64String,
    state: MaintenanceAcquireState
});
result_struct!(RenewResult { renewed: bool });
result_struct!(SessionModeResult { mode: SessionMode });
result_struct!(ConfigurationResult {
    revision: U64String
});
result_struct!(EnabledResult { enabled: bool });
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LeaseDisposition {
    Neutral,
    Draining,
}
result_struct!(ReleaseResult {
    disposition: LeaseDisposition
});
result_struct!(RollbackResult {
    latched: bool,
    disposition: LeaseDisposition
});
result_struct!(MaintenancePrepareResult {
    ready_to_exit: bool,
    owner_handoff: Bytes32
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteRefusalReason {
    PermissionDenied,
    ConflictingModifiers,
    SecureInput,
    TargetUnavailable,
    ClipboardChanged,
    NativeUnavailable,
    NativeRejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PasteResult {
    ClipboardOnly {
        reason: PasteRefusalReason,
    },
    Waiting {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
    Committed {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
    Indeterminate {
        #[serde(rename = "operationId")]
        operation_id: Bytes32,
    },
}

result_struct!(HealthResult {
    owner_instance_id: Bytes32,
    reported_state: OwnerReportedState,
    process_state: ProcessState,
    rollback_latched: bool,
    native_state_unknown: bool,
    maintenance_sealed: bool,
    keyboard_build_eligible: bool,
    paste_ready: bool,
    permissions_eligible: bool,
    hook_healthy: bool
});
result_struct!(PermissionsResult {
    accessibility: PermissionState,
    input_monitoring: PermissionState,
    event_post: PermissionState
});
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FrontAppWindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

result_struct!(FrontAppResult {
    available: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    application_token: Option<WireToken>
});
result_struct!(FrontAppMetadataResult {
    available: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    process_name: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    window_title: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    window_bounds: Option<FrontAppWindowBounds>
});
