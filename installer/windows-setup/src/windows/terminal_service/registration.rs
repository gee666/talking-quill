//! Service handle acquisition and registered identity verification.
use super::*;

pub(in super::super) fn terminal_service_exists(name: &str) -> Result<bool> {
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

pub(in super::super) fn terminal_service_handle(
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

pub(in super::super) fn terminal_service_restarts_are_exact(service: SC_HANDLE) -> Result<bool> {
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
    if actions.dwResetPeriod != RESTART_RESET_SECONDS
        || actions.cActions != RESTART_ACTION_COUNT as u32
        || actions.lpsaActions.is_null()
    {
        return Ok(false);
    }
    let actions = unsafe { std::slice::from_raw_parts(actions.lpsaActions, RESTART_ACTION_COUNT) };
    if !actions
        .iter()
        .all(|action| action.Type == SC_ACTION_RESTART && action.Delay == RESTART_DELAY_MS)
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

pub(in super::super) fn verify_terminal_service_registration_mode(
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

pub(in super::super) fn verify_terminal_service_registration(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<()> {
    verify_terminal_service_registration_mode(paths, record, true)
}
