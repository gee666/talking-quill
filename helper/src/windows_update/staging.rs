//! Retain and stage verified installer snapshots.
use super::*;

pub(super) fn stage_bootstrap(argument: &str) -> Result<(), i32> {
    let (bound_generation, suffix) =
        if let Some(value) = argument.strip_prefix("--windows-update-bootstrap-bound-v1=") {
            let (generation, suffix) = value.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
            validate_generation(generation)?;
            (Some(generation), suffix)
        } else {
            (
                None,
                argument
                    .strip_prefix("--windows-update-bootstrap-v2=")
                    .ok_or(EXIT_INVALID_REQUEST)?,
            )
        };
    // The installed helper authorizes the exact outer installer and candidate
    // policy before it resumes any staged elevated executable.
    let request = parse_and_authorize_request(suffix)?;
    let authorized_package = decode_hash(&request.sha256).ok_or(EXIT_INVALID_REQUEST)?;
    let mut installer =
        open_locked(Path::new(&request.installer_path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut installer).map_err(|_| EXIT_IDENTITY_MISMATCH)? != authorized_package {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let (current, mut trusted, trusted_identity, expected_hash) = trusted_installed_bootstrap()?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let recovery_state = RecoveryStateLock::acquire()?;
    let recovery_launcher = ensure_medium_launcher(&current)?;
    reclaim_incomplete_recovery_directories(&program_data)?;
    let final_suffix = getrandom::u64().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let pending_token = new_recovery_generation()?;
    let pending = program_data.join(format!(
        ".Talking Quill.update-bootstrap-pending-{pending_token}"
    ));
    let directory = program_data.join(format!(
        ".Talking Quill.update-bootstrap-{final_suffix:016x}"
    ));
    create_restricted_directory(&pending)?;
    let mut directory_guard = match StagedDirectoryGuard::new(pending.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = std::fs::remove_dir(&pending);
            return Err(error);
        }
    };
    directory_guard.persist_identity_marker()?;
    let pending_staged = pending.join("talking-quill-update-bootstrap.exe");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&pending_staged)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&pending_staged, RESTRICTED_FILE_SDDL)?;
    trusted
        .seek(SeekFrom::Start(0))
        .and_then(|_| std::io::copy(&mut trusted, &mut output).map(|_| ()))
        .and_then(|_| output.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    stage_predecessor_evidence(&current, &pending)?;
    let mut retained = open_locked(&pending_staged).map_err(|_| EXIT_LAUNCH_FAILED)?;
    let expected_identity = file_identity(&retained).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut retained).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash
        || file_identity(&trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != trusted_identity
        || hash_file(&mut trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(retained);
    directory_guard.publish(directory.clone())?;
    directory_guard.register_prelaunch_cleanup(&recovery_launcher, bound_generation)?;
    let cleanup_generation = directory_guard
        .cleanup_generation
        .as_deref()
        .ok_or(EXIT_LAUNCH_FAILED)?
        .to_owned();
    drop(recovery_state);
    let staged = directory.join("talking-quill-update-bootstrap.exe");
    let mut retained = open_locked(&staged).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut retained).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let staged_argument =
        format!("--windows-update-bootstrap-staged-v2={cleanup_generation}:{suffix}");
    let mut application = wide_nul(&staged)?;
    let mut command = wide_nul(&PathBuf::from(format!(
        "\"{}\" {}",
        staged.display(),
        staged_argument
    )))?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_mut_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(process.hThread) };
    if file_identity(&trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != trusted_identity
        || hash_file(&mut trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash
        || !paths_equal(
            &current,
            &std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?,
        )
        || verify_suspended_process(
            process_handle.as_raw_handle(),
            &staged,
            expected_identity,
            expected_hash,
        )
        .is_err()
    {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_IDENTITY_MISMATCH as u32,
            )
        };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if unsafe { ResumeThread(thread_handle.as_raw_handle()) } == u32::MAX {
        unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    let ownership_marker = directory.join("recovery-owned-v2");
    let ownership_deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if std::fs::read(&ownership_marker).is_ok_and(|value| value == b"owned-v2") {
            directory_guard.transfer_to_launched_recovery();
            break;
        }
        if unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 0) } == WAIT_OBJECT_0
            || std::time::Instant::now() >= ownership_deadline
        {
            unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
            unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
            return Err(EXIT_LAUNCH_FAILED);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let wait = unsafe {
        WaitForSingleObject(
            process_handle.as_raw_handle(),
            INSTALLER_SUPERVISION_TIMEOUT_MS,
        )
    };
    if wait == WAIT_TIMEOUT {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_INSTALLER_STILL_RUNNING as u32,
            )
        };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_INSTALLER_STILL_RUNNING);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process_handle.as_raw_handle(), &mut code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if code != 0 {
        return Err(code as i32);
    }
    Ok(())
}

pub(super) fn copy_restricted_snapshot(source: &Path, target: &Path) -> Result<(), i32> {
    let mut input = open_locked(source).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(target)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(target, RESTRICTED_FILE_SDDL)?;
    std::io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn stage_predecessor_evidence(
    installed_gateway: &Path,
    directory: &Path,
) -> Result<(), i32> {
    let helper_directory = installed_gateway.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let resources = helper_directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    copy_restricted_snapshot(
        &resources.join("keyboard-owner-release-v1.json"),
        &directory.join("predecessor-release.json"),
    )?;
    copy_restricted_snapshot(
        &helper_directory.join("talking-quill-keyboard-owner.exe"),
        &directory.join("predecessor-owner.exe"),
    )?;
    copy_restricted_snapshot(
        &helper_directory.join("talking-quill-update-recovery-launcher.exe"),
        &directory.join("predecessor-recovery-launcher.exe"),
    )
}

pub(super) fn parse_request_envelope(encoded: &str) -> Result<UpdateRequest, i32> {
    let request: UpdateRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if request.version != 2
        || request.installer_path.contains('\0')
        || request.sha256.len() != 64
        || !request
            .sha256
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    Ok(request)
}

pub(super) fn parse_and_authorize_request(encoded: &str) -> Result<UpdateRequest, i32> {
    let request = parse_request_envelope(encoded)?;
    verify_update_relation(&request.candidate, &request.sha256)?;
    verify_update_authorization(&request.candidate)?;
    Ok(request)
}

pub(super) fn verify_post_install_request(request: &UpdateRequest) -> Result<(), i32> {
    if request.candidate.package_sha256 != request.sha256
        || request.candidate.release_build_digest != request.candidate.package_layout_digest
        || canonical_candidate_layout(&request.candidate)?
            != request.candidate.package_layout_digest
        || !installed_candidate_committed(&request.candidate)
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_update_authorization(&request.candidate)
}
