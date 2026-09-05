//! Capture state accessors and authoritative owner queries.

use super::*;

impl OwnerCaptureClient {
    #[must_use]
    pub fn build_id(&self) -> &str {
        &self.build_id
    }
    #[must_use]
    pub const fn health(&self) -> &HealthResult {
        &self.health
    }
    #[must_use]
    pub const fn permissions(&self) -> &PermissionsResult {
        &self.permissions
    }
    #[must_use]
    pub fn lease_epoch(&self) -> u64 {
        self.lease.capture_lease_epoch.get()
    }
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn last_failure(&self) -> Option<OwnerClientDiagnostic> {
        self.last_failure
    }

    pub fn refresh_health(&mut self) -> Result<&HealthResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.refresh_health_until(deadline, &cancelled)
    }

    pub fn refresh_health_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<&HealthResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        self.health = match self.call_until(Request::HealthGet(Empty {}), deadline, cancelled)? {
            SuccessResult::Health(value) => value,
            _ => return self.protocol_failure("health.get", "matched"),
        };
        Ok(&self.health)
    }

    pub fn refresh_permissions(&mut self) -> Result<&PermissionsResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.refresh_permissions_until(deadline, &cancelled)
    }

    pub fn refresh_permissions_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<&PermissionsResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        self.permissions =
            match self.call_until(Request::PermissionsGet(Empty {}), deadline, cancelled)? {
                SuccessResult::Permissions(value) => value,
                _ => return self.protocol_failure("permissions.get", "matched"),
            };
        Ok(&self.permissions)
    }

    pub fn front_app(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.renew_if_due_until(deadline, &cancelled)?;
        match self.call_until(Request::FrontAppGet(Empty {}), deadline, &cancelled)? {
            SuccessResult::FrontApp(value) => Ok(value),
            _ => self.protocol_failure("front_app.get", "matched"),
        }
    }

    #[must_use]
    pub fn supports_front_app_metadata(&self) -> bool {
        self.client
            .supports_feature(talking_quill_owner_protocol::FRONT_APP_METADATA_V1)
    }

    pub fn front_app_metadata(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppMetadataResult, OwnerClientError>
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.front_app_metadata_until(deadline, &cancelled)
    }

    pub fn front_app_metadata_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<talking_quill_owner_protocol::schema::FrontAppMetadataResult, OwnerClientError>
    {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if !self.supports_front_app_metadata() {
            self.last_failure = Some(OwnerClientDiagnostic {
                category: "rejected",
                operation: "front_app.metadata_get",
                correlation_status: "not_established",
                transport_status: "open",
            });
            return Err(OwnerClientError::Rejected(ErrorCode::Incompatible));
        }
        self.renew_if_due_until(deadline, cancelled)?;
        match self.call_until(Request::FrontAppMetadataGet(Empty {}), deadline, cancelled)? {
            SuccessResult::FrontAppMetadata(value) => Ok(value),
            _ => self.protocol_failure("front_app.metadata_get", "matched"),
        }
    }

    pub fn observability(
        &mut self,
    ) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.observability_until(deadline, &cancelled)
    }

    pub fn observability_until(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
        self.last_failure = None;
        self.renew_if_due_until(deadline, cancelled)?;
        match self.call_until(Request::ObservabilityGet(Empty {}), deadline, cancelled)? {
            SuccessResult::Observability(value) => Ok(*value),
            _ => self.protocol_failure("observability.get", "matched"),
        }
    }
}
