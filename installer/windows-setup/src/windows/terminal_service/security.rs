//! SCM security descriptor and restart policy configuration.
use super::*;

pub(super) const RESTART_DELAY_MS: u32 = 60_000;
pub(super) const RESTART_RESET_SECONDS: u32 = 86_400;
pub(super) const RESTART_ACTION_COUNT: usize = 3;

pub(in super::super) fn terminal_service_dacl_is_exact(service: SC_HANDLE) -> Result<bool> {
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
    let expected_text = convert(expected);
    // Release the converted descriptor even when normalization fails.
    unsafe { LocalFree(expected) };
    let expected_text = expected_text?;
    Ok(convert(actual.as_mut_ptr().cast())? == expected_text)
}

pub(in super::super) fn apply_terminal_service_dacl(service: SC_HANDLE) -> Result<()> {
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

pub(in super::super) fn configure_terminal_service_restarts(service: SC_HANDLE) -> Result<()> {
    let mut actions = [SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: RESTART_DELAY_MS,
    }; RESTART_ACTION_COUNT];
    let failure_actions = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: RESTART_RESET_SECONDS,
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
