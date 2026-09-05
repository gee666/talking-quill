//! Exclusive machine lifecycle ownership and durable lock publication.
use super::*;

mod legacy_mutex;
pub(super) use legacy_mutex::*;

mod publication;
pub(super) use publication::*;

mod tree;
pub(super) use tree::*;

mod protected_markers;
pub(super) use protected_markers::*;

pub(super) struct MachineLock {
    pub(super) _legacy: Option<LegacyMutexPair>,
    pub(super) file: File,
}
impl MachineLock {
    pub(super) fn acquire(
        paths: &Paths,
        timeout: u32,
        predecessor_policy_epoch: u8,
    ) -> Result<Self> {
        let legacy = if predecessor_policy_epoch < LEGACY_LOCK_RETIREMENT_EPOCH {
            Some(LegacyMutexPair::acquire()?)
        } else {
            None
        };
        let path = machine_lock_file(paths, predecessor_policy_epoch)?;
        let deadline = Instant::now() + Duration::from_millis(timeout.into());
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
            {
                Ok(file) => {
                    let expected = fs::read_to_string(path.with_extension("identity-v1"))
                        .map_err(io_failure)?;
                    if !marker_security_is_exact(&path, machine_lock_file_sddl())?
                        || file_identity_text(&file)? != expected
                    {
                        return Err(fail(EXIT_REJECTED, "Machine lock identity is invalid."));
                    }
                    validate_acquired_machine_lock_state(
                        &machine_lock_program_data(&paths.program_data)?,
                        &path,
                    )?;
                    return Ok(Self {
                        _legacy: legacy,
                        file,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(fail(EXIT_FAILURE, "Machine setup lock timed out.")),
            }
        }
    }
}
pub(super) fn validate_acquired_machine_lock_state(program_data: &Path, lock: &Path) -> Result<()> {
    let suffix = lock
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock path is invalid."))?;
    let registry_key = machine_lock_registry_key()?;
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            machine_lock_registry_hive(),
            wide(OsStr::new(&registry_key)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_REJECTED, "Machine lock publication is missing."));
    }
    let publication = read_machine_lock_registry_string(key)?;
    unsafe { RegCloseKey(key) };
    match publication.as_deref() {
        Some(value) if value == suffix => Ok(()),
        Some(value)
            if value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX) == Some(suffix)
                && path_present(
                    &program_data
                        .join("Talking Quill Update Recovery")
                        .join(TERMINAL_UNINSTALL_RECORD_NAME),
                )? =>
        {
            Ok(())
        }
        _ => Err(fail(EXIT_REJECTED, "Machine lock publication changed.")),
    }
}

impl MachineLock {
    pub(super) fn take_legacy(&mut self) -> Option<LegacyMutexPair> {
        self._legacy.take()
    }
}
impl Drop for MachineLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
    }
}

pub(super) fn validate_machine_lock_suffix(suffix: &str) -> Result<()> {
    if suffix.len() == 32
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(fail(
            EXIT_REJECTED,
            "Machine lock registry identity is invalid.",
        ))
    }
}

pub(super) fn new_machine_lock_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(EXIT_FAILURE, "Cannot generate the machine lock identity."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
