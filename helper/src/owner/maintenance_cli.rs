//! Strict maintenance-only helper entry point.
//!
//! Arguments contain only public transaction/build digests. Authentication
//! credentials and streams are supplied by the platform authority connector.

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::schema::{MaintenanceAcquireParams, MaintenanceOperation};

use super::client::{ConnectError, OwnerClientError, OwnerConnector, OwnerMaintenanceClient};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum MaintenanceCliExit {
    Success = 0,
    Usage = 64,
    Unavailable = 69,
    Failed = 70,
    TemporaryFailure = 75,
    Authentication = 77,
    Incompatible = 78,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerExitVerificationError {
    Unavailable,
    Failed,
    Timeout,
    Cancelled,
}

/// Launcher-provided R8 boundary proving that the owner process exited and its
/// stable singleton was released. Implementations must honor `cancelled` and
/// `deadline`; R6 never infers exit from a maintenance response alone.
pub trait OwnerExitVerifier: Send + Sync {
    fn wait_for_owner_exit(
        &self,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<bool, OwnerExitVerificationError>;
}

#[derive(Debug, Default)]
pub struct UnavailableOwnerExitVerifier;

impl OwnerExitVerifier for UnavailableOwnerExitVerifier {
    fn wait_for_owner_exit(
        &self,
        _cancelled: &AtomicBool,
        _deadline: Instant,
    ) -> Result<bool, OwnerExitVerificationError> {
        Err(OwnerExitVerificationError::Unavailable)
    }
}

impl MaintenanceCliExit {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// Runs maintenance without constructing an Electron protocol server or any
/// capture client. Exact syntax:
///
/// `update|rollback TRANSACTION SOURCE_BUILD TARGET_BUILD TARGET_OWNER_SHA256`
/// `uninstall TRANSACTION SOURCE_BUILD`
///
/// Every value is exactly 64 lowercase hexadecimal characters.
pub fn run_maintenance_only(
    connector: &mut dyn OwnerConnector,
    arguments: &[String],
) -> MaintenanceCliExit {
    run_maintenance_only_with_verifier(
        connector,
        &UnavailableOwnerExitVerifier,
        &AtomicBool::new(false),
        arguments,
    )
}

pub fn run_maintenance_only_with_verifier(
    connector: &mut dyn OwnerConnector,
    verifier: &dyn OwnerExitVerifier,
    cancelled: &AtomicBool,
    arguments: &[String],
) -> MaintenanceCliExit {
    let Some((params, transaction_id, operation)) = parse(arguments) else {
        return MaintenanceCliExit::Usage;
    };
    let mut client = match OwnerMaintenanceClient::acquire(connector, params) {
        Ok(client) => client,
        Err(error) => return classify_acquisition_error(&error),
    };
    if client.prepare(transaction_id, operation).is_err() {
        return MaintenanceCliExit::Failed;
    }
    verify_owner_exit(
        verifier,
        cancelled,
        Instant::now() + Duration::from_secs(30),
    )
}

fn verify_owner_exit(
    verifier: &dyn OwnerExitVerifier,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> MaintenanceCliExit {
    match verifier.wait_for_owner_exit(cancelled, deadline) {
        Ok(true) => MaintenanceCliExit::Success,
        Ok(false) | Err(OwnerExitVerificationError::Failed) => MaintenanceCliExit::Failed,
        Err(OwnerExitVerificationError::Unavailable) => MaintenanceCliExit::Unavailable,
        Err(OwnerExitVerificationError::Timeout | OwnerExitVerificationError::Cancelled) => {
            MaintenanceCliExit::TemporaryFailure
        }
    }
}

fn classify_acquisition_error(error: &OwnerClientError) -> MaintenanceCliExit {
    use talking_quill_owner_protocol::schema::ErrorCode;
    match error {
        OwnerClientError::Connect(ConnectError::Incompatible)
        | OwnerClientError::AcquireRejected(ErrorCode::Incompatible) => {
            MaintenanceCliExit::Incompatible
        }
        OwnerClientError::Connect(ConnectError::Authentication)
        | OwnerClientError::AcquireRejected(ErrorCode::SecurityFault) => {
            MaintenanceCliExit::Authentication
        }
        OwnerClientError::Connect(ConnectError::Busy | ConnectError::SingletonCollision)
        | OwnerClientError::AcquireRejected(
            ErrorCode::Busy | ErrorCode::Draining | ErrorCode::Unavailable,
        ) => MaintenanceCliExit::TemporaryFailure,
        OwnerClientError::Connect(
            ConnectError::Unavailable
            | ConnectError::PrivateOfferUnavailable
            | ConnectError::MacosProvisioningUnavailable
            | ConnectError::UnsupportedPlatform,
        ) => MaintenanceCliExit::Unavailable,
        OwnerClientError::AcquireRejected(
            ErrorCode::Rollback
            | ErrorCode::InvalidState
            | ErrorCode::NativeFailure
            | ErrorCode::Indeterminate,
        )
        | OwnerClientError::Disconnected
        | OwnerClientError::Uncertain
        | OwnerClientError::Protocol
        | OwnerClientError::Rejected(_)
        | OwnerClientError::SequenceExhausted
        | OwnerClientError::Transport
        | OwnerClientError::Cancelled => MaintenanceCliExit::Failed,
    }
}

fn parse(
    arguments: &[String],
) -> Option<(MaintenanceAcquireParams, Bytes32, MaintenanceOperation)> {
    let operation = arguments.first()?.as_str();
    let transaction_id = decode_digest(arguments.get(1)?)?;
    let source_build_digest = decode_digest(arguments.get(2)?)?;
    match (operation, arguments.len()) {
        ("uninstall", 3) => Some((
            MaintenanceAcquireParams::Uninstall {
                transaction_id,
                source_build_digest,
            },
            transaction_id,
            MaintenanceOperation::Uninstall,
        )),
        ("update", 5) | ("rollback", 5) => {
            let target_build_digest = decode_digest(arguments.get(3)?)?;
            let target_owner_sha256 = decode_digest(arguments.get(4)?)?;
            let (params, operation) = if operation == "update" {
                (
                    MaintenanceAcquireParams::Update {
                        transaction_id,
                        source_build_digest,
                        target_build_digest,
                        target_owner_sha256,
                    },
                    MaintenanceOperation::Update,
                )
            } else {
                (
                    MaintenanceAcquireParams::Rollback {
                        transaction_id,
                        source_build_digest,
                        target_build_digest,
                        target_owner_sha256,
                    },
                    MaintenanceOperation::Rollback,
                )
            };
            Some((params, transaction_id, operation))
        }
        _ => None,
    }
}

fn decode_digest(value: &str) -> Option<Bytes32> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = decode_nibble(pair[0])? << 4 | decode_nibble(pair[1])?;
    }
    Some(Bytes32::new(bytes))
}

const fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::thread;

    use talking_quill_owner_protocol::schema::ErrorCode;

    use super::*;

    struct ScriptedVerifier {
        delay: Duration,
        result: Result<bool, OwnerExitVerificationError>,
    }

    impl OwnerExitVerifier for ScriptedVerifier {
        fn wait_for_owner_exit(
            &self,
            cancelled: &AtomicBool,
            deadline: Instant,
        ) -> Result<bool, OwnerExitVerificationError> {
            if cancelled.load(Ordering::Acquire) {
                return Err(OwnerExitVerificationError::Cancelled);
            }
            thread::sleep(self.delay);
            if Instant::now() >= deadline {
                return Err(OwnerExitVerificationError::Timeout);
            }
            self.result
        }
    }

    #[test]
    fn owner_exit_verification_is_required_and_stably_classified() {
        let cancelled = AtomicBool::new(false);
        for (verifier, expected) in [
            (
                ScriptedVerifier {
                    delay: Duration::from_millis(5),
                    result: Ok(true),
                },
                MaintenanceCliExit::Success,
            ),
            (
                ScriptedVerifier {
                    delay: Duration::ZERO,
                    result: Ok(false),
                },
                MaintenanceCliExit::Failed,
            ),
            (
                ScriptedVerifier {
                    delay: Duration::ZERO,
                    result: Err(OwnerExitVerificationError::Failed),
                },
                MaintenanceCliExit::Failed,
            ),
            (
                ScriptedVerifier {
                    delay: Duration::ZERO,
                    result: Err(OwnerExitVerificationError::Unavailable),
                },
                MaintenanceCliExit::Unavailable,
            ),
        ] {
            assert_eq!(
                verify_owner_exit(
                    &verifier,
                    &cancelled,
                    Instant::now() + Duration::from_secs(1),
                ),
                expected
            );
        }
        let timeout = ScriptedVerifier {
            delay: Duration::from_millis(10),
            result: Ok(true),
        };
        assert_eq!(
            verify_owner_exit(
                &timeout,
                &cancelled,
                Instant::now() + Duration::from_millis(1),
            ),
            MaintenanceCliExit::TemporaryFailure
        );
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            verify_owner_exit(
                &ScriptedVerifier {
                    delay: Duration::ZERO,
                    result: Ok(true),
                },
                &cancelled,
                Instant::now() + Duration::from_secs(1),
            ),
            MaintenanceCliExit::TemporaryFailure
        );
    }

    #[test]
    fn acquisition_exit_classification_is_exact_and_stable() {
        for (error, expected) in [
            (
                OwnerClientError::Connect(ConnectError::Authentication),
                MaintenanceCliExit::Authentication,
            ),
            (
                OwnerClientError::Connect(ConnectError::Busy),
                MaintenanceCliExit::TemporaryFailure,
            ),
            (
                OwnerClientError::Connect(ConnectError::Incompatible),
                MaintenanceCliExit::Incompatible,
            ),
            (
                OwnerClientError::Connect(ConnectError::PrivateOfferUnavailable),
                MaintenanceCliExit::Unavailable,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::SecurityFault),
                MaintenanceCliExit::Authentication,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Busy),
                MaintenanceCliExit::TemporaryFailure,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Draining),
                MaintenanceCliExit::TemporaryFailure,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Unavailable),
                MaintenanceCliExit::TemporaryFailure,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Incompatible),
                MaintenanceCliExit::Incompatible,
            ),
            (
                OwnerClientError::Connect(ConnectError::Unavailable),
                MaintenanceCliExit::Unavailable,
            ),
            (
                OwnerClientError::Connect(ConnectError::MacosProvisioningUnavailable),
                MaintenanceCliExit::Unavailable,
            ),
            (
                OwnerClientError::Connect(ConnectError::UnsupportedPlatform),
                MaintenanceCliExit::Unavailable,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Rollback),
                MaintenanceCliExit::Failed,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::InvalidState),
                MaintenanceCliExit::Failed,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::NativeFailure),
                MaintenanceCliExit::Failed,
            ),
            (
                OwnerClientError::AcquireRejected(ErrorCode::Indeterminate),
                MaintenanceCliExit::Failed,
            ),
        ] {
            assert_eq!(classify_acquisition_error(&error), expected);
        }
    }
}
