//! Terminal observability and bounded diagnostic reporting.

use super::TERMINAL_DIAGNOSTIC_WRITE_TIMEOUT;
use crate::diagnostic_transport;
use serde::Serialize;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminalObservabilityOutcome {
    Shutdown,
    Failure,
}

pub(super) fn publish_terminal_observability(
    snapshot: Option<serde_json::Value>,
    outcome: TerminalObservabilityOutcome,
) {
    let Some(mut observability) = snapshot else {
        return;
    };
    if matches!(outcome, TerminalObservabilityOutcome::Failure) {
        observability["keyboardCapture"]["terminalDisablements"] = serde_json::json!(1);
    }
    let record = serde_json::json!({
        "event": "helper.runtime.terminal",
        "outcome": outcome,
        "observability": observability,
    });
    let Ok(mut line) = serde_json::to_string(&record) else {
        return;
    };
    line.push('\n');
    write_stderr_bounded(line, "talking-quill-helper-terminal-diagnostic");
}

fn write_stderr_bounded(line: String, _thread_name: &'static str) {
    diagnostic_transport::report_raw(line);
    // A blocked parent cannot delay shutdown. The dedicated writer retains a
    // bounded raw queue and all owner aggregates, then resumes on the same pipe
    // if the parent starts reading again.
    let _ = diagnostic_transport::flush(TERMINAL_DIAGNOSTIC_WRITE_TIMEOUT);
}

#[doc(hidden)]
pub fn report_owner_connection_diagnostic(
    diagnostic: crate::owner::client::OwnerClientDiagnostic,
    health_refresh: &'static str,
    owner_process_state: crate::owner::client::OwnerProcessState,
) -> Result<(), diagnostic_transport::DiagnosticDurabilityError> {
    diagnostic_transport::report_owner(diagnostic, health_refresh, owner_process_state)
}

#[doc(hidden)]
pub fn report_run_error(error: &str) {
    write_stderr_bounded(
        format!("talking-quill-helper: {error}\n"),
        "talking-quill-helper-error-diagnostic",
    );
}
