//! Finalize uninstall while retaining crash recovery evidence.
use super::*;

pub(super) fn terminal_recovery_tombstone(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_RECOVERY_TOMBSTONE_PREFIX}{generation}")))
}

pub(super) fn terminal_final_launcher(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_FINAL_LAUNCHER_PREFIX}{generation}.exe")))
}

pub(super) fn publish_terminal_final_launcher(paths: &Paths, generation: &str) -> Result<PathBuf> {
    let source = paths.recovery_launcher.clone();
    let target = terminal_final_launcher(paths, generation)?;
    if !path_present(&target)? {
        let temporary = paths.program_data.join(format!(
            ".Talking Quill.terminal-relaunch-pending-{}.exe",
            random_machine_lock_suffix()?
        ));
        fs::copy(&source, &temporary).map_err(io_failure)?;
        apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
        flush_file(&temporary)?;
        durable_replace(&temporary, &target)?;
    }
    if file_hash(&source)? != file_hash(&target)?
        || !marker_security_is_exact(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(fail(EXIT_REJECTED, "Terminal final launcher is invalid."));
    }
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
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
        return Err(fail(
            EXIT_FAILURE,
            "Cannot publish terminal final launcher.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    Ok(target)
}

pub(super) fn pending_deletion_is_owned(path: &Path) -> Result<bool> {
    let expected = canonical(path)?;
    let key = open_session_manager(KEY_READ)?;
    let pairs = read_pending_rename_pairs(key)?;
    unsafe { RegCloseKey(key) };
    Ok(pairs.iter().any(|(source, destination)| {
        destination.is_empty() && normalized_pending_source(source) == expected
    }))
}

pub(super) fn remove_terminal_recovery_tombstone(_paths: &Paths, tombstone: &Path) -> Result<()> {
    if !path_present(tombstone)? {
        return Ok(());
    }
    if !medium_launcher_directory_is_protected(tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is unprotected.",
        ));
    }
    let marker = tombstone.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        let identity = owned_tree_identity(tombstone)
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
        let mut tree = Vec::new();
        collect_finalizer_deletion_paths(tombstone, &mut tree)?;
        let record = tombstone.join(TERMINAL_UNINSTALL_RECORD_NAME);
        let record_present = path_present(&record)?;
        if !record_present {
            let entries = fs::read_dir(tombstone)
                .map_err(io_failure)?
                .map(|entry| entry.map(|value| value.file_name()))
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(io_failure)?;
            if entries != [OsString::from("launcher-tree-identity-v1")] {
                return Err(fail(
                    EXIT_REJECTED,
                    "Terminal marker-only tombstone inventory is invalid.",
                ));
            }
        }
        for target in tree {
            if target == tombstone || target == marker || target == record {
                continue;
            }
            let metadata = fs::symlink_metadata(&target).map_err(io_failure)?;
            if metadata.is_dir() {
                fs::remove_dir(&target).map_err(io_failure)?;
            } else {
                fs::remove_file(&target).map_err(io_failure)?;
            }
        }
        flush_setup_directory(tombstone)?;
        terminal_maintenance_crash_at("post-tombstone-content-removal");
        if record_present {
            fs::remove_file(&record).map_err(io_failure)?;
            flush_setup_directory(tombstone)?;
            terminal_maintenance_crash_at("post-tombstone-record-removal");
        }
        fs::remove_file(&marker).map_err(io_failure)?;
        flush_setup_directory(tombstone)?;
        terminal_maintenance_crash_at("post-tombstone-marker-removal");
    } else if fs::read_dir(tombstone)
        .map_err(io_failure)?
        .next()
        .transpose()
        .map_err(io_failure)?
        .is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is invalid.",
        ));
    }
    flush_setup_directory(tombstone)?;
    terminal_maintenance_crash_at("post-tombstone-removal");
    Ok(())
}

pub(super) fn recover_terminal_recovery_tombstones(paths: &Paths) -> Result<()> {
    let mut tombstones = Vec::new();
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(TERMINAL_RECOVERY_TOMBSTONE_PREFIX) else {
            continue;
        };
        validate_machine_lock_suffix(generation)?;
        let tombstone = entry.path();
        let marker = tombstone.join("launcher-tree-identity-v1");
        if path_present(&marker)? {
            let record_path = tombstone.join(TERMINAL_UNINSTALL_RECORD_NAME);
            if path_present(&record_path)? {
                let bytes = fs::read(&record_path).map_err(io_failure)?;
                let record: TerminalUninstallRecord = serde_json::from_slice(&bytes)
                    .map_err(|_| fail(EXIT_REJECTED, "Terminal tombstone record is invalid."))?;
                if record.generation != generation
                    || !matches!(
                        record.phase.as_str(),
                        "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
                {
                    return Err(fail(EXIT_REJECTED, "Terminal tombstone record is invalid."));
                }
            }
        }
        remove_terminal_recovery_tombstone(paths, &tombstone)?;
        tombstones.push(tombstone);
    }
    if !tombstones.is_empty() || !path_present(&terminal_uninstall_root(paths))? {
        for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
            let entry = entry.map_err(io_failure)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = name
                .strip_prefix(TERMINAL_FINAL_LAUNCHER_PREFIX)
                .and_then(|value| value.strip_suffix(".exe"))
            else {
                continue;
            };
            validate_machine_lock_suffix(suffix)?;
            let path = entry.path();
            if !marker_security_is_exact(&path, MEDIUM_LAUNCHER_FILE_SDDL)? {
                return Err(fail(EXIT_REJECTED, "Terminal final launcher is invalid."));
            }
            if !pending_deletion_is_owned(&path)? {
                schedule_terminal_service_deletion(&path)?;
            }
        }
        for tombstone in &tombstones {
            if !pending_deletion_is_owned(tombstone)? {
                schedule_empty_terminal_tombstone_deletion(tombstone)?;
            }
        }
        if !tombstones.is_empty() {
            clear_machine_relaunch_owner(paths)?;
        }
        flush_setup_directory(&paths.program_data)?;
    }
    Ok(())
}

pub(super) fn finish_terminal_uninstall(paths: &Paths) -> Result<()> {
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if !matches!(
        record.phase.as_str(),
        "cleanup-complete"
            | "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "maintenance-deleted"
            | "uninstall-unregistered"
            | "journal-removed"
    ) {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall cleanup is not complete.",
        ));
    }
    if path_present(&paths.transaction)? {
        require_uninstall_cleanup_complete(paths)?;
    }
    if !registry_key_absent(&format!(
        r"SYSTEM\CurrentControlSet\Services\{}",
        record.service_name
    ))? || path_present(Path::new(&record.service_image))?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service retirement is incomplete.",
        ));
    }
    let mut machine_lock = Some(MachineLock::acquire(
        paths,
        120_000,
        installed_recovery_policy_epoch(paths)?,
    )?);
    clear_update_recovery(paths)?;
    clear_legacy_profile_relaunch_owners(paths)?;
    remove_uninstall_finalizer_residue(paths)?;
    let root = terminal_uninstall_root(paths);
    remove_plain_tree(&root.join("Relaunch Records"))?;
    require_machine_relaunch_owner(paths)?;
    if record.phase == "cleanup-complete" {
        publish_terminal_final_launcher(paths, &record.generation)?;
        write_terminal_uninstall_phase(paths, &record.generation, "final-launcher-owned")?;
        terminal_maintenance_crash_at("post-final-launcher-ownership");
    }
    let published_phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if published_phase == "final-launcher-owned" {
        terminal_maintenance_crash_at("pre-maintenance-deletion-ownership");
        if hex_hash(&file_hash(&paths.maintenance_uninstaller)?) != record.maintenance_sha256 {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance image is invalid.",
            ));
        }
        schedule_terminal_service_deletion(&paths.maintenance_uninstaller)?;
        write_terminal_uninstall_phase(paths, &record.generation, "maintenance-deletion-owned")?;
        terminal_maintenance_crash_at("post-maintenance-deletion-ownership");
    }
    let mut phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if phase == "maintenance-deletion-owned" {
        if !path_present(&paths.maintenance_uninstaller)?
            || !pending_deletion_is_owned(&paths.maintenance_uninstaller)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance deletion ownership is invalid.",
            ));
        }
        publish_terminal_final_launcher(paths, &record.generation)?;
        unregister_uninstall()?;
        write_terminal_uninstall_phase(paths, &record.generation, "uninstall-unregistered")?;
        terminal_maintenance_crash_at("post-uninstall-unregister");
        phase = "uninstall-unregistered".into();
    }
    if phase == "uninstall-unregistered" {
        remove_transaction(paths)?;
        write_terminal_uninstall_phase(paths, &record.generation, "journal-removed")?;
        terminal_maintenance_crash_at("post-journal-removal");
        phase = "journal-removed".into();
    }
    if phase != "journal-removed" {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal maintenance phase is invalid.",
        ));
    }
    let identity =
        owned_tree_identity(&root).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&root)?
        || !fs::read_to_string(root.join("launcher-tree-identity-v1"))
            .is_ok_and(|value| value == identity)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery root identity is invalid.",
        ));
    }
    let tombstone = terminal_recovery_tombstone(paths, &record.generation)?;
    if path_present(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone already exists.",
        ));
    }
    fs::rename(&root, &tombstone).map_err(io_failure)?;
    flush_setup_directory(&paths.program_data)?;
    terminal_maintenance_crash_at("post-root-tombstone-rename");
    remove_terminal_recovery_tombstone(paths, &tombstone)?;
    if path_present(&paths.maintenance_uninstaller)? {
        arm_mapped_image_deletion(&paths.maintenance_uninstaller)?;
    }
    terminal_maintenance_crash_at("post-maintenance-posix-delete");
    let final_launcher = terminal_final_launcher(paths, &record.generation)?;
    if !pending_deletion_is_owned(&final_launcher)? {
        schedule_terminal_service_deletion(&final_launcher)?;
    }
    if !pending_deletion_is_owned(&tombstone)? {
        schedule_empty_terminal_tombstone_deletion(&tombstone)?;
    }
    if !pending_deletion_is_owned(&final_launcher)? || !pending_deletion_is_owned(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal final deletion ownership is invalid.",
        ));
    }
    terminal_maintenance_crash_at("post-final-deletion-ownership");
    terminal_maintenance_crash_at("post-final-launcher-posix-delete");
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    drop(legacy);
    terminal_maintenance_crash_at("pre-machine-relaunch-owner-clear");
    clear_machine_relaunch_owner(paths)?;
    terminal_maintenance_crash_at("post-machine-relaunch-owner-clear");
    if path_present(&final_launcher)? {
        arm_mapped_image_deletion(&final_launcher)?;
    }
    if path_present(&tombstone)? {
        fs::remove_dir(&tombstone).map_err(io_failure)?;
        flush_setup_directory(&paths.program_data)?;
    }
    terminal_maintenance_crash_at("post-owner-clear-posix-cleanup");
    Ok(())
}

pub(super) fn clear_machine_relaunch_owner(paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Talking Quill Update Relaunch";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(());
    }
    if opened != 0 {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
    }
    let launcher = paths.recovery_launcher.clone();
    let expected = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    let actual = read_registry_value(key, VALUE, 1024)?;
    let final_owner = actual.as_deref().is_some_and(|value| {
        let prefix = format!(
            "\"{}\\{TERMINAL_FINAL_LAUNCHER_PREFIX}",
            paths.program_data.display()
        );
        let suffix = ".exe\" --windows-update-relaunch-owner-v1";
        value.starts_with(&prefix)
            && value.ends_with(suffix)
            && value[prefix.len()..value.len() - suffix.len()].len() == 32
            && value[prefix.len()..value.len() - suffix.len()]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    });
    if actual
        .as_deref()
        .is_some_and(|value| value != expected && !final_owner)
    {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_REJECTED, "Machine relaunch owner was replaced."));
    }
    if actual.is_some() && unsafe { RegDeleteValueW(key, wide(OsStr::new(VALUE)).as_ptr()) } != 0 {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_FAILURE, "Cannot retire machine relaunch owner."));
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot flush machine relaunch retirement.",
        ))
    }
}

pub(super) fn remove_update_recovery_launcher_residue(paths: &Paths) -> Result<()> {
    let launcher = paths.program_data.join("Talking Quill Update Recovery");
    if !path_present(&launcher)? {
        return Ok(());
    }
    let identity =
        owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&launcher)? {
        return Err(fail(
            EXIT_REJECTED,
            "Update recovery launcher identity is invalid.",
        ));
    }
    let marker = launcher.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
    } else {
        // Older setup could fail after creating only a protected temporary copy.
        verify_unpublished_launcher_directory(&launcher)?;
    }
    let mut tree = Vec::new();
    collect_finalizer_deletion_paths(&launcher, &mut tree)?;
    for target in tree {
        if target == launcher {
            continue;
        }
        let metadata = fs::symlink_metadata(&target).map_err(io_failure)?;
        if metadata.is_dir() {
            fs::remove_dir(&target).map_err(io_failure)?;
        } else {
            fs::remove_file(&target).map_err(io_failure)?;
        }
    }
    // Keep the exact protected directory as a non-squattable namespace until HKLM Run is
    // flushed. Its identity was retained above and no executable remains in it.
    if owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
        != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update launcher directory was replaced.",
        ));
    }
    Ok(())
}

pub(super) fn relocated_uninstall_matches_maintenance(
    target: &Path,
    paths: &Paths,
) -> Result<bool> {
    let canonical_target = std::fs::canonicalize(target).map_err(io_failure)?;
    let canonical_temp = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = canonical_target
        .file_name()
        .and_then(|value| value.to_str());
    Ok(canonical_target.parent() == Some(canonical_temp.as_path())
        && name.is_some_and(|value| {
            value.starts_with(".TalkingQuill-uninstall-")
                && value.ends_with(".exe")
                && value.len() == ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        })
        && path_present(&paths.maintenance_uninstaller)?
        && file_hash(target)? == file_hash(&paths.maintenance_uninstaller)?)
}

pub(super) fn collect_finalizer_deletion_paths(
    path: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(path).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(io_failure)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Finalizer tree contains a reparse point.",
            ));
        }
        if metadata.is_dir() {
            collect_finalizer_deletion_paths(&child, output)?;
        } else if metadata.is_file() {
            output.push(child);
        } else {
            return Err(fail(EXIT_REJECTED, "Finalizer tree entry type is invalid."));
        }
    }
    output.push(path.to_path_buf());
    Ok(())
}
