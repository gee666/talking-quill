//! Predecessor lease terminal wire events.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevocationReason {
    Eof,
    Heartbeat,
    Maintenance,
    Release,
    Protocol,
    Rollback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipKind {
    Candidate,
    Activation,
    Session,
    ReplayCleanup,
    Paste,
    Multiple,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
pub enum PredecessorTerminalEvent {
    #[serde(rename = "lease.revoked")]
    LeaseRevoked {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        reason: RevocationReason,
    },
    #[serde(rename = "lease.draining")]
    LeaseDraining {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        ownership: OwnershipKind,
    },
    #[serde(rename = "lease.neutral")]
    LeaseNeutral {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        disposition: LeaseDisposition,
    },
    #[serde(rename = "lease.unavailable")]
    LeaseUnavailable {
        #[serde(rename = "captureLeaseId")]
        capture_lease_id: Bytes32,
        #[serde(rename = "captureLeaseEpoch")]
        capture_lease_epoch: U64String,
        #[serde(rename = "terminalSequence")]
        terminal_sequence: U64String,
        reason: TerminalUnavailableReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalUnavailableReason {
    NativeFault,
    OwnershipUnknown,
}

impl PredecessorTerminalEvent {
    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_predecessor_terminal_json(&bytes)?;
        Ok(bytes)
    }
}

pub fn parse_predecessor_terminal_json(
    bytes: &[u8],
) -> Result<PredecessorTerminalEvent, SchemaError> {
    let event: PredecessorTerminalEvent = strict_json(bytes)?;
    if matches!(
        event,
        PredecessorTerminalEvent::LeaseNeutral {
            disposition: LeaseDisposition::Draining,
            ..
        }
    ) {
        return Err(SchemaError::InvalidEvent);
    }
    Ok(event)
}
