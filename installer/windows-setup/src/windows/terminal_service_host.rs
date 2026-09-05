//! SCM dispatcher and status callbacks. The service role never displays UI.
use super::*;

static TERMINAL_SERVICE_GENERATION: OnceLock<String> = OnceLock::new();

pub(super) fn terminal_service_name(generation: &str) -> Result<String> {
    validate_machine_lock_suffix(generation)?;
    Ok(format!("{TERMINAL_SERVICE_PREFIX}{generation}"))
}

pub(super) fn run_terminal_service_dispatcher(generation: &str) -> Result<i32> {
    validate_machine_lock_suffix(generation)?;
    TERMINAL_SERVICE_GENERATION
        .set(generation.to_owned())
        .map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Terminal service generation was already set.",
            )
        })?;
    let mut service_name = wide(OsStr::new(&terminal_service_name(generation)?));
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_mut_ptr(),
            lpServiceProc: Some(terminal_service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Terminal cleanup service dispatcher failed.",
        ));
    }
    Ok(0)
}

unsafe extern "system" fn terminal_service_control(_control: u32) {}

unsafe extern "system" fn terminal_service_main(_argc: u32, _argv: *mut *mut u16) {
    let Some(generation) = TERMINAL_SERVICE_GENERATION.get() else {
        return;
    };
    let service_name = match terminal_service_name(generation) {
        Ok(value) => value,
        Err(_) => return,
    };
    let handle = unsafe {
        RegisterServiceCtrlHandlerW(
            wide(OsStr::new(&service_name)).as_ptr(),
            Some(terminal_service_control),
        )
    };
    if handle.is_null() {
        return;
    }
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_START_PENDING,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 1,
        dwWaitHint: 120_000,
    };
    unsafe { SetServiceStatus(handle, &status) };
    status.dwCurrentState = SERVICE_RUNNING;
    status.dwCheckPoint = 0;
    status.dwWaitHint = 0;
    unsafe { SetServiceStatus(handle, &status) };
    let result = run_terminal_cleanup_service(generation);
    status.dwCurrentState = SERVICE_STOPPED;
    match result {
        Ok(()) => {
            status.dwWin32ExitCode = 0;
            status.dwServiceSpecificExitCode = 0;
        }
        Err(error) => {
            status.dwWin32ExitCode = ERROR_SERVICE_SPECIFIC_ERROR;
            status.dwServiceSpecificExitCode = error.code as u32;
        }
    }
    unsafe { SetServiceStatus(handle, &status) };
}
