//! Ordered capture configuration and session mutations.

use super::*;

impl OwnerCaptureClient {
    /// Full replacement followed by an explicit enable decision. If capture was
    /// enabled, disabling is a separate preceding mutation. No request is ever
    /// retransmitted after a timeout/disconnect.
    pub fn configure(
        &mut self,
        bindings: talking_quill_owner_protocol::schema::Bindings,
        enabled: bool,
    ) -> Result<(), OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.configure_until(bindings, enabled, deadline, &cancelled)
    }

    pub fn configure_until(
        &mut self,
        bindings: talking_quill_owner_protocol::schema::Bindings,
        enabled: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        self.check_budget(deadline, cancelled)?;
        if self.enabled {
            self.set_enabled_until(false, deadline, cancelled)?;
        }
        self.check_budget(deadline, cancelled)?;
        self.revision = match self.revision.checked_add(1) {
            Some(revision) => revision,
            None => return self.sequence_failure("capture.replace_configuration"),
        };
        let base = self.capture_params_until(deadline, cancelled)?;
        let revision = match U64String::try_from(self.revision) {
            Ok(revision) => revision,
            Err(_) => {
                return self.protocol_failure("capture.replace_configuration", "not_established");
            }
        };
        let params = ReplaceConfigurationParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            revision,
            bindings,
        };
        match self.call_until(
            Request::CaptureReplaceConfiguration(params),
            deadline,
            cancelled,
        )? {
            SuccessResult::Configuration(value) if value.revision.get() == self.revision => {}
            _ => return self.protocol_failure("capture.replace_configuration", "matched"),
        }
        self.check_budget(deadline, cancelled)?;
        self.set_enabled_until(enabled, deadline, cancelled)
    }

    pub fn set_session_mode(&mut self, mode: SessionMode) -> Result<(), OwnerClientError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let deadline = self.operation_deadline();
        self.set_session_mode_until(mode, deadline, &cancelled)
    }

    pub fn set_session_mode_until(
        &mut self,
        mode: SessionMode,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        let base = self.capture_params_until(deadline, cancelled)?;
        let params = SessionSetModeParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            mode,
        };
        match self.call_until(Request::SessionSetMode(params), deadline, cancelled)? {
            SuccessResult::SessionMode(value) if value.mode == mode => Ok(()),
            _ => self.protocol_failure("session.set_mode", "matched"),
        }
    }

    pub(super) fn set_enabled_until(
        &mut self,
        enabled: bool,
        deadline: Instant,
        cancelled: &AtomicBool,
    ) -> Result<(), OwnerClientError> {
        self.last_failure = None;
        let base = self.capture_params_until(deadline, cancelled)?;
        let params = SetEnabledParams {
            capture_lease_id: base.capture_lease_id,
            capture_lease_epoch: base.capture_lease_epoch,
            command_sequence: base.command_sequence,
            enabled,
        };
        match self.call_until(Request::CaptureSetEnabled(params), deadline, cancelled)? {
            SuccessResult::Enabled(value) if value.enabled == enabled => {
                self.enabled = enabled;
                Ok(())
            }
            _ => self.protocol_failure("capture.set_enabled", "matched"),
        }
    }
}
