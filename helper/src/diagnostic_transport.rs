//! Durable, replaying helper diagnostic transport.
//!
//! Every owner disconnect updates a bounded on-disk cumulative journal before
//! the reporting call returns. Stderr carries replay records. Electron sends a
//! framed RPC acknowledgement only after its own checkpoint is durable.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
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

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct OwnerDiagnosticKey {
    category: String,
    operation: String,
    correlation_status: String,
    health_refresh: String,
    transport_status: String,
    owner_process_state: String,
}

impl OwnerDiagnosticKey {
    fn from_diagnostic(
        diagnostic: OwnerClientDiagnostic,
        health_refresh: &'static str,
        owner_process_state: OwnerProcessState,
    ) -> Self {
        Self {
            category: diagnostic.category.into(),
            operation: diagnostic.operation.into(),
            correlation_status: diagnostic.correlation_status.into(),
            health_refresh: health_refresh.into(),
            transport_status: diagnostic.transport_status.into(),
            owner_process_state: owner_process_state.as_str().into(),
        }
    }

    fn valid(&self) -> bool {
        one_of(
            &self.category,
            &[
                "connect",
                "disconnected",
                "uncertain",
                "protocol",
                "acquire_rejected",
                "rejected",
                "sequence_exhausted",
                "transport",
            ],
        ) && one_of(
            &self.operation,
            &[
                "capture.replace_configuration",
                "capture.set_enabled",
                "command.sequence",
                "connect.reconcile",
                "established_operation",
                "front_app.get",
                "front_app.metadata.get",
                "front_app.metadata_get",
                "health.get",
                "lease.acquire",
                "lease.release",
                "lease.renew",
                "observability.get",
                "paste.await_commit",
                "paste.inject",
                "permissions.get",
                "runtime.rollback",
                "service",
                "service.poll",
                "session.reconcile_off",
                "session.set_mode",
            ],
        ) && one_of(
            &self.correlation_status,
            &[
                "none",
                "not_established",
                "pending",
                "matched",
                "matched_initial_response",
                "mismatched",
                "unexpected_response",
                "unknown",
            ],
        ) && one_of(
            &self.health_refresh,
            &["not_attempted", "succeeded", "failed"],
        ) && one_of(
            &self.transport_status,
            &["open", "eof", "closed", "error", "backpressured", "unknown"],
        ) && one_of(&self.owner_process_state, &["running", "exited", "unknown"])
    }
}

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

mod u128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JournalCounter {
    #[serde(with = "u128_decimal")]
    total: u128,
    #[serde(with = "u128_decimal")]
    acknowledged: u128,
    overflowed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JournalEntry {
    dimensions: OwnerDiagnosticKey,
    counter: JournalCounter,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct JournalFile {
    version: u8,
    journal_id: String,
    journal_nonce: String,
    process_generation: u64,
    #[serde(with = "u128_decimal")]
    writer_start_failures: u128,
    #[serde(with = "u128_decimal")]
    synchronization_recoveries: u128,
    #[serde(with = "u128_decimal")]
    durability_failures: u128,
    entries: Vec<JournalEntry>,
}

impl JournalFile {
    fn new(entropy: &dyn EntropySource) -> io::Result<Self> {
        let (journal_id, journal_nonce) = new_journal_identities(entropy)?;
        Ok(Self {
            version: JOURNAL_VERSION,
            journal_id,
            journal_nonce,
            process_generation: 0,
            writer_start_failures: 0,
            synchronization_recoveries: 0,
            durability_failures: 0,
            entries: Vec::new(),
        })
    }

    fn valid(&self) -> bool {
        self.version == JOURNAL_VERSION
            && valid_identity(&self.journal_id)
            && valid_identity(&self.journal_nonce)
            && self.journal_id != self.journal_nonce
            && self.entries.len() <= 100_000
            && self.entries.iter().all(|entry| {
                entry.dimensions.valid() && entry.counter.acknowledged <= entry.counter.total
            })
            && self
                .entries
                .windows(2)
                .all(|pair| pair[0].dimensions < pair[1].dimensions)
    }
}

trait EntropySource: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> io::Result<()>;
}

struct OsEntropy;

impl EntropySource for OsEntropy {
    fn fill(&self, bytes: &mut [u8]) -> io::Result<()> {
        getrandom::fill(bytes).map_err(io::Error::other)
    }
}

trait JournalStore: Send + Sync {
    fn load(&self) -> io::Result<Option<JournalFile>>;
    fn store(&self, journal: &JournalFile) -> io::Result<()>;
    fn quarantine_corrupt(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "journal quarantine unavailable",
        ))
    }
}

struct FileJournalStore {
    path: PathBuf,
    lifetime_lock: Mutex<Option<File>>,
}

impl FileJournalStore {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            lifetime_lock: Mutex::new(None),
        }
    }

    fn ensure_exclusive(&self) -> io::Result<()> {
        let mut held = self
            .lifetime_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if held.is_some() {
            return Ok(());
        }
        let lock_path = self.path.with_extension("journal.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock_file_exclusive(&file)?;
        *held = Some(file);
        Ok(())
    }

    fn quarantine_corrupt(&self) -> io::Result<()> {
        self.ensure_exclusive()?;
        let quarantine = self
            .path
            .with_extension(format!("corrupt-{}.json", os_random_identity()?));
        atomic_replace(&self.path, &quarantine)
    }
}

#[cfg(not(debug_assertions))]
struct UnavailableJournalStore;

#[cfg(debug_assertions)]
#[derive(Default)]
struct TestJournalStore(Mutex<Option<JournalFile>>);

#[cfg(debug_assertions)]
impl JournalStore for TestJournalStore {
    fn load(&self) -> io::Result<Option<JournalFile>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn store(&self, journal: &JournalFile) -> io::Result<()> {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(journal.clone());
        Ok(())
    }
}

#[cfg(not(debug_assertions))]
impl JournalStore for UnavailableJournalStore {
    fn load(&self) -> io::Result<Option<JournalFile>> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "diagnostic journal path was not configured",
        ))
    }

    fn store(&self, _journal: &JournalFile) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "diagnostic journal path was not configured",
        ))
    }
}

impl JournalStore for FileJournalStore {
    fn load(&self) -> io::Result<Option<JournalFile>> {
        self.ensure_exclusive()?;
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > JOURNAL_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "diagnostic journal is too large",
            ));
        }
        let mut bytes = Vec::new();
        file.take(JOURNAL_MAX_BYTES + 1).read_to_end(&mut bytes)?;
        let journal: JournalFile = serde_json::from_slice(&bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid diagnostic journal")
        })?;
        if !journal.valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe diagnostic journal",
            ));
        }
        Ok(Some(journal))
    }

    fn store(&self, journal: &JournalFile) -> io::Result<()> {
        self.ensure_exclusive()?;
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "diagnostic journal has no parent",
            )
        })?;
        std::fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec(journal)
            .map_err(|_| io::Error::other("diagnostic journal encoding failed"))?;
        if bytes.len() as u64 > JOURNAL_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "diagnostic journal is full",
            ));
        }
        if !journal.valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to store an invalid diagnostic journal",
            ));
        }
        let temporary = parent.join(format!(".owner-diagnostic-{}.tmp", os_random_identity()?));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            atomic_replace(&temporary, &self.path)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)?
                .sync_all()?;
            sync_parent_directory(parent)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&temporary);
        result
    }

    fn quarantine_corrupt(&self) -> io::Result<()> {
        FileJournalStore::quarantine_corrupt(self)
    }
}

#[cfg(windows)]
fn lock_file_exclusive(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };
    // SAFETY: The handle stays open in FileJournalStore for the process lifetime.
    if unsafe {
        LockFileEx(
            file.as_raw_handle() as _,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &raw mut overlapped,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn lock_file_exclusive(file: &File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock does not retain the pointer and the fd remains open.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(windows, unix)))]
fn lock_file_exclusive(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive diagnostic journal locking is unavailable",
    ))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // SAFETY: Both strings are terminated, immutable UTF-16 buffers for this call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnerReplayRecord<'a> {
    event: &'static str,
    journal_id: &'a str,
    journal_nonce: &'a str,
    stream_id: &'a str,
    process_generation: String,
    category: &'a str,
    operation: &'a str,
    correlation_status: &'a str,
    health_refresh: &'a str,
    transport_status: &'a str,
    owner_process_state: &'a str,
    count: String,
    counter_overflow: bool,
    durable: bool,
    durability_failures: String,
    writer_start_failures: String,
    synchronization_recoveries: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RawOverflowRecord {
    event: &'static str,
    dropped_count: String,
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

impl DiagnosticTransport {
    fn start(writer: impl Write + Send + 'static, store: Arc<dyn JournalStore>) -> Self {
        Self::start_with_options(writer, store, Arc::new(OsEntropy), true)
    }

    #[cfg(test)]
    fn start_with_entropy(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        entropy: Arc<dyn EntropySource>,
    ) -> Self {
        Self::start_with_options(writer, store, entropy, true)
    }

    #[cfg(test)]
    fn start_with_spawn(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        permit_spawn: bool,
    ) -> Self {
        Self::start_with_options(writer, store, Arc::new(OsEntropy), permit_spawn)
    }

    fn start_with_options(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        entropy: Arc<dyn EntropySource>,
        permit_spawn: bool,
    ) -> Self {
        let loaded = store.load();
        let (journal, needs_initial_load) = match loaded {
            Ok(Some(journal)) => (journal, false),
            Ok(None) => (
                JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal()),
                false,
            ),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let quarantined = store.quarantine_corrupt().is_ok();
                let mut journal =
                    JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal());
                journal.durability_failures = 1;
                (journal, !quarantined)
            }
            Err(_) => {
                let mut journal =
                    JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal());
                journal.durability_failures = 1;
                (journal, true)
            }
        };
        let durable_totals = journal
            .entries
            .iter()
            .map(|entry| (entry.dimensions.clone(), entry.counter))
            .collect();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                journal,
                durable_totals,
                journal_dirty: true,
                journal_available: false,
                needs_initial_load,
                journal_revision: 1,
                commit_waiters: VecDeque::new(),
                persistence_worker_started: false,
                generation_prepared: false,
                stream_id: String::new(),
                raw: VecDeque::new(),
                raw_dropped: 0,
            }),
            changed: Condvar::new(),
            store,
            entropy,
        });
        let persistence_shared = Arc::clone(&shared);
        let persistence_spawned = std::thread::Builder::new()
            .name("talking-quill-diagnostic-journal".into())
            .spawn(move || persistence_loop(persistence_shared))
            .is_ok();
        {
            let mut state = lock_state(&shared);
            state.persistence_worker_started = persistence_spawned;
            shared.changed.notify_all();
        }
        let worker_shared = Arc::clone(&shared);
        let spawned = permit_spawn
            && std::thread::Builder::new()
                .name("talking-quill-diagnostic-writer".into())
                .spawn(move || writer_loop(worker_shared, writer))
                .is_ok();
        if !spawned {
            let mut state = lock_state(&shared);
            state.journal.writer_start_failures =
                state.journal.writer_start_failures.saturating_add(1);
            state.journal_dirty = true;
            state.journal_revision = state.journal_revision.saturating_add(1);
            shared.changed.notify_all();
        }
        Self {
            shared,
            writer_started: spawned,
        }
    }

    fn report_owner(
        &self,
        diagnostic: OwnerClientDiagnostic,
        health_refresh: &'static str,
        owner_process_state: OwnerProcessState,
    ) -> Result<(), DiagnosticDurabilityError> {
        let key =
            OwnerDiagnosticKey::from_diagnostic(diagnostic, health_refresh, owner_process_state);
        let (reply_tx, reply_rx) = bounded(1);
        let mut state = lock_state(&self.shared);
        let total = match state
            .journal
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&key))
        {
            Ok(index) => {
                let counter = &mut state.journal.entries[index].counter;
                match counter.total.checked_add(1) {
                    Some(total) => counter.total = total,
                    None => counter.overflowed = true,
                }
                counter.total
            }
            Err(index) => {
                state.journal.entries.insert(
                    index,
                    JournalEntry {
                        dimensions: key.clone(),
                        counter: JournalCounter {
                            total: 1,
                            ..JournalCounter::default()
                        },
                    },
                );
                1
            }
        };
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
        if !state.persistence_worker_started {
            self.shared.changed.notify_all();
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if state.commit_waiters.len() >= COMMIT_WAITER_CAPACITY {
            self.shared.changed.notify_all();
            return Err(DiagnosticDurabilityError::QueueFull);
        }
        state.commit_waiters.push_back(CommitWaiter {
            target: CommitTarget::Report(key, total),
            reply: reply_tx,
        });
        self.shared.changed.notify_all();
        drop(state);
        reply_rx
            .recv_timeout(DURABILITY_WAIT)
            .unwrap_or(Err(DiagnosticDurabilityError::TimedOut))
    }

    fn report_raw(&self, line: String) {
        let mut state = lock_state(&self.shared);
        if state.raw.len() < RAW_QUEUE_CAPACITY {
            state.raw.push_back(line);
        } else {
            state.raw_dropped = state.raw_dropped.saturating_add(1);
        }
        self.shared.changed.notify_all();
    }

    fn acknowledge(&self, ack: &DiagnosticAck) -> Result<(), DiagnosticDurabilityError> {
        if !valid_identity(&ack.journal_id)
            || !valid_identity(&ack.journal_nonce)
            || !ack.dimensions.valid()
        {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        let Ok(count) = ack.count.parse::<u128>() else {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        };
        let mut state = lock_state(&self.shared);
        if !state.generation_prepared || !valid_identity(&state.stream_id) {
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if ack.journal_id != state.journal.journal_id
            || ack.journal_nonce != state.journal.journal_nonce
        {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        let Ok(index) = state
            .journal
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&ack.dimensions))
        else {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        };
        let previous = state.journal.entries[index].counter.acknowledged;
        let total = state.journal.entries[index].counter.total;
        if count < previous || count > total {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        if !state.persistence_worker_started {
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if state.commit_waiters.len() >= COMMIT_WAITER_CAPACITY {
            return Err(DiagnosticDurabilityError::QueueFull);
        }
        let (reply_tx, reply_rx) = bounded(1);
        state.journal.entries[index].counter.acknowledged = count;
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
        state.commit_waiters.push_back(CommitWaiter {
            target: CommitTarget::Acknowledgement(ack.dimensions.clone(), count),
            reply: reply_tx,
        });
        self.shared.changed.notify_all();
        drop(state);
        reply_rx
            .recv_timeout(DURABILITY_WAIT)
            .unwrap_or(Err(DiagnosticDurabilityError::TimedOut))
    }

    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock_state(&self.shared);
        loop {
            if !state.journal_dirty
                && pending_owner(&state).is_none()
                && state.raw.is_empty()
                && state.raw_dropped == 0
            {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || !self.writer_started {
                return false;
            }
            let waited = self
                .shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(now));
            match waited {
                Ok((updated, result)) => {
                    state = updated;
                    if result.timed_out() {
                        return !state.journal_dirty
                            && pending_owner(&state).is_none()
                            && state.raw.is_empty()
                            && state.raw_dropped == 0;
                    }
                }
                Err(poisoned) => {
                    let (mut updated, _) = poisoned.into_inner();
                    updated.journal.synchronization_recoveries =
                        updated.journal.synchronization_recoveries.saturating_add(1);
                    updated.journal_dirty = true;
                    updated.journal_revision = updated.journal_revision.saturating_add(1);
                    self.shared.state.clear_poison();
                    self.shared.changed.notify_all();
                    state = updated;
                }
            }
        }
    }

    #[cfg(test)]
    fn poison_state(&self) {
        let shared = Arc::clone(&self.shared);
        let _ = std::thread::spawn(move || {
            let _guard = shared.state.lock().unwrap();
            panic!("injected diagnostic state poison");
        })
        .join();
    }
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

fn persistence_loop(shared: Arc<Shared>) {
    loop {
        let needs_initial_load = lock_state(&shared).needs_initial_load;
        if needs_initial_load {
            match shared.store.load() {
                Ok(Some(mut loaded)) => {
                    let mut state = lock_state(&shared);
                    merge_pending_into_loaded(&state.journal, &mut loaded);
                    state.journal = loaded;
                    state.needs_initial_load = false;
                    state.journal_available = false;
                    state.generation_prepared = false;
                    state.stream_id.clear();
                    state.journal_dirty = true;
                    state.journal_revision = state.journal_revision.saturating_add(1);
                    shared.changed.notify_all();
                }
                Ok(None) => {
                    let mut state = lock_state(&shared);
                    state.needs_initial_load = false;
                    state.journal_available = false;
                    state.journal_dirty = true;
                    state.journal_revision = state.journal_revision.saturating_add(1);
                    shared.changed.notify_all();
                }
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    if shared.store.quarantine_corrupt().is_ok() {
                        let mut state = lock_state(&shared);
                        state.needs_initial_load = false;
                        state.journal_available = false;
                        state.generation_prepared = false;
                        state.journal.journal_id.clear();
                        state.journal.journal_nonce.clear();
                        state.stream_id.clear();
                        state.journal.durability_failures =
                            state.journal.durability_failures.saturating_add(1);
                        state.journal_dirty = true;
                        state.journal_revision = state.journal_revision.saturating_add(1);
                        shared.changed.notify_all();
                    } else {
                        std::thread::sleep(RETRY_INTERVAL);
                    }
                }
                Err(_) => {
                    std::thread::sleep(RETRY_INTERVAL);
                }
            }
            continue;
        }

        let identities_ready = {
            let mut state = lock_state(&shared);
            let result = prepare_process_identities(&mut state, shared.entropy.as_ref());
            if result.is_err() {
                state.journal_available = false;
                state.journal.durability_failures =
                    state.journal.durability_failures.saturating_add(1);
                state.journal_dirty = true;
            }
            result.is_ok()
        };
        if !identities_ready {
            shared.changed.notify_all();
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        }

        let (snapshot, revision) = {
            let mut state = lock_state(&shared);
            while !state.journal_dirty {
                state = match shared.changed.wait(state) {
                    Ok(updated) => updated,
                    Err(poisoned) => {
                        let mut updated = poisoned.into_inner();
                        updated.journal.synchronization_recoveries =
                            updated.journal.synchronization_recoveries.saturating_add(1);
                        updated.journal_dirty = true;
                        updated.journal_revision = updated.journal_revision.saturating_add(1);
                        shared.state.clear_poison();
                        updated
                    }
                };
            }
            (state.journal.clone(), state.journal_revision)
        };

        match shared.store.store(&snapshot) {
            Ok(()) => {
                let durable = snapshot
                    .entries
                    .iter()
                    .map(|entry| (entry.dimensions.clone(), entry.counter))
                    .collect::<BTreeMap<_, _>>();
                let mut state = lock_state(&shared);
                state.durable_totals = durable;
                state.journal_available = snapshot.valid()
                    && state.generation_prepared
                    && valid_identity(&state.stream_id)
                    && state.stream_id != snapshot.journal_id
                    && state.stream_id != snapshot.journal_nonce;
                if state.journal_revision == revision {
                    state.journal_dirty = false;
                }
                let mut pending = VecDeque::new();
                while let Some(waiter) = state.commit_waiters.pop_front() {
                    if target_is_durable(&waiter.target, &state.durable_totals) {
                        let _ = waiter.reply.try_send(Ok(()));
                    } else {
                        pending.push_back(waiter);
                    }
                }
                state.commit_waiters = pending;
                shared.changed.notify_all();
            }
            Err(_) => {
                let mut state = lock_state(&shared);
                state.journal_available = false;
                state.journal.durability_failures =
                    state.journal.durability_failures.saturating_add(1);
                state.journal_dirty = true;
                state.journal_revision = state.journal_revision.saturating_add(1);
                shared.changed.notify_all();
                drop(state);
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}

fn merge_pending_into_loaded(pending: &JournalFile, loaded: &mut JournalFile) {
    loaded.writer_start_failures = loaded
        .writer_start_failures
        .saturating_add(pending.writer_start_failures);
    loaded.synchronization_recoveries = loaded
        .synchronization_recoveries
        .saturating_add(pending.synchronization_recoveries);
    loaded.durability_failures = loaded
        .durability_failures
        .saturating_add(pending.durability_failures);
    for pending_entry in &pending.entries {
        match loaded
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&pending_entry.dimensions))
        {
            Ok(index) => {
                loaded.entries[index].counter.total = loaded.entries[index]
                    .counter
                    .total
                    .saturating_add(pending_entry.counter.total);
                loaded.entries[index].counter.overflowed |= pending_entry.counter.overflowed;
            }
            Err(index) => loaded.entries.insert(index, pending_entry.clone()),
        }
    }
}

fn target_is_durable(
    target: &CommitTarget,
    durable: &BTreeMap<OwnerDiagnosticKey, JournalCounter>,
) -> bool {
    match target {
        CommitTarget::Report(key, total) => durable
            .get(key)
            .is_some_and(|counter| counter.total >= *total),
        CommitTarget::Acknowledgement(key, count) => durable
            .get(key)
            .is_some_and(|counter| counter.acknowledged >= *count),
    }
}

fn pending_owner(state: &State) -> Option<(OwnerDiagnosticKey, JournalCounter)> {
    if !state.journal_available || !state.generation_prepared || !valid_identity(&state.stream_id) {
        return None;
    }
    state.durable_totals.iter().find_map(|(key, counter)| {
        (counter.total > counter.acknowledged || counter.overflowed)
            .then(|| (key.clone(), *counter))
    })
}

fn writer_loop(shared: Arc<Shared>, mut writer: impl Write) {
    let mut last_sent: Option<(OwnerDiagnosticKey, u128, Instant)> = None;
    loop {
        enum Pending {
            Owner(OwnerDiagnosticKey, JournalCounter),
            Raw(String),
            RawOverflow(u128),
        }
        let pending = {
            let mut state = lock_state(&shared);
            loop {
                if let Some((key, counter)) = pending_owner(&state) {
                    let replay_due = last_sent.as_ref().is_none_or(|(sent_key, sent_count, at)| {
                        sent_key != &key
                            || *sent_count != counter.total
                            || at.elapsed() >= REPLAY_INTERVAL
                    });
                    if replay_due {
                        break Pending::Owner(key, counter);
                    }
                } else if state.raw_dropped != 0 {
                    break Pending::RawOverflow(state.raw_dropped);
                } else if let Some(line) = state.raw.front().cloned() {
                    break Pending::Raw(line);
                }
                let waited = shared.changed.wait_timeout(state, RETRY_INTERVAL);
                state = match waited {
                    Ok((updated, _)) => updated,
                    Err(poisoned) => {
                        let (mut updated, _) = poisoned.into_inner();
                        updated.journal.synchronization_recoveries =
                            updated.journal.synchronization_recoveries.saturating_add(1);
                        updated.journal_dirty = true;
                        updated.journal_revision = updated.journal_revision.saturating_add(1);
                        shared.state.clear_poison();
                        updated
                    }
                };
            }
        };

        let line = {
            let state = lock_state(&shared);
            match &pending {
                Pending::Owner(key, counter) => serde_json::to_string(&OwnerReplayRecord {
                    event: "helper.owner.connection.replay",
                    journal_id: &state.journal.journal_id,
                    journal_nonce: &state.journal.journal_nonce,
                    stream_id: &state.stream_id,
                    process_generation: state.journal.process_generation.to_string(),
                    category: &key.category,
                    operation: &key.operation,
                    correlation_status: &key.correlation_status,
                    health_refresh: &key.health_refresh,
                    transport_status: &key.transport_status,
                    owner_process_state: &key.owner_process_state,
                    count: counter.total.to_string(),
                    counter_overflow: counter.overflowed,
                    durable: state.journal_available && !state.journal_dirty,
                    durability_failures: state.journal.durability_failures.to_string(),
                    writer_start_failures: state.journal.writer_start_failures.to_string(),
                    synchronization_recoveries: state
                        .journal
                        .synchronization_recoveries
                        .to_string(),
                }),
                Pending::Raw(line) => Ok(line.trim_end_matches(['\r', '\n']).to_owned()),
                Pending::RawOverflow(count) => serde_json::to_string(&RawOverflowRecord {
                    event: "helper.diagnostic.raw_overflow",
                    dropped_count: count.to_string(),
                }),
            }
        };
        let Ok(line) = line else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        let framed = format!("\n{line}\n");
        if writer
            .write_all(framed.as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
        {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        }

        let mut state = lock_state(&shared);
        match pending {
            Pending::Owner(key, sent) => last_sent = Some((key, sent.total, Instant::now())),
            Pending::Raw(line) => {
                if state.raw.front() == Some(&line) {
                    state.raw.pop_front();
                }
            }
            Pending::RawOverflow(sent) => {
                state.raw_dropped = state.raw_dropped.saturating_sub(sent);
            }
        }
        shared.changed.notify_all();
    }
}

fn prepare_process_identities(state: &mut State, entropy: &dyn EntropySource) -> io::Result<()> {
    if !valid_identity(&state.journal.journal_id)
        || !valid_identity(&state.journal.journal_nonce)
        || state.journal.journal_id == state.journal.journal_nonce
    {
        let (journal_id, journal_nonce) = new_journal_identities(entropy)?;
        state.journal.journal_id = journal_id;
        state.journal.journal_nonce = journal_nonce;
        state.generation_prepared = false;
    }
    if !valid_identity(&state.stream_id)
        || state.stream_id == state.journal.journal_id
        || state.stream_id == state.journal.journal_nonce
    {
        let stream_id = fresh_identity(entropy)?;
        if stream_id == state.journal.journal_id || stream_id == state.journal.journal_nonce {
            return Err(io::Error::other("diagnostic identity collision"));
        }
        state.stream_id = stream_id;
        state.generation_prepared = false;
    }
    if !state.generation_prepared {
        state.journal.process_generation = state.journal.process_generation.saturating_add(1);
        state.generation_prepared = true;
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
    }
    Ok(())
}

fn new_journal_identities(entropy: &dyn EntropySource) -> io::Result<(String, String)> {
    let journal_id = fresh_identity(entropy)?;
    let journal_nonce = fresh_identity(entropy)?;
    if journal_id == journal_nonce {
        return Err(io::Error::other("diagnostic identity collision"));
    }
    Ok((journal_id, journal_nonce))
}

fn fresh_identity(entropy: &dyn EntropySource) -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    entropy.fill(&mut bytes)?;
    let identity = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !valid_identity(&identity) {
        return Err(io::Error::other("invalid diagnostic entropy output"));
    }
    Ok(identity)
}

fn os_random_identity() -> io::Result<String> {
    fresh_identity(&OsEntropy)
}

#[cfg(test)]
fn random_identity() -> io::Result<String> {
    os_random_identity()
}

fn uninitialized_journal() -> JournalFile {
    JournalFile {
        version: JOURNAL_VERSION,
        journal_id: String::new(),
        journal_nonce: String::new(),
        process_generation: 0,
        writer_start_failures: 0,
        synchronization_recoveries: 0,
        durability_failures: 1,
        entries: Vec::new(),
    }
}

fn valid_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && !value.bytes().all(|byte| byte == b'0')
        && !value.bytes().all(|byte| byte == b'f')
}

#[cfg(debug_assertions)]
fn default_unconfigured_store() -> Arc<dyn JournalStore> {
    Arc::new(TestJournalStore::default())
}

#[cfg(not(debug_assertions))]
fn default_unconfigured_store() -> Arc<dyn JournalStore> {
    Arc::new(UnavailableJournalStore)
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
mod tests {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    #[derive(Clone, Default)]
    struct SharedWriter {
        blocked: Arc<AtomicBool>,
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            while self.blocked.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            lock_vec(&self.bytes).extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct InjectedEntropy {
        failures_remaining: AtomicUsize,
        next_value: AtomicUsize,
    }

    impl InjectedEntropy {
        fn new(failures: usize) -> Self {
            Self {
                failures_remaining: AtomicUsize::new(failures),
                next_value: AtomicUsize::new(1),
            }
        }

        fn recover(&self) {
            self.failures_remaining.store(0, Ordering::Release);
        }
    }

    impl EntropySource for InjectedEntropy {
        fn fill(&self, bytes: &mut [u8]) -> io::Result<()> {
            if self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_sub(1)
                })
                .is_ok()
            {
                return Err(io::Error::other("injected getrandom failure"));
            }
            let value = self.next_value.fetch_add(1, Ordering::AcqRel);
            let byte = u8::try_from((value % 254) + 1).unwrap();
            bytes.fill(byte);
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryStore {
        journal: Mutex<Option<JournalFile>>,
        failures_remaining: AtomicUsize,
        load_failures_remaining: AtomicUsize,
        corrupt: AtomicBool,
        quarantines: AtomicUsize,
    }

    impl JournalStore for MemoryStore {
        fn load(&self) -> io::Result<Option<JournalFile>> {
            if self
                .load_failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_sub(1)
                })
                .is_ok()
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected denial",
                ));
            }
            if self.corrupt.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "injected corruption",
                ));
            }
            Ok(lock_option(&self.journal).clone())
        }

        fn store(&self, journal: &JournalFile) -> io::Result<()> {
            if self
                .failures_remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_sub(1)
                })
                .is_ok()
            {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected disk full",
                ));
            }
            *lock_option(&self.journal) = Some(journal.clone());
            Ok(())
        }

        fn quarantine_corrupt(&self) -> io::Result<()> {
            self.corrupt.store(false, Ordering::Release);
            self.quarantines.fetch_add(1, Ordering::AcqRel);
            *lock_option(&self.journal) = None;
            Ok(())
        }
    }

    fn lock_vec(mutex: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_option(mutex: &Mutex<Option<JournalFile>>) -> MutexGuard<'_, Option<JournalFile>> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn diagnostic(operation: &'static str) -> OwnerClientDiagnostic {
        OwnerClientDiagnostic {
            category: "disconnected",
            operation,
            correlation_status: "pending",
            transport_status: "eof",
        }
    }

    fn records(writer: &SharedWriter) -> Vec<serde_json::Value> {
        String::from_utf8(lock_vec(&writer.bytes).clone())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|record: &serde_json::Value| {
                record["event"] == "helper.owner.connection.replay"
            })
            .collect()
    }

    fn ack_for(record: &serde_json::Value) -> DiagnosticAck {
        DiagnosticAck {
            journal_id: record["journalId"].as_str().unwrap().into(),
            journal_nonce: record["journalNonce"].as_str().unwrap().into(),
            dimensions: OwnerDiagnosticKey {
                category: record["category"].as_str().unwrap().into(),
                operation: record["operation"].as_str().unwrap().into(),
                correlation_status: record["correlationStatus"].as_str().unwrap().into(),
                health_refresh: record["healthRefresh"].as_str().unwrap().into(),
                transport_status: record["transportStatus"].as_str().unwrap().into(),
                owner_process_state: record["ownerProcessState"].as_str().unwrap().into(),
            },
            count: record["count"].as_str().unwrap().into(),
        }
    }

    fn wait_for_records(writer: &SharedWriter, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while records(writer).len() < count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(records(writer).len() >= count);
    }

    #[test]
    fn entropy_failure_for_new_journal_retries_without_replay_or_acknowledgement() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        let entropy = Arc::new(InjectedEntropy::new(10_000));
        let transport =
            DiagnosticTransport::start_with_entropy(writer.clone(), store.clone(), entropy.clone());
        assert_eq!(
            transport.report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            ),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(records(&writer).is_empty());
        assert!(lock_option(&store.journal).is_none());
        assert_eq!(
            transport.acknowledge(&DiagnosticAck {
                journal_id: "0".repeat(64),
                journal_nonce: "f".repeat(64),
                dimensions: OwnerDiagnosticKey::from_diagnostic(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                ),
                count: "1".into(),
            }),
            Err(DiagnosticDurabilityError::InvalidAcknowledgement)
        );

        entropy.recover();
        assert!(
            transport
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
        wait_for_records(&writer, 1);
        let record = records(&writer).pop().unwrap();
        assert_eq!(record["durable"], true);
        let identities = [
            record["journalId"].as_str().unwrap(),
            record["journalNonce"].as_str().unwrap(),
            record["streamId"].as_str().unwrap(),
        ];
        assert!(identities.iter().all(|identity| valid_identity(identity)));
        assert_ne!(identities[0], identities[1]);
        assert_ne!(identities[0], identities[2]);
        assert_ne!(identities[1], identities[2]);
        assert!(transport.acknowledge(&ack_for(&record)).is_ok());
    }

    #[test]
    fn entropy_failure_for_new_stream_blocks_existing_journal_replay_and_ack() {
        let store = Arc::new(MemoryStore::default());
        let mut seeded = JournalFile::new(&OsEntropy).unwrap();
        let dimensions = OwnerDiagnosticKey::from_diagnostic(
            diagnostic("health.get"),
            "failed",
            OwnerProcessState::Exited,
        );
        seeded.entries.push(JournalEntry {
            dimensions: dimensions.clone(),
            counter: JournalCounter {
                total: 1,
                ..JournalCounter::default()
            },
        });
        *lock_option(&store.journal) = Some(seeded.clone());
        let writer = SharedWriter::default();
        let entropy = Arc::new(InjectedEntropy::new(10_000));
        let transport =
            DiagnosticTransport::start_with_entropy(writer.clone(), store, entropy.clone());
        std::thread::sleep(DURABILITY_WAIT);
        assert!(records(&writer).is_empty());
        assert_eq!(
            transport.acknowledge(&DiagnosticAck {
                journal_id: seeded.journal_id.clone(),
                journal_nonce: seeded.journal_nonce.clone(),
                dimensions,
                count: "1".into(),
            }),
            Err(DiagnosticDurabilityError::WorkerUnavailable)
        );

        entropy.recover();
        wait_for_records(&writer, 1);
        let record = records(&writer).pop().unwrap();
        assert_eq!(record["journalId"], seeded.journal_id);
        assert_eq!(record["journalNonce"], seeded.journal_nonce);
        assert_ne!(record["streamId"], record["journalId"]);
        assert_ne!(record["streamId"], record["journalNonce"]);
        assert!(transport.acknowledge(&ack_for(&record)).is_ok());
    }

    #[test]
    fn corrupt_recovery_waits_for_fresh_random_identity_set() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        store.corrupt.store(true, Ordering::Release);
        let entropy = Arc::new(InjectedEntropy::new(10_000));
        let transport =
            DiagnosticTransport::start_with_entropy(writer.clone(), store.clone(), entropy.clone());
        assert_eq!(
            transport.report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            ),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(records(&writer).is_empty());
        assert!(lock_option(&store.journal).is_none());
        assert_eq!(store.quarantines.load(Ordering::Acquire), 1);

        entropy.recover();
        assert!(
            transport
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
        wait_for_records(&writer, 1);
        let record = records(&writer).pop().unwrap();
        assert_eq!(record["durable"], true);
        assert_ne!(record["journalId"], record["journalNonce"]);
        assert_ne!(record["journalId"], record["streamId"]);
        assert_ne!(record["journalNonce"], record["streamId"]);
    }

    #[test]
    fn valid_identity_rejects_removed_deterministic_sentinels() {
        assert!(!valid_identity(&"0".repeat(64)));
        assert!(!valid_identity(&"f".repeat(64)));
        assert!(valid_identity(&format!("{}1", "0".repeat(63))));
    }

    #[test]
    fn blocked_writer_replays_ten_thousand_after_durable_journal_commit_and_ack() {
        let writer = SharedWriter::default();
        writer.blocked.store(true, Ordering::Release);
        let store = Arc::new(MemoryStore::default());
        let transport = DiagnosticTransport::start(writer.clone(), store.clone());
        for _ in 0..10_000 {
            transport
                .report_owner(
                    diagnostic("lease.renew"),
                    "not_attempted",
                    OwnerProcessState::Running,
                )
                .unwrap();
        }
        let journal = lock_option(&store.journal).clone().unwrap();
        assert_eq!(journal.entries[0].counter.total, 10_000);
        writer.blocked.store(false, Ordering::Release);
        wait_for_records(&writer, 1);
        let latest = records(&writer).pop().unwrap();
        assert_eq!(latest["count"], "10000");
        assert!(transport.acknowledge(&ack_for(&latest)).is_ok());
        assert!(transport.flush(Duration::from_secs(2)));
    }

    #[test]
    fn ack_loss_replays_and_restart_keeps_the_same_journal_high_water() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        let first = DiagnosticTransport::start(writer.clone(), store.clone());
        for _ in 0..40 {
            first
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .unwrap();
        }
        wait_for_records(&writer, 2);
        let replay = records(&writer).pop().unwrap();
        assert_eq!(replay["count"], "40");
        drop(first);
        let second_writer = SharedWriter::default();
        let second = DiagnosticTransport::start(second_writer.clone(), store);
        wait_for_records(&second_writer, 1);
        let restarted = records(&second_writer).pop().unwrap();
        assert_eq!(restarted["journalId"], replay["journalId"]);
        assert_ne!(restarted["streamId"], replay["streamId"]);
        assert!(second.acknowledge(&ack_for(&restarted)).is_ok());
        assert!(second.flush(Duration::from_secs(2)));
    }

    #[test]
    fn report_failure_returns_error_emits_no_false_durable_record_and_recovers_later() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        store.failures_remaining.store(10_000, Ordering::Release);
        let transport = DiagnosticTransport::start(writer.clone(), store.clone());
        let started = Instant::now();
        assert_eq!(
            transport.report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            ),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(records(&writer).is_empty());
        store.failures_remaining.store(0, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let durable = records(&writer)
                .into_iter()
                .find(|record| record["durable"] == true);
            if let Some(record) = durable {
                assert!(transport.acknowledge(&ack_for(&record)).is_ok());
                break;
            }
            assert!(Instant::now() < deadline, "durable replay did not recover");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn disk_full_poison_and_writer_spawn_failure_are_durable_and_recoverable() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        store.failures_remaining.store(10_000, Ordering::Release);
        let failed_writer =
            DiagnosticTransport::start_with_spawn(writer.clone(), store.clone(), false);
        failed_writer.poison_state();
        let started = Instant::now();
        assert_eq!(
            failed_writer.report_owner(
                diagnostic("health.get"),
                "failed",
                OwnerProcessState::Exited,
            ),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(records(&writer).is_empty());
        store.failures_remaining.store(0, Ordering::Release);
        let recovery_deadline = Instant::now() + Duration::from_secs(2);
        while lock_option(&store.journal).is_none() && Instant::now() < recovery_deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(lock_option(&store.journal).is_some());
        let successor_writer = SharedWriter::default();
        let successor = DiagnosticTransport::start(successor_writer.clone(), store.clone());
        wait_for_records(&successor_writer, 1);
        let record = records(&successor_writer).pop().unwrap();
        assert_eq!(record["count"], "1");
        assert!(
            record["writerStartFailures"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                >= 1
        );
        assert!(
            record["synchronizationRecoveries"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                >= 1
        );
        assert!(
            record["durabilityFailures"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                >= 1
        );
        assert!(successor.acknowledge(&ack_for(&record)).is_ok());
    }

    #[test]
    fn acknowledgement_persistence_has_a_bounded_wait_and_replays_after_failure() {
        let writer = SharedWriter::default();
        let store = Arc::new(MemoryStore::default());
        let transport = DiagnosticTransport::start(writer.clone(), store.clone());
        transport
            .report_owner(
                diagnostic("lease.renew"),
                "failed",
                OwnerProcessState::Exited,
            )
            .unwrap();
        wait_for_records(&writer, 1);
        let record = records(&writer).pop().unwrap();
        store.failures_remaining.store(10_000, Ordering::Release);
        let started = Instant::now();
        assert_eq!(
            transport.acknowledge(&ack_for(&record)),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        store.failures_remaining.store(0, Ordering::Release);
        assert!(transport.acknowledge(&ack_for(&record)).is_ok());
    }

    #[test]
    fn corrupt_and_initially_denied_journals_recover_in_process() {
        let corrupt_store = Arc::new(MemoryStore::default());
        corrupt_store.corrupt.store(true, Ordering::Release);
        let corrupt = DiagnosticTransport::start(SharedWriter::default(), corrupt_store.clone());
        assert!(
            corrupt
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
        assert_eq!(corrupt_store.quarantines.load(Ordering::Acquire), 1);

        let denied_store = Arc::new(MemoryStore::default());
        denied_store
            .load_failures_remaining
            .store(2, Ordering::Release);
        let denied = DiagnosticTransport::start(SharedWriter::default(), denied_store);
        assert!(
            denied
                .report_owner(
                    diagnostic("health.get"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
    }

    #[test]
    fn concurrent_transport_processes_fail_closed_then_take_over_after_owner_death() {
        const CHILD_ENV: &str = "TALKING_QUILL_DIAGNOSTIC_OVERLAP_CHILD";
        const PATH_ENV: &str = "TALKING_QUILL_DIAGNOSTIC_OVERLAP_PATH";
        if std::env::var_os(CHILD_ENV).is_some() {
            let path = PathBuf::from(std::env::var_os(PATH_ENV).unwrap());
            let transport = DiagnosticTransport::start(
                io::sink(),
                Arc::new(FileJournalStore::new(path.clone())),
            );
            transport
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .unwrap();
            std::fs::write(path.with_extension("ready"), b"ready").unwrap();
            std::thread::sleep(Duration::from_millis(750));
            return;
        }

        let directory = PathBuf::from("tmp/tests");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!(
            "owner-process-overlap-{}.json",
            random_identity().unwrap()
        ));
        let ready = path.with_extension("ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("diagnostic_transport::tests::concurrent_transport_processes_fail_closed_then_take_over_after_owner_death")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .env(PATH_ENV, &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ready_deadline = Instant::now() + Duration::from_secs(2);
        while !ready.is_file() && Instant::now() < ready_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.is_file());

        let successor =
            DiagnosticTransport::start(io::sink(), Arc::new(FileJournalStore::new(path.clone())));
        assert_eq!(
            successor.report_owner(
                diagnostic("health.get"),
                "failed",
                OwnerProcessState::Exited,
            ),
            Err(DiagnosticDurabilityError::TimedOut)
        );
        assert!(child.wait().unwrap().success());
        assert!(
            successor
                .report_owner(
                    diagnostic("health.get"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
        let _ = std::fs::remove_file(ready);
    }

    #[test]
    fn truncated_file_is_quarantined_and_replaced_without_exposing_content_in_its_name() {
        let directory = PathBuf::from("tmp/tests");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!(
            "owner-truncated-{}.json",
            random_identity().unwrap()
        ));
        std::fs::write(&path, br#"{\"private":"truncated""#).unwrap();
        let transport =
            DiagnosticTransport::start(io::sink(), Arc::new(FileJournalStore::new(path.clone())));
        assert!(
            transport
                .report_owner(
                    diagnostic("service.poll"),
                    "failed",
                    OwnerProcessState::Exited,
                )
                .is_ok()
        );
        let prefix = path.file_stem().unwrap().to_string_lossy().into_owned();
        let quarantines = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&format!("{prefix}.corrupt-")))
            .collect::<Vec<_>>();
        assert_eq!(quarantines.len(), 1);
        assert!(!quarantines[0].contains("private"));
    }

    #[test]
    fn exclusive_store_rejects_overlap_and_allows_takeover_after_release() {
        let directory = PathBuf::from("tmp/tests");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("owner-overlap-{}.json", random_identity().unwrap()));
        let first = FileJournalStore::new(path.clone());
        let second = FileJournalStore::new(path.clone());
        assert!(first.load().unwrap().is_none());
        assert!(second.load().is_err());
        drop(first);
        assert!(second.load().unwrap().is_none());
        drop(second);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("journal.lock"));
    }

    #[test]
    fn file_journal_atomically_round_trips_in_project_tmp() {
        let directory = PathBuf::from("tmp/tests");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!(
            "owner-diagnostic-{}.json",
            random_identity().unwrap()
        ));
        let store = FileJournalStore::new(path.clone());
        let mut journal = JournalFile::new(&OsEntropy).unwrap();
        journal.process_generation = 1;
        store.store(&journal).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.journal_id, journal.journal_id);
        drop(store);
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_file(path.with_extension("journal.lock"));
    }

    #[test]
    fn identity_is_256_bit_lower_hex_and_generation_survives_random_stream_collision() {
        let store = Arc::new(MemoryStore::default());
        let first = DiagnosticTransport::start(SharedWriter::default(), store.clone());
        first
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .unwrap();
        let first_state = lock_state(&first.shared);
        let journal_id = first_state.journal.journal_id.clone();
        let first_generation = first_state.journal.process_generation;
        assert!(valid_identity(&journal_id));
        assert!(valid_identity(&first_state.stream_id));
        drop(first_state);
        drop(first);
        let second = DiagnosticTransport::start(SharedWriter::default(), store);
        second
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .unwrap();
        let second_state = lock_state(&second.shared);
        assert_eq!(second_state.journal.journal_id, journal_id);
        assert_eq!(
            second_state.journal.process_generation,
            first_generation + 1
        );
    }
}
