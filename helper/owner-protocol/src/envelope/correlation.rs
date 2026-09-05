//! Bounded outstanding requests and exact response expectations.
use super::*;

pub(super) enum ResponseExpectation {
    Method(Method),
    SessionMode(Method, SessionMode),
    Configuration(U64Expectation),
    Enabled(bool),
    Paste(Bytes32),
}

pub(super) struct U64Expectation {
    method: Method,
    value: u64,
}

impl fmt::Debug for ResponseExpectation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseExpectation([REDACTED])")
    }
}

impl ResponseExpectation {
    pub(super) fn from_request(request: &Request) -> Self {
        match request {
            Request::SessionReconcileOff(_) => {
                Self::SessionMode(request.method(), SessionMode::Off)
            }
            Request::SessionSetMode(params) => Self::SessionMode(request.method(), params.mode),
            Request::CaptureReplaceConfiguration(params) => Self::Configuration(U64Expectation {
                method: request.method(),
                value: params.revision.get(),
            }),
            Request::CaptureSetEnabled(params) => Self::Enabled(params.enabled),
            Request::PasteInject(params) => Self::Paste(params.operation_id),
            _ => Self::Method(request.method()),
        }
    }

    const fn method(&self) -> Method {
        match self {
            Self::Method(method) | Self::SessionMode(method, _) => *method,
            Self::Configuration(value) => value.method,
            Self::Enabled(_) => Method::CaptureSetEnabled,
            Self::Paste(_) => Method::PasteInject,
        }
    }

    pub(super) fn validates(&self, response: &Response) -> bool {
        let Response::Success(result) = response else {
            return true;
        };
        match (self, result) {
            (Self::SessionMode(_, expected), SuccessResult::SessionMode(actual)) => {
                *expected == actual.mode
            }
            (Self::Configuration(expected), SuccessResult::Configuration(actual)) => {
                expected.value == actual.revision.get()
            }
            (Self::Enabled(expected), SuccessResult::Enabled(actual)) => {
                *expected == actual.enabled
            }
            (Self::Paste(expected), SuccessResult::Paste(actual)) => match actual {
                crate::schema::PasteResult::ClipboardOnly { .. } => true,
                crate::schema::PasteResult::Waiting { operation_id }
                | crate::schema::PasteResult::Committed { operation_id }
                | crate::schema::PasteResult::Indeterminate { operation_id } => {
                    expected == operation_id
                }
            },
            (Self::Method(expected), actual) => success_matches_method(*expected, actual),
            _ => false,
        }
    }
}

fn success_matches_method(method: Method, result: &SuccessResult) -> bool {
    matches!(
        (method, result),
        (Method::LeaseAcquire, SuccessResult::LeaseAcquire(_))
            | (
                Method::MaintenanceAcquire,
                SuccessResult::MaintenanceAcquire(_)
            )
            | (Method::HealthGet, SuccessResult::Health(_))
            | (Method::PermissionsGet, SuccessResult::Permissions(_))
            | (Method::ObservabilityGet, SuccessResult::Observability(_))
            | (Method::FrontAppGet, SuccessResult::FrontApp(_))
            | (
                Method::FrontAppMetadataGet,
                SuccessResult::FrontAppMetadata(_),
            )
            | (Method::LeaseRenew, SuccessResult::Renew(_))
            | (Method::MaintenanceRenew, SuccessResult::Renew(_))
            | (
                Method::LeaseRelease | Method::OwnerExitWhenNeutral,
                SuccessResult::Release(_)
            )
            | (Method::RuntimeRollback, SuccessResult::Rollback(_))
            | (
                Method::MaintenancePrepare,
                SuccessResult::MaintenancePrepare(_)
            )
    )
}

#[derive(Default)]
pub struct CorrelationTracker {
    outstanding: BTreeMap<u64, ResponseExpectation>,
}

impl CorrelationTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            outstanding: BTreeMap::new(),
        }
    }

    pub fn register_request(
        &mut self,
        transport_sequence: u64,
        request: &Request,
    ) -> Result<(), CorrelationError> {
        if transport_sequence == 0 {
            return Err(CorrelationError::Zero);
        }
        if self.outstanding.contains_key(&transport_sequence) {
            return Err(CorrelationError::Duplicate);
        }
        if self.outstanding.len() >= MAX_OUTSTANDING_REQUESTS {
            return Err(CorrelationError::Capacity);
        }
        self.outstanding.insert(
            transport_sequence,
            ResponseExpectation::from_request(request),
        );
        Ok(())
    }

    pub fn expected_method(&self, correlation_sequence: u64) -> Result<Method, CorrelationError> {
        if correlation_sequence == 0 {
            return Err(CorrelationError::Zero);
        }
        self.outstanding
            .get(&correlation_sequence)
            .map(ResponseExpectation::method)
            .ok_or(CorrelationError::Unknown)
    }

    pub fn accept_response(
        &mut self,
        correlation_sequence: u64,
        response: &Response,
    ) -> Result<Method, CorrelationError> {
        let expected =
            self.outstanding
                .get(&correlation_sequence)
                .ok_or(if correlation_sequence == 0 {
                    CorrelationError::Zero
                } else {
                    CorrelationError::Unknown
                })?;
        if !expected.validates(response) {
            return Err(CorrelationError::Mismatch);
        }
        let method = expected.method();
        self.outstanding.remove(&correlation_sequence);
        Ok(method)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.outstanding.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outstanding.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CorrelationError {
    #[error("owner-protocol correlation cannot be zero")]
    Zero,
    #[error("owner-protocol request correlation is duplicated")]
    Duplicate,
    #[error("owner-protocol response correlation is unknown")]
    Unknown,
    #[error("owner-protocol response does not match its correlated request")]
    Mismatch,
    #[error("owner-protocol outstanding request capacity exceeded")]
    Capacity,
}
