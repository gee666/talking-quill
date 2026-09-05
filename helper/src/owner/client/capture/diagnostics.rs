//! Capture failure classification and post-grant reporting.

use super::*;

impl OwnerCaptureClient {
    pub(super) fn report_post_grant_failure(
        &self,
        connector: &dyn OwnerConnector,
        error: &OwnerClientError,
    ) -> Result<(), OwnerClientError> {
        let diagnostic = self.last_failure.unwrap_or(OwnerClientDiagnostic {
            category: owner_client_error_category(error),
            operation: "connect.reconcile",
            correlation_status: "unknown",
            transport_status: "unknown",
        });
        let _ = crate::report_owner_connection_diagnostic(
            diagnostic,
            "not_attempted",
            connector.owner_process_state(),
        );
        Ok(())
    }

    pub(super) fn protocol_failure<T>(
        &mut self,
        operation: &'static str,
        correlation_status: &'static str,
    ) -> Result<T, OwnerClientError> {
        self.last_failure = Some(OwnerClientDiagnostic {
            category: "protocol",
            operation,
            correlation_status,
            transport_status: "open",
        });
        self.client.abort();
        Err(OwnerClientError::Protocol)
    }

    pub(super) fn sequence_failure<T>(
        &mut self,
        operation: &'static str,
    ) -> Result<T, OwnerClientError> {
        self.last_failure = Some(OwnerClientDiagnostic {
            category: "sequence_exhausted",
            operation,
            correlation_status: "not_established",
            transport_status: "open",
        });
        self.client.abort();
        Err(OwnerClientError::SequenceExhausted)
    }
}

fn owner_client_error_category(error: &OwnerClientError) -> &'static str {
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

pub(super) fn client_failure_diagnostic(
    operation: &'static str,
    correlation_status: &'static str,
    error: &ClientError,
) -> OwnerClientDiagnostic {
    if let ClientError::Transport(talking_quill_owner_protocol::TransportError::Io(io)) = error {
        eprintln!(
            "keyboard-owner {operation} I/O failure: kind={:?}, os={:?}",
            io.kind(),
            io.raw_os_error()
        );
    }
    let (category, transport_status) = match error {
        ClientError::Transport(talking_quill_owner_protocol::TransportError::PeerClosed) => {
            ("disconnected", "eof")
        }
        ClientError::Transport(_) => ("transport", "error"),
        ClientError::Codec(_) | ClientError::TransportAuthenticationBoundary => {
            ("protocol", "open")
        }
        ClientError::Closed => ("disconnected", "closed"),
        ClientError::Backpressured => ("transport", "backpressured"),
    };
    OwnerClientDiagnostic {
        category,
        operation,
        correlation_status,
        transport_status,
    }
}
