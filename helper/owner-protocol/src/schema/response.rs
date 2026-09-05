//! Strict response union, method matching, and result bounds.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuccessResult {
    LeaseAcquire(LeaseAcquireResult),
    MaintenanceAcquire(MaintenanceAcquireResult),
    Health(HealthResult),
    Permissions(PermissionsResult),
    Observability(Box<ObservabilityResult>),
    FrontApp(FrontAppResult),
    FrontAppMetadata(FrontAppMetadataResult),
    Renew(RenewResult),
    SessionMode(SessionModeResult),
    Configuration(ConfigurationResult),
    Enabled(EnabledResult),
    Paste(PasteResult),
    Release(ReleaseResult),
    Rollback(RollbackResult),
    MaintenancePrepare(MaintenancePrepareResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Success(SuccessResult),
    Error(ErrorBody),
}

pub fn parse_response_json(method: Method, bytes: &[u8]) -> Result<Response, SchemaError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SuccessWire {
        ok: bool,
        result: Box<RawValue>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ErrorWire {
        ok: bool,
        error: ErrorBody,
    }
    // `RawValue` cannot be buffered through Serde's untagged-enum content
    // representation. Select only the boolean discriminator first, then parse
    // the exact deny-unknown-fields union arm from the original bytes. The
    // concrete parse still rejects duplicate `ok` and every extra field.
    let discriminator: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SchemaError::Json)?;
    let ok = discriminator
        .as_object()
        .and_then(|object| object.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .ok_or(SchemaError::ResponseUnion)?;
    let result = if ok {
        let SuccessWire { ok: true, result } = strict_json::<SuccessWire>(bytes)? else {
            return Err(SchemaError::ResponseUnion);
        };
        result
    } else {
        let ErrorWire { ok: false, error } = strict_json::<ErrorWire>(bytes)? else {
            return Err(SchemaError::ResponseUnion);
        };
        return Ok(Response::Error(error));
    };
    let raw_text = result.get();
    let raw = raw_text.as_bytes();
    let success = match method {
        Method::LeaseAcquire => SuccessResult::LeaseAcquire(strict_json(raw)?),
        Method::MaintenanceAcquire => SuccessResult::MaintenanceAcquire(strict_json(raw)?),
        Method::HealthGet => bounded_result(raw_text, 1024).map(SuccessResult::Health)?,
        Method::PermissionsGet => bounded_result(raw_text, 512).map(SuccessResult::Permissions)?,
        Method::ObservabilityGet => bounded_result(raw_text, 2048)
            .map(Box::new)
            .map(SuccessResult::Observability)?,
        Method::FrontAppGet => bounded_result(raw_text, 1024).map(SuccessResult::FrontApp)?,
        Method::FrontAppMetadataGet => {
            bounded_result(raw_text, 1024).map(SuccessResult::FrontAppMetadata)?
        }
        Method::LeaseRenew | Method::MaintenanceRenew => SuccessResult::Renew(strict_json(raw)?),
        Method::SessionReconcileOff | Method::SessionSetMode => {
            SuccessResult::SessionMode(strict_json(raw)?)
        }
        Method::CaptureReplaceConfiguration => SuccessResult::Configuration(strict_json(raw)?),
        Method::CaptureSetEnabled => SuccessResult::Enabled(strict_json(raw)?),
        Method::PasteInject => SuccessResult::Paste(strict_json(raw)?),
        Method::LeaseRelease | Method::OwnerExitWhenNeutral => {
            SuccessResult::Release(strict_json(raw)?)
        }
        Method::RuntimeRollback => SuccessResult::Rollback(strict_json(raw)?),
        Method::MaintenancePrepare => SuccessResult::MaintenancePrepare(strict_json(raw)?),
    };
    validate_success(&success)?;
    Ok(Response::Success(success))
}

impl Response {
    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum Wire<'a, T> {
            Success { ok: bool, result: &'a T },
            Error { ok: bool, error: &'a ErrorBody },
        }
        fn success<T: Serialize>(result: &T, maximum: usize) -> Result<Vec<u8>, SchemaError> {
            let raw = serde_json::to_vec(result).map_err(|_| SchemaError::Json)?;
            if raw.len() > maximum {
                return Err(SchemaError::Bounds);
            }
            serde_json::to_vec(&Wire::Success { ok: true, result }).map_err(|_| SchemaError::Json)
        }
        match self {
            Self::Error(error) => serde_json::to_vec(&Wire::<()>::Error { ok: false, error })
                .map_err(|_| SchemaError::Json),
            Self::Success(result) => {
                validate_success(result)?;
                match result {
                    SuccessResult::LeaseAcquire(v) => success(v, usize::MAX),
                    SuccessResult::MaintenanceAcquire(v) => success(v, usize::MAX),
                    SuccessResult::Health(v) => success(v, 1024),
                    SuccessResult::Permissions(v) => success(v, 512),
                    SuccessResult::Observability(v) => success(v, 2048),
                    SuccessResult::FrontApp(v) => success(v, 1024),
                    SuccessResult::FrontAppMetadata(v) => success(v, 1024),
                    SuccessResult::Renew(v) => success(v, usize::MAX),
                    SuccessResult::SessionMode(v) => success(v, usize::MAX),
                    SuccessResult::Configuration(v) => success(v, usize::MAX),
                    SuccessResult::Enabled(v) => success(v, usize::MAX),
                    SuccessResult::Paste(v) => success(v, usize::MAX),
                    SuccessResult::Release(v) => success(v, usize::MAX),
                    SuccessResult::Rollback(v) => success(v, usize::MAX),
                    SuccessResult::MaintenancePrepare(v) => success(v, usize::MAX),
                }
            }
        }
    }
}

fn validate_success(result: &SuccessResult) -> Result<(), SchemaError> {
    match result {
        SuccessResult::LeaseAcquire(LeaseAcquireResult {
            capture_lease_id, ..
        }) if capture_lease_id.as_bytes().iter().all(|byte| *byte == 0) => {
            Err(SchemaError::InvalidSuccess)
        }
        SuccessResult::MaintenanceAcquire(MaintenanceAcquireResult {
            maintenance_capability_id,
            ..
        }) if maintenance_capability_id
            .as_bytes()
            .iter()
            .all(|byte| *byte == 0) =>
        {
            Err(SchemaError::InvalidSuccess)
        }
        SuccessResult::Renew(RenewResult { renewed: true })
        | SuccessResult::Rollback(RollbackResult { latched: true, .. }) => Ok(()),
        SuccessResult::MaintenancePrepare(MaintenancePrepareResult {
            ready_to_exit: true,
            owner_handoff,
        }) if owner_handoff.as_bytes().iter().any(|byte| *byte != 0) => Ok(()),
        SuccessResult::Renew(_)
        | SuccessResult::MaintenancePrepare(_)
        | SuccessResult::Rollback(_) => Err(SchemaError::InvalidSuccess),
        SuccessResult::FrontApp(FrontAppResult {
            available,
            application_token,
        }) if *available == application_token.is_some() => Ok(()),
        SuccessResult::FrontApp(_) => Err(SchemaError::InvalidSuccess),
        SuccessResult::FrontAppMetadata(FrontAppMetadataResult {
            available,
            process_name,
            window_title,
            window_bounds,
        }) if if *available {
            process_name.is_some() && window_title.is_some()
        } else {
            process_name.is_none() && window_title.is_none() && window_bounds.is_none()
        } =>
        {
            Ok(())
        }
        SuccessResult::FrontAppMetadata(_) => Err(SchemaError::InvalidSuccess),
        _ => Ok(()),
    }
}

fn bounded_result<T: DeserializeOwned>(raw: &str, maximum: usize) -> Result<T, SchemaError> {
    if raw.len() > maximum {
        return Err(SchemaError::Bounds);
    }
    strict_json(raw.as_bytes())
}
