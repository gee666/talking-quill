//! Capability identity and command sequencing.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    Capture,
    Maintenance,
}

pub struct CapabilitySequenceValidator {
    kind: CapabilityKind,
    capability_id: Bytes32,
    capability_epoch: u64,
    sequence: SequenceValidator,
}

impl fmt::Debug for CapabilitySequenceValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilitySequenceValidator([REDACTED])")
    }
}

impl CapabilitySequenceValidator {
    pub fn new(
        kind: CapabilityKind,
        capability_id: Bytes32,
        capability_epoch: u64,
    ) -> Result<Self, CapabilitySequenceError> {
        if capability_epoch == 0 || capability_id.as_bytes().iter().all(|byte| *byte == 0) {
            return Err(CapabilitySequenceError::InvalidCapability);
        }
        Ok(Self {
            kind,
            capability_id,
            capability_epoch,
            sequence: SequenceValidator::new(),
        })
    }

    /// Consumes a command sequence before semantic state validation. Any error
    /// is a capability protocol fault and leaves the high-water unchanged.
    pub fn accept(&mut self, request: &Request) -> Result<(), CapabilitySequenceError> {
        let (kind, capability_id, capability_epoch, command_sequence) = match request {
            Request::LeaseRenew(value)
            | Request::SessionReconcileOff(value)
            | Request::LeaseRelease(value)
            | Request::OwnerExitWhenNeutral(value)
            | Request::RuntimeRollback(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::SessionSetMode(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::CaptureReplaceConfiguration(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::CaptureSetEnabled(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::PasteInject(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::MaintenanceRenew(value) => (
                CapabilityKind::Maintenance,
                &value.maintenance_capability_id,
                value.maintenance_capability_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::MaintenancePrepare(value) => (
                CapabilityKind::Maintenance,
                &value.maintenance_capability_id,
                value.maintenance_capability_epoch.get(),
                value.command_sequence.get(),
            ),
            _ => return Err(CapabilitySequenceError::NotCapabilityCommand),
        };
        if kind != self.kind
            || capability_id != &self.capability_id
            || capability_epoch != self.capability_epoch
        {
            return Err(CapabilitySequenceError::WrongCapability);
        }
        self.sequence
            .accept(command_sequence)
            .map_err(CapabilitySequenceError::Sequence)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CapabilitySequenceError {
    #[error("owner-protocol capability identity is invalid")]
    InvalidCapability,
    #[error("owner-protocol request is not capability-sequenced")]
    NotCapabilityCommand,
    #[error("owner-protocol command presents the wrong capability or epoch")]
    WrongCapability,
    #[error(transparent)]
    Sequence(SequenceError),
}
