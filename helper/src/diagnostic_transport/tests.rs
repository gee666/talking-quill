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
        .filter(|record: &serde_json::Value| record["event"] == "helper.owner.connection.replay")
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

mod durability;
mod identities;
mod storage;

#[test]
fn concurrent_transport_processes_fail_closed_then_take_over_after_owner_death() {
    const CHILD_ENV: &str = "TALKING_QUILL_DIAGNOSTIC_OVERLAP_CHILD";
    const PATH_ENV: &str = "TALKING_QUILL_DIAGNOSTIC_OVERLAP_PATH";
    if std::env::var_os(CHILD_ENV).is_some() {
        let path = PathBuf::from(std::env::var_os(PATH_ENV).unwrap());
        let transport =
            DiagnosticTransport::start(io::sink(), Arc::new(FileJournalStore::new(path.clone())));
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
