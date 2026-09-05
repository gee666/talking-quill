//! Relocated and elevated process launch and completion.
use super::*;

pub(in super::super) fn launch_relocated(
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

pub(in super::super) fn launch_same_token_uninstall_cleanup(image: &Path) -> Result<()> {
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

pub(in super::super) fn elevate(
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
