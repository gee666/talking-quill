use super::*;

pub(super) fn nul_handles() -> Result<(Handle, Handle), &'static str> {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let name: Vec<u16> = "NUL".encode_utf16().chain([0]).collect();
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let read = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    let write = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    let read = Handle(read);
    let write = Handle(write);
    if read.0 == INVALID_HANDLE_VALUE || write.0 == INVALID_HANDLE_VALUE {
        return Err("nul handles");
    }
    Ok((read, write))
}

pub(super) fn verify_parent_and_creation(pid: u32, process: HANDLE) -> Result<(), &'static str> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snapshot.0 == INVALID_HANDLE_VALUE {
        return Err("process ancestry");
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut found = false;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == pid {
                found = entry.th32ParentProcessID == unsafe { GetCurrentProcessId() };
                break;
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if !found
        || unsafe {
            windows_sys::Win32::System::Threading::GetProcessTimes(
                process,
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
    {
        return Err("process parent identity");
    }
    let mut parent_creation = FILETIME::default();
    if unsafe {
        windows_sys::Win32::System::Threading::GetProcessTimes(
            GetCurrentProcess(),
            &mut parent_creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
        || filetime_value(creation) < filetime_value(parent_creation)
    {
        return Err("process creation identity");
    }
    Ok(())
}

fn filetime_value(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

pub(super) fn spawn_writer(
    mut output: File,
    bytes: Vec<u8>,
    error: &'static str,
) -> std::sync::mpsc::Receiver<Result<(), &'static str>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = output.write_all(&bytes).map_err(|_| error);
        drop(output);
        let _ = sender.send(result);
    });
    receiver
}

fn raw_pipe() -> Result<(HANDLE, HANDLE), &'static str> {
    let mut read = null_mut();
    let mut write = null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err("pipe create");
    }
    Ok((read, write))
}

pub(super) fn input_pipe() -> Result<(Handle, File), &'static str> {
    let (read, write) = raw_pipe()?;
    let read = Handle(read);
    let write = unsafe { File::from_raw_handle(write) };
    make_non_inheritable(write.as_raw_handle())?;
    Ok((read, write))
}

pub(super) fn output_pipe() -> Result<(File, Handle), &'static str> {
    let (read, write) = raw_pipe()?;
    let read = unsafe { File::from_raw_handle(read) };
    let write = Handle(write);
    make_non_inheritable(read.as_raw_handle())?;
    Ok((read, write))
}

pub(super) fn attribute_list(handles: &[HANDLE]) -> Result<Attributes, &'static str> {
    let mut bytes = 0usize;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
    if bytes == 0 {
        return Err("attribute size");
    }
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let list = storage.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
        return Err("attribute list");
    }
    let attributes = Attributes {
        list,
        _storage: storage,
    };
    if unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of_val(handles),
            null_mut(),
            null_mut(),
        )
    } == 0
    {
        return Err("attribute list");
    }
    Ok(attributes)
}

pub(super) fn kill_job() -> Result<Handle, &'static str> {
    let job = Handle(unsafe { CreateJobObjectW(null(), null()) });
    if job.0.is_null() {
        return Err("job create");
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
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
        return Err("job policy");
    }
    Ok(job)
}

pub(super) fn make_inheritable(handle: HANDLE) -> Result<(), &'static str> {
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        Err("handle inherit")
    } else {
        Ok(())
    }
}
pub(super) fn make_non_inheritable(handle: HANDLE) -> Result<(), &'static str> {
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        Err("handle seal")
    } else {
        Ok(())
    }
}
