use super::client::{ConnectError, OwnerClientError};
use crate::gateway::PlatformError;
use talking_quill_owner_protocol::schema as wire;

pub(super) fn client_error_category(error: &OwnerClientError) -> &'static str {
    match error {
        OwnerClientError::Connect(_) => "connect",
        OwnerClientError::Disconnected => "disconnected",
        OwnerClientError::Uncertain => "uncertain",
        OwnerClientError::Protocol => "protocol",
        OwnerClientError::AcquireRejected(_) => "acquire_rejected",
        OwnerClientError::Rejected(_) => "rejected",
        OwnerClientError::SequenceExhausted => "sequence_exhausted",
        OwnerClientError::Transport => "transport",
        OwnerClientError::Cancelled => "cancelled",
    }
}

pub(super) fn map_client_error(error: &OwnerClientError) -> PlatformError {
    match error {
        OwnerClientError::Connect(ConnectError::Authentication) => {
            PlatformError::OwnerAuthentication
        }
        OwnerClientError::Connect(ConnectError::Incompatible) => PlatformError::OwnerIncompatible,
        OwnerClientError::Connect(ConnectError::Busy) => PlatformError::OwnerBusy,
        OwnerClientError::Connect(ConnectError::SingletonCollision) => {
            PlatformError::OwnerSingletonCollision
        }
        OwnerClientError::Connect(_) => PlatformError::OwnerUnavailable,
        OwnerClientError::AcquireRejected(wire::ErrorCode::Busy)
        | OwnerClientError::Rejected(wire::ErrorCode::Busy) => PlatformError::OwnerBusy,
        OwnerClientError::AcquireRejected(
            wire::ErrorCode::Draining | wire::ErrorCode::InvalidState,
        )
        | OwnerClientError::Rejected(wire::ErrorCode::Draining | wire::ErrorCode::InvalidState) => {
            PlatformError::OwnerDraining
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Incompatible)
        | OwnerClientError::Rejected(wire::ErrorCode::Incompatible) => {
            PlatformError::OwnerIncompatible
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Rollback)
        | OwnerClientError::Rejected(wire::ErrorCode::Rollback) => PlatformError::OwnerRollback,
        OwnerClientError::AcquireRejected(wire::ErrorCode::SecurityFault)
        | OwnerClientError::Rejected(wire::ErrorCode::SecurityFault) => {
            PlatformError::OwnerSecurityFault
        }
        OwnerClientError::AcquireRejected(wire::ErrorCode::Indeterminate)
        | OwnerClientError::Rejected(wire::ErrorCode::Indeterminate)
        | OwnerClientError::Uncertain => PlatformError::Indeterminate,
        OwnerClientError::AcquireRejected(wire::ErrorCode::NativeFailure)
        | OwnerClientError::Rejected(wire::ErrorCode::NativeFailure) => {
            PlatformError::NativeFailure
        }
        OwnerClientError::Cancelled => PlatformError::OwnerUnavailable,
        _ => PlatformError::OwnerUnavailable,
    }
}
