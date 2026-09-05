//! Persist and retire authenticated relaunch records.
use super::*;

pub(super) fn relaunch_root() -> Result<PathBuf, i32> {
    #[cfg(test)]
    if let Some(root) = TEST_RELAUNCH_ROOT
        .lock()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .clone()
    {
        return Ok(root);
    }
    Ok(known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery/Relaunch Records"))
}

pub(super) fn relaunch_identity_key(identity: &RelaunchIdentity) -> String {
    hex_digest(&Sha256::digest(identity.user_sid.as_bytes()))[..16].to_owned()
}

pub(super) fn relaunch_generation_directory(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    let identity = current_relaunch_identity()?;
    Ok(relaunch_root()?.join(format!("{}-{generation}", relaunch_identity_key(&identity))))
}

pub(super) fn relaunch_marker_value(record: &PersistedRelaunchRecord) -> String {
    format!(
        "{}:{}:{}",
        record.generation,
        record.nonce,
        hex_digest(&Sha256::digest(record.request.as_bytes()))
    )
}

pub(super) fn publish_relaunch_record(record: &PersistedRelaunchRecord) -> Result<(), i32> {
    let root = relaunch_root()?;
    if !root.exists() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = current_relaunch_identity()?;
    if record.user_sid != identity.user_sid || record.logon_sid != identity.logon_sid {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let directory = relaunch_generation_directory(&record.generation)?;
    std::fs::create_dir(&directory).map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&directory, &identity)?;
    write_persisted_relaunch_record(&directory, record)?;
    write_protected_relaunch_file(
        &directory.join(RELAUNCH_MARKER_NAME),
        relaunch_marker_value(record).as_bytes(),
    )?;
    flush_directory(&directory)?;
    Ok(())
}

pub(super) fn write_persisted_relaunch_record(
    directory: &Path,
    record: &PersistedRelaunchRecord,
) -> Result<(), i32> {
    let bytes = serde_json::to_vec(record).map_err(|_| EXIT_LAUNCH_FAILED)?;
    write_protected_relaunch_file(&directory.join(RELAUNCH_RECORD_NAME), &bytes)
}

pub(super) fn write_protected_relaunch_file(path: &Path, bytes: &[u8]) -> Result<(), i32> {
    let parent = path.parent().ok_or(EXIT_INVALID_REQUEST)?;
    let temporary = parent.join(format!(".relaunch-pending-{}", new_recovery_generation()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&temporary, &current_relaunch_identity()?)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(file);
    verify_protected_relaunch_file(&temporary, bytes)?;
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(path)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let _ = std::fs::remove_file(temporary);
        return Err(EXIT_LAUNCH_FAILED);
    }
    flush_directory(parent)?;
    verify_protected_relaunch_file(path, bytes)
}

pub(super) fn verify_protected_relaunch_file(path: &Path, expected: &[u8]) -> Result<(), i32> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut actual = Vec::new();
    file.read_to_end(&mut actual)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if actual == expected && has_relaunch_dacl(path, &current_relaunch_identity()?)? {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn read_persisted_relaunch_record(
    generation: &str,
) -> Result<PersistedRelaunchRecord, i32> {
    let directory = relaunch_generation_directory(generation)?;
    let identity = current_relaunch_identity()?;
    if !has_relaunch_dacl(&directory, &identity)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let path = directory.join(RELAUNCH_RECORD_NAME);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verify_protected_relaunch_file(&path, &bytes)?;
    let record: PersistedRelaunchRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if record.schema_version != 3
        || record.generation != generation
        || record.user_sid != identity.user_sid
        || validate_generation(&record.recovery_generation).is_err()
        || record.logon_sid != identity.logon_sid
        || !valid_nonce(&record.nonce)
        || !valid_version(&record.source_version)
        || !valid_version(&record.target_version)
        || !matches!(
            record.phase.as_str(),
            "armed" | "setup-started" | "setup-complete" | "launch-started" | "app-ready"
        )
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let marker = directory.join(RELAUNCH_MARKER_NAME);
    verify_protected_relaunch_file(&marker, relaunch_marker_value(&record).as_bytes())?;
    Ok(record)
}

pub(super) fn retire_stale_schema2_relaunch_record(generation: &str) -> Result<(), i32> {
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(generation)?;
    let _lock = acquire_relaunch_record_lock(&directory)?;
    let identity = current_relaunch_identity()?;
    if !has_relaunch_dacl(&directory, &identity)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let path = directory.join(RELAUNCH_RECORD_NAME);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verify_protected_relaunch_file(&path, &bytes)?;
    let record: PersistedRelaunchRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if record.schema_version != 2
        || record.generation != generation
        || record.user_sid != identity.user_sid
        || record.logon_sid != identity.logon_sid
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_protected_relaunch_file(
        &directory.join(RELAUNCH_MARKER_NAME),
        relaunch_marker_value(&record).as_bytes(),
    )?;
    drop(_lock);
    remove_relaunch_record_directory(&directory)
}

pub(super) fn acquire_relaunch_record_lock(directory: &Path) -> Result<std::fs::File, i32> {
    let path = directory.join("relaunch-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&path, &current_relaunch_identity()?)?;
    Ok(file)
}

pub(super) fn relaunch_generations() -> Result<Vec<String>, i32> {
    let root = relaunch_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    let prefix = format!("{}-", relaunch_identity_key(&current_relaunch_identity()?));
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(&prefix) else {
            continue;
        };
        if entry.file_type().map_err(|_| EXIT_LAUNCH_FAILED)?.is_dir()
            && validate_generation(generation).is_ok()
        {
            values.push(generation.to_owned());
        }
    }
    Ok(values)
}

pub(super) fn remove_relaunch_record_directory(directory: &Path) -> Result<(), i32> {
    for name in [
        RELAUNCH_RECORD_NAME,
        RELAUNCH_MARKER_NAME,
        "relaunch-state-v1.lock",
    ] {
        let path = directory.join(name);
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    std::fs::remove_dir(directory).map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn launch_elevated_installed_helper(argument: &str) -> Result<(), i32> {
    let helper = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill/resources/helper/talking-quill-helper.exe");
    launch_elevated_executable(&helper, argument)
}

pub(super) fn defer_launch_until_parent_exit(version: &str, generation: &str) -> Result<(), i32> {
    let parent_pid = current_parent_process_id()?;
    if verify_installed_application_process(parent_pid).is_err() {
        return launch_program_files_application_for_generation(generation);
    }
    let request = DeferredLaunchRequest {
        parent_pid,
        generation: generation.into(),
        version: version.into(),
    };
    let encoded = base64_encode(&serde_json::to_vec(&request).map_err(|_| EXIT_LAUNCH_FAILED)?);
    std::process::Command::new(
        known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill/resources/helper/talking-quill-helper.exe"),
    )
    .arg(format!("--windows-update-launch-after-parent-v1={encoded}"))
    .spawn()
    .map(|_| ())
    .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn launch_after_parent_exit(encoded: &str) -> Result<u32, i32> {
    let request: DeferredLaunchRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if request.parent_pid == 0
        || !valid_version(&request.version)
        || validate_generation(&request.generation).is_err()
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    let raw = unsafe { OpenProcess(SYNCHRONIZE, 0, request.parent_pid) };
    if !raw.is_null() {
        let parent = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { WaitForSingleObject(parent.as_raw_handle(), 120_000) } != WAIT_OBJECT_0 {
            return Err(EXIT_LAUNCH_FAILED);
        }
    }
    if installed_manifest_version()? != request.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    launch_program_files_application_for_generation(&request.generation)?;
    Ok(0)
}
