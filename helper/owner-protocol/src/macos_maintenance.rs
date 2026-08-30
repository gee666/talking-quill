//! Fixed macOS installed-maintenance record shared by the owner and finalizer.

use crate::Bytes32;

pub const MACOS_MAINTENANCE_RECORD_BYTES: usize = 168;
const MAGIC: &[u8; 5] = b"TQKM2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosMaintenancePhase {
    InProgress,
    InstallationComplete,
    RolledBack,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosMaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MacosMaintenanceRecord {
    pub phase: MacosMaintenancePhase,
    pub operation: MacosMaintenanceOperation,
    pub transaction: Bytes32,
    pub source_build: Bytes32,
    pub target_build: Option<Bytes32>,
    pub target_owner: Option<Bytes32>,
    pub owner_handoff: Bytes32,
}

impl std::fmt::Debug for MacosMaintenanceRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MacosMaintenanceRecord(<redacted>)")
    }
}

impl MacosMaintenanceRecord {
    pub fn in_progress(
        operation: MacosMaintenanceOperation,
        transaction: Bytes32,
        source_build: Bytes32,
        target_build: Option<Bytes32>,
        target_owner: Option<Bytes32>,
        owner_handoff: Bytes32,
    ) -> Result<Self, MacosMaintenanceRecordError> {
        let record = Self {
            phase: MacosMaintenancePhase::InProgress,
            operation,
            transaction,
            source_build,
            target_build,
            target_owner,
            owner_handoff,
        };
        record.validate()?;
        Ok(record)
    }

    #[must_use]
    pub fn with_phase(mut self, phase: MacosMaintenancePhase) -> Self {
        self.phase = phase;
        self
    }

    pub fn encode(
        self,
    ) -> Result<[u8; MACOS_MAINTENANCE_RECORD_BYTES], MacosMaintenanceRecordError> {
        self.validate()?;
        let mut bytes = [0_u8; MACOS_MAINTENANCE_RECORD_BYTES];
        bytes[..5].copy_from_slice(MAGIC);
        bytes[5] = match self.phase {
            MacosMaintenancePhase::InProgress => 1,
            MacosMaintenancePhase::InstallationComplete => 2,
            MacosMaintenancePhase::RolledBack => 3,
        };
        bytes[6] = match self.operation {
            MacosMaintenanceOperation::Update => 1,
            MacosMaintenanceOperation::Uninstall => 2,
            MacosMaintenanceOperation::Rollback => 3,
        };
        bytes[8..40].copy_from_slice(self.transaction.as_bytes());
        bytes[40..72].copy_from_slice(self.source_build.as_bytes());
        if let Some(value) = self.target_build {
            bytes[72..104].copy_from_slice(value.as_bytes());
        }
        if let Some(value) = self.target_owner {
            bytes[104..136].copy_from_slice(value.as_bytes());
        }
        bytes[136..168].copy_from_slice(self.owner_handoff.as_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, MacosMaintenanceRecordError> {
        if bytes.len() != MACOS_MAINTENANCE_RECORD_BYTES {
            return Err(MacosMaintenanceRecordError);
        }
        if bytes.iter().all(|byte| *byte == 0) {
            return Ok(None);
        }
        if &bytes[..5] != MAGIC || bytes[7] != 0 {
            return Err(MacosMaintenanceRecordError);
        }
        let phase = match bytes[5] {
            1 => MacosMaintenancePhase::InProgress,
            2 => MacosMaintenancePhase::InstallationComplete,
            3 => MacosMaintenancePhase::RolledBack,
            _ => return Err(MacosMaintenanceRecordError),
        };
        let operation = match bytes[6] {
            1 => MacosMaintenanceOperation::Update,
            2 => MacosMaintenanceOperation::Uninstall,
            3 => MacosMaintenanceOperation::Rollback,
            _ => return Err(MacosMaintenanceRecordError),
        };
        let digest = |range: std::ops::Range<usize>| {
            Bytes32::new(bytes[range].try_into().expect("fixed digest range"))
        };
        let target_build = digest(72..104);
        let target_owner = digest(104..136);
        let record = Self {
            phase,
            operation,
            transaction: digest(8..40),
            source_build: digest(40..72),
            target_build: (!zero(target_build)).then_some(target_build),
            target_owner: (!zero(target_owner)).then_some(target_owner),
            owner_handoff: digest(136..168),
        };
        record.validate()?;
        Ok(Some(record))
    }

    fn validate(&self) -> Result<(), MacosMaintenanceRecordError> {
        let has_target = self.target_build.is_some() && self.target_owner.is_some();
        if zero(self.transaction)
            || zero(self.source_build)
            || zero(self.owner_handoff)
            || (self.operation == MacosMaintenanceOperation::Uninstall && has_target)
            || (self.operation != MacosMaintenanceOperation::Uninstall && !has_target)
            || self.target_build.is_some() != self.target_owner.is_some()
            || (self.phase == MacosMaintenancePhase::InstallationComplete
                && self.operation == MacosMaintenanceOperation::Uninstall)
        {
            return Err(MacosMaintenanceRecordError);
        }
        Ok(())
    }
}

fn zero(value: Bytes32) -> bool {
    value.as_bytes().iter().all(|byte| *byte == 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("the macOS maintenance record is invalid")]
pub struct MacosMaintenanceRecordError;

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: u8) -> Bytes32 {
        Bytes32::new([value; 32])
    }

    #[test]
    fn exact_record_round_trip_and_phase_transition() {
        let record = MacosMaintenanceRecord::in_progress(
            MacosMaintenanceOperation::Update,
            bytes(1),
            bytes(2),
            Some(bytes(3)),
            Some(bytes(4)),
            bytes(5),
        )
        .unwrap();
        assert_eq!(
            MacosMaintenanceRecord::decode(&record.encode().unwrap()).unwrap(),
            Some(record)
        );
        let complete = record.with_phase(MacosMaintenancePhase::InstallationComplete);
        assert_eq!(
            MacosMaintenanceRecord::decode(&complete.encode().unwrap()).unwrap(),
            Some(complete)
        );
    }

    #[test]
    fn baseline_malformed_missing_handoff_and_partial_target_fail_closed() {
        assert_eq!(
            MacosMaintenanceRecord::decode(&[0; MACOS_MAINTENANCE_RECORD_BYTES]).unwrap(),
            None
        );
        let mut malformed = [0_u8; MACOS_MAINTENANCE_RECORD_BYTES];
        malformed[..5].copy_from_slice(MAGIC);
        malformed[5] = 1;
        malformed[6] = 1;
        malformed[8..40].fill(1);
        malformed[40..72].fill(2);
        malformed[72..104].fill(3);
        assert!(MacosMaintenanceRecord::decode(&malformed).is_err());
    }
}
