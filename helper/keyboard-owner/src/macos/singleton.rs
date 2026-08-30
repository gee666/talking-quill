#![cfg(target_os = "macos")]

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Mutex, OnceLock};

use crate::runtime::{SingletonCoordinator, SingletonError};

use super::runtime_directory::{MAINTENANCE_FILE, MacosRuntimeDirectory, SINGLETON_FILE};

static POISONED_SINGLETON_FDS: OnceLock<Mutex<Vec<OwnedFd>>> = OnceLock::new();

pub struct MacosFlockSingleton {
    directory: MacosRuntimeDirectory,
    maintenance_fd: Option<OwnedFd>,
    owner_fd: Option<OwnedFd>,
}

impl MacosFlockSingleton {
    #[must_use]
    pub const fn new(directory: MacosRuntimeDirectory) -> Self {
        Self {
            directory,
            maintenance_fd: None,
            owner_fd: None,
        }
    }
}

impl std::fmt::Debug for MacosFlockSingleton {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MacosFlockSingleton(<redacted>)")
    }
}

impl SingletonCoordinator for MacosFlockSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        if self.owner_fd.is_some() {
            return Ok(true);
        }
        // Shared maintenance exclusion is acquired first and retained for the
        // complete owner lifetime. R8-M maintenance must acquire LOCK_EX, so it
        // cannot race between this check and owner singleton acquisition.
        let maintenance = self
            .directory
            .open_lock_file(MAINTENANCE_FILE)
            .map_err(|_| SingletonError)?;
        if unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(false)
            } else {
                Err(SingletonError)
            };
        }

        let fd = self
            .directory
            .open_lock_file(SINGLETON_FILE)
            .map_err(|_| SingletonError)?;
        // SAFETY: flock operates on the retained verified regular file.
        if unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_UN) };
            return if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(false)
            } else {
                Err(SingletonError)
            };
        }
        self.maintenance_fd = Some(maintenance);
        self.owner_fd = Some(fd);
        Ok(true)
    }

    fn release(&mut self) {
        if let Some(fd) = self.maintenance_fd.take() {
            // Endpoint shutdown/quiescence precedes this call. Drop shared
            // maintenance exclusion while owner election remains held, giving
            // a waiting R8-M exclusive coordinator the handoff opportunity.
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_UN) };
        }
        if let Some(fd) = self.owner_fd.take() {
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_UN) };
        }
    }

    fn preserve_process_lifetime(&mut self) {
        let storage = POISONED_SINGLETON_FDS.get_or_init(|| Mutex::new(Vec::new()));
        let mut storage = storage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(fd) = self.maintenance_fd.take() {
            storage.push(fd);
        }
        if let Some(fd) = self.owner_fd.take() {
            storage.push(fd);
        }
    }
}

impl Drop for MacosFlockSingleton {
    fn drop(&mut self) {
        self.release();
    }
}
