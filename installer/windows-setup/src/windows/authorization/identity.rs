//! Process ancestry, image hashing, and identity encoding.
use super::*;

pub(in super::super) fn process_image(pid: u32) -> Result<PathBuf> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the setup peer process.",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    process_image_from_handle(&process)
}
pub(in super::super) fn hash_reader(file: &mut File) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_failure)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}
pub(in super::super) fn file_hash(path: &Path) -> Result<[u8; 32]> {
    hash_reader(&mut File::open(path).map_err(io_failure)?)
}
pub(in super::super) fn retained_file_hash(path: &Path) -> Result<(File, [u8; 32])> {
    let mut file = File::from(open_plain_handle(path, false, false)?);
    let hash = hash_reader(&mut file)?;
    Ok((file, hash))
}

pub(in super::super) fn parent_process_id() -> Result<u32> {
    process_parent_id(std::process::id())
}

pub(in super::super) fn process_parent_id(process_id: u32) -> Result<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the update parent process.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while available {
        if entry.th32ProcessID == process_id {
            return Ok(entry.th32ParentProcessID);
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Err(fail(
        EXIT_REJECTED,
        "The update parent process is unavailable.",
    ))
}

pub(in super::super) fn hex_bytes(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(in super::super) fn hex_hash(value: &[u8; 32]) -> String {
    hex_bytes(value)
}
