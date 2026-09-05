//! Machine lifecycle lock acquisition, validation, and durable publication.
use super::*;

pub(super) struct RecoveryStateLock {
    pub(super) _legacy: Option<LegacyMutexPair>,
    pub(super) file: File,
    pub(super) path: PathBuf,
}

impl RecoveryStateLock {
    pub(super) fn acquire() -> Result<Self, i32> {
        Self::acquire_for_epoch(installed_recovery_policy_epoch()?)
    }

    pub(super) fn acquire_for_epoch(predecessor_policy_epoch: u8) -> Result<Self, i32> {
        let legacy = if predecessor_policy_epoch < LEGACY_LOCK_RETIREMENT_EPOCH {
            Some(LegacyMutexPair::acquire()?)
        } else {
            None
        };
        let path = machine_lock_file(predecessor_policy_epoch)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
            {
                Ok(file) => {
                    if !has_exact_security(&path, MACHINE_LOCK_FILE_SDDL)?
                        || file_identity_text(&file)?
                            != std::fs::read_to_string(path.with_extension("identity-v1"))
                                .map_err(|_| EXIT_IDENTITY_MISMATCH)?
                    {
                        return Err(EXIT_IDENTITY_MISMATCH);
                    }
                    validate_acquired_machine_lock_state(&path)?;
                    return Ok(Self {
                        _legacy: legacy,
                        file,
                        path,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(EXIT_LAUNCH_FAILED),
            }
        }
    }

    pub(super) fn retire(self) -> Result<(), i32> {
        let registry_key = machine_lock_registry_key()?;
        let directory = self
            .path
            .parent()
            .ok_or(EXIT_IDENTITY_MISMATCH)?
            .to_path_buf();
        let suffix = directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
            .ok_or(EXIT_IDENTITY_MISMATCH)?;
        let identity = owned_tree_identity(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let mut key = std::ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                machine_lock_registry_hive(),
                wide_nul(Path::new(&registry_key))?.as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut key,
            )
        } != 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        let value = wide_nul(Path::new(&format!("{MACHINE_LOCK_RETIRED_PREFIX}{suffix}")))?;
        let status = unsafe {
            RegSetValueExW(
                key,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
        unsafe { RegCloseKey(key) };
        if !flushed {
            return Err(EXIT_LAUNCH_FAILED);
        }
        drop(self);
        remove_owned_tree(&directory, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        let deleted = unsafe {
            RegDeleteTreeW(
                machine_lock_registry_hive(),
                wide_nul(Path::new(&registry_key))?.as_ptr(),
            )
        };
        if deleted == 0 || deleted == 2 {
            Ok(())
        } else {
            Err(EXIT_LAUNCH_FAILED)
        }
    }
}

pub(super) fn validate_acquired_machine_lock_state(lock: &Path) -> Result<(), i32> {
    let suffix = lock
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let registry_key = machine_lock_registry_key()?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            machine_lock_registry_hive(),
            wide_nul(Path::new(&registry_key))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let publication = read_registry_string(key, MACHINE_LOCK_REGISTRY_VALUE)?;
    unsafe { RegCloseKey(key) };
    match publication.as_deref() {
        Some(value) if value == suffix => Ok(()),
        Some(value)
            if value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX) == Some(suffix)
                && machine_lock_terminal_owner_present()? =>
        {
            Ok(())
        }
        _ => Err(EXIT_IDENTITY_MISMATCH),
    }
}

impl Drop for RecoveryStateLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
    }
}

pub(super) fn installed_recovery_policy_epoch() -> Result<u8, i32> {
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let bytes = std::fs::read(current).map_err(|_| EXIT_LAUNCH_FAILED)?;
    const PREFIX: &[u8] = b"TALKING_QUILL_WINDOWS_RECOVERY_POLICY_EPOCH=";
    let matches = bytes
        .windows(PREFIX.len() + 1)
        .filter_map(|window| {
            window
                .strip_prefix(PREFIX)
                .map(|value| value[0])
                .filter(u8::is_ascii_digit)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(1);
    }
    if matches.iter().any(|value| *value != matches[0]) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(matches[0] - b'0')
}

pub(super) struct LegacyMutexPair(pub(super) [OwnedHandle; 2]);

impl LegacyMutexPair {
    pub(super) fn acquire() -> Result<Self, i32> {
        let names = machine_lock_mutex_names()?;
        let first = acquire_verified_legacy_mutex(&names[0])?;
        let second = acquire_verified_legacy_mutex(&names[1])?;
        Ok(Self([first, second]))
    }
}

impl Drop for LegacyMutexPair {
    fn drop(&mut self) {
        for handle in self.0.iter().rev() {
            unsafe { ReleaseMutex(handle.as_raw_handle()) };
        }
    }
}

pub(super) fn acquire_verified_legacy_mutex(name: &str) -> Result<OwnedHandle, i32> {
    let descriptor = SecurityDescriptor::restricted("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let raw = unsafe { CreateMutexW(&attributes, 0, wide_nul(Path::new(name))?.as_ptr()) };
    if raw.is_null() {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if !matches!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) },
        WAIT_OBJECT_0 | 0x80
    ) || !legacy_mutex_security_is_exact(handle.as_raw_handle())?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(handle)
}

pub(super) fn legacy_mutex_security_is_exact(handle: HANDLE) -> Result<bool, i32> {
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let text = security_descriptor_text(descriptor)?;
    unsafe { LocalFree(descriptor.cast()) };
    let normalized = text.to_ascii_uppercase();
    Ok(normalized.starts_with("O:BA")
        && (normalized.contains("(A;;GA;;;SY)") || normalized.contains("(A;;0X1F0001;;;SY)"))
        && (normalized.contains("(A;;GA;;;BA)") || normalized.contains("(A;;0X1F0001;;;BA)"))
        && normalized.matches("(A;;").count() == 2
        && !normalized.contains(";;;AU)"))
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_terminal_owner_present() -> Result<bool, i32> {
    Ok(false)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_terminal_owner_present() -> Result<bool, i32> {
    Ok(terminal_uninstall_record()?.is_some())
}

pub(super) fn machine_lock_file(predecessor_policy_epoch: u8) -> Result<PathBuf, i32> {
    let root = machine_lock_program_data()?;
    let registry_key = machine_lock_registry_key()?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            machine_lock_registry_hive(),
            wide_nul(Path::new(&registry_key))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    reclaim_machine_lock_pending(&root)?;
    let terminal_owner_present = machine_lock_terminal_owner_present()?;
    let published = read_registry_string(key, MACHINE_LOCK_REGISTRY_VALUE)?;
    let published = if let Some(retired) = published
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_generation(retired)?;
        let retired_directory = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{retired}"));
        if terminal_owner_present && retired_directory.exists() {
            Some(retired.to_owned())
        } else {
            if unsafe {
                RegDeleteValueW(
                    key,
                    wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                )
            } != 0
                || unsafe { RegFlushKey(key) } != 0
            {
                unsafe { RegCloseKey(key) };
                return Err(EXIT_LAUNCH_FAILED);
            }
            if retired_directory.exists() {
                reclaim_retired_machine_lock_directory(&retired_directory)?;
            }
            None
        }
    } else {
        published
    };
    let directory = if let Some(suffix) = published {
        validate_generation(&suffix)?;
        root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH && !terminal_owner_present {
            unsafe { RegCloseKey(key) };
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        reclaim_unpublished_machine_lock_directories(&root)?;
        let suffix = new_recovery_generation()?;
        let token = new_recovery_generation()?;
        let pending = root.join(format!("{MACHINE_LOCK_PENDING_PREFIX}{token}"));
        let published = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_directory_with_sddl(&pending, MACHINE_LOCK_DIRECTORY_SDDL)?;
        apply_restricted_dacl(&pending, MACHINE_LOCK_DIRECTORY_SDDL)?;
        let identity = owned_tree_identity(&pending).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        create_or_verify_marker(
            &pending.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
            MACHINE_LOCK_FILE_SDDL,
        )?;
        initialize_machine_lock_tree(&pending, &identity)?;
        flush_directory(&pending)?;
        if unsafe {
            MoveFileExW(
                wide_nul(&pending)?.as_ptr(),
                wide_nul(&published)?.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        flush_directory(&root)?;
        let value = wide_nul(Path::new(&suffix))?;
        if unsafe {
            RegSetValueExW(
                key,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } != 0
            || unsafe { RegFlushKey(key) } != 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        published
    };
    unsafe { RegCloseKey(key) };
    verify_machine_lock_tree(&directory)
}

pub(super) fn initialize_machine_lock_tree(
    directory: &Path,
    directory_identity: &str,
) -> Result<(), i32> {
    let lock = directory.join("recovery-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&lock)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&lock, MACHINE_LOCK_FILE_SDDL)?;
    file.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    create_or_verify_marker(
        &lock.with_extension("identity-v1"),
        &identity,
        MACHINE_LOCK_FILE_SDDL,
    )?;
    create_or_verify_marker(
        &directory.join("lock-tree-identity-v1"),
        directory_identity,
        MACHINE_LOCK_FILE_SDDL,
    )
}

pub(super) fn verify_machine_lock_tree(directory: &Path) -> Result<PathBuf, i32> {
    if !has_exact_security(directory, MACHINE_LOCK_DIRECTORY_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if std::fs::read_to_string(directory.join("lock-tree-identity-v1"))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?
        != identity
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let lock = directory.join("recovery-state-v1.lock");
    if !has_exact_security(&lock, MACHINE_LOCK_FILE_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(lock)
}

pub(super) fn reclaim_retired_machine_lock_directory(directory: &Path) -> Result<(), i32> {
    verify_machine_lock_tree(directory)?;
    let identity = owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    remove_owned_tree(directory, &identity).map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn reclaim_unpublished_machine_lock_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        else {
            continue;
        };
        if validate_generation(suffix).is_err() {
            continue;
        }
        let path = entry.path();
        if !has_exact_security(&path, MACHINE_LOCK_DIRECTORY_SDDL)? {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let marker = path.join("publication-pending-v1");
        let expected = format!("{suffix}:{identity}");
        if std::fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    Ok(())
}

pub(super) fn reclaim_machine_lock_pending(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_PENDING_PREFIX))
            .is_some_and(|suffix| validate_generation(suffix).is_ok())
        {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, MACHINE_LOCK_DIRECTORY_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

pub(super) fn flush_directory(path: &Path) -> Result<(), i32> {
    #[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
    let share_mode = FILE_SHARE_READ | FILE_SHARE_WRITE;
    #[cfg(any(test, feature = "machine-lock-test-namespace"))]
    let share_mode = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let handle = unsafe {
        CreateFileW(
            wide_nul(path)?.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            share_mode,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

pub(super) fn create_or_verify_marker(path: &Path, value: &str, sddl: &str) -> Result<(), i32> {
    if path.exists() {
        return verify_marker(path, value, sddl, None);
    }
    let parent = path.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let name = path
        .file_name()
        .ok_or(EXIT_IDENTITY_MISMATCH)?
        .to_string_lossy();
    let temporary = parent.join(format!("{name}.tmp-{}", new_recovery_generation()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, sddl)?;
    file.write_all(value.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    verify_marker(&temporary, value, sddl, Some(&identity))?;
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(path)?.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    flush_directory(parent)?;
    verify_marker(path, value, sddl, Some(&identity))
}

pub(super) fn verify_marker(
    path: &Path,
    value: &str,
    sddl: &str,
    expected_identity: Option<&str>,
) -> Result<(), i32> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || content != value
        || !has_exact_security(path, sddl)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

pub(super) fn file_identity_text(file: &File) -> Result<String, i32> {
    let identity = file_identity(file).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    Ok(format!(
        "{}:{}",
        identity.volume,
        (u64::from(identity.index_high) << 32) | u64::from(identity.index_low)
    ))
}

pub(super) fn read_registry_string(key: *mut c_void, name: &str) -> Result<Option<String>, i32> {
    let name = wide_nul(Path::new(name))?;
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || !(2..=1024).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}
