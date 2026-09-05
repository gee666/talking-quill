//! Inspect lifecycle namespace ownership and remaining registrations.
use super::*;

pub(super) fn directory_names(path: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(path)
        .map_err(io_failure)?
        .map(|entry| entry.map(|value| value.file_name().to_string_lossy().into_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(io_failure)?;
    names.sort_unstable();
    Ok(names)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NamespaceSupervisorClaim {
    pub(super) pid: u32,
    pub(super) creation_time: u64,
    pub(super) image_path: String,
    pub(super) image_identity: String,
    pub(super) image_sha256: String,
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn authenticated_namespace_supervisor() -> Result<Option<u32>> {
    let Some(raw) = std::env::var_os("TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM") else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .ok_or_else(|| fail(EXIT_REJECTED, "Namespace supervisor claim is not UTF-8."))?;
    authenticate_namespace_supervisor_claim(raw).map(Some)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn authenticate_namespace_supervisor_claim(raw: &str) -> Result<u32> {
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
pub(super) fn authenticated_namespace_supervisor() -> Result<Option<u32>> {
    Ok(None)
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

pub(super) fn hive_has_owned_run_value(hive: HKEY) -> Result<bool> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(false);
    }
    if status != 0 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect a Run owner hive."));
    }
    let result = (|| {
        let mut index = 0;
        loop {
            let mut name = [0_u16; 512];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if status == 259 {
                return Ok(false);
            }
            if status != 0 {
                return Err(fail(EXIT_REJECTED, "Cannot enumerate Run owners."));
            }
            if String::from_utf16_lossy(&name[..length as usize])
                .to_ascii_lowercase()
                .starts_with("talking quill")
            {
                return Ok(true);
            }
            index += 1;
        }
    })();
    unsafe { RegCloseKey(key) };
    result
}

pub(super) fn no_owned_run_values() -> Result<bool> {
    if hive_has_owned_run_value(HKEY_LOCAL_MACHINE)? {
        return Ok(false);
    }
    let loaded = enumerate_registry_subkeys(HKEY_USERS)?;
    for hive_name in &loaded {
        if !hive_name.starts_with("S-1-5-") || hive_name.ends_with("_Classes") {
            continue;
        }
        let mut hive = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                wide(OsStr::new(hive_name)).as_ptr(),
                0,
                KEY_READ,
                &mut hive,
            )
        } != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A loaded user hive changed during inspection.",
            ));
        }
        let owned = hive_has_owned_run_value(hive);
        unsafe { RegCloseKey(hive) };
        if owned? {
            return Ok(false);
        }
    }
    const PROFILE_LIST: &str = r"Software\Microsoft\Windows NT\CurrentVersion\ProfileList";
    let mut profiles = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(PROFILE_LIST)).as_ptr(),
            0,
            KEY_READ,
            &mut profiles,
        )
    } != 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect offline profile Run owners.",
        ));
    }
    let profile_sids = match enumerate_registry_subkeys(profiles) {
        Ok(value) => value,
        Err(error) => {
            unsafe { RegCloseKey(profiles) };
            return Err(error);
        }
    };
    for sid in profile_sids {
        if !sid.starts_with("S-1-5-") || loaded.iter().any(|value| value == &sid) {
            continue;
        }
        let mut profile = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                profiles,
                wide(OsStr::new(&sid)).as_ptr(),
                0,
                KEY_READ,
                &mut profile,
            )
        } != 0
        {
            unsafe { RegCloseKey(profiles) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect an offline profile path.",
            ));
        }
        let profile_path = read_profile_image_path(profile);
        unsafe { RegCloseKey(profile) };
        let profile_path = match profile_path {
            Ok(value) => value,
            Err(error) => {
                unsafe { RegCloseKey(profiles) };
                return Err(error);
            }
        };
        let Some(ntuser) = profile_path.map(|path| path.join("NTUSER.DAT")) else {
            continue;
        };
        if !path_present(&ntuser)? {
            continue;
        }
        let mut offline = ptr::null_mut();
        if unsafe {
            RegLoadAppKeyW(
                wide(ntuser.as_os_str()).as_ptr(),
                &mut offline,
                KEY_READ,
                0,
                0,
            )
        } != 0
        {
            unsafe { RegCloseKey(profiles) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect an offline profile Run owner.",
            ));
        }
        let owned = hive_has_owned_run_value(offline);
        unsafe { RegCloseKey(offline) };
        if owned? {
            unsafe { RegCloseKey(profiles) };
            return Ok(false);
        }
    }
    unsafe { RegCloseKey(profiles) };
    Ok(true)
}

pub(super) fn enumerate_registry_value_names(key: HKEY) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut index = 0;
    loop {
        let mut name = [0_u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == 259 {
            break;
        }
        if status != 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot enumerate cleanup registry values.",
            ));
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
        index += 1;
    }
    names.sort_unstable();
    Ok(names)
}

pub(super) fn registry_value_names(root: HKEY, path: &str) -> Result<Option<Vec<String>>> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect cleanup registry values.",
        ));
    }
    let result = enumerate_registry_value_names(key);
    unsafe { RegCloseKey(key) };
    result.map(Some)
}

pub(super) fn registry_subkeys(root: HKEY, path: &str) -> Result<Option<Vec<String>>> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale registry inventory.",
        ));
    }
    let values = enumerate_registry_subkeys(key);
    unsafe { RegCloseKey(key) };
    let mut values = values?;
    values.sort_unstable();
    Ok(Some(values))
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

pub(super) const STALE_REGISTRY_HARDENED_SDDL: &str = "O:BAG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)";
pub(super) const STALE_REGISTRY_LEGACY_SDDL: &str = "O:S-1-5-21-1333774511-1103852894-3119617217-1001G:S-1-5-21-1333774511-1103852894-3119617217-513D:AI(A;CIID;KR;;;BU)(A;CIID;KA;;;BA)(A;CIID;KA;;;SY)(A;ID;KA;;;S-1-5-21-1333774511-1103852894-3119617217-1001)(A;CIIOID;KA;;;CO)(A;CIID;KR;;;AC)(A;CIID;KR;;;S-1-15-3-1024-1065365936-1281604716-3511738428-1654721687-432734479-3232135806-4053264122-3456934681)";
pub(super) const REGISTRY_DESCRIPTOR_INFORMATION: u32 =
    OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;

pub(super) const SYNTHETIC_SCHEMA2_GENERATION: &str = "78bd88811b14faf1e11ba59620088aa0";
pub(super) const SYNTHETIC_SCHEMA2_PENDING: &str =
    ".relaunch-pending-f3d466fe9027728be142ba84d6258074";
pub(super) const SYNTHETIC_SCHEMA2_SHA256: &str =
    "abb2d6183c58b6ec52e28f6befbe43d949d2eeaf1998122921118272da8f3bad";
pub(super) const SYNTHETIC_SCHEMA2_BYTES: &[u8] = br#"{"schemaVersion":2,"generation":"78bd88811b14faf1e11ba59620088aa0","userSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","logonSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","request":"--windows-update-bootstrap-v2=dGVzdA==","nonce":"11111111111111111111111111111111","sourceVersion":"0.0.69","targetVersion":"0.0.70","phase":"armed","completedVersion":null,"predecessor":{"version":"0.0.69","platform":"win32","architecture":"x64","sourceCommit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceTree":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","releaseBuildDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","roles":[]}}"#;
