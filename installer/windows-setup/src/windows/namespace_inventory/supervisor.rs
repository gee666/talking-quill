//! Authenticated test namespace supervisor identity and production fallback.
use super::*;

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in super::super) struct NamespaceSupervisorClaim {
    pub(in super::super) pid: u32,
    pub(in super::super) creation_time: u64,
    pub(in super::super) image_path: String,
    pub(in super::super) image_identity: String,
    pub(in super::super) image_sha256: String,
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(in super::super) fn authenticated_namespace_supervisor() -> Result<Option<u32>> {
    let Some(raw) = std::env::var_os("TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM") else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .ok_or_else(|| fail(EXIT_REJECTED, "Namespace supervisor claim is not UTF-8."))?;
    authenticate_namespace_supervisor_claim(raw).map(Some)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(in super::super) fn authenticate_namespace_supervisor_claim(raw: &str) -> Result<u32> {
    let claim: NamespaceSupervisorClaim = serde_json::from_str(raw)
        .map_err(|_| fail(EXIT_REJECTED, "Namespace supervisor claim is malformed."))?;
    let identity_valid = claim
        .image_identity
        .split_once(':')
        .is_some_and(|(volume, file)| volume.parse::<u32>().is_ok() && file.parse::<u64>().is_ok());
    if claim.pid == 0
        || claim.image_path.is_empty()
        || !identity_valid
        || claim.image_sha256.len() != 64
        || !claim
            .image_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err(fail(
            EXIT_REJECTED,
            "Namespace supervisor claim fields are invalid.",
        ));
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot authenticate namespace supervisor ancestry.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut parents = std::collections::HashMap::new();
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot enumerate namespace supervisor ancestry.",
        ));
    }
    loop {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            if unsafe { GetLastError() } != 18 {
                return Err(fail(
                    EXIT_REJECTED,
                    "Namespace supervisor ancestry changed.",
                ));
            }
            break;
        }
    }
    let mut ancestor = unsafe { GetCurrentProcessId() };
    let mut matched = false;
    for _ in 0..parents.len() {
        let Some(parent) = parents.get(&ancestor).copied() else {
            break;
        };
        if parent == claim.pid {
            matched = true;
            break;
        }
        if parent == 0 || parent == ancestor {
            break;
        }
        ancestor = parent;
    }
    if !matched {
        return Err(fail(
            EXIT_REJECTED,
            "Namespace supervisor is not a process ancestor.",
        ));
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
            0,
            claim.pid,
        )
    };
    if process.is_null() {
        return Err(fail(EXIT_REJECTED, "Cannot open the namespace supervisor."));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT {
        return Err(fail(EXIT_REJECTED, "Namespace supervisor is not alive."));
    }
    let mut creation: FILETIME = unsafe { mem::zeroed() };
    let mut exit: FILETIME = unsafe { mem::zeroed() };
    let mut kernel: FILETIME = unsafe { mem::zeroed() };
    let mut user: FILETIME = unsafe { mem::zeroed() };
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
        || (((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
            != claim.creation_time
    {
        return Err(fail(
            EXIT_REJECTED,
            "Namespace supervisor creation identity changed.",
        ));
    }
    let image = process_image(claim.pid)?;
    let expected_image = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp/cargo-target/machine-lock-test-wrapper/debug/talking-quill-test-tree-delete.exe")
        .canonicalize()
        .map_err(io_failure)?;
    let canonical_image = image.canonicalize().map_err(io_failure)?;
    if canonical_image != expected_image
        || image.as_os_str().to_string_lossy().to_lowercase() != claim.image_path.to_lowercase()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Namespace supervisor image path changed.",
        ));
    }
    let file = File::open(&canonical_image).map_err(io_failure)?;
    if file_identity_text(&file)? != claim.image_identity
        || hex_hash(&file_hash(&canonical_image)?) != claim.image_sha256
    {
        return Err(fail(
            EXIT_REJECTED,
            "Namespace supervisor image identity changed.",
        ));
    }
    Ok(claim.pid)
}

#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(in super::super) fn authenticated_namespace_supervisor() -> Result<Option<u32>> {
    Ok(None)
}
