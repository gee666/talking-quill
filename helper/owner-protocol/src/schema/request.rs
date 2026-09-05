//! Strict method-specific request serialization and parsing.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    LeaseAcquire(Empty),
    MaintenanceAcquire(MaintenanceAcquireParams),
    HealthGet(Empty),
    PermissionsGet(Empty),
    ObservabilityGet(Empty),
    FrontAppGet(Empty),
    FrontAppMetadataGet(Empty),
    LeaseRenew(CaptureCommandParams),
    SessionReconcileOff(CaptureCommandParams),
    SessionSetMode(SessionSetModeParams),
    CaptureReplaceConfiguration(ReplaceConfigurationParams),
    CaptureSetEnabled(SetEnabledParams),
    PasteInject(PasteInjectParams),
    LeaseRelease(CaptureCommandParams),
    OwnerExitWhenNeutral(CaptureCommandParams),
    RuntimeRollback(CaptureCommandParams),
    MaintenanceRenew(MaintenanceCommandParams),
    MaintenancePrepare(MaintenancePrepareParams),
}

impl Request {
    #[must_use]
    pub const fn method(&self) -> Method {
        match self {
            Self::LeaseAcquire(_) => Method::LeaseAcquire,
            Self::MaintenanceAcquire(_) => Method::MaintenanceAcquire,
            Self::HealthGet(_) => Method::HealthGet,
            Self::PermissionsGet(_) => Method::PermissionsGet,
            Self::ObservabilityGet(_) => Method::ObservabilityGet,
            Self::FrontAppGet(_) => Method::FrontAppGet,
            Self::FrontAppMetadataGet(_) => Method::FrontAppMetadataGet,
            Self::LeaseRenew(_) => Method::LeaseRenew,
            Self::SessionReconcileOff(_) => Method::SessionReconcileOff,
            Self::SessionSetMode(_) => Method::SessionSetMode,
            Self::CaptureReplaceConfiguration(_) => Method::CaptureReplaceConfiguration,
            Self::CaptureSetEnabled(_) => Method::CaptureSetEnabled,
            Self::PasteInject(_) => Method::PasteInject,
            Self::LeaseRelease(_) => Method::LeaseRelease,
            Self::OwnerExitWhenNeutral(_) => Method::OwnerExitWhenNeutral,
            Self::RuntimeRollback(_) => Method::RuntimeRollback,
            Self::MaintenanceRenew(_) => Method::MaintenanceRenew,
            Self::MaintenancePrepare(_) => Method::MaintenancePrepare,
        }
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        fn encode<T: Serialize>(method: Method, params: &T) -> Result<Vec<u8>, SchemaError> {
            #[derive(Serialize)]
            struct Wire<'a, T> {
                method: Method,
                params: &'a T,
            }
            serde_json::to_vec(&Wire { method, params }).map_err(|_| SchemaError::Json)
        }
        let encoded = match self {
            Self::LeaseAcquire(v) => encode(self.method(), v),
            Self::MaintenanceAcquire(v) => encode(self.method(), v),
            Self::HealthGet(v) => encode(self.method(), v),
            Self::PermissionsGet(v) => encode(self.method(), v),
            Self::ObservabilityGet(v) => encode(self.method(), v),
            Self::FrontAppGet(v) => encode(self.method(), v),
            Self::FrontAppMetadataGet(v) => encode(self.method(), v),
            Self::LeaseRenew(v) => encode(self.method(), v),
            Self::SessionReconcileOff(v) => encode(self.method(), v),
            Self::SessionSetMode(v) => encode(self.method(), v),
            Self::CaptureReplaceConfiguration(v) => encode(self.method(), v),
            Self::CaptureSetEnabled(v) => encode(self.method(), v),
            Self::PasteInject(v) => encode(self.method(), v),
            Self::LeaseRelease(v) => encode(self.method(), v),
            Self::OwnerExitWhenNeutral(v) => encode(self.method(), v),
            Self::RuntimeRollback(v) => encode(self.method(), v),
            Self::MaintenanceRenew(v) => encode(self.method(), v),
            Self::MaintenancePrepare(v) => encode(self.method(), v),
        }?;
        parse_request_json(&encoded)?;
        Ok(encoded)
    }
}

pub fn parse_request_json(bytes: &[u8]) -> Result<Request, SchemaError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        method: String,
        params: Box<RawValue>,
    }
    let wire: Wire = strict_json(bytes)?;
    let method = Method::parse(&wire.method).ok_or(SchemaError::UnknownMethod)?;
    macro_rules! parse {
        ($variant:ident, $type:ty) => {
            strict_json::<$type>(wire.params.get().as_bytes()).map(Request::$variant)
        };
    }
    match method {
        Method::LeaseAcquire => parse!(LeaseAcquire, Empty),
        Method::MaintenanceAcquire => parse!(MaintenanceAcquire, MaintenanceAcquireParams),
        Method::HealthGet => parse!(HealthGet, Empty),
        Method::PermissionsGet => parse!(PermissionsGet, Empty),
        Method::ObservabilityGet => parse!(ObservabilityGet, Empty),
        Method::FrontAppGet => parse!(FrontAppGet, Empty),
        Method::FrontAppMetadataGet => parse!(FrontAppMetadataGet, Empty),
        Method::LeaseRenew => parse!(LeaseRenew, CaptureCommandParams),
        Method::SessionReconcileOff => parse!(SessionReconcileOff, CaptureCommandParams),
        Method::SessionSetMode => parse!(SessionSetMode, SessionSetModeParams),
        Method::CaptureReplaceConfiguration => {
            parse!(CaptureReplaceConfiguration, ReplaceConfigurationParams)
        }
        Method::CaptureSetEnabled => parse!(CaptureSetEnabled, SetEnabledParams),
        Method::PasteInject => parse!(PasteInject, PasteInjectParams),
        Method::LeaseRelease => parse!(LeaseRelease, CaptureCommandParams),
        Method::OwnerExitWhenNeutral => parse!(OwnerExitWhenNeutral, CaptureCommandParams),
        Method::RuntimeRollback => parse!(RuntimeRollback, CaptureCommandParams),
        Method::MaintenanceRenew => parse!(MaintenanceRenew, MaintenanceCommandParams),
        Method::MaintenancePrepare => parse!(MaintenancePrepare, MaintenancePrepareParams),
    }
}
