//! Durable uninstall journal and relaunch ownership.
use super::*;

pub(super) fn terminal_uninstall_root(paths: &Paths) -> PathBuf {
    paths.program_data.join("Talking Quill Update Recovery")
}

pub(super) fn terminal_uninstall_record_path(paths: &Paths) -> PathBuf {
    terminal_uninstall_root(paths).join(TERMINAL_UNINSTALL_RECORD_NAME)
}

pub(super) fn write_terminal_uninstall_record(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    let record_path = terminal_uninstall_record_path(paths);
    let temporary = root.join(format!(
        ".terminal-uninstall.tmp-{}",
        random_machine_lock_suffix()?
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(io_failure)?;
    apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    let mut published = record.clone();
    published.record_file_identity = file_identity_text(&file)?;
    let bytes = serde_json::to_vec(&published)
        .map_err(|_| fail(EXIT_FAILURE, "Cannot encode terminal uninstall recovery."))?;
    file.write_all(&bytes).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    if identity != published.record_file_identity {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal record identity changed before publication.",
        ));
    }
    durable_replace(&temporary, &record_path)?;
    flush_setup_directory(&root)?;
    let reopened = File::open(&record_path).map_err(io_failure)?;
    let reopened_identity = file_identity_text(&reopened)?;
    drop(reopened);
    if reopened_identity != published.record_file_identity
        || !marker_security_is_exact(&record_path, MEDIUM_LAUNCHER_FILE_SDDL)?
        || fs::read(&record_path).map_err(io_failure)? != bytes
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal record publication is invalid.",
        ));
    }
    Ok(())
}

pub(super) fn random_machine_lock_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn read_terminal_uninstall_record(
    paths: &Paths,
) -> Result<Option<TerminalUninstallRecord>> {
    let path = terminal_uninstall_record_path(paths);
    if !path_present(&path)? {
        return Ok(None);
    }
    assert_plain_file(&path)?;
    let bytes = fs::read(&path).map_err(io_failure)?;
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err(fail(EXIT_REJECTED, "Terminal uninstall record is invalid."));
    }
    let record: TerminalUninstallRecord = serde_json::from_slice(&bytes)
        .map_err(|_| fail(EXIT_REJECTED, "Terminal uninstall record is invalid."))?;
    let file = File::open(&path).map_err(io_failure)?;
    let record_identity = file_identity_text(&file)?;
    drop(file);
    let (uninstall_command, quiet_uninstall_command) = terminal_uninstall_commands(paths);
    if record.schema_version != 3
        || validate_machine_lock_suffix(&record.generation).is_err()
        || !matches!(
            record.phase.as_str(),
            "armed"
                | "machine-retired"
                | "cleanup-complete"
                | "final-launcher-owned"
                | "maintenance-deletion-owned"
                | "maintenance-deleted"
                | "uninstall-unregistered"
                | "journal-removed"
        )
        || record.maintenance_sha256.len() != 64
        || !record
            .maintenance_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
        || record.uninstall_command != uninstall_command
        || record.quiet_uninstall_command != quiet_uninstall_command
        || record.service_name != terminal_service_name(&record.generation)?
        || record.service_image
            != terminal_service_image(paths, &record.generation)?.to_string_lossy()
        || record.service_sha256.len() != 64
        || !record
            .service_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
        || record.service_file_identity.is_empty()
        || record.record_file_identity != record_identity
    {
        return Err(fail(EXIT_REJECTED, "Terminal uninstall record is invalid."));
    }
    Ok(Some(record))
}

pub(super) fn ensure_machine_relaunch_owner_installed(paths: &Paths) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    if !path_present(&root)? {
        create_directory_with_security(&root, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
        apply_lock_dacl(&root, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    }
    if !medium_launcher_directory_is_protected(&root)? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine recovery launcher is not protected.",
        ));
    }
    let identity =
        owned_tree_identity(&root).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let marker = root.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
    } else {
        verify_unpublished_launcher_directory(&root)?;
        // Publish directory ownership before any executable copy can be interrupted.
        create_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL)?;
    }
    for entry in fs::read_dir(&root).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        if unpublished_launcher_name(&entry.file_name().to_string_lossy()) {
            assert_plain_file(&entry.path())?;
            if marker_security_is_exact(&entry.path(), MEDIUM_LAUNCHER_FILE_SDDL)? {
                fs::remove_file(entry.path()).map_err(io_failure)?;
            }
        }
    }
    let source = paths
        .install
        .join("resources/helper/talking-quill-update-recovery-launcher.exe");
    assert_plain_file(&source)?;
    let target = paths.recovery_launcher.clone();
    let temporary = root.join(format!(".launcher.tmp-{}", random_machine_lock_suffix()?));
    fs::copy(&source, &temporary).map_err(io_failure)?;
    apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    flush_file(&temporary)?;
    durable_replace(&temporary, &target)?;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
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
        return Err(fail(EXIT_FAILURE, "Cannot create machine recovery owner."));
    }
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        target.display()
    );
    let value = wide(OsStr::new(&command));
    let status = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Talking Quill Update Relaunch")).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(EXIT_FAILURE, "Cannot flush machine recovery owner."));
    }
    flush_setup_directory(&root)
}

fn unpublished_launcher_name(name: &str) -> bool {
    name.strip_prefix(".launcher.tmp-")
        .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
}

pub(super) fn verify_unpublished_launcher_directory(root: &Path) -> Result<()> {
    if !medium_launcher_directory_is_protected(root)? {
        return Err(fail(
            EXIT_REJECTED,
            "Unpublished launcher directory is not protected.",
        ));
    }
    for entry in fs::read_dir(root).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        assert_plain_file(&entry.path())?;
        if !unpublished_launcher_name(&entry.file_name().to_string_lossy())
            || !marker_security_is_exact(&entry.path(), MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Unpublished launcher directory contains an unrecognized file.",
            ));
        }
    }
    Ok(())
}

pub(super) fn require_machine_relaunch_owner(paths: &Paths) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    let launcher = paths.recovery_launcher.clone();
    if !medium_launcher_directory_is_protected(&root)? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine recovery launcher is not protected.",
        ));
    }
    assert_plain_file(&launcher)?;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_REJECTED, "Machine recovery owner is missing."));
    }
    let expected = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    let actual = read_registry_value(key, "Talking Quill Update Relaunch", 1024)?;
    unsafe { RegCloseKey(key) };
    if actual.as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(fail(EXIT_REJECTED, "Machine recovery owner is invalid."))
    }
}
