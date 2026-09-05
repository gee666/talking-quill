//! Exact method names and purpose authority.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    LeaseAcquire,
    MaintenanceAcquire,
    HealthGet,
    PermissionsGet,
    ObservabilityGet,
    FrontAppGet,
    FrontAppMetadataGet,
    LeaseRenew,
    SessionReconcileOff,
    SessionSetMode,
    CaptureReplaceConfiguration,
    CaptureSetEnabled,
    PasteInject,
    LeaseRelease,
    OwnerExitWhenNeutral,
    RuntimeRollback,
    MaintenanceRenew,
    MaintenancePrepare,
}

impl Method {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeaseAcquire => "lease.acquire",
            Self::MaintenanceAcquire => "maintenance.acquire",
            Self::HealthGet => "health.get",
            Self::PermissionsGet => "permissions.get",
            Self::ObservabilityGet => "observability.get",
            Self::FrontAppGet => "front_app.get",
            Self::FrontAppMetadataGet => "front_app.metadata.get",
            Self::LeaseRenew => "lease.renew",
            Self::SessionReconcileOff => "session.reconcile_off",
            Self::SessionSetMode => "session.set_mode",
            Self::CaptureReplaceConfiguration => "capture.replace_configuration",
            Self::CaptureSetEnabled => "capture.set_enabled",
            Self::PasteInject => "paste.inject",
            Self::LeaseRelease => "lease.release",
            Self::OwnerExitWhenNeutral => "owner.exit_when_neutral",
            Self::RuntimeRollback => "runtime.rollback",
            Self::MaintenanceRenew => "maintenance.renew",
            Self::MaintenancePrepare => "maintenance.prepare",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "lease.acquire" => Self::LeaseAcquire,
            "maintenance.acquire" => Self::MaintenanceAcquire,
            "health.get" => Self::HealthGet,
            "permissions.get" => Self::PermissionsGet,
            "observability.get" => Self::ObservabilityGet,
            "front_app.get" => Self::FrontAppGet,
            "front_app.metadata.get" => Self::FrontAppMetadataGet,
            "lease.renew" => Self::LeaseRenew,
            "session.reconcile_off" => Self::SessionReconcileOff,
            "session.set_mode" => Self::SessionSetMode,
            "capture.replace_configuration" => Self::CaptureReplaceConfiguration,
            "capture.set_enabled" => Self::CaptureSetEnabled,
            "paste.inject" => Self::PasteInject,
            "lease.release" => Self::LeaseRelease,
            "owner.exit_when_neutral" => Self::OwnerExitWhenNeutral,
            "runtime.rollback" => Self::RuntimeRollback,
            "maintenance.renew" => Self::MaintenanceRenew,
            "maintenance.prepare" => Self::MaintenancePrepare,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn is_capability_mutation(self) -> bool {
        !matches!(
            self,
            Self::LeaseAcquire
                | Self::MaintenanceAcquire
                | Self::HealthGet
                | Self::PermissionsGet
                | Self::ObservabilityGet
                | Self::FrontAppGet
                | Self::FrontAppMetadataGet
        )
    }

    #[must_use]
    pub const fn allowed_for(self, purpose: Purpose) -> bool {
        match purpose {
            Purpose::Observe => matches!(
                self,
                Self::HealthGet
                    | Self::PermissionsGet
                    | Self::ObservabilityGet
                    | Self::FrontAppGet
                    | Self::FrontAppMetadataGet
            ),
            Purpose::Capture => !matches!(
                self,
                Self::MaintenanceAcquire | Self::MaintenanceRenew | Self::MaintenancePrepare
            ),
            Purpose::Maintenance => matches!(
                self,
                Self::HealthGet
                    | Self::ObservabilityGet
                    | Self::MaintenanceAcquire
                    | Self::MaintenanceRenew
                    | Self::MaintenancePrepare
            ),
        }
    }
}

impl Serialize for Method {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}
