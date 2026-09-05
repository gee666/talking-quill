//! Feature-gated crash and failure injection.
use super::*;

#[cfg(feature = "acceptance-faults")]
pub(super) fn take_terminal_acceptance_fault(phase: &str) -> Result<bool> {
    const KEY: &str = r"Software\Talking Quill\AcceptanceTerminalFault";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(false);
    }
    if opened != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal acceptance fault.",
        ));
    }
    let selected = read_registry_value(key, "Phase", 128)?;
    unsafe { RegCloseKey(key) };
    if selected.as_deref() != Some(phase) {
        return Ok(false);
    }
    let deleted = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(OsStr::new(KEY)).as_ptr()) };
    let mut parent = ptr::null_mut();
    let flushed = deleted == 0
        && unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                wide(OsStr::new(r"Software")).as_ptr(),
                0,
                KEY_READ,
                &mut parent,
            )
        } == 0
        && unsafe { RegFlushKey(parent) } == 0;
    if !parent.is_null() {
        unsafe { RegCloseKey(parent) };
    }
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot consume terminal acceptance fault.",
        ));
    }
    Ok(true)
}

#[cfg(feature = "acceptance-faults")]
pub(super) fn terminal_maintenance_crash_at(phase: &str) {
    if std::env::var("TQ_TERMINAL_FAULT").as_deref() == Ok(phase)
        || take_terminal_acceptance_fault(phase).unwrap_or(false)
    {
        std::process::exit(197);
    }
}

#[cfg(not(feature = "acceptance-faults"))]
pub(super) fn terminal_maintenance_crash_at(_phase: &str) {}

#[cfg(feature = "acceptance-faults")]
pub(super) fn terminal_force_pending_delete() -> bool {
    std::env::var("TQ_TERMINAL_FAULT").as_deref() == Ok("reboot-pending-delete")
        || take_terminal_acceptance_fault("reboot-pending-delete").unwrap_or(false)
}

#[cfg(not(feature = "acceptance-faults"))]
pub(super) fn terminal_force_pending_delete() -> bool {
    false
}

#[cfg(feature = "acceptance-faults")]
pub(super) fn terminal_service_fail_once(phase: &str) -> Result<()> {
    if take_terminal_acceptance_fault(phase)? {
        Err(fail(EXIT_FAILURE, "Injected terminal service failure."))
    } else {
        Ok(())
    }
}

#[cfg(not(feature = "acceptance-faults"))]
pub(super) fn terminal_service_fail_once(_phase: &str) -> Result<()> {
    Ok(())
}
