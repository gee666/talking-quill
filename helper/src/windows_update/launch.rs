//! Start authenticated installers, native cleanup, and relaunch helpers.
use super::*;

pub(super) fn set_machine_relaunch_run_value(command: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            std::ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new(RELAUNCH_RUN_VALUE))?;
    let value = wide_nul(Path::new(command))?;
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = unsafe { RegFlushKey(key) };
    unsafe { RegCloseKey(key) };
    if status == 0 && flushed == 0 {
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

pub(super) fn launch_program_files_application_for_generation(generation: &str) -> Result<(), i32> {
    validate_generation(generation)?;
    let application = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill/Talking Quill.exe");
    let metadata = std::fs::symlink_metadata(&application).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    std::process::Command::new(application)
        .arg(format!(
            "--windows-update-relaunch-generation-v1={generation}"
        ))
        .spawn()
        .map(|_| ())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn launch_elevated_bootstrap(argument: &str) -> Result<(), i32> {
    let executable = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    launch_elevated_executable(&executable, argument)
}

pub(super) fn launch_elevated_executable(executable: &Path, argument: &str) -> Result<(), i32> {
    let verb = wide_nul(Path::new("runas"))?;
    let file = wide_nul(executable)?;
    let parameters = wide_nul(Path::new(argument))?;
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: 1,
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { ShellExecuteExW(&mut execute) } == 0 || execute.hProcess.is_null() {
        let error = unsafe { GetLastError() } as i32;
        return Err(if error == 1223 {
            1223
        } else {
            EXIT_LAUNCH_FAILED
        });
    }
    let process = unsafe { OwnedHandle::from_raw_handle(execute.hProcess) };
    let wait =
        unsafe { WaitForSingleObject(process.as_raw_handle(), INSTALLER_SUPERVISION_TIMEOUT_MS) };
    if wait == WAIT_TIMEOUT {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_INSTALLER_STILL_RUNNING as u32) };
        unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(EXIT_INSTALLER_STILL_RUNNING);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if code != 0 {
        return Err(code as i32);
    }
    Ok(())
}

pub(super) fn run_native_cleanup(binding: &str) -> Result<String, i32> {
    let (suffix, generation) = binding.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    validate_generation(generation)?;
    let path = known_folder(&FOLDERID_ProgramData)?
        .join(format!(".Talking Quill.update-bootstrap-{suffix}"));
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(generation.to_owned());
        }
        Err(_) => return Err(EXIT_LAUNCH_FAILED),
        Ok(_) => {}
    }
    let expected_identity = std::fs::read_to_string(path.join("cleanup-tree-identity-v1"))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if expected_identity.is_empty() || expected_identity.len() > 256 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    match read_active_generation(&path) {
        Ok(active) if active != generation => return Ok(generation.to_owned()),
        Ok(_) => {}
        Err(_) if !active_generation_path(&path).exists() => {}
        Err(error) => return Err(error),
    }
    for _ in 0..120 {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || metadata.file_attributes()
                        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                        != 0 =>
            {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            Ok(_) => match remove_owned_tree(&path, &expected_identity) {
                Ok(()) => return Ok(generation.to_owned()),
                Err(crate::owned_tree::OwnedTreeError::IdentityMismatch) => {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                Err(_) => {}
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(generation.to_owned());
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(EXIT_LAUNCH_FAILED)
}

pub(super) fn execute_staged_request(
    encoded: &str,
    previous_generation: Option<&str>,
    resuming: bool,
) -> Result<(u32, String), i32> {
    let request = parse_and_authorize_request(encoded)?;
    let recovery_generation = previous_generation.ok_or(EXIT_INVALID_REQUEST)?.to_owned();
    if resuming && installed_candidate_committed(&request.candidate) {
        return Ok((0, recovery_generation));
    }
    let expected_hash = decode_hash(&request.sha256).ok_or(EXIT_INVALID_REQUEST)?;
    let staged = copy_verified_installer(Path::new(&request.installer_path), expected_hash)?;
    persist_request(encoded)?;
    if !resuming {
        let _state = RecoveryStateLock::acquire()?;
        persist_restart_recovery(&recovery_directory()?, &recovery_generation)?;
    }
    acknowledge_recovery_ownership()?;
    let code = launch_verified_installer(&staged, expected_hash, &request.candidate)?;
    Ok((code, recovery_generation))
}

pub(super) fn installed_candidate_committed(candidate: &UpdateCandidate) -> bool {
    let Ok(program_files) = known_folder(&FOLDERID_ProgramFiles) else {
        return false;
    };
    if program_files
        .join(".Talking Quill.native-transaction-v2.json")
        .exists()
    {
        return false;
    }
    let manifest_path = program_files
        .join("Talking Quill")
        .join("resources")
        .join("keyboard-owner-release-v1.json");
    let Ok(bytes) = std::fs::read(manifest_path) else {
        return false;
    };
    let Ok(installed) = serde_json::from_slice::<InstalledManifest>(&bytes) else {
        return false;
    };
    installed.version == candidate.version
        && installed.platform == candidate.platform
        && installed.architecture == candidate.architecture
        && installed.source_commit == candidate.source_commit
        && installed.source_tree == candidate.source_tree
        && installed.release_build_digest == candidate.release_build_digest
        && candidate.roles.len() == installed.roles.len()
        && candidate.roles.iter().all(|expected| {
            installed.roles.iter().any(|actual| {
                actual.role == expected.role
                    && actual.path == expected.path
                    && actual.sha256 == expected.sha256
                    && actual.suppression_capable == expected.suppression_capable
            })
        })
}
