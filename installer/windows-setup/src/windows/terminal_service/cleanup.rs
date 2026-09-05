//! Terminal cleanup execution and service retirement.
use super::*;

pub(in super::super) fn registry_key_absent(path: &str) -> Result<bool> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(path)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 0 {
        unsafe { RegCloseKey(key) };
        Ok(false)
    } else if status == 2 {
        Ok(true)
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal cleanup registry.",
        ))
    }
}

pub(in super::super) fn self_retire_absent_terminal_service(
    paths: &Paths,
    generation: &str,
    current: &Path,
) -> Result<()> {
    let expected = terminal_service_image(paths, generation)?;
    if canonical(current)? != canonical(&expected)?
        || !marker_security_is_exact(current, TERMINAL_SERVICE_FILE_SDDL)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Recordless terminal service image is invalid.",
        ));
    }
    let file = File::open(current).map_err(io_failure)?;
    let record = TerminalUninstallRecord {
        schema_version: 3,
        generation: generation.to_owned(),
        phase: "armed".into(),
        maintenance_sha256: String::new(),
        uninstall_command: String::new(),
        quiet_uninstall_command: String::new(),
        service_name: terminal_service_name(generation)?,
        service_image: expected.to_string_lossy().into_owned(),
        service_sha256: hex_hash(&file_hash(current)?),
        service_file_identity: file_identity_text(&file)?,
        record_file_identity: String::new(),
    };
    drop(file);
    verify_terminal_service_registration_mode(paths, &record, false)?;
    let (_manager, service) = terminal_service_handle(&record, DELETE | SERVICE_QUERY_CONFIG)?;
    if unsafe { DeleteService(service.0) } == 0 && unsafe { GetLastError() } != 1072 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retire recordless terminal service.",
        ));
    }
    Ok(())
}

pub(in super::super) fn finish_terminal_service_cleanup(paths: &Paths) -> Result<()> {
    require_uninstall_cleanup_complete(paths)?;
    clear_update_recovery(paths)?;
    clear_legacy_profile_relaunch_owners(paths)?;
    remove_plain_tree(&terminal_uninstall_root(paths).join("Relaunch Records"))?;
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    remove_uninstall_finalizer_residue(paths)?;
    flush_setup_directory(&paths.program_data)
}

pub(in super::super) fn service_cleanup_topology_is_complete(
    paths: &Paths,
    generation: &str,
) -> Result<bool> {
    require_uninstall_cleanup_complete(paths)?;
    require_machine_relaunch_owner(paths)?;
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation || record.phase != "cleanup-complete" {
        return Ok(false);
    }
    for path in [&paths.install, &paths.staging, &paths.backup] {
        if path_present(path)? {
            return Ok(false);
        }
    }
    if !registry_key_absent(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe",
    )? || registry_key_absent(UNINSTALL_KEY)?
        || registry_key_absent(MACHINE_LOCK_REGISTRY_KEY)?
    {
        return Ok(false);
    }
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let name = entry.map_err(io_failure)?.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(UNINSTALL_FINALIZER_PREFIX)
            || name.starts_with(UNINSTALL_FINALIZER_PENDING_PREFIX)
            || name.starts_with(MACHINE_LOCK_PENDING_PREFIX)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(in super::super) fn run_terminal_cleanup_service(generation: &str) -> Result<()> {
    let paths = paths()?;
    let current = std::env::current_exe().map_err(io_failure)?;
    let Some(record) = read_terminal_uninstall_record(&paths)? else {
        return self_retire_absent_terminal_service(&paths, generation, &current);
    };
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    validate_terminal_service_image(&record, &current)?;
    verify_terminal_service_registration(&paths, &record)?;
    terminal_service_fail_once("failure-action-restart")?;
    let step = terminal_uninstall_recovery_step(&record.phase, path_present(&paths.transaction)?)?;
    if step == TerminalUninstallRecoveryStep::CleanupComplete {
        if !service_cleanup_topology_is_complete(&paths, generation)? {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal cleanup topology is incomplete.",
            ));
        }
        return Ok(());
    }
    let mut machine_lock = Some(MachineLock::acquire(
        &paths,
        120_000,
        installed_recovery_policy_epoch(&paths)?,
    )?);
    if step == TerminalUninstallRecoveryStep::RetireMachine {
        retire_terminal_machine_state(&paths, &WindowsNativeSystem, &current, generation)?;
    }
    let retired = read_terminal_uninstall_record(&paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if terminal_uninstall_recovery_step(&retired.phase, path_present(&paths.transaction)?)?
        != TerminalUninstallRecoveryStep::FinishCleanup
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall machine state is not retired.",
        ));
    }
    finish_terminal_service_cleanup(&paths)?;
    drop(machine_lock.take());
    write_terminal_uninstall_phase(&paths, generation, "cleanup-complete")?;
    if !service_cleanup_topology_is_complete(&paths, generation)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal cleanup topology is incomplete.",
        ));
    }
    Ok(())
}

mod retirement;
pub(in super::super) use retirement::*;
