//! Installation path and maintenance generation selection.
use super::*;

pub(in super::super) fn maintenance_generation_from_name(name: &str) -> Option<&str> {
    name.strip_prefix("Talking Quill Maintenance-")
        .and_then(|value| value.strip_suffix(".exe"))
        .filter(|generation| validate_machine_lock_suffix(generation).is_ok())
}

pub(in super::super) fn registered_maintenance_generation(
    program_files: &Path,
) -> Result<Option<String>> {
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

pub(in super::super) fn current_install_generation(
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

pub(in super::super) fn paths() -> Result<Paths> {
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
