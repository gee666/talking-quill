//! Decode update requests and verify the installed application.
use super::*;

pub(super) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

pub(super) fn current_parent_process_id() -> Result<u32, i32> {
    let snapshot_raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot_raw == -1_isize as HANDLE {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot_raw) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let current = unsafe { GetCurrentProcessId() };
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == current && entry.th32ParentProcessID != 0 {
                return Ok(entry.th32ParentProcessID);
            }
            if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
                break;
            }
        }
    }
    Err(EXIT_IDENTITY_MISMATCH)
}

pub(super) fn verify_app_ready_parent() -> Result<(), i32> {
    verify_installed_application_process(current_parent_process_id()?)
}

pub(super) fn verify_installed_application_process(process_id: u32) -> Result<(), i32> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if raw.is_null() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut image = vec![0_u16; 32_768];
    let mut length = image.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, image.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    image.truncate(length as usize);
    let actual = PathBuf::from(String::from_utf16(&image).map_err(|_| EXIT_IDENTITY_MISMATCH)?);
    let expected = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill/Talking Quill.exe");
    if paths_equal(&actual, &expected) {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn native_setup_transaction_present() -> Result<bool, i32> {
    let path =
        known_folder(&FOLDERID_ProgramFiles)?.join(".Talking Quill.native-transaction-v2.json");
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(
            metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(EXIT_IDENTITY_MISMATCH),
    }
}

pub(super) fn authorize_public_update_bootstrap(argument: &str) -> Result<(), i32> {
    let encoded = argument
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    let request = parse_and_authorize_request(encoded)?;
    let installed = installed_manifest_version()?;
    if request.candidate.predecessor.version != installed
        || !version_at_least(&installed, PUBLIC_UPDATE_TRUST_ROOT)
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

pub(super) fn version_at_least(value: &str, minimum: &str) -> bool {
    let parse = |version: &str| -> Option<(u64, u64, u64)> {
        let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
        let result = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(result)
    };
    parse(value)
        .zip(parse(minimum))
        .is_some_and(|(value, minimum)| value >= minimum)
}

pub(super) fn valid_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part == &"0" || !part.starts_with('0'))
        })
}

pub(super) fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn acquire_relaunch_intent_lock(path: &Path) -> Result<std::fs::File, i32> {
    let lock = path.with_file_name("windows-update-relaunch-intent-v1.lock");
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(lock)
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn read_relaunch_intent(
    path: &Path,
    expected_nonce: &str,
) -> Result<RelaunchIntent, i32> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if !path.is_absolute()
        || path.file_name().and_then(|value| value.to_str())
            != Some("windows-update-relaunch-intent-v1.json")
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case("Talking Quill"))
        || !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() > 4096
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let intent: RelaunchIntent =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?)
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if intent.schema_version != 1
        || intent.nonce != expected_nonce
        || !valid_version(&intent.source_version)
        || !valid_version(&intent.target_version)
        || intent.source_version == intent.target_version
        || !matches!(
            intent.phase.as_str(),
            "armed" | "setup-started" | "setup-complete" | "launch-started" | "app-ready"
        )
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(intent)
}

pub(super) fn installed_manifest() -> Result<InstalledManifest, i32> {
    let path = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill/resources/keyboard-owner-release-v1.json");
    serde_json::from_slice(&std::fs::read(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}

pub(super) fn installed_manifest_version() -> Result<String, i32> {
    Ok(installed_manifest()?.version)
}

pub(super) fn verified_surviving_version(
    candidate: &UpdateCandidate,
    stored_predecessor: &InstalledManifest,
) -> Result<String, i32> {
    if installed_candidate_committed(candidate) {
        verify_installed_candidate_files(candidate)?;
        return Ok(candidate.version.clone());
    }
    let installed = installed_manifest()?;
    let predecessor = &candidate.predecessor;
    if stored_predecessor.version != predecessor.version
        || stored_predecessor.platform != predecessor.platform
        || stored_predecessor.architecture != predecessor.architecture
        || stored_predecessor.release_build_digest != predecessor.release_build_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let role_hash = |role: &str| {
        installed
            .roles
            .iter()
            .find(|value| value.role == role)
            .map(|value| value.sha256.as_str())
    };
    if installed.version == predecessor.version
        && installed.platform == predecessor.platform
        && installed.architecture == predecessor.architecture
        && installed.release_build_digest == predecessor.release_build_digest
        && role_hash("gateway") == Some(predecessor.gateway_sha256.as_str())
        && role_hash("owner") == Some(predecessor.owner_sha256.as_str())
    {
        verify_installed_snapshot(stored_predecessor)?;
        Ok(predecessor.version.clone())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn verify_installed_candidate_files(candidate: &UpdateCandidate) -> Result<(), i32> {
    let root = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill");
    for role in &candidate.roles {
        let mut file = open_locked(&root.join(&role.path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&role.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    Ok(())
}
