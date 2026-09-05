//! Capture and maintenance request parameter schemas.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

macro_rules! capture_params {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            pub capture_lease_id: Bytes32,
            pub capture_lease_epoch: U64String,
            pub command_sequence: U64String,
            $($(#[$meta])* pub $field: $type),*
        }
    };
}

macro_rules! maintenance_params {
    ($name:ident { $($field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            pub maintenance_capability_id: Bytes32,
            pub maintenance_capability_epoch: U64String,
            pub command_sequence: U64String,
            $(pub $field: $type),*
        }
    };
}

capture_params!(CaptureCommandParams {});
capture_params!(SessionSetModeParams { mode: SessionMode });
capture_params!(ReplaceConfigurationParams {
    revision: U64String,
    bindings: Bindings
});
capture_params!(SetEnabledParams { enabled: bool });
capture_params!(PasteInjectParams {
    operation_id: Bytes32,
    owner_instance_id: Bytes32,
    activation_generation: U64String,
    #[serde(deserialize_with = "deserialize_required_option")]
    target_token: Option<WireToken>,
    fallback_text_sha256: Bytes32
});
maintenance_params!(MaintenanceCommandParams {});
maintenance_params!(MaintenancePrepareParams {
    transaction_id: Bytes32,
    operation: MaintenanceOperation
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase", deny_unknown_fields)]
pub enum MaintenanceAcquireParams {
    Update {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
        #[serde(rename = "targetBuildDigest")]
        target_build_digest: Bytes32,
        #[serde(rename = "targetOwnerSha256")]
        target_owner_sha256: Bytes32,
    },
    Rollback {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
        #[serde(rename = "targetBuildDigest")]
        target_build_digest: Bytes32,
        #[serde(rename = "targetOwnerSha256")]
        target_owner_sha256: Bytes32,
    },
    Uninstall {
        #[serde(rename = "transactionId")]
        transaction_id: Bytes32,
        #[serde(rename = "sourceBuildDigest")]
        source_build_digest: Bytes32,
    },
}

impl MaintenanceAcquireParams {
    #[must_use]
    pub const fn operation(&self) -> MaintenanceOperation {
        match self {
            Self::Update { .. } => MaintenanceOperation::Update,
            Self::Rollback { .. } => MaintenanceOperation::Rollback,
            Self::Uninstall { .. } => MaintenanceOperation::Uninstall,
        }
    }

    #[must_use]
    pub const fn transaction_id(&self) -> Bytes32 {
        match self {
            Self::Update { transaction_id, .. }
            | Self::Rollback { transaction_id, .. }
            | Self::Uninstall { transaction_id, .. } => *transaction_id,
        }
    }

    #[must_use]
    pub const fn source_build_digest(&self) -> Bytes32 {
        match self {
            Self::Update {
                source_build_digest,
                ..
            }
            | Self::Rollback {
                source_build_digest,
                ..
            }
            | Self::Uninstall {
                source_build_digest,
                ..
            } => *source_build_digest,
        }
    }

    #[must_use]
    pub const fn target_build_digest(&self) -> Option<Bytes32> {
        match self {
            Self::Update {
                target_build_digest,
                ..
            }
            | Self::Rollback {
                target_build_digest,
                ..
            } => Some(*target_build_digest),
            Self::Uninstall { .. } => None,
        }
    }

    #[must_use]
    pub const fn target_owner_sha256(&self) -> Option<Bytes32> {
        match self {
            Self::Update {
                target_owner_sha256,
                ..
            }
            | Self::Rollback {
                target_owner_sha256,
                ..
            } => Some(*target_owner_sha256),
            Self::Uninstall { .. } => None,
        }
    }
}
