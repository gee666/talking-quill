//! Terminal recovery tombstone validation and recovery.
use super::*;

pub(in super::super) fn terminal_recovery_tombstone(
    paths: &Paths,
    generation: &str,
) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_RECOVERY_TOMBSTONE_PREFIX}{generation}")))
}

pub(in super::super) fn remove_terminal_recovery_tombstone(
    _paths: &Paths,
    tombstone: &Path,
) -> Result<()> {
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

pub(in super::super) fn recover_terminal_recovery_tombstones(paths: &Paths) -> Result<()> {
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
