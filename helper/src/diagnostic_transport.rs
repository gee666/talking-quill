//! Durable, replaying helper diagnostic transport.
//!
//! Every owner disconnect updates a bounded on-disk cumulative journal before
//! the reporting call returns. Stderr carries replay records. Electron sends a
//! framed RPC acknowledgement only after its own checkpoint is durable.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use serde::{Deserialize, Serialize};

use crate::owner::client::{OwnerClientDiagnostic, OwnerProcessState};

const JOURNAL_ENV: &str = "TALKING_QUILL_OWNER_DIAGNOSTIC_JOURNAL_V1";
const JOURNAL_VERSION: u8 = 1;
const JOURNAL_MAX_BYTES: u64 = 32 * 1024 * 1024;
const RAW_QUEUE_CAPACITY: usize = 8;
const REPLAY_INTERVAL: Duration = Duration::from_millis(250);
const RETRY_INTERVAL: Duration = Duration::from_millis(20);
const DURABILITY_WAIT: Duration = Duration::from_millis(250);
const COMMIT_WAITER_CAPACITY: usize = 64;

mod filesystem;
mod identity;
mod journal;
mod persistence;
mod replay;
mod store;
mod transport;

#[cfg(test)]
use identity::random_identity;
use identity::{
    EntropySource, OsEntropy, new_journal_identities, os_random_identity,
    prepare_process_identities, uninitialized_journal, valid_identity,
};
pub(crate) use journal::OwnerDiagnosticKey;
use journal::{JournalCounter, JournalEntry, JournalFile};
use persistence::persistence_loop;
use replay::{pending_owner, writer_loop};
use store::{FileJournalStore, JournalStore, default_unconfigured_store};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticDurabilityError {
    QueueFull,
    TimedOut,
    WorkerUnavailable,
    InvalidAcknowledgement,
}

#[derive(Clone)]
enum CommitTarget {
    Report(OwnerDiagnosticKey, u128),
    Acknowledgement(OwnerDiagnosticKey, u128),
}

struct CommitWaiter {
    target: CommitTarget,
    reply: Sender<Result<(), DiagnosticDurabilityError>>,
}

struct State {
    journal: JournalFile,
    durable_totals: BTreeMap<OwnerDiagnosticKey, JournalCounter>,
    journal_dirty: bool,
    journal_available: bool,
    needs_initial_load: bool,
    journal_revision: u64,
    commit_waiters: VecDeque<CommitWaiter>,
    persistence_worker_started: bool,
    generation_prepared: bool,
    stream_id: String,
    raw: VecDeque<String>,
    raw_dropped: u128,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    store: Arc<dyn JournalStore>,
    entropy: Arc<dyn EntropySource>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct DiagnosticAck {
    pub(crate) journal_id: String,
    pub(crate) journal_nonce: String,
    pub(crate) dimensions: OwnerDiagnosticKey,
    pub(crate) count: String,
}

pub(crate) struct DiagnosticTransport {
    shared: Arc<Shared>,
    writer_started: bool,
}

fn lock_state(shared: &Shared) -> MutexGuard<'_, State> {
    match shared.state.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.journal.synchronization_recoveries =
                state.journal.synchronization_recoveries.saturating_add(1);
            state.journal_dirty = true;
            state.journal_revision = state.journal_revision.saturating_add(1);
            shared.state.clear_poison();
            state
        }
    }
}

fn global() -> &'static DiagnosticTransport {
    static TRANSPORT: OnceLock<DiagnosticTransport> = OnceLock::new();
    TRANSPORT.get_or_init(|| {
        let store: Arc<dyn JournalStore> = match std::env::var_os(JOURNAL_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            Some(path) => Arc::new(FileJournalStore::new(path)),
            None => default_unconfigured_store(),
        };
        DiagnosticTransport::start(io::stderr(), store)
    })
}

pub(crate) fn report_owner(
    diagnostic: OwnerClientDiagnostic,
    health_refresh: &'static str,
    owner_process_state: OwnerProcessState,
) -> Result<(), DiagnosticDurabilityError> {
    global().report_owner(diagnostic, health_refresh, owner_process_state)
}

pub(crate) fn report_raw(line: String) {
    global().report_raw(line);
}

pub(crate) fn acknowledge(ack: &DiagnosticAck) -> Result<(), DiagnosticDurabilityError> {
    global().acknowledge(ack)
}

pub(crate) fn flush(timeout: Duration) -> bool {
    global().flush(timeout)
}

#[cfg(test)]
mod tests;
