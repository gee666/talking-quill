//! Inspect lifecycle namespace ownership and remaining registrations.
use super::*;

mod supervisor;
pub(super) use supervisor::*;

mod run_values;
pub(super) use run_values::*;

mod registry;
pub(super) use registry::*;

mod policy;
pub(super) use policy::*;

pub(super) fn directory_names(path: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(path)
        .map_err(io_failure)?
        .map(|entry| entry.map(|value| value.file_name().to_string_lossy().into_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(io_failure)?;
    names.sort_unstable();
    Ok(names)
}

pub(super) fn no_talking_quill_process_except_authenticated_pair(
    authenticated_parent: bool,
) -> Result<bool> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale coordination processes.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let parent = authenticated_parent.then(parent_process_id).transpose()?;
    let namespace_supervisor = authenticated_namespace_supervisor()?;
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot enumerate stale coordination processes.",
        ));
    }
    loop {
        let executable_length = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let snapshot_name =
            String::from_utf16_lossy(&entry.szExeFile[..executable_length]).to_ascii_lowercase();
        let candidate = snapshot_name.starts_with("talking quill")
            || snapshot_name.starts_with("talking-quill");
        if candidate
            && entry.th32ProcessID != std::process::id()
            && Some(entry.th32ProcessID) != parent
            && Some(entry.th32ProcessID) != namespace_supervisor
        {
            let image = process_image(entry.th32ProcessID)?;
            let name = image
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if name == snapshot_name {
                return Ok(false);
            }
            return Err(fail(EXIT_REJECTED, "A candidate process identity changed."));
        }
        if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            if unsafe { GetLastError() } != 18 {
                return Err(fail(
                    EXIT_REJECTED,
                    "Process inventory changed during inspection.",
                ));
            }
            break;
        }
    }
    Ok(true)
}

pub(super) fn no_owned_task_files(system: &Path) -> Result<bool> {
    let tasks = system.join("Tasks");
    Ok(!directory_names(&tasks)?
        .iter()
        .any(|name| name.to_ascii_lowercase().starts_with("talkingquill")))
}

pub(super) fn no_owned_service_keys() -> Result<bool> {
    Ok(
        !registry_subkeys(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services")?
            .unwrap_or_default()
            .iter()
            .any(|name| name.to_ascii_lowercase().starts_with("talkingquill")),
    )
}
