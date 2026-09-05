//! Token identity and exact verified installer process launch.
use super::*;

#[derive(Clone, Eq, PartialEq)]
pub(super) struct RelaunchIdentity {
    pub(super) user_sid: String,
    pub(super) logon_sid: String,
}

pub(super) fn sid_string(sid: *mut core::ffi::c_void) -> Result<String, i32> {
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let length = unsafe { (0..).take_while(|&index| *text.add(index) != 0).count() };
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| EXIT_IDENTITY_MISMATCH);
    unsafe { LocalFree(text.cast()) };
    result
}

pub(super) fn token_information(class: i32) -> Result<Vec<u8>, i32> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut value = vec![0_u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            value.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(value)
}

pub(super) fn current_relaunch_identity() -> Result<RelaunchIdentity, i32> {
    let user = token_information(TokenUser)?;
    let user = unsafe { &*(user.as_ptr().cast::<TOKEN_USER>()) };
    let user_sid = sid_string(user.User.Sid)?;
    // TokenUser is the durable SID of the real interactive logon identity. Unlike the
    // per-session logon-group SID, it remains stable across reboot/login recovery.
    Ok(RelaunchIdentity {
        logon_sid: user_sid.clone(),
        user_sid,
    })
}

pub(super) fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) != 0
            && returned == size_of::<TOKEN_ELEVATION>() as u32
            && elevation.TokenIsElevated == 1
    }
}

pub(super) fn copy_verified_installer(
    path: &Path,
    expected_hash: [u8; 32],
) -> Result<PathBuf, i32> {
    if !path.is_absolute() {
        return Err(EXIT_INVALID_REQUEST);
    }
    let directory = std::env::current_exe()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .parent()
        .ok_or(EXIT_LAUNCH_FAILED)?
        .to_owned();
    let target = directory.join("verified-update-installer.exe");
    if target.exists() {
        let mut retained = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)? == expected_hash {
            return Ok(target);
        }
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut source = open_locked(&canonical).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut source).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&target)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&target, RESTRICTED_FILE_SDDL)?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    std::io::copy(&mut source, &mut output).map_err(|_| EXIT_LAUNCH_FAILED)?;
    output.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    let mut copied = open_locked(&target).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut copied).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(target)
}

pub(super) fn launch_verified_installer(
    path: &Path,
    expected_hash: [u8; 32],
    candidate: &UpdateCandidate,
) -> Result<u32, i32> {
    if !path.is_absolute() {
        return Err(EXIT_INVALID_REQUEST);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut retained = open_locked(&canonical).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let expected_identity = file_identity(&retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }

    let mut application = wide_nul(&canonical)?;
    // The candidate derives update mode and predecessor identity from its
    // authenticated TQPKG2 manifest and installed machine state. Arguments
    // carry no install authority.
    let mut command = wide_nul(&PathBuf::from(format!(
        "\"{}\" /S /TQUPDATE={} /TQGATEWAYHASH={} /TQOWNERHASH={} /TQLAYOUT={}",
        canonical.display(),
        candidate.package_sha256,
        candidate.predecessor.gateway_sha256,
        candidate.predecessor.owner_sha256,
        candidate.predecessor.release_build_digest,
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

    let verified = verify_suspended_process(
        process_handle.as_raw_handle(),
        &canonical,
        expected_identity,
        expected_hash,
    );
    if verified.is_err() {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_IDENTITY_MISMATCH as u32,
            )
        };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if unsafe { ResumeThread(thread_handle.as_raw_handle()) } == u32::MAX {
        unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    // The verified copy lives in the elevated bootstrap's protected directory,
    // so same-user code cannot replace it after this point. Release the file
    // lock because native setup removes its own executable during normal teardown.
    drop(retained);
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
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process_handle.as_raw_handle(), &mut exit_code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(exit_code)
}

pub(super) fn spawn_staged_cleanup(directory: &Path, generation: &str) -> Result<(), i32> {
    let installed = medium_launcher_path()?;
    let suffix = directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    std::process::Command::new(installed)
        .arg(format!("--windows-update-cleanup-v1={suffix}:{generation}"))
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}
