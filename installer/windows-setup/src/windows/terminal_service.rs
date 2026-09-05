//! Install, verify, execute, and retire the final cleanup service.
use super::*;

pub(super) fn terminal_service_image(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_SERVICE_IMAGE_PREFIX}{generation}.exe")))
}

pub(super) fn terminal_service_command(paths: &Paths, generation: &str) -> Result<String> {
    Ok(format!(
        "\"{}\" /TQ-TERMINAL-SERVICE={generation}",
        terminal_service_image(paths, generation)?.display()
    ))
}

pub(super) fn publish_terminal_service_image(
    paths: &Paths,
    source: &Path,
    generation: &str,
) -> Result<(String, String)> {
    assert_plain_file(source)?;
    let pending = paths.program_data.join(format!(
        "{TERMINAL_SERVICE_PENDING_PREFIX}{}.exe",
        random_machine_lock_suffix()?
    ));
    let published = terminal_service_image(paths, generation)?;
    assert_plain_absent(&pending)?;
    assert_plain_absent(&published)?;
    fs::copy(source, &pending).map_err(io_failure)?;
    apply_lock_dacl(&pending, TERMINAL_SERVICE_FILE_SDDL)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pending)
        .map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let file_identity = file_identity_text(&file)?;
    drop(file);
    let sha256 = hex_hash(&file_hash(&pending)?);
    if sha256 != hex_hash(&file_hash(source)?) {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service image changed during publication.",
        ));
    }
    durable_rename(&pending, &published)?;
    flush_setup_directory(&paths.program_data)?;
    assert_plain_file(&published)?;
    let published_file = File::open(&published).map_err(io_failure)?;
    let published_identity = file_identity_text(&published_file)?;
    drop(published_file);
    if published_identity != file_identity
        || !marker_security_is_exact(&published, TERMINAL_SERVICE_FILE_SDDL)?
        || file_hash(&published)? != file_hash(source)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service publication is invalid.",
        ));
    }
    Ok((sha256, file_identity))
}

mod security;
pub(super) use security::*;

pub(super) fn install_terminal_service(
    paths: &Paths,
    record: &TerminalUninstallRecord,
    start: bool,
) -> Result<()> {
    let manager = unsafe {
        OpenSCManagerW(
            ptr::null(),
            ptr::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open the service manager."));
    }
    // Own the SCM handle before any fallible command construction.
    let manager = ServiceHandle(manager);
    let command = terminal_service_command(paths, &record.generation)?;
    let mut service = unsafe {
        CreateServiceW(
            manager.0,
            wide(OsStr::new(&record.service_name)).as_ptr(),
            wide(OsStr::new(&record.service_name)).as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            wide(OsStr::new(&command)).as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    };
    if service.is_null() && unsafe { GetLastError() } == 1073 {
        service = unsafe {
            OpenServiceW(
                manager.0,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_ALL_ACCESS,
            )
        };
    }
    if service.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create or open terminal cleanup service.",
        ));
    }
    let service = ServiceHandle(service);
    apply_terminal_service_dacl(service.0)
        .and_then(|()| configure_terminal_service_restarts(service.0))
        .and_then(|()| {
            if start
                && unsafe { StartServiceW(service.0, 0, ptr::null()) } == 0
                && unsafe { GetLastError() } != 1056
            {
                Err(fail(EXIT_FAILURE, "Cannot start terminal cleanup service."))
            } else {
                Ok(())
            }
        })
}

pub(super) fn resume_terminal_service(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<()> {
    let image = PathBuf::from(&record.service_image);
    validate_terminal_service_image(record, &image)?;
    install_terminal_service(paths, record, true)?;
    verify_terminal_service_registration(paths, record)
}

pub(super) fn terminal_uninstall_commands(paths: &Paths) -> (String, String) {
    let executable = format!("\"{}\"", paths.maintenance_uninstaller.display());
    (executable.clone(), format!("{executable} /S"))
}

pub(super) fn reclaim_unpublished_terminal_service_images(paths: &Paths) -> Result<()> {
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let suffix = name
            .strip_prefix(TERMINAL_SERVICE_IMAGE_PREFIX)
            .or_else(|| name.strip_prefix(TERMINAL_SERVICE_PENDING_PREFIX));
        let Some(suffix) = suffix else {
            continue;
        };
        let generation = suffix.strip_suffix(".exe").unwrap_or("");
        if validate_machine_lock_suffix(generation).is_err() {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal service namespace is invalid.",
            ));
        }
        let path = entry.path();
        assert_plain_file(&path)?;
        if name.starts_with(TERMINAL_SERVICE_IMAGE_PREFIX)
            && terminal_service_exists(&terminal_service_name(generation)?)?
        {
            self_retire_absent_terminal_service(paths, generation, &path)?;
        }
        if !marker_security_is_exact(&path, TERMINAL_SERVICE_FILE_SDDL)? {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal service residue is unprotected.",
            ));
        }
        fs::remove_file(path).map_err(io_failure)?;
    }
    flush_setup_directory(&paths.program_data)
}

pub(super) fn publish_terminal_uninstall_record(paths: &Paths, current: &Path) -> Result<String> {
    require_machine_relaunch_owner(paths)?;
    reclaim_unpublished_terminal_service_images(paths)?;
    let generation = random_machine_lock_suffix()?;
    let (service_sha256, service_file_identity) =
        publish_terminal_service_image(paths, current, &generation)?;
    let (uninstall_command, quiet_uninstall_command) = terminal_uninstall_commands(paths);
    let record = TerminalUninstallRecord {
        schema_version: 3,
        generation: generation.clone(),
        phase: "armed".into(),
        maintenance_sha256: hex_hash(&file_hash(&paths.maintenance_uninstaller)?),
        uninstall_command,
        quiet_uninstall_command,
        service_name: terminal_service_name(&generation)?,
        service_image: terminal_service_image(paths, &generation)?
            .to_string_lossy()
            .into_owned(),
        service_sha256,
        service_file_identity,
        record_file_identity: String::new(),
    };
    terminal_maintenance_crash_at("pre-CreateService");
    install_terminal_service(paths, &record, false)?;
    terminal_maintenance_crash_at("post-service-pre-record");
    write_terminal_uninstall_record(paths, &record)?;
    terminal_maintenance_crash_at("post-record-pre-start");
    let published = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    install_terminal_service(paths, &published, true)?;
    Ok(generation)
}

pub(super) fn write_terminal_uninstall_phase(
    paths: &Paths,
    generation: &str,
    phase: &str,
) -> Result<()> {
    let mut record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation
        || !matches!(
            phase,
            "armed"
                | "machine-retired"
                | "cleanup-complete"
                | "final-launcher-owned"
                | "maintenance-deletion-owned"
                | "maintenance-deleted"
                | "uninstall-unregistered"
                | "journal-removed"
        )
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    record.phase = phase.into();
    write_terminal_uninstall_record(paths, &record)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminalUninstallRecoveryStep {
    RetireMachine,
    FinishCleanup,
    CleanupComplete,
}

pub(super) fn terminal_uninstall_recovery_step(
    phase: &str,
    journal_present: bool,
) -> Result<TerminalUninstallRecoveryStep> {
    match (phase, journal_present) {
        ("armed", true) => Ok(TerminalUninstallRecoveryStep::RetireMachine),
        ("machine-retired", true) => Ok(TerminalUninstallRecoveryStep::FinishCleanup),
        (
            "cleanup-complete"
            | "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "maintenance-deleted"
            | "uninstall-unregistered"
            | "journal-removed",
            _,
        ) => Ok(TerminalUninstallRecoveryStep::CleanupComplete),
        _ => Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall recovery phase is invalid.",
        )),
    }
}

pub(super) fn validate_terminal_service_image(
    record: &TerminalUninstallRecord,
    current: &Path,
) -> Result<()> {
    let file = File::open(current).map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    if canonical(current)? != canonical(Path::new(&record.service_image))?
        || record.service_name != terminal_service_name(&record.generation)?
        || hex_hash(&file_hash(current)?) != record.service_sha256
        || identity != record.service_file_identity
        || !marker_security_is_exact(current, TERMINAL_SERVICE_FILE_SDDL)?
    {
        return Err(fail(EXIT_REJECTED, "Terminal service image is invalid."));
    }
    Ok(())
}

mod registration;
pub(super) use registration::*;

mod cleanup;
pub(super) use cleanup::*;
