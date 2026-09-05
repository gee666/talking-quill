//! Removal of obsolete service and task registrations.

use super::*;

pub(super) fn retire_legacy_authority(paths: &Paths) -> Result<()> {
    const LEGACY_SERVICE: &str = "TalkingQuillKeyboardAuthority";
    let manager_raw = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager_raw.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open Windows Service Control Manager.",
        ));
    }
    let manager = ServiceHandle(manager_raw);
    let service_name = wide(OsStr::new(LEGACY_SERVICE));
    let service_raw = unsafe {
        OpenServiceW(
            manager.0,
            service_name.as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_STOP | DELETE,
        )
    };
    if service_raw.is_null() && unsafe { GetLastError() } != 1060 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect the exact legacy service registration.",
        ));
    }
    if !service_raw.is_null() {
        let service = ServiceHandle(service_raw);
        let mut needed = 0;
        unsafe { QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect legacy service identity.",
            ));
        }
        let mut config = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
        if unsafe {
            QueryServiceConfigW(
                service.0,
                config.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(fail(EXIT_REJECTED, "Cannot read legacy service identity."));
        }
        let config = unsafe { &*(config.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()) };
        let binary = if config.lpBinaryPathName.is_null() {
            String::new()
        } else {
            unsafe { wide_ptr_string(config.lpBinaryPathName) }
        };
        let executable = service_executable(&binary)
            .ok_or_else(|| fail(EXIT_REJECTED, "Legacy service binary command is invalid."))?;
        if !owned_legacy_executable(&executable, paths)? {
            return Err(fail(
                EXIT_REJECTED,
                "Legacy service name belongs to an unexpected executable identity.",
            ));
        }
        let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(fail(EXIT_FAILURE, "Cannot query legacy service status."));
        }
        if status.dwCurrentState != SERVICE_STOPPED {
            // A broken service can reject STOP or never report completion.
            let _ = unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) };
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
                return Err(fail(EXIT_FAILURE, "Cannot poll legacy service stop."));
            }
            if status.dwCurrentState == SERVICE_STOPPED {
                break;
            }
            if Instant::now() >= deadline {
                terminate_tree_processes(&paths.legacy_authority, None)?;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if unsafe { DeleteService(service.0) } == 0 {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot delete the exact legacy service.",
            ));
        }
    }
    retire_legacy_task(paths)?;
    terminate_tree_processes(&paths.legacy_authority, None)?;
    if paths.legacy_authority.exists() {
        assert_plain_absent(&paths.legacy_quarantine)?;
        durable_rename(&paths.legacy_authority, &paths.legacy_quarantine)?;
    }
    remove_plain_tree(&paths.legacy_quarantine)?;
    Ok(())
}

pub(super) fn service_executable(command: &str) -> Option<PathBuf> {
    let value = command.trim();
    let executable = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"')?.0
    } else {
        value.split_whitespace().next()?
    };
    if executable.is_empty() {
        None
    } else {
        Some(PathBuf::from(executable))
    }
}

pub(super) fn owned_legacy_executable(executable: &Path, paths: &Paths) -> Result<bool> {
    // An obsolete registration often survives deletion of its executable.
    // Accept only a normalized path within our exact legacy directory, and
    // reject reparse points on every surviving ancestor before removing it.
    let normalized = executable
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase();
    let root = paths.legacy_authority.to_string_lossy().to_lowercase();
    if !normalized
        .strip_prefix(&root)
        .is_some_and(|suffix| suffix.starts_with('\\'))
        || executable
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Ok(false);
    }
    for ancestor in executable.ancestors() {
        if path_present(ancestor)? {
            let metadata = fs::symlink_metadata(ancestor).map_err(io_failure)?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) fn retire_legacy_task(paths: &Paths) -> Result<()> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let result = (|| {
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| fail(EXIT_FAILURE, "Cannot create Task Scheduler service."))?;
        unsafe {
            service.Connect(
                &VARIANT::default(),
                &VARIANT::default(),
                &VARIANT::default(),
                &VARIANT::default(),
            )
        }
        .map_err(|_| fail(EXIT_FAILURE, "Cannot connect to Task Scheduler."))?;
        let root = unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|_| fail(EXIT_FAILURE, "Cannot open the Task Scheduler root."))?;
        let name = BSTR::from("TalkingQuillKeyboardAuthority");
        match unsafe { root.GetTask(&name) } {
            Ok(task) => {
                let actions = unsafe { task.Definition() }
                    .and_then(|definition| unsafe { definition.Actions() })
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot inspect legacy task actions."))?;
                let mut count = 0;
                unsafe { actions.Count(&mut count) }
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot count legacy task actions."))?;
                if count != 1 {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Legacy task must have exactly one owned action.",
                    ));
                }
                let action = unsafe { actions.get_Item(1) }
                    .and_then(|action| action.cast::<IExecAction>())
                    .map_err(|_| {
                        fail(
                            EXIT_REJECTED,
                            "Legacy task action is not an executable action.",
                        )
                    })?;
                let mut executable = BSTR::new();
                unsafe { action.Path(&mut executable) }
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot read legacy task executable."))?;
                let executable = PathBuf::from(executable.to_string());
                if !owned_legacy_executable(&executable, paths)? {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Legacy task name belongs to an unexpected executable identity.",
                    ));
                }
                let _ = unsafe { task.Stop(0) };
                unsafe { root.DeleteTask(&name, 0) }.map_err(|_| {
                    fail(
                        EXIT_FAILURE,
                        "Cannot delete the exact legacy scheduled task.",
                    )
                })?;
            }
            Err(error) if error.code().0 == -2147024894 => {}
            Err(_) => return Err(fail(EXIT_FAILURE, "Cannot inspect legacy scheduled task.")),
        }
        if paths.legacy_task_file.exists() {
            return Err(fail(
                EXIT_REJECTED,
                "Task Scheduler retained the legacy task file.",
            ));
        }
        Ok(())
    })();
    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}
