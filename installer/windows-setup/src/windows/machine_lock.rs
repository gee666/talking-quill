//! Exclusive machine lifecycle ownership and durable lock publication.
use super::*;

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

pub(super) struct LegacyMutexPair([OwnedHandle; 2]);
impl LegacyMutexPair {
    pub(super) fn acquire() -> Result<Self> {
        let names = machine_lock_mutex_names()?;
        Ok(Self([
            acquire_verified_legacy_mutex(&names[0])?,
            acquire_verified_legacy_mutex(&names[1])?,
        ]))
    }
}
impl Drop for LegacyMutexPair {
    fn drop(&mut self) {
        for handle in self.0.iter().rev() {
            unsafe { ReleaseMutex(handle.as_raw_handle()) };
        }
    }
}

pub(super) fn acquire_verified_legacy_mutex(name: &str) -> Result<OwnedHandle> {
    let sddl = wide(OsStr::new("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create the legacy lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let raw = unsafe { CreateMutexW(&attributes, 0, wide(OsStr::new(name)).as_ptr()) };
    unsafe { LocalFree(descriptor) };
    if raw.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open the legacy machine lock."));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if !matches!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) },
        0 | 0x80
    ) || !legacy_mutex_security_is_exact(handle.as_raw_handle())?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Legacy machine lock identity is invalid.",
        ));
    }
    Ok(handle)
}

pub(super) fn legacy_mutex_security_is_exact(handle: *mut c_void) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the legacy machine lock.",
        ));
    }
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode the legacy machine lock ACL.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
        .to_ascii_uppercase();
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.starts_with("O:BA")
        && (value.contains("(A;;GA;;;SY)") || value.contains("(A;;0X1F0001;;;SY)"))
        && (value.contains("(A;;GA;;;BA)") || value.contains("(A;;0X1F0001;;;BA)"))
        && value.matches("(A;;").count() == 2
        && !value.contains(";;;AU)"))
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_terminal_owner_present(_paths: &Paths) -> Result<bool> {
    Ok(false)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_terminal_owner_present(paths: &Paths) -> Result<bool> {
    Ok(read_terminal_uninstall_record(paths)?.is_some())
}

pub(super) fn machine_lock_file(paths: &Paths, predecessor_policy_epoch: u8) -> Result<PathBuf> {
    let program_data = machine_lock_program_data(&paths.program_data)?;
    let registry_key = machine_lock_registry_key()?;
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            machine_lock_registry_hive(),
            wide(OsStr::new(&registry_key)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open the machine lock registry key.",
        ));
    }
    reclaim_machine_lock_pending(&program_data)?;
    let terminal_owner_present = machine_lock_terminal_owner_present(paths)?;
    let publication = read_machine_lock_registry_string(key)?;
    let publication = if let Some(retired) = publication
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_machine_lock_suffix(retired)?;
        let retired_directory =
            program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{retired}"));
        if terminal_owner_present && path_present(&retired_directory)? {
            Some(retired.to_owned())
        } else {
            if unsafe {
                RegDeleteValueW(key, wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr())
            } != 0
                || unsafe { RegFlushKey(key) } != 0
            {
                unsafe { RegCloseKey(key) };
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot clear retired machine lock publication.",
                ));
            }
            if path_present(&retired_directory)? {
                reclaim_retired_machine_lock_directory(&program_data, retired)?;
            }
            None
        }
    } else {
        publication
    };
    let directory = if let Some(suffix) = publication {
        validate_machine_lock_suffix(&suffix)?;
        program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH && !terminal_owner_present {
            unsafe { RegCloseKey(key) };
            return Err(fail(
                EXIT_REJECTED,
                "A retired recovery policy cannot republish the machine lock.",
            ));
        }
        reclaim_unpublished_machine_lock_directories(&program_data)?;
        let suffix = new_machine_lock_suffix()?;
        let token = new_machine_lock_suffix()?;
        let pending = program_data.join(format!("{MACHINE_LOCK_PENDING_PREFIX}{token}"));
        let published = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_restricted_lock_directory(&pending)?;
        apply_lock_dacl(&pending, machine_lock_directory_sddl())?;
        let identity = owned_tree_identity(&pending).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Machine lock directory identity is unavailable.",
            )
        })?;
        create_or_verify_lock_marker(
            &pending.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
        )?;
        initialize_machine_lock_tree(&pending, &identity)?;
        flush_setup_directory(&pending)?;
        if unsafe {
            MoveFileExW(
                wide(pending.as_os_str()).as_ptr(),
                wide(published.as_os_str()).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot publish the machine lock directory.",
            ));
        }
        flush_setup_directory(&program_data)?;
        let value = wide(OsStr::new(&suffix));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } != 0
            || unsafe { RegFlushKey(key) } != 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot persist the machine lock identity.",
            ));
        }
        published
    };
    unsafe { RegCloseKey(key) };
    verify_machine_lock_tree(&directory)
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

pub(super) fn initialize_machine_lock_tree(
    directory: &Path,
    directory_identity: &str,
) -> Result<()> {
    let lock = directory.join("recovery-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&lock)
        .map_err(io_failure)?;
    apply_lock_dacl(&lock, machine_lock_file_sddl())?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    create_or_verify_lock_marker(&lock.with_extension("identity-v1"), &identity)?;
    create_or_verify_lock_marker(&directory.join("lock-tree-identity-v1"), directory_identity)
}

pub(super) fn verify_machine_lock_tree(directory: &Path) -> Result<PathBuf> {
    if !marker_security_is_exact(directory, machine_lock_directory_sddl())? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory is not protected.",
        ));
    }
    let identity = owned_tree_identity(directory).map_err(|_| {
        fail(
            EXIT_REJECTED,
            "Machine lock directory identity is unavailable.",
        )
    })?;
    if fs::read_to_string(directory.join("lock-tree-identity-v1")).map_err(io_failure)? != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory identity changed.",
        ));
    }
    let lock = directory.join("recovery-state-v1.lock");
    if !marker_security_is_exact(&lock, machine_lock_file_sddl())? {
        return Err(fail(EXIT_REJECTED, "Machine lock file is not protected."));
    }
    Ok(lock)
}

pub(super) fn reclaim_retired_machine_lock_directory(
    program_data: &Path,
    suffix: &str,
) -> Result<()> {
    let path = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path_present(&path)? {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity =
        owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match remove_owned_tree(&path, &identity) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(fail(EXIT_FAILURE, error.to_string())),
        }
    }
}

pub(super) fn retire_machine_lock_publication(_paths: &Paths) -> Result<String> {
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
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retire the machine lock publication.",
        ));
    }
    let publication = read_machine_lock_registry_string(key)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock publication is missing."))?;
    unsafe { RegCloseKey(key) };
    let suffix = publication
        .strip_prefix(MACHINE_LOCK_RETIRED_PREFIX)
        .unwrap_or(&publication)
        .to_owned();
    validate_machine_lock_suffix(&suffix)?;
    delete_machine_lock_registry_durable(
        &registry_key,
        &machine_lock_registry_parent()?,
        "machine lock publication",
    )?;
    Ok(suffix)
}

pub(super) fn reclaim_unpublished_machine_lock_directories(program_data: &Path) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|name| name.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        else {
            continue;
        };
        if validate_machine_lock_suffix(suffix).is_err() {
            continue;
        }
        let path = entry.path();
        if !marker_security_is_exact(&path, machine_lock_directory_sddl())? {
            continue;
        }
        let identity =
            owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let marker = path.join("publication-pending-v1");
        let expected = format!("{suffix}:{identity}");
        if fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            remove_owned_tree(&path, &identity)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        }
    }
    Ok(())
}

pub(super) fn reclaim_machine_lock_pending(program_data: &Path) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_PENDING_PREFIX))
            .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !marker_security_is_exact(&path, machine_lock_directory_sddl())?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Pending machine lock identity is unavailable.",
            )
        })?;
        remove_owned_tree(&path, &identity)
            .map_err(|_| fail(EXIT_FAILURE, "Cannot reclaim pending machine lock state."))?;
    }
    Ok(())
}

pub(super) fn flush_setup_directory(path: &Path) -> Result<()> {
    let handle = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open a durable machine lock directory.",
        ));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        return Err(fail(EXIT_FAILURE, "Cannot flush a machine lock directory."));
    }
    Ok(())
}

pub(super) fn new_machine_lock_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(EXIT_FAILURE, "Cannot generate the machine lock identity."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn read_machine_lock_registry_string(key: *mut c_void) -> Result<Option<String>> {
    let name = wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE));
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || !(2..=256).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock registry value is invalid.",
        ));
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read the machine lock registry value.",
        ));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lock registry value is invalid."))
}

pub(super) fn create_restricted_lock_directory(path: &Path) -> Result<()> {
    create_directory_with_security(path, machine_lock_directory_sddl())
}

pub(super) fn create_directory_with_security(path: &Path, descriptor_sddl: &str) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create the machine lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) };
    unsafe { LocalFree(descriptor) };
    if created == 0 && !path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock directory.",
        ));
    }
    Ok(())
}

pub(super) fn apply_lock_dacl(path: &Path, descriptor_sddl: &str) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock file ACL.",
        ));
    }
    let status = unsafe {
        SetFileSecurityW(
            wide(path.as_os_str()).as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor) };
    if status == 0 {
        Err(fail(EXIT_FAILURE, "Cannot protect the machine lock file."))
    } else {
        Ok(())
    }
}

pub(super) fn create_or_verify_lock_marker(path: &Path, value: &str) -> Result<()> {
    create_atomic_marker(path, value, machine_lock_file_sddl())
}

pub(super) fn create_atomic_marker(path: &Path, value: &str, sddl: &str) -> Result<()> {
    if path_present(path)? {
        return verify_atomic_marker(path, value, sddl, None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no parent."))?;
    let name = path
        .file_name()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no name."))?
        .to_string_lossy();
    let temporary = parent.join(format!("{name}.tmp-{}", new_machine_lock_suffix()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(io_failure)?;
    apply_lock_dacl(&temporary, sddl)?;
    file.write_all(value.as_bytes()).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    verify_atomic_marker(&temporary, value, sddl, Some(&identity))?;
    if unsafe {
        MoveFileExW(
            wide(temporary.as_os_str()).as_ptr(),
            wide(path.as_os_str()).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot publish the protected marker."));
    }
    flush_setup_directory(parent)?;
    verify_atomic_marker(path, value, sddl, Some(&identity))
}

pub(super) fn verify_atomic_marker(
    path: &Path,
    value: &str,
    sddl: &str,
    expected_identity: Option<&str>,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(io_failure)?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || content != value
        || !marker_security_is_exact(path, sddl)?
    {
        return Err(fail(EXIT_REJECTED, "Protected marker identity changed."));
    }
    Ok(())
}

pub(super) fn file_identity_text(file: &File) -> Result<String> {
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock file identity is unavailable.",
        ));
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(format!("{}:{index}", information.dwVolumeSerialNumber))
}
