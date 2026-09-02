#[cfg(not(windows))]
fn main() {
    std::process::exit(64);
}

#[cfg(windows)]
fn main() {
    use std::io::Read;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        relative_path: String,
        directory: bool,
        identity: String,
    }

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let [mode, command] = arguments.as_slice()
        && mode == "--supervise"
    {
        let Some(command) = command.to_str() else {
            std::process::exit(64);
        };
        match supervise(command) {
            Ok(code) => std::process::exit(code as i32),
            Err(()) => std::process::exit(74),
        }
    }
    match arguments.as_slice() {
        [mode, path, prefix] if mode == "--create-protected-root" => {
            let Some(prefix) = prefix.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::create_protected_root(
                std::path::Path::new(path),
                prefix,
            ) {
                Ok(identity) => {
                    println!("{identity}");
                    return;
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, prefix] if mode == "--remove-interrupted-root" => {
            let Some(prefix) = prefix.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::remove_interrupted_root(
                std::path::Path::new(path),
                prefix,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode] if mode == "--registry-delete-empty-root" => {
            match talking_quill_helper::machine_lock_test_namespace::delete_empty_registry_root() {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        [mode] if mode == "--registry-root-inventory" => {
            match talking_quill_helper::machine_lock_test_namespace::registry_root_inventory() {
                Ok(inventory) => {
                    println!("{}", serde_json::to_string(&inventory).unwrap());
                    return;
                }
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-create" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::create_registry_namespace(
                namespace,
            ) {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-inventory" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::registry_inventory(namespace) {
                Ok(inventory) => {
                    println!("{}", serde_json::to_string(&inventory).unwrap());
                    return;
                }
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-delete-exact" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            let mut json = String::new();
            if std::io::stdin().read_to_string(&mut json).is_err() {
                std::process::exit(65);
            }
            let Ok(inventory) = serde_json::from_str(&json) else {
                std::process::exit(65);
            };
            match talking_quill_helper::machine_lock_test_namespace::delete_registry_exact(
                namespace, &inventory,
            ) {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        _ => {}
    }
    let result = match arguments.as_slice() {
        [mode, path] if mode == "--flush-directory" => {
            talking_quill_helper::owned_tree::flush_owned_directory(std::path::Path::new(path))
        }
        [mode, path] if mode == "--identity" => {
            match talking_quill_helper::owned_tree::owned_tree_identity(std::path::Path::new(path))
            {
                Ok(identity) => {
                    println!("{identity}");
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        [mode, path, identity] if mode == "--exact" || mode == "--resume-exact" => {
            let Some(identity) = identity.to_str() else {
                std::process::exit(64);
            };
            let mut json = String::new();
            if std::io::stdin().read_to_string(&mut json).is_err() {
                std::process::exit(65);
            }
            let Ok(entries) = serde_json::from_str::<Vec<Entry>>(&json) else {
                std::process::exit(65);
            };
            let entries = entries
                .into_iter()
                .map(
                    |entry| talking_quill_helper::owned_tree::ExactOwnedTreeEntry {
                        relative_path: entry.relative_path,
                        directory: entry.directory,
                        identity: entry.identity,
                    },
                )
                .collect::<Vec<_>>();
            if mode == "--resume-exact" {
                talking_quill_helper::owned_tree::remove_remaining_exact_owned_tree(
                    std::path::Path::new(path),
                    identity,
                    &entries,
                )
            } else {
                talking_quill_helper::owned_tree::remove_exact_owned_tree(
                    std::path::Path::new(path),
                    identity,
                    &entries,
                )
            }
        }
        [path, identity] => {
            let Some(identity) = identity.to_str() else {
                std::process::exit(64);
            };
            talking_quill_helper::owned_tree::remove_owned_tree(
                std::path::Path::new(path),
                identity,
            )
        }
        _ => std::process::exit(64),
    };
    if result.is_err() {
        std::process::exit(78);
    }
}

#[cfg(windows)]
fn supervise(command: &str) -> Result<u32, ()> {
    use std::{mem::zeroed, os::windows::ffi::OsStrExt, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, INFINITE,
                PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, WaitForSingleObject,
            },
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    let job = Handle(unsafe { CreateJobObjectW(null_mut(), null_mut()) });
    if job.0.is_null() {
        return Err(());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(());
    }
    let mut command_line = std::ffi::OsStr::new(&format!("cmd.exe /d /s /c \"{command}\""))
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            null_mut(),
            command_line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            1,
            CREATE_SUSPENDED,
            null_mut(),
            null_mut(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(());
    }
    let process_handle = Handle(process.hProcess);
    let thread_handle = Handle(process.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0
        || unsafe { ResumeThread(thread_handle.0) } == u32::MAX
    {
        unsafe { TerminateJobObject(job.0, 74) };
        return Err(());
    }
    if unsafe { WaitForSingleObject(process_handle.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(());
    }
    loop {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        if unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(());
        }
        if accounting.ActiveProcesses == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
        return Err(());
    }
    Ok(exit_code)
}
