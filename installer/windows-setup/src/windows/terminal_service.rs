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

pub(super) fn terminal_service_dacl_is_exact(service: SC_HANDLE) -> Result<bool> {
    let mut needed = 0;
    unsafe {
        QueryServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 || needed > 64 * 1024 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect terminal service ACL."));
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        QueryServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(fail(EXIT_REJECTED, "Cannot read terminal service ACL."));
    }
    let expected_wide = wide(OsStr::new(TERMINAL_SERVICE_SDDL));
    let mut expected = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            expected_wide.as_ptr(),
            SDDL_REVISION_1,
            &mut expected,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create terminal service ACL."));
    }
    let convert = |descriptor: *mut c_void| -> Result<String> {
        let mut text = ptr::null_mut();
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot normalize terminal service ACL.",
            ));
        }
        let value = unsafe { wide_ptr_string(text) };
        unsafe { LocalFree(text.cast()) };
        Ok(value)
    };
    let expected_text = convert(expected)?;
    unsafe { LocalFree(expected) };
    Ok(convert(actual.as_mut_ptr().cast())? == expected_text)
}

pub(super) fn apply_terminal_service_dacl(service: SC_HANDLE) -> Result<()> {
    let sddl = wide(OsStr::new(TERMINAL_SERVICE_SDDL));
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
        return Err(fail(EXIT_FAILURE, "Cannot create terminal service ACL."));
    }
    let applied = unsafe {
        SetServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    } != 0;
    unsafe { LocalFree(descriptor) };
    if applied && terminal_service_dacl_is_exact(service)? {
        Ok(())
    } else {
        Err(fail(EXIT_FAILURE, "Cannot protect terminal service."))
    }
}

pub(super) fn configure_terminal_service_restarts(service: SC_HANDLE) -> Result<()> {
    let mut actions = [SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: 60_000,
    }; 3];
    let failure_actions = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: ptr::null_mut(),
        lpCommand: ptr::null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            (&failure_actions as *const SERVICE_FAILURE_ACTIONSW).cast(),
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot configure terminal service recovery.",
        ));
    }
    let non_crash = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&non_crash as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        )
    } == 0
    {
        Err(fail(
            EXIT_FAILURE,
            "Cannot enable terminal service failure recovery.",
        ))
    } else {
        Ok(())
    }
}

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
    let command = terminal_service_command(paths, &record.generation)?;
    let mut service = unsafe {
        CreateServiceW(
            manager,
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
                manager,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_ALL_ACCESS,
            )
        };
    }
    if service.is_null() {
        unsafe { CloseServiceHandle(manager) };
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create or open terminal cleanup service.",
        ));
    }
    let result = apply_terminal_service_dacl(service)
        .and_then(|()| configure_terminal_service_restarts(service))
        .and_then(|()| {
            if start
                && unsafe { StartServiceW(service, 0, ptr::null()) } == 0
                && unsafe { GetLastError() } != 1056
            {
                Err(fail(EXIT_FAILURE, "Cannot start terminal cleanup service."))
            } else {
                Ok(())
            }
        });
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    result
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

pub(super) fn terminal_service_exists(name: &str) -> Result<bool> {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal service manager.",
        ));
    }
    let service = unsafe {
        OpenServiceW(
            manager,
            wide(OsStr::new(name)).as_ptr(),
            SERVICE_QUERY_STATUS,
        )
    };
    let error = if service.is_null() {
        unsafe { GetLastError() }
    } else {
        0
    };
    if !service.is_null() {
        unsafe { CloseServiceHandle(service) };
    }
    unsafe { CloseServiceHandle(manager) };
    if error == 0 {
        Ok(true)
    } else if error == 1060 {
        Ok(false)
    } else {
        Err(fail(EXIT_FAILURE, "Cannot inspect terminal service."))
    }
}

pub(super) fn terminal_service_handle(
    record: &TerminalUninstallRecord,
    access: u32,
) -> Result<(ServiceHandle, ServiceHandle)> {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open terminal service manager."));
    }
    let manager = ServiceHandle(manager);
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide(OsStr::new(&record.service_name)).as_ptr(),
            access,
        )
    };
    if service.is_null() {
        return Err(fail(EXIT_REJECTED, "Terminal cleanup service is missing."));
    }
    Ok((manager, ServiceHandle(service)))
}

pub(super) fn terminal_service_restarts_are_exact(service: SC_HANDLE) -> Result<bool> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 || needed > 64 * 1024 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect terminal service recovery.",
        ));
    }
    let mut storage = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service recovery.",
        ));
    }
    let actions = unsafe { &*storage.as_ptr().cast::<SERVICE_FAILURE_ACTIONSW>() };
    if actions.dwResetPeriod != 86_400 || actions.cActions != 3 || actions.lpsaActions.is_null() {
        return Ok(false);
    }
    let actions = unsafe { std::slice::from_raw_parts(actions.lpsaActions, 3) };
    if !actions
        .iter()
        .all(|action| action.Type == SC_ACTION_RESTART && action.Delay == 60_000)
    {
        return Ok(false);
    }
    let mut flag: SERVICE_FAILURE_ACTIONS_FLAG = unsafe { mem::zeroed() };
    let mut flag_needed = 0;
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&mut flag as *mut SERVICE_FAILURE_ACTIONS_FLAG).cast(),
            mem::size_of::<SERVICE_FAILURE_ACTIONS_FLAG>() as u32,
            &mut flag_needed,
        )
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service failure flag.",
        ));
    }
    Ok(flag.fFailureActionsOnNonCrashFailures != 0)
}

pub(super) fn verify_terminal_service_registration_mode(
    paths: &Paths,
    record: &TerminalUninstallRecord,
    require_hardening: bool,
) -> Result<()> {
    let (_manager, service) =
        terminal_service_handle(record, SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS)?;
    let mut needed = 0;
    unsafe { QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect terminal service identity.",
        ));
    }
    let mut storage = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe { QueryServiceConfigW(service.0, storage.as_mut_ptr().cast(), needed, &mut needed) }
        == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service identity.",
        ));
    }
    let config = unsafe { &*storage.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    let binary = unsafe { wide_ptr_string(config.lpBinaryPathName) };
    let account = if config.lpServiceStartName.is_null() {
        String::new()
    } else {
        unsafe { wide_ptr_string(config.lpServiceStartName) }
    };
    if config.dwServiceType != SERVICE_WIN32_OWN_PROCESS
        || config.dwStartType != SERVICE_AUTO_START
        || config.dwErrorControl != SERVICE_ERROR_NORMAL
        || !account.eq_ignore_ascii_case("LocalSystem")
        || binary != terminal_service_command(paths, &record.generation)?
        || (require_hardening && !terminal_service_dacl_is_exact(service.0)?)
        || (require_hardening && !terminal_service_restarts_are_exact(service.0)?)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service registration is invalid.",
        ));
    }
    Ok(())
}

pub(super) fn verify_terminal_service_registration(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<()> {
    verify_terminal_service_registration_mode(paths, record, true)
}

pub(super) fn registry_key_absent(path: &str) -> Result<bool> {
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

pub(super) fn self_retire_absent_terminal_service(
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

pub(super) fn finish_terminal_service_cleanup(paths: &Paths) -> Result<()> {
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

pub(super) fn service_cleanup_topology_is_complete(
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

pub(super) fn run_terminal_cleanup_service(generation: &str) -> Result<()> {
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

pub(super) fn remove_retired_terminal_service_image(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<bool> {
    let image = PathBuf::from(&record.service_image);
    if !path_present(&image)? {
        return Ok(true);
    }
    validate_terminal_service_image(record, &image)?;
    if terminal_force_pending_delete() {
        schedule_terminal_service_deletion(&image)?;
        return Ok(false);
    }
    match fs::remove_file(&image) {
        Ok(()) => {
            flush_setup_directory(&paths.program_data)?;
            Ok(true)
        }
        Err(_) => {
            // Registration is already absent. A reboot may now own deletion of only this inert,
            // authenticated image; maintenance and the journal remain callable until it is gone.
            schedule_terminal_service_deletion(&image)?;
            Ok(false)
        }
    }
}

pub(super) fn wait_for_terminal_service_retirement(paths: &Paths, generation: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(240);
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    loop {
        let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
        if manager.is_null() {
            return Err(fail(EXIT_FAILURE, "Cannot poll terminal cleanup service."));
        }
        let service = unsafe {
            OpenServiceW(
                manager,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG | DELETE,
            )
        };
        if service.is_null() {
            let error = unsafe { GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            if error == 1072 {
                if Instant::now() >= deadline {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Terminal service deletion did not commit.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            if error != 1060 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot inspect terminal cleanup service.",
                ));
            }
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if !complete {
                return Err(fail(
                    EXIT_REJECTED,
                    "Terminal service disappeared before cleanup completed.",
                ));
            }
            terminal_maintenance_crash_at("post-delete-pre-image-removal");
            if !remove_retired_terminal_service_image(paths, &record)? {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal service image deletion requires reboot.",
                ));
            }
            return finish_terminal_uninstall(paths);
        }
        let service = ServiceHandle(service);
        let manager = ServiceHandle(manager);
        let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(fail(EXIT_FAILURE, "Cannot query terminal cleanup service."));
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if status.dwWin32ExitCode == 0 && complete {
                verify_terminal_service_registration(paths, &record)?;
                terminal_maintenance_crash_at("service-stopped-pre-DeleteService");
                if unsafe { DeleteService(service.0) } == 0 && unsafe { GetLastError() } != 1072 {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Cannot delete terminal cleanup service.",
                    ));
                }
                drop(service);
                drop(manager);
                continue;
            }
            if status.dwWin32ExitCode == ERROR_SERVICE_SPECIFIC_ERROR {
                // Failure actions are configured for non-crash failures. Poll through the bounded
                // SCM restart interval without converting the test into a manual restart.
            } else if status.dwWin32ExitCode != 0 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal cleanup service stopped unexpectedly.",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(fail(
                EXIT_FAILURE,
                "Terminal cleanup service did not complete.",
            ));
        }
        drop(service);
        drop(manager);
        std::thread::sleep(Duration::from_millis(250));
    }
}
