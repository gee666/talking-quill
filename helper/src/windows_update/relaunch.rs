//! Update entry points, durable relaunch, and app-ready acknowledgement.
use super::*;

pub(super) fn run_recovery_launcher_argument_inner(argument: &std::ffi::OsStr) -> Result<u32, i32> {
    let argument = argument.to_str().ok_or(EXIT_INVALID_REQUEST)?;
    if argument.starts_with("--windows-update-bootstrap-v2=") {
        authorize_public_update_bootstrap(argument)?;
    }
    if let Some(generation) = argument.strip_prefix("--windows-update-relaunch-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        validate_generation(generation)?;
        return retire_stale_legacy_relaunch(generation);
    }
    if argument == "--windows-update-relaunch-owner-v1" {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return run_machine_relaunch_owner();
    }
    if argument == "--windows-update-relaunch-owner-retire-v1" {
        if !is_elevated() {
            return Err(EXIT_NOT_ELEVATED);
        }
        return retire_no_work_machine_relaunch_owner();
    }
    let generation = if let Some(generation) = argument.strip_prefix("--windows-update-resume-v2=")
    {
        validate_generation(generation)?;
        generation
    } else if let Some(binding) = argument.strip_prefix("--windows-update-cleanup-v1=") {
        let (_, generation) = binding.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
        validate_generation(generation)?;
        generation
    } else {
        return Err(EXIT_INVALID_REQUEST);
    };
    let cleanup_binding = argument.strip_prefix("--windows-update-cleanup-v1=");
    if !is_elevated() {
        let directory = medium_recovery_directory(generation)?;
        if cleanup_binding.is_some() && !directory.exists() {
            if !recovery_command_present(generation)? {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
        } else {
            let attempt = begin_protected_visible_retry(&directory, generation)?;
            if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
                show_visible_retry_paused();
                return Err(EXIT_LAUNCH_FAILED);
            }
        }
        launch_elevated_bootstrap(argument)?;
        if cleanup_binding.is_none() {
            launch_program_files_application_for_generation(generation)?;
        }
        return Ok(0);
    }
    if let Some(binding) = cleanup_binding {
        let _state = RecoveryStateLock::acquire()?;
        let directory = medium_recovery_directory(generation)?;
        if directory.exists() {
            let directory = find_recovery_directory(generation)?;
            let attempt =
                protected_visible_attempt(&directory, generation).ok_or(EXIT_IDENTITY_MISMATCH)?;
            if attempt == 0 {
                begin_protected_visible_retry(&directory, generation)?;
            } else if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
        } else if !recovery_command_present(generation)? {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let generation = run_native_cleanup(binding)?;
        clear_restart_recovery(&generation)?;
        return Ok(0);
    }
    let state = RecoveryStateLock::acquire()?;
    let directory = find_recovery_directory(generation)?;
    let attempt =
        protected_visible_attempt(&directory, generation).ok_or(EXIT_IDENTITY_MISMATCH)?;
    if attempt == 0 {
        begin_protected_visible_retry(&directory, generation)?;
    } else if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(state);
    let staged = directory.join("talking-quill-update-bootstrap.exe");
    let _retained = open_locked(&staged).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let status = std::process::Command::new(&staged)
        .arg(argument)
        .status()
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let code = status.code().ok_or(EXIT_LAUNCH_FAILED)?;
    if code == 0 { Ok(0) } else { Err(code) }
}

pub(super) fn run_from_argument_inner(argument: &std::ffi::OsStr) -> Result<u32, i32> {
    let argument = argument.to_str().ok_or(EXIT_INVALID_REQUEST)?;
    if argument.starts_with("--windows-update-bootstrap-v2=") {
        authorize_public_update_bootstrap(argument)?;
    }
    if let Some(generation) = argument.strip_prefix("--windows-update-relaunch-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        validate_generation(generation)?;
        return retire_stale_legacy_relaunch(generation);
    }
    if argument == "--windows-update-relaunch-owner-install-v1" {
        if !is_elevated() {
            return Err(EXIT_NOT_ELEVATED);
        }
        return install_machine_relaunch_owner().map(|()| 0);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-launch-after-parent-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return launch_after_parent_exit(encoded);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-app-ready-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return acknowledge_app_ready(encoded);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-bootstrap-v3=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return run_relaunch_wrapper(encoded);
    }
    if (argument.starts_with("--windows-update-bootstrap-v2=")
        || argument.starts_with("--windows-update-resume-v2=")
        || argument.starts_with("--windows-update-cleanup-v1="))
        && !is_elevated()
    {
        return launch_elevated_bootstrap(argument).map(|()| 0);
    }
    if !is_elevated() {
        return Err(EXIT_NOT_ELEVATED);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-cleanup-v1=") {
        let _state = RecoveryStateLock::acquire()?;
        let generation = run_native_cleanup(encoded)?;
        clear_restart_recovery(&generation)?;
        return Ok(0);
    }
    if argument.starts_with("--windows-update-bootstrap-v2=")
        || argument.starts_with("--windows-update-bootstrap-bound-v1=")
    {
        return stage_bootstrap(argument).map(|()| 0);
    }
    let (encoded, previous_generation, resuming) =
        if let Some(generation) = argument.strip_prefix("--windows-update-resume-v2=") {
            validate_generation(generation)?;
            let directory = recovery_directory()?;
            if read_active_generation(&directory)? != generation {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            if protected_visible_attempt(&directory, generation)
                .is_none_or(|attempt| attempt == 0 || attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS)
            {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            (read_persisted_request()?, Some(generation.to_owned()), true)
        } else {
            let staged = argument
                .strip_prefix("--windows-update-bootstrap-staged-v2=")
                .ok_or(EXIT_INVALID_REQUEST)?;
            let (generation, encoded) = staged.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
            validate_generation(generation)?;
            (encoded.to_owned(), Some(generation.to_owned()), false)
        };
    let execution = execute_staged_request(&encoded, previous_generation.as_deref(), resuming);
    let result = execution
        .as_ref()
        .map(|(code, _)| *code)
        .map_err(|code| *code);
    // Only a truthful successful setup exit proves that protected retry ownership can move
    // to cleanup. UAC, launch, and installer failures retain the same generation and counter.
    if result == Ok(0) {
        let generation = execution
            .as_ref()
            .map(|(_, generation)| generation.as_str())
            .map_err(|code| *code)?;
        schedule_staged_cleanup(Some(generation))?;
    } else if resuming
        && previous_generation.as_deref().is_some_and(|generation| {
            recovery_directory().ok().is_some_and(|directory| {
                protected_visible_attempt(&directory, generation)
                    == Some(MAX_VISIBLE_RECOVERY_ATTEMPTS)
            })
        })
    {
        show_visible_retry_paused();
    }
    result
}

pub(super) const RELAUNCH_RUN_VALUE: &str = "Talking Quill Update Relaunch";
pub(super) const RELAUNCH_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub(super) const RELAUNCH_ROOT_SDDL: &str =
    "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x00000025;;;AU)";
pub(super) const RELAUNCH_RECORD_NAME: &str = "relaunch-record-v1.json";
pub(super) const RELAUNCH_MARKER_NAME: &str = "relaunch-record-marker-v1";
pub(super) const TERMINAL_UNINSTALL_RECORD_NAME: &str = "terminal-uninstall-record-v1.json";

pub(super) fn relaunch_record_sddl(identity: &RelaunchIdentity) -> String {
    if identity.user_sid == identity.logon_sid {
        format!(
            "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})",
            identity.user_sid
        )
    } else {
        format!(
            "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})(A;OICI;FA;;;{})",
            identity.user_sid, identity.logon_sid
        )
    }
}

pub(super) fn run_relaunch_wrapper(encoded: &str) -> Result<u32, i32> {
    if encoded.len() > 16_384 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let wrapper: RelaunchWrapper =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    let suffix = wrapper
        .request
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    if wrapper.request.len() > 12_288 || !valid_nonce(&wrapper.nonce) {
        return Err(EXIT_INVALID_REQUEST);
    }
    let request = parse_and_authorize_request(suffix)?;
    // Install under the shared lifecycle lock, then reacquire in the global order before
    // taking any user intent or generation lock.
    launch_elevated_installed_helper("--windows-update-relaunch-owner-install-v1")?;
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    let intent_path = PathBuf::from(&wrapper.intent_path);
    let _intent_lock = acquire_relaunch_intent_lock(&intent_path)?;
    let intent = read_relaunch_intent(&intent_path, &wrapper.nonce)?;
    if intent.source_version != request.candidate.predecessor.version
        || intent.target_version != request.candidate.version
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = current_relaunch_identity()?;
    let predecessor = installed_manifest()?;
    if predecessor.version != request.candidate.predecessor.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let generation = new_recovery_generation()?;
    let record = PersistedRelaunchRecord {
        schema_version: 3,
        generation: generation.clone(),
        user_sid: identity.user_sid,
        logon_sid: identity.logon_sid,
        request: wrapper.request,
        nonce: wrapper.nonce,
        source_version: intent.source_version,
        target_version: intent.target_version,
        phase: "armed".into(),
        completed_version: None,
        recovery_generation: generation.clone(),
        predecessor,
    };
    publish_relaunch_record(&record)?;
    std::fs::remove_file(&intent_path).map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(_intent_lock);
    drop(machine_lifecycle);
    let result = run_persisted_relaunch(&generation);
    let _ =
        std::fs::remove_file(intent_path.with_file_name("windows-update-relaunch-intent-v1.lock"));
    result
}

pub(super) fn retire_stale_legacy_relaunch(generation: &str) -> Result<u32, i32> {
    let legacy_root =
        known_folder(&FOLDERID_LocalAppData)?.join("Talking Quill/Windows Update Recovery");
    let legacy_directory = legacy_root.join(generation);
    clear_legacy_relaunch_owner(generation)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&legacy_directory)
        && metadata.is_dir()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && stale_local_relaunch_record_is_exact(&legacy_directory, generation)
    {
        let _ = remove_relaunch_record_directory(&legacy_directory);
        let _ = std::fs::remove_dir(&legacy_root);
    }
    Ok(0)
}

pub(super) fn stale_local_relaunch_record_is_exact(directory: &Path, generation: &str) -> bool {
    let Ok(bytes) = std::fs::read(directory.join(RELAUNCH_RECORD_NAME)) else {
        return false;
    };
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    matches!(
        value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64),
        Some(1 | 2)
    ) && value.get("generation").and_then(serde_json::Value::as_str) == Some(generation)
        && value
            .get("request")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|request| request.starts_with("--windows-update-bootstrap-v2="))
}

pub(super) fn clear_legacy_relaunch_owner(generation: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened != 0 {
        return Ok(());
    }
    let name = format!("Talking Quill Update Relaunch {generation}");
    let Ok(launcher) = medium_launcher_path() else {
        unsafe { RegCloseKey(key) };
        return Ok(());
    };
    let expected = format!(
        "\"{}\" --windows-update-relaunch-v1={generation}",
        launcher.display()
    );
    let Ok(actual) = read_registry_string(key, &name) else {
        unsafe { RegCloseKey(key) };
        return Ok(());
    };
    if actual.as_deref().is_some_and(|value| value != expected) {
        unsafe { RegCloseKey(key) };
        return Ok(());
    }
    if actual.is_some() {
        let _ = unsafe { RegDeleteValueW(key, wide_nul(Path::new(&name))?.as_ptr()) };
        let _ = unsafe { RegFlushKey(key) };
    }
    unsafe { RegCloseKey(key) };
    Ok(())
}

pub(super) fn maintenance_path_from_command(command: &str) -> Result<PathBuf, i32> {
    let path = command
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(PathBuf::from)
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let valid_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix("Talking Quill Maintenance-"))
        .and_then(|value| value.strip_suffix(".exe"))
        .is_some_and(|generation| validate_generation(generation).is_ok());
    if path.parent() != Some(program_files.as_path()) || !valid_name {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(path)
}

pub(super) fn terminal_uninstall_record() -> Result<Option<TerminalUninstallRecord>, i32> {
    let root = medium_launcher_directory()?;
    let path = root.join(TERMINAL_UNINSTALL_RECORD_NAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(EXIT_IDENTITY_MISMATCH),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(EXIT_IDENTITY_MISMATCH),
    }
    let file = File::open(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let record_identity = file_identity_text(&file)?;
    drop(file);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if bytes.is_empty()
        || bytes.len() > 4096
        || !has_exact_security(&path, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let record: TerminalUninstallRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let maintenance = maintenance_path_from_command(&record.uninstall_command)?;
    let uninstall_command = format!("\"{}\"", maintenance.display());
    if record.schema_version != 3
        || validate_generation(&record.generation).is_err()
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
        || decode_hash(&record.maintenance_sha256).is_none()
        || record.uninstall_command != uninstall_command
        || record.quiet_uninstall_command != format!("{uninstall_command} /S")
        || record.service_name != format!("TalkingQuillTerminalCleanup-{}", record.generation)
        || !record.service_image.ends_with(&format!(
            ".Talking Quill Terminal Cleanup-{}.exe",
            record.generation
        ))
        || decode_hash(&record.service_sha256).is_none()
        || record.service_file_identity.is_empty()
        || record.record_file_identity != record_identity
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(Some(record))
}

pub(super) fn journal_owned_terminal_maintenance() -> Result<Option<PathBuf>, i32> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let transaction_path = program_files.join(".Talking Quill.native-transaction-v2.json");
    let bytes = match std::fs::read(&transaction_path) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= 4096 => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(EXIT_IDENTITY_MISMATCH),
    };
    let transaction: SetupTransaction =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if transaction.schema_version != 2
        || transaction.action != "uninstall"
        || transaction.phase != "uninstall-cleanup-complete"
        || !transaction.had_predecessor
    {
        return Ok(None);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill",
            ))?
            .as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let quiet = read_registry_string(key, "QuietUninstallString")?;
    unsafe { RegCloseKey(key) };
    let maintenance = quiet
        .as_deref()
        .and_then(|value| value.strip_suffix(" /S"))
        .ok_or(EXIT_IDENTITY_MISMATCH)
        .and_then(maintenance_path_from_command)?;
    let retained = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    drop(retained);
    Ok(Some(maintenance))
}

pub(super) fn run_machine_relaunch_owner() -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    if let Some(record) = terminal_uninstall_record()? {
        let maintenance = maintenance_path_from_command(&record.uninstall_command)?;
        if !maintenance.exists()
            && matches!(
                record.phase.as_str(),
                "final-launcher-owned"
                    | "maintenance-deletion-owned"
                    | "uninstall-unregistered"
                    | "journal-removed"
            )
            && std::env::current_exe()
                .ok()
                .and_then(|path| path.file_name().map(|name| name.to_owned()))
                .and_then(|name| name.to_str().map(str::to_owned))
                .is_some_and(|name| name.starts_with(".Talking Quill Terminal Relaunch-"))
        {
            let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
            drop(machine_lifecycle);
            launch_elevated_executable(&current, "--windows-update-relaunch-owner-retire-v1")?;
            return Ok(0);
        }
        let mut retained = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&record.maintenance_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(retained);
        drop(machine_lifecycle);
        launch_elevated_executable(&maintenance, "/S")?;
        return Ok(0);
    }
    if let Some(maintenance) = journal_owned_terminal_maintenance()? {
        drop(machine_lifecycle);
        launch_elevated_executable(&maintenance, "/S")?;
        return Ok(0);
    }
    let identity = current_relaunch_identity()?;
    let generations = relaunch_generations()?;
    if generations.is_empty()
        && !known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill")
            .exists()
    {
        let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
        drop(machine_lifecycle);
        launch_elevated_executable(&current, "--windows-update-relaunch-owner-retire-v1")?;
        return Ok(0);
    }
    drop(machine_lifecycle);
    for generation in generations {
        let Ok(record) = read_persisted_relaunch_record(&generation) else {
            let _ = retire_stale_schema2_relaunch_record(&generation);
            continue;
        };
        if record.user_sid == identity.user_sid && record.logon_sid == identity.logon_sid {
            let _ = run_persisted_relaunch(&generation);
        }
    }
    Ok(0)
}

pub(super) fn run_persisted_relaunch(generation: &str) -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(generation)?;
    let _lock = acquire_relaunch_record_lock(&directory)?;
    let mut record = read_persisted_relaunch_record(generation)?;
    let identity = current_relaunch_identity()?;
    if record.user_sid != identity.user_sid || record.logon_sid != identity.logon_sid {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if record.phase == "app-ready" {
        drop(_lock);
        remove_relaunch_record_directory(&directory)?;
        return Ok(0);
    }
    let suffix = record
        .request
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    let request = parse_request_envelope(suffix)?;
    verify_update_authorization(&request.candidate)?;
    if record.source_version != request.candidate.predecessor.version
        || record.target_version != request.candidate.version
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if matches!(record.phase.as_str(), "setup-complete" | "launch-started") {
        let surviving = verified_surviving_version(&request.candidate, &record.predecessor)?;
        if record.completed_version.as_deref() != Some(surviving.as_str()) {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if record.phase == "setup-complete" {
            record.phase = "launch-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        defer_launch_until_parent_exit(&surviving, generation)?;
        return Ok(0);
    }
    let target_committed = verify_post_install_request(&request).is_ok();
    let recovering_setup = matches!(record.phase.as_str(), "armed" | "setup-started");
    if recovering_setup
        && native_setup_transaction_present()?
        && verified_surviving_version(&request.candidate, &record.predecessor).is_err()
    {
        let recovery_directory = medium_recovery_directory(&record.recovery_generation)?;
        if !recovery_directory.exists()
            || read_active_generation(&recovery_directory)? != record.recovery_generation
            || !recovery_command_present(&record.recovery_generation)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if record.phase == "armed" {
            record.phase = "setup-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        let resume = format!("--windows-update-resume-v2={}", record.recovery_generation);
        drop(_lock);
        drop(machine_lifecycle);
        launch_elevated_bootstrap(&resume)?;
        return run_persisted_relaunch(generation);
    }
    let recovered_survivor = recovering_setup
        && verified_surviving_version(&request.candidate, &record.predecessor).is_ok();
    if !target_committed && !recovered_survivor {
        verify_update_relation_against_snapshot(
            &request.candidate,
            &request.sha256,
            &record.predecessor,
        )?;
        if record.phase == "armed" {
            record.phase = "setup-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        drop(_lock);
        drop(machine_lifecycle);
        let setup_result = launch_elevated_installed_helper(&format!(
            "--windows-update-bootstrap-bound-v1={}:{}",
            record.recovery_generation,
            record
                .request
                .strip_prefix("--windows-update-bootstrap-v2=")
                .ok_or(EXIT_INVALID_REQUEST)?
        ));
        let _machine_lifecycle = RecoveryStateLock::acquire()?;
        let _lock = acquire_relaunch_record_lock(&directory)?;
        record = read_persisted_relaunch_record(generation)?;
        if setup_result == Err(1223) {
            // Login recovery cancellation is not terminal ownership evidence. Keep the exact
            // armed generation for a later logon or maintenance recovery attempt.
            return Err(1223);
        }
    }
    let surviving = verified_surviving_version(&request.candidate, &record.predecessor)?;
    record.phase = "setup-complete".into();
    record.completed_version = Some(surviving.clone());
    write_persisted_relaunch_record(&directory, &record)?;
    record.phase = "launch-started".into();
    write_persisted_relaunch_record(&directory, &record)?;
    defer_launch_until_parent_exit(&surviving, generation)?;
    Ok(0)
}

pub(super) fn acknowledge_app_ready(encoded: &str) -> Result<u32, i32> {
    let request: AppReadyRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if !valid_version(&request.version) || validate_generation(&request.generation).is_err() {
        return Err(EXIT_INVALID_REQUEST);
    }
    verify_app_ready_parent()?;
    if installed_manifest_version()? != request.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(&request.generation)?;
    let lock = acquire_relaunch_record_lock(&directory)?;
    let mut record = read_persisted_relaunch_record(&request.generation)?;
    if record.completed_version.as_deref() != Some(request.version.as_str())
        || !matches!(record.phase.as_str(), "launch-started" | "app-ready")
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if record.phase != "app-ready" {
        record.phase = "app-ready".into();
        write_persisted_relaunch_record(&directory, &record)?;
    }
    drop(lock);
    remove_relaunch_record_directory(&directory)?;
    Ok(0)
}

#[cfg(test)]
pub(super) static TEST_RELAUNCH_ROOT: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);
