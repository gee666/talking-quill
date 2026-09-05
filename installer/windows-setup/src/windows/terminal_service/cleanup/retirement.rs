//! Wait for service retirement before removing the authenticated image.
use super::*;

pub(in super::super::super) fn remove_retired_terminal_service_image(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<bool> {
    let image = PathBuf::from(&record.service_image);
    if !path_present(&image)? {
        return Ok(true);
    }
    validate_terminal_service_image(record, &image)?;
    if terminal_force_pending_delete() {
        schedule_terminal_service_deletion(&image)?;
        return Ok(false);
    }
    match fs::remove_file(&image) {
        Ok(()) => {
            flush_setup_directory(&paths.program_data)?;
            Ok(true)
        }
        Err(_) => {
            // Registration is already absent. A reboot may now own deletion of only this inert,
            // authenticated image; maintenance and the journal remain callable until it is gone.
            schedule_terminal_service_deletion(&image)?;
            Ok(false)
        }
    }
}

pub(in super::super::super) fn wait_for_terminal_service_retirement(
    paths: &Paths,
    generation: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(240);
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    loop {
        let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
        if manager.is_null() {
            return Err(fail(EXIT_FAILURE, "Cannot poll terminal cleanup service."));
        }
        let service = unsafe {
            OpenServiceW(
                manager,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG | DELETE,
            )
        };
        if service.is_null() {
            let error = unsafe { GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            if error == 1072 {
                if Instant::now() >= deadline {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Terminal service deletion did not commit.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            if error != 1060 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot inspect terminal cleanup service.",
                ));
            }
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if !complete {
                return Err(fail(
                    EXIT_REJECTED,
                    "Terminal service disappeared before cleanup completed.",
                ));
            }
            terminal_maintenance_crash_at("post-delete-pre-image-removal");
            if !remove_retired_terminal_service_image(paths, &record)? {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal service image deletion requires reboot.",
                ));
            }
            return finish_terminal_uninstall(paths);
        }
        let service = ServiceHandle(service);
        let manager = ServiceHandle(manager);
        let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(fail(EXIT_FAILURE, "Cannot query terminal cleanup service."));
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if status.dwWin32ExitCode == 0 && complete {
                verify_terminal_service_registration(paths, &record)?;
                terminal_maintenance_crash_at("service-stopped-pre-DeleteService");
                if unsafe { DeleteService(service.0) } == 0 && unsafe { GetLastError() } != 1072 {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Cannot delete terminal cleanup service.",
                    ));
                }
                drop(service);
                drop(manager);
                continue;
            }
            if status.dwWin32ExitCode == ERROR_SERVICE_SPECIFIC_ERROR {
                // Failure actions are configured for non-crash failures. Poll through the bounded
                // SCM restart interval without converting the test into a manual restart.
            } else if status.dwWin32ExitCode != 0 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal cleanup service stopped unexpectedly.",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(fail(
                EXIT_FAILURE,
                "Terminal cleanup service did not complete.",
            ));
        }
        drop(service);
        drop(manager);
        std::thread::sleep(Duration::from_millis(250));
    }
}
