//! Normal-user setup entry, image retention, relocation, and UAC launch.
use super::*;

pub(super) fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_diagnostic_arguments(&arguments) {
        return run_direct_stale_schema2_diagnostic(&arguments)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    let elevated = token_is_elevated()?;
    let relocated = !elevated
        && arguments.iter().any(|value| value == "/TQ-RELOCATED")
        && arguments
            .iter()
            .all(|value| value == "/TQ-RELOCATED" || value == "/S");
    let legacy_predecessor = elevated && legacy_predecessor_arguments(&arguments);
    #[cfg(feature = "stale-schema2-cleanup")]
    let cleanup_requested =
        !elevated && arguments.len() == 1 && arguments[0] == "/TQ-CLEAN-STALE-SCHEMA2";
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let cleanup_requested = false;
    #[cfg(feature = "stale-schema2-cleanup")]
    let direct_cleanup_requested = direct_cleanup_arguments(&arguments, elevated);
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let direct_cleanup_requested = false;
    if !((arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "/S"))
        || legacy_predecessor
        || relocated
        || cleanup_requested
        || direct_cleanup_requested)
    {
        return Err(fail(EXIT_USAGE, "The native setup accepts only /S."));
    }
    let mut silent = arguments.first().is_some_and(|value| value == "/S") || cleanup_requested;
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_cleanup_requested {
        return run_direct_elevated_stale_schema2_cleanup()
            .map(|()| 0)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    if !elevated {
        let current =
            std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        #[cfg(feature = "stale-schema2-cleanup")]
        if cleanup_requested {
            let retained = retain_controller_image(&current, false)?;
            let channel =
                ControllerChannel::create(Action::CleanStaleSchema2, true, std::process::id())?;
            let result = elevate(&current, true, &channel, None);
            drop(retained);
            return result;
        }
        let controller_paths = paths()?;
        let retained = retain_controller_image(&current, relocated)?;
        let mut lifecycle_parent = 0;
        let mut relocation_server = None;
        let mut relocated_finalizer = false;
        let mut relocated_identity_guard = None;
        let action = if relocated {
            let original = process_image(parent_process_id()?)?;
            let installed = controller_paths.install.join("Uninstall Talking Quill.exe");
            let expected =
                if canonical(&original)? == canonical(&controller_paths.maintenance_uninstaller)? {
                    controller_paths.maintenance_uninstaller.clone()
                } else if canonical(&original)? == canonical(&installed)? {
                    installed
                } else if is_uninstall_finalizer(&original)? {
                    relocated_finalizer = true;
                    original.clone()
                } else {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Relocated uninstall source is not an authenticated maintenance image.",
                    ));
                };
            relocated_identity_guard = Some(validate_relocated_uninstall_image(
                &current,
                &original,
                &expected,
                &controller_paths.maintenance_uninstaller,
            )?);
            let (action, server, requested_silent, requested_lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&current, Some(&expected))?;
            silent = requested_silent;
            lifecycle_parent = requested_lifecycle_parent;
            relocation_server = Some(server);
            if action != Action::Uninstall {
                return Err(fail(
                    EXIT_REJECTED,
                    "Relocation requested an invalid operation.",
                ));
            }
            Action::Uninstall
        } else {
            derive_action(&current, &controller_paths)?
        };
        if action == Action::Uninstall && !relocated {
            let _ = request_runtime_exit(&controller_paths, None);
            let (path, lock) = create_relocated_image(&current)?;
            let channel = ControllerChannel::create(Action::Uninstall, silent, std::process::id())?;
            let process = launch_relocated(&path, &channel)?;
            drop(retained);
            let code = channel.wait_relocated_status(&process, &current)?;
            drop(lock);
            return Ok(code);
        }
        if !silent && !confirm_controller(action)? {
            return Ok(ERROR_CANCELLED as i32);
        }
        let delete_profile = !silent
            && action == Action::Uninstall
            && message_box(
                "Also delete your Talking Quill profile?",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES;
        if action != Action::Install {
            // The elevated worker retries cleanup and can close administrator processes.
            let _ = request_runtime_exit(
                &controller_paths,
                (lifecycle_parent != 0).then_some(lifecycle_parent),
            );
        }
        let retained = if relocated {
            drop(retained);
            retain_controller_image(&current, true)?
        } else {
            retained
        };
        let arm_deletion = || -> Result<()> {
            let server = relocation_server.as_ref().ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                b"TQ-ARM-DELETE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
            if pipe_read::<1>(
                server.as_raw_handle(),
                None,
                Instant::now() + Duration::from_secs(30),
            )? != [1]
            {
                return Err(fail(
                    EXIT_FAILURE,
                    "Installed uninstall image deletion was not armed.",
                ));
            }
            Ok(())
        };
        let before_accept = (relocated && lifecycle_parent != 0 && !relocated_finalizer)
            .then_some(&arm_deletion as &dyn Fn() -> Result<()>);
        if relocated_finalizer {
            pipe_write(
                relocation_server
                    .as_ref()
                    .ok_or_else(|| fail(EXIT_FAILURE, "Finalizer relocation channel is missing."))?
                    .as_raw_handle(),
                b"TQ-KEEP-IMAGE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        let channel = ControllerChannel::create(action, silent, lifecycle_parent)?;
        let result = elevate(&current, silent, &channel, before_accept);
        drop(relocated_identity_guard);
        drop(retained);
        if relocated && lifecycle_parent != 0 {
            let status = result.as_ref().copied().unwrap_or_else(|error| error.code);
            let server = relocation_server.ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                &status.to_le_bytes(),
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        if result.as_ref().is_ok_and(|code| *code == 0) && delete_profile {
            remove_plain_tree(&controller_paths.profile)?;
        }
        return result;
    }
    run_worker(silent, legacy_predecessor)
}

pub(super) fn legacy_predecessor_arguments(arguments: &[OsString]) -> bool {
    if arguments.len() != 5 || arguments[0] != "/S" {
        return false;
    }
    [
        "/TQUPDATE=",
        "/TQGATEWAYHASH=",
        "/TQOWNERHASH=",
        "/TQLAYOUT=",
    ]
    .iter()
    .all(|prefix| {
        arguments
            .iter()
            .skip(1)
            .any(|value| value.to_string_lossy().starts_with(prefix))
    })
}

pub(super) fn confirm_controller(action: Action) -> Result<bool> {
    let text = match action {
        Action::Install => "Install Talking Quill for all users?",
        Action::Update => "Update Talking Quill for all users?",
        Action::Repair => "Repair Talking Quill for all users?",
        Action::Uninstall => {
            "Uninstall Talking Quill?\n\nYour profile is preserved unless you delete it in the application first."
        }
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => "Remove the exact stale schema-2 test residue?",
    };
    let text = format!(
        "{text}\n\nVersion {} for 64-bit Windows.\nWindows will ask for administrator approval to continue.",
        env!("CARGO_PKG_VERSION")
    );
    let response = message_box(&text, MB_OKCANCEL | MB_ICONQUESTION);
    Ok(response == IDOK)
}

pub(super) fn retain_controller_image(path: &Path, delete_on_close: bool) -> Result<OwnedHandle> {
    let access = FILE_GENERIC_READ | if delete_on_close { DELETE } else { 0 };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if delete_on_close {
            FILE_FLAG_DELETE_ON_CLOSE
        } else {
            0
        };
    let sharing = FILE_SHARE_READ | FILE_SHARE_DELETE;
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            access,
            sharing,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

pub(super) fn rename_handle(
    handle: std::os::windows::io::RawHandle,
    destination: &Path,
) -> Result<()> {
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(mem::size_of::<u16>())
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let fixed = mem::offset_of!(FILE_RENAME_INFO, FileName);
    let total = fixed
        .checked_add(name_bytes)
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let mut storage = vec![0_usize; total.div_ceil(mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = u32::try_from(name_bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
        ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileRenameInfo,
            info.cast(),
            u32::try_from(total)
                .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(())
}

pub(super) fn rename_retained(handle: &OwnedHandle, destination: &Path) -> Result<()> {
    rename_handle(handle.as_raw_handle(), destination)
}

pub(super) fn arm_mapped_image_deletion(path: &Path) -> Result<()> {
    let rename_handle = open_plain_handle(path, false, true)?;
    let stream = PathBuf::from(format!(":tq-uninstall-{:08x}", std::process::id()));
    rename_retained(&rename_handle, &stream).map_err(|error| {
        fail(
            error.code,
            format!("Mapped image stream rename failed: {}", error.message),
        )
    })?;
    drop(rename_handle);
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            format!(
                "Renamed mapped image reopen failed: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let delete_handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            delete_handle.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    drop(delete_handle);
    if path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Windows did not commit mapped uninstall image deletion.",
        ));
    }
    Ok(())
}

pub(super) fn validate_relocated_uninstall_image(
    relocated: &Path,
    original: &Path,
    expected: &Path,
    maintenance: &Path,
) -> Result<File> {
    let relocated_canonical = std::fs::canonicalize(relocated).map_err(io_failure)?;
    let temp_canonical = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = relocated_canonical
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Relocated uninstall path is invalid."))?;
    if relocated_canonical.parent() != Some(temp_canonical.as_path())
        || !name.starts_with(".TalkingQuill-uninstall-")
        || !name.ends_with(".exe")
        || name.len() != ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        || canonical(original)? != canonical(expected)?
    {
        return Err(fail(EXIT_REJECTED, "Relocated uninstall path is invalid."));
    }
    let original_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(original)
        .map_err(io_failure)?;
    let expected_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(expected)
        .map_err(io_failure)?;
    let mut relocated_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(relocated)
        .map_err(io_failure)?;
    if file_identity_text(&original_file)? != file_identity_text(&expected_file)?
        || file_hash(original)? != file_hash(expected)?
        || file_hash(original)? != file_hash(maintenance)?
        || hash_reader(&mut relocated_file)? != file_hash(maintenance)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Relocated uninstall source identity does not match maintenance authority.",
        ));
    }
    drop((original_file, expected_file));
    Ok(relocated_file)
}

pub(super) fn create_relocated_image(source: &Path) -> Result<(PathBuf, File)> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!(".TalkingQuill-uninstall-{suffix}.exe"));
    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(&path)
        .map_err(io_failure)?;
    let mut source = File::open(source).map_err(io_failure)?;
    std::io::copy(&mut source, &mut target).map_err(io_failure)?;
    target.sync_all().map_err(io_failure)?;
    Ok((path, target))
}

pub(super) fn launch_relocated(
    executable: &Path,
    channel: &ControllerChannel,
) -> Result<OwnedHandle> {
    let file = wide(executable.as_os_str());
    let parameters = wide(OsStr::new(if channel.silent {
        "/TQ-RELOCATED /S"
    } else {
        "/TQ-RELOCATED"
    }));
    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 || info.hProcess.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot relocate the uninstall controller.",
        ));
    }
    let shell_process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    channel.authenticate(&shell_process, executable, None)
}

pub(super) fn launch_same_token_uninstall_cleanup(image: &Path) -> Result<()> {
    let channel = ControllerChannel::create(Action::Uninstall, true, 0)?;
    let mut child = Command::new(image).arg("/S").spawn().map_err(io_failure)?;
    let mut duplicate = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            child.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        let _ = child.kill();
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retain the elevated uninstall cleanup process.",
        ));
    }
    let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let process = channel.authenticate(&shell, image, None)?;
    let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), 700_000) };
    if wait != WAIT_OBJECT_0 {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_FAILURE as u32) };
        unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(fail(
            EXIT_FAILURE,
            "Elevated uninstall cleanup did not complete.",
        ));
    }
    let mut code = EXIT_FAILURE as u32;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 || code != 0 {
        return Err(fail(code as i32, "Elevated uninstall cleanup failed."));
    }
    let _ = child.wait();
    Ok(())
}

pub(super) fn elevate(
    executable: &Path,
    _silent: bool,
    channel: &ControllerChannel,
    before_accept: Option<&dyn Fn() -> Result<()>>,
) -> Result<i32> {
    let file = wide(executable.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let parameters = wide(OsStr::new("/S"));
    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(fail(
            EXIT_ELEVATION,
            if unsafe { GetLastError() } == ERROR_CANCELLED {
                "Setup elevation was cancelled."
            } else {
                "Setup elevation failed."
            },
        ));
    }
    if info.hProcess.is_null() {
        return Err(fail(
            EXIT_ELEVATION,
            "The elevated worker handle is missing.",
        ));
    }
    let shell_process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let process = channel.authenticate(&shell_process, executable, before_accept)?;
    let deadline = Instant::now() + Duration::from_secs(600);
    let completion = pipe_read::<COMPLETION_BYTES>(
        channel.handle.as_raw_handle(),
        Some(process.as_raw_handle()),
        deadline,
    );
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .min(30_000) as u32;
    let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), remaining) };
    if wait != WAIT_OBJECT_0 {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_ELEVATION as u32) };
        let _ = unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(fail(
            EXIT_ELEVATION,
            if wait == WAIT_TIMEOUT {
                "The elevated worker exceeded its absolute lifecycle deadline."
            } else {
                "Waiting for the elevated worker failed."
            },
        ));
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(fail(
            EXIT_ELEVATION,
            "Cannot read the elevated worker result.",
        ));
    }
    let reported = decode_completion(&completion?)?;
    if reported != code as i32 {
        return Err(fail(
            EXIT_FAILURE,
            "Setup worker completion and exit status did not match.",
        ));
    }
    Ok(reported)
}
