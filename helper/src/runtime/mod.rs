//! Runtime coordination and exclusive, prioritized protocol output.

use std::time::Duration;

mod coordinator;
mod diagnostics;
mod stdio;
mod stream;
mod writer;

pub use diagnostics::{report_owner_connection_diagnostic, report_run_error};
pub use stdio::run;
#[cfg(debug_assertions)]
pub use stream::run_framed_stream_with_factory_and_gate;
pub use stream::{run_framed_stream, run_framed_stream_started};

const OUTBOUND_QUEUE_CAPACITY: usize = 256;
const CRITICAL_OUTBOUND_QUEUE_CAPACITY: usize = 1;
const FINAL_OUTBOUND_QUEUE_CAPACITY: usize = 1;
const INPUT_QUEUE_CAPACITY: usize = 8;
const CLEAN_WRITER_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const TERMINAL_DIAGNOSTIC_WRITE_TIMEOUT: Duration = Duration::from_millis(250);

#[cfg(test)]
mod tests;
