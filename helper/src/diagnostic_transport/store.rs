//! Journal storage and lifetime lock ownership.

use super::filesystem::{atomic_replace, lock_file_exclusive, sync_parent_directory};
use super::*;
use std::fs::{File, OpenOptions};
use std::io::Read;

pub(super) trait JournalStore: Send + Sync {
    fn load(&self) -> io::Result<Option<JournalFile>>;
    fn store(&self, journal: &JournalFile) -> io::Result<()>;
    fn quarantine_corrupt(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "journal quarantine unavailable",
        ))
    }
}

pub(super) struct FileJournalStore {
    path: PathBuf,
    lifetime_lock: Mutex<Option<File>>,
}

impl FileJournalStore {
    pub(super) fn new(path: PathBuf) -> Self {
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
pub(super) struct UnavailableJournalStore;

#[cfg(debug_assertions)]
#[derive(Default)]
pub(super) struct TestJournalStore(Mutex<Option<JournalFile>>);

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

#[cfg(debug_assertions)]
pub(super) fn default_unconfigured_store() -> Arc<dyn JournalStore> {
    Arc::new(TestJournalStore::default())
}

#[cfg(not(debug_assertions))]
pub(super) fn default_unconfigured_store() -> Arc<dyn JournalStore> {
    Arc::new(UnavailableJournalStore)
}
