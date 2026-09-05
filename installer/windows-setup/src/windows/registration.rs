//! Installation paths, maintenance images, and application registration.
use super::*;

pub(super) fn maintenance_generation_from_name(name: &str) -> Option<&str> {
    name.strip_prefix("Talking Quill Maintenance-")
        .and_then(|value| value.strip_suffix(".exe"))
        .filter(|generation| validate_machine_lock_suffix(generation).is_ok())
}

pub(super) fn registered_maintenance_generation(program_files: &Path) -> Result<Option<String>> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Ok(None);
    }
    let quiet = read_registry_value(key, "QuietUninstallString", 2048)?;
    unsafe { RegCloseKey(key) };
    let Some(command) = quiet else {
        return Ok(None);
    };
    let Some(path) = command
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix("\" /S"))
        .map(PathBuf::from)
    else {
        return Err(fail(
            EXIT_REJECTED,
            "Registered maintenance command is invalid.",
        ));
    };
    if path.parent() != Some(program_files) {
        return Err(fail(
            EXIT_REJECTED,
            "Registered maintenance path is invalid.",
        ));
    }
    Ok(path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(maintenance_generation_from_name)
        .map(str::to_owned))
}

pub(super) fn current_install_generation(
    program_files: &Path,
    program_data: &Path,
    generation_record: &Path,
) -> Result<String> {
    let current = std::env::current_exe().map_err(io_failure)?;
    if let Some(generation) = current
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(maintenance_generation_from_name)
    {
        return Ok(generation.to_owned());
    }
    let current_text = current.to_string_lossy();
    let recovery_context = current.starts_with(program_files)
        || current.starts_with(program_data)
        || current_text.contains(".TalkingQuill-uninstall-");
    if recovery_context && let Some(generation) = registered_maintenance_generation(program_files)?
    {
        return Ok(generation);
    }
    if path_present(generation_record)? {
        assert_plain_file(generation_record)?;
        let generation = fs::read_to_string(generation_record).map_err(io_failure)?;
        validate_machine_lock_suffix(&generation)?;
        return Ok(generation);
    }
    let generation = random_machine_lock_suffix()?;
    // Only the elevated worker publishes this protected generation. If it is interrupted, every
    // recovery process reuses the durable record rather than trusting caller-controlled state.
    if token_is_elevated()? {
        let mut record = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(generation_record)
            .map_err(io_failure)?;
        record
            .write_all(generation.as_bytes())
            .map_err(io_failure)?;
        record.sync_all().map_err(io_failure)?;
        drop(record);
        flush_setup_directory(program_files)?;
    }
    Ok(generation)
}

pub(super) fn paths() -> Result<Paths> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = known_folder(&FOLDERID_System)?;
    let profile = known_folder(&FOLDERID_RoamingAppData)?.join("Talking Quill");
    let maintenance_generation_record =
        program_files.join(".Talking Quill.maintenance-generation-v1");
    let maintenance_generation = current_install_generation(
        &program_files,
        &program_data,
        &maintenance_generation_record,
    )?;
    let maintenance_uninstaller = program_files.join(format!(
        "Talking Quill Maintenance-{maintenance_generation}.exe"
    ));
    let recovery_launcher = program_data.join(format!(
        "Talking Quill Update Recovery/talking-quill-update-recovery-launcher-{maintenance_generation}.exe"
    ));
    Ok(Paths {
        install: program_files.join("Talking Quill"),
        staging: program_files.join(".Talking Quill.native-staging"),
        backup: program_files.join(".Talking Quill.native-backup"),
        transaction: program_files.join(".Talking Quill.native-transaction-v2.json"),
        maintenance_generation_record,
        maintenance_uninstaller,
        recovery_launcher,
        profile,
        legacy_authority: program_data.join("Talking Quill/KeyboardAuthority"),
        legacy_quarantine: program_data
            .join("Talking Quill/.KeyboardAuthority.retirement-quarantine"),
        legacy_task_file: system.join("Tasks/TalkingQuillKeyboardAuthority"),
        program_data,
    })
}

pub(super) const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill";
pub(super) const APP_PATH_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe";

pub(super) fn restore_repair_controller(paths: &Paths) -> Result<()> {
    let source = paths.backup.join("Uninstall Talking Quill.exe");
    let target = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    assert_plain_file(&target)?;
    let temporary = paths.install.join(".repair-controller-recovery.tmp");
    if path_present(&temporary)? {
        assert_plain_file(&temporary)?;
        if file_hash(&temporary)? == file_hash(&source)? {
            return durable_replace(&temporary, &target);
        }
        // The fixed plain file is installer-owned inside the protected target tree;
        // a mismatched value is an interrupted copy and is safe to recreate.
        fs::remove_file(&temporary).map_err(io_failure)?;
    }
    let mut input = File::open(source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    durable_replace(&temporary, &target)
}

pub(super) fn remove_maintenance_temporary_files(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let owned = name
            .to_str()
            .and_then(|value| value.strip_prefix("Talking Quill Maintenance."))
            .and_then(|value| value.strip_suffix(".tmp"))
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if owned {
            assert_plain_file(&entry.path())?;
            fs::remove_file(entry.path()).map_err(io_failure)?;
        }
    }
    Ok(())
}

pub(super) fn remove_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    if path_present(&paths.maintenance_uninstaller)? {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        fs::remove_file(&paths.maintenance_uninstaller).map_err(io_failure)?;
    }
    Ok(())
}

pub(super) fn ensure_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    let source = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    let replacing = path_present(&paths.maintenance_uninstaller)?;
    if replacing {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        if file_hash(&source)? == file_hash(&paths.maintenance_uninstaller)? {
            return Ok(());
        }
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = paths
        .maintenance_uninstaller
        .with_extension(format!("{suffix}.tmp"));
    let mut input = File::open(&source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    if file_hash(&source)? != file_hash(&temporary)? {
        return Err(fail(
            EXIT_REJECTED,
            "Maintenance uninstaller verification failed.",
        ));
    }
    if replacing {
        durable_replace(&temporary, &paths.maintenance_uninstaller)
    } else {
        durable_rename(&temporary, &paths.maintenance_uninstaller)
    }
}

pub(super) fn ensure_uninstall_finalizer_registered(
    current: &Path,
    paths: &Paths,
) -> Result<PathBuf> {
    if let Some(existing) = registered_uninstall_executable()?
        && existing
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(UNINSTALL_FINALIZER_NAME))
        && path_present(&existing)?
        && is_uninstall_finalizer(&existing)?
        && marker_security_is_exact(&existing, MEDIUM_FINALIZER_FILE_SDDL)?
        && file_hash(&existing)? == file_hash(current)?
    {
        return Ok(existing);
    }
    write_transaction(
        paths,
        "uninstall-finalizer-publishing",
        Action::Uninstall,
        true,
    )?;
    let token = new_machine_lock_suffix()?;
    let suffix = new_machine_lock_suffix()?;
    let pending = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PENDING_PREFIX}{token}"));
    let published = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PREFIX}{suffix}"));
    create_directory_with_security(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    apply_lock_dacl(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    let identity =
        owned_tree_identity(&pending).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    create_or_verify_finalizer_marker(&pending.join("finalizer-tree-identity-v1"), &identity)?;
    let executable = pending.join(UNINSTALL_FINALIZER_NAME);
    let mut source = File::open(current).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&executable)
        .map_err(io_failure)?;
    apply_lock_dacl(&executable, MEDIUM_FINALIZER_FILE_SDDL)?;
    std::io::copy(&mut source, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    if file_hash(current)? != file_hash(&executable)? {
        return Err(fail(EXIT_REJECTED, "Uninstall finalizer copy changed."));
    }
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
            "Cannot publish the uninstall finalizer.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    let executable = published.join(UNINSTALL_FINALIZER_NAME);
    register_uninstall_executable(&executable)?;
    write_transaction(
        paths,
        "uninstall-finalizer-published",
        Action::Uninstall,
        true,
    )?;
    Ok(executable)
}

pub(super) fn create_or_verify_finalizer_marker(path: &Path, identity: &str) -> Result<()> {
    create_atomic_marker(path, identity, MEDIUM_FINALIZER_FILE_SDDL)
}

pub(super) fn register_uninstall_executable(executable: &Path) -> Result<()> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null_mut(),
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
            "Cannot open uninstall recovery registration.",
        ));
    }
    let values = [
        ("UninstallString", format!("\"{}\"", executable.display())),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", executable.display()),
        ),
    ];
    for (name, value) in values {
        let bytes = wide(OsStr::new(&value));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        } != 0
        {
            unsafe { RegCloseKey(key) };
            return Err(fail(EXIT_FAILURE, "Cannot register uninstall recovery."));
        }
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(EXIT_FAILURE, "Cannot flush uninstall recovery."))
    }
}

pub(super) fn registered_uninstall_executable() -> Result<Option<PathBuf>> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read uninstall recovery registration.",
        ));
    }
    let value = read_registry_value(key, "UninstallString", 4096)?;
    unsafe { RegCloseKey(key) };
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    let path = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'));
    Ok(path.map(PathBuf::from))
}

pub(super) fn read_registry_value(
    key: *mut c_void,
    name: &str,
    maximum: u32,
) -> Result<Option<String>> {
    let name = wide(OsStr::new(name));
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
    if first != 0 || kind != REG_SZ || bytes < 2 || bytes > maximum || !bytes.is_multiple_of(2) {
        return Err(fail(EXIT_REJECTED, "Registry string is invalid."));
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
        return Err(fail(EXIT_FAILURE, "Cannot read registry string."));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Registry string is invalid."))
}

pub(super) fn reclaim_stale_maintenance_uninstallers(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let path = entry.path();
        if path == paths.maintenance_uninstaller {
            continue;
        }
        let stale = entry
            .file_name()
            .to_str()
            .and_then(maintenance_generation_from_name)
            .is_some();
        if stale {
            assert_plain_file(&path)?;
            fs::remove_file(path).map_err(io_failure)?;
        }
    }
    flush_setup_directory(parent)
}

pub(super) fn register_installed_uninstall(paths: &Paths) -> Result<()> {
    ensure_maintenance_uninstaller(paths)?;
    let manifest = paths
        .install
        .join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&manifest)?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest).map_err(io_failure)?)
        .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
    let version = value
        .get("version")
        .and_then(|item| item.as_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Installed release version is invalid."))?;
    register_uninstall(paths, version)?;
    register_app_path(paths)?;
    reclaim_stale_maintenance_uninstallers(paths)
}

pub(super) fn register_uninstall(paths: &Paths, version: &str) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native uninstall registration.",
        ));
    }
    let values = [
        ("DisplayName", "Talking Quill".to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "Talking Quill contributors".to_string()),
        ("InstallLocation", paths.install.display().to_string()),
        (
            "UninstallString",
            format!("\"{}\"", paths.maintenance_uninstaller.display()),
        ),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", paths.maintenance_uninstaller.display()),
        ),
    ];
    let result = values.iter().try_for_each(|(name, value)| {
        let bytes = wide(OsStr::new(value));
        let status = unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(fail(
                EXIT_FAILURE,
                "Cannot write the native uninstall registration.",
            ))
        }
    });
    let flushed = result.is_ok() && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        result
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot durably write the native uninstall registration.",
        ))
    }
}

pub(super) fn register_app_path(paths: &Paths) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(APP_PATH_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native application registration.",
        ));
    }
    let executable = wide(paths.install.join("Talking Quill.exe").as_os_str());
    let directory = wide(paths.install.as_os_str());
    let first = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("")).as_ptr(),
            0,
            REG_SZ,
            executable.as_ptr().cast(),
            (executable.len() * 2) as u32,
        )
    };
    let second = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Path")).as_ptr(),
            0,
            REG_SZ,
            directory.as_ptr().cast(),
            (directory.len() * 2) as u32,
        )
    };
    let flushed = first == 0 && second == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot write the native application registration.",
        ))
    }
}

pub(super) fn unregister_app_path() -> Result<()> {
    delete_registry_tree_durable(
        APP_PATH_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        "native application registration",
    )
}
