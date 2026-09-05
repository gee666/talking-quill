//! Immutable predecessor lease terminal routing.
use super::*;

pub struct PredecessorRouteValidator {
    capture_lease_id: Bytes32,
    capture_lease_epoch: u64,
    terminal_sequence: SequenceValidator,
    revoked: bool,
    final_seen: bool,
}

impl fmt::Debug for PredecessorRouteValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorRouteValidator([REDACTED])")
    }
}

impl PredecessorRouteValidator {
    #[must_use]
    pub fn new(capture_lease_id: Bytes32, capture_lease_epoch: u64) -> Self {
        Self {
            capture_lease_id,
            capture_lease_epoch,
            terminal_sequence: SequenceValidator::new(),
            revoked: false,
            final_seen: false,
        }
    }

    pub fn accept(
        &mut self,
        event: &crate::schema::PredecessorTerminalEvent,
    ) -> Result<(), TerminalRouteError> {
        use crate::schema::PredecessorTerminalEvent as Event;
        let (lease_id, lease_epoch, sequence, is_revoked, is_final) = match event {
            Event::LeaseRevoked {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                true,
                false,
            ),
            Event::LeaseDraining {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                false,
                false,
            ),
            Event::LeaseNeutral {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            }
            | Event::LeaseUnavailable {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                false,
                true,
            ),
        };
        if self.final_seen
            || lease_id != &self.capture_lease_id
            || lease_epoch != self.capture_lease_epoch
            || is_revoked == self.revoked
            || (!self.revoked && !is_revoked)
            || self.terminal_sequence.next() != Some(sequence)
        {
            return Err(TerminalRouteError::Invalid);
        }
        self.terminal_sequence
            .accept(sequence)
            .map_err(|_| TerminalRouteError::Invalid)?;
        self.revoked |= is_revoked;
        self.final_seen |= is_final;
        Ok(())
    }

    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.final_seen
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum TerminalRouteError {
    #[error("owner-protocol predecessor terminal route is invalid")]
    Invalid,
}
