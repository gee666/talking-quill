//! Relaunch ownership and terminal cleanup recovery.
use super::*;

pub(super) fn relaunch_run_command(launcher: &Path) -> Result<String, i32> {
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    if command.encode_utf16().count() > 260 {
        Err(EXIT_LAUNCH_FAILED)
    } else {
        Ok(command)
    }
}

pub(super) fn install_machine_relaunch_owner() -> Result<(), i32> {
    // Fixed global order: verified legacy mutex pair, then protected machine file lock.
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    if terminal_uninstall_record()?.is_some() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let launcher = ensure_medium_launcher(&current)?;
    let root = relaunch_root()?;
    if !root.exists() {
        create_directory_with_sddl(&root, RELAUNCH_ROOT_SDDL)?;
        apply_restricted_dacl(&root, RELAUNCH_ROOT_SDDL)?;
        flush_directory(root.parent().ok_or(EXIT_IDENTITY_MISMATCH)?)?;
    }
    let command = relaunch_run_command(&launcher)?;
    set_machine_relaunch_run_value(&command)
}

pub(super) fn verify_machine_relaunch_owner() -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let final_launcher = current.parent() == Some(program_data.as_path())
        && current
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(".Talking Quill Terminal Relaunch-"))
            .and_then(|suffix| suffix.strip_suffix(".exe"))
            .is_some_and(|generation| validate_generation(generation).is_ok());
    let expected_path = if final_launcher {
        current
    } else {
        medium_launcher_path()?
    };
    let expected = relaunch_run_command(&expected_path)?;
    let actual = read_registry_string(key, RELAUNCH_RUN_VALUE)?;
    unsafe { RegCloseKey(key) };
    if actual.as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn finish_terminal_without_maintenance() -> Result<(), i32> {
    let Some(record) = terminal_uninstall_record()? else {
        return Ok(());
    };
    if !matches!(
        record.phase.as_str(),
        "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "uninstall-unregistered"
            | "journal-removed"
    ) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let generation = current
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".Talking Quill Terminal Relaunch-"))
        .and_then(|suffix| suffix.strip_suffix(".exe"))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    if generation != record.generation {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let uninstall = wide_nul(Path::new(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill",
    ))?;
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, uninstall.as_ptr()) };
    if status != 0 && status != 2 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let transaction = program_files.join(".Talking Quill.native-transaction-v2.json");
    if transaction.exists() {
        std::fs::remove_file(transaction).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let root = known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery");
    if root.exists() {
        let tombstone = known_folder(&FOLDERID_ProgramData)?
            .join(format!(".Talking Quill.recovery-tombstone-{generation}"));
        if tombstone.exists() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        std::fs::rename(root, tombstone).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

pub(super) fn cleanup_terminal_recovery_tombstones() -> Result<Vec<PathBuf>, i32> {
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let mut tombstones = Vec::new();
    let mut maintenance_images = Vec::new();
    for entry in std::fs::read_dir(&program_data).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(".Talking Quill.recovery-tombstone-") else {
            continue;
        };
        validate_generation(generation)?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, RELAUNCH_ROOT_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let marker = path.join("launcher-tree-identity-v1");
        if marker.exists() {
            let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
            if !std::fs::read_to_string(&marker).is_ok_and(|value| value == identity) {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            let record_path = path.join("terminal-uninstall-record-v1.json");
            let mut inventory = std::fs::read_dir(&path)
                .map_err(|_| EXIT_LAUNCH_FAILED)?
                .map(|entry| {
                    entry
                        .map(|value| value.file_name().to_string_lossy().into_owned())
                        .map_err(|_| EXIT_LAUNCH_FAILED)
                })
                .collect::<Result<Vec<_>, _>>()?;
            inventory.sort();
            if record_path.exists() {
                let launcher_names = inventory
                    .iter()
                    .filter(|name| {
                        name.strip_prefix(RECOVERY_LAUNCHER_PUBLISHED_PREFIX)
                            .and_then(|value| value.strip_suffix(".exe"))
                            .is_some_and(|value| validate_generation(value).is_ok())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut expected = vec![
                    "launcher-tree-identity-v1".to_owned(),
                    "terminal-uninstall-record-v1.json".to_owned(),
                ];
                expected.extend(launcher_names.iter().cloned());
                expected.sort();
                if inventory != expected {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                let record: TerminalUninstallRecord = serde_json::from_slice(
                    &std::fs::read(&record_path).map_err(|_| EXIT_IDENTITY_MISMATCH)?,
                )
                .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
                if record.generation != generation
                    || !matches!(
                        record.phase.as_str(),
                        "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
                {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                maintenance_images.push((
                    maintenance_path_from_command(&record.uninstall_command)?,
                    decode_hash(&record.maintenance_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?,
                ));
                for launcher_name in launcher_names {
                    std::fs::remove_file(path.join(launcher_name))
                        .map_err(|_| EXIT_LAUNCH_FAILED)?;
                }
                std::fs::remove_file(&record_path).map_err(|_| EXIT_LAUNCH_FAILED)?;
            } else {
                let current_generation = std::env::current_exe()
                    .ok()
                    .and_then(|value| value.file_name().map(|name| name.to_owned()))
                    .and_then(|name| name.to_str().map(str::to_owned))
                    .and_then(|name| {
                        name.strip_prefix(".Talking Quill Terminal Relaunch-")
                            .and_then(|suffix| suffix.strip_suffix(".exe"))
                            .map(str::to_owned)
                    });
                if inventory != ["launcher-tree-identity-v1"]
                    || current_generation.as_deref() != Some(generation)
                {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
            }
            std::fs::remove_file(&marker).map_err(|_| EXIT_LAUNCH_FAILED)?;
            tombstones.push(path.clone());
        } else if std::fs::read_dir(&path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?
            .next()
            .is_none()
        {
            tombstones.push(path.clone());
        } else {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    for (maintenance, expected) in maintenance_images {
        if !maintenance.exists() {
            continue;
        }
        let mut file = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(file);
        let metadata = std::fs::symlink_metadata(&maintenance).map_err(|_| EXIT_LAUNCH_FAILED)?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        std::fs::remove_file(maintenance).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(tombstones)
}

pub(super) fn normalized_pending_delete_source(value: &str) -> String {
    let replaced = value.replace('/', "\\");
    replaced
        .strip_prefix(r"\??\")
        .or_else(|| replaced.strip_prefix(r"\\?\"))
        .unwrap_or(&replaced)
        .to_ascii_lowercase()
}

pub(super) fn decode_pending_delete_pairs(data: &[u16]) -> Result<Vec<(String, String)>, i32> {
    let mut pairs = Vec::new();
    let mut cursor = 0;
    let mut terminated = false;
    while cursor < data.len() {
        let source_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if cursor == source_start {
            let required = if pairs.is_empty() { 2 } else { 1 };
            if data.len() - cursor < required || data[cursor..].iter().any(|value| *value != 0) {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            terminated = true;
            break;
        }
        let source =
            String::from_utf16(&data[source_start..cursor]).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        cursor += 1;
        let destination_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let destination = String::from_utf16(&data[destination_start..cursor])
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        cursor += 1;
        pairs.push((source, destination));
    }
    if terminated {
        Ok(pairs)
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn pending_delete_owned(path: &Path) -> Result<bool, i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(
                r"SYSTEM\CurrentControlSet\Control\Session Manager",
            ))?
            .as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new("PendingFileRenameOperations"))?;
    let mut bytes = 0_u32;
    let mut value_type = 0_u32;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if queried == 2 {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    if queried != 0
        || value_type != REG_MULTI_SZ
        || bytes == 0
        || bytes > 1024 * 1024
        || !bytes.is_multiple_of(2)
    {
        unsafe { RegCloseKey(key) };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let capacity = bytes;
    let mut actual = bytes;
    let mut actual_type = 0_u32;
    let mut data = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut actual_type,
            data.as_mut_ptr().cast(),
            &mut actual,
        )
    } != 0
        || actual_type != REG_MULTI_SZ
        || actual > capacity
        || !actual.is_multiple_of(2)
    {
        unsafe { RegCloseKey(key) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    unsafe { RegCloseKey(key) };
    data.truncate(actual as usize / 2);
    let pairs = decode_pending_delete_pairs(&data)?;
    let expected = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .to_ascii_lowercase();
    Ok(pairs.iter().any(|(source, destination)| {
        destination.is_empty() && normalized_pending_delete_source(source) == expected
    }))
}

pub(super) fn schedule_and_verify_pending_delete(path: &Path) -> Result<(), i32> {
    if !pending_delete_owned(path)?
        && unsafe {
            MoveFileExW(
                wide_nul(path)?.as_ptr(),
                std::ptr::null(),
                MOVEFILE_DELAY_UNTIL_REBOOT,
            )
        } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if pending_delete_owned(path)? {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn retire_no_work_machine_relaunch_owner() -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    finish_terminal_without_maintenance()?;
    let tombstones = cleanup_terminal_recovery_tombstones()?;
    if terminal_uninstall_record()?.is_some()
        || journal_owned_terminal_maintenance()?.is_some()
        || !relaunch_generations()?.is_empty()
        || known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill")
            .exists()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    schedule_and_verify_pending_delete(&current)?;
    for tombstone in &tombstones {
        schedule_and_verify_pending_delete(tombstone)?;
    }
    machine_lifecycle.retire()?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let deleted =
        unsafe { RegDeleteValueW(key, wide_nul(Path::new(RELAUNCH_RUN_VALUE))?.as_ptr()) };
    let flushed = deleted == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(EXIT_LAUNCH_FAILED);
    }
    for tombstone in tombstones {
        if tombstone.exists() {
            std::fs::remove_dir(tombstone).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    Ok(0)
}
