#[doc(hidden)]
pub mod diagnostic_transport;
pub mod framing;
pub mod gateway;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod macho;
#[cfg(any(target_os = "macos", test))]
pub(crate) mod macos_outer_identity;
#[cfg(target_os = "macos")]
pub mod macos_service_bridge;

#[cfg(target_os = "macos")]
#[used]
static MACOS_BUILD_VARIANT: &str = env!("TALKING_QUILL_MACOS_BUILD_VARIANT");
#[used]
static SOURCE_COMMIT_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_COMMIT=",
    env!("TALKING_QUILL_SOURCE_COMMIT")
);
#[used]
static SOURCE_TREE_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_TREE=",
    env!("TALKING_QUILL_SOURCE_TREE")
);

pub fn retain_source_identity() {
    std::hint::black_box(SOURCE_COMMIT_MARKER);
    std::hint::black_box(SOURCE_TREE_MARKER);
}

#[cfg(all(windows, feature = "machine-lock-test-namespace"))]
pub mod machine_lock_test_namespace;
pub mod owned_tree;
pub mod owner;
pub mod protocol;
#[cfg(any(
    all(windows, feature = "windows-installed-acceptance"),
    all(test, feature = "windows-installed-acceptance")
))]
pub mod windows_acceptance_launcher;
#[cfg(windows)]
pub mod windows_harness;
#[cfg(windows)]
pub mod windows_update;

mod runtime;

pub use runtime::run;
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use runtime::run_framed_stream_with_factory_and_gate;
#[doc(hidden)]
pub use runtime::{
    report_owner_connection_diagnostic, report_run_error, run_framed_stream,
    run_framed_stream_started,
};

use crate::{
    framing::FrameError,
    gateway::{PlatformError, TerminalReason},
    protocol::Outbound,
};
use crossbeam_channel::{Receiver, Sender};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RunError {
    #[error("protocol framing failed: {0}")]
    Framing(#[from] FrameError),
    #[error("gateway backend startup failed: {0}")]
    Gateway(#[from] PlatformError),
    #[error("stdin reader thread failed")]
    ReaderThread,
    #[error("stdout writer thread failed")]
    WriterThread,
    #[error("stdout protocol writer failed: {0}")]
    Writer(FrameError),
    #[error("stdout protocol writer did not finish within the shutdown deadline")]
    WriterFlushTimeout,
    #[error("terminal helper failure: {0:?}")]
    Terminal(TerminalReason),
}

/// A slot reserved before an irreversible paste operation. The writer owns the
/// slot before native dispatch begins, then receives commit and response as one
/// ordered batch.
#[doc(hidden)]
pub struct CriticalDelivery {
    acquired: Sender<()>,
    completion: Receiver<Vec<Outbound>>,
}

impl CriticalDelivery {
    pub(crate) const fn new(acquired: Sender<()>, completion: Receiver<Vec<Outbound>>) -> Self {
        Self {
            acquired,
            completion,
        }
    }

    /// Acquires this writer reservation and returns its complete ordered batch.
    #[doc(hidden)]
    pub fn accept(self) -> Option<Vec<Outbound>> {
        self.acquired.send(()).ok()?;
        self.completion.recv().ok()
    }
}
