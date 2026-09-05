//! Validate and schedule deletion of exact retained cleanup images.
use super::*;

pub(super) fn decode_pending_rename_pairs(data: &[u16]) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut cursor = 0;
    let mut terminated = false;
    while cursor < data.len() {
        let source_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(fail(EXIT_REJECTED, "Pending deletion data is truncated."));
        }
        if cursor == source_start {
            let required_terminators = if pairs.is_empty() { 2 } else { 1 };
            if data.len() - cursor < required_terminators
                || data[cursor..].iter().any(|value| *value != 0)
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Pending deletion terminator is invalid.",
                ));
            }
            terminated = true;
            break;
        }
        let source = String::from_utf16(&data[source_start..cursor])
            .map_err(|_| fail(EXIT_REJECTED, "Pending deletion source is invalid."))?;
        cursor += 1;
        let destination_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(fail(EXIT_REJECTED, "Pending deletion pair is truncated."));
        }
        let destination = String::from_utf16(&data[destination_start..cursor])
            .map_err(|_| fail(EXIT_REJECTED, "Pending deletion destination is invalid."))?;
        cursor += 1;
        pairs.push((source, destination));
    }
    if !terminated {
        return Err(fail(
            EXIT_REJECTED,
            "Pending deletion data lacks its final terminator.",
        ));
    }
    Ok(pairs)
}

pub(super) fn read_pending_rename_pairs(key: HKEY) -> Result<Vec<(String, String)>> {
    const VALUE: &str = "PendingFileRenameOperations";
    let mut bytes = 0_u32;
    let mut value_type = 0_u32;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            wide(OsStr::new(VALUE)).as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if queried == 2 {
        return Ok(Vec::new());
    }
    if queried != 0
        || value_type != REG_MULTI_SZ
        || bytes == 0
        || bytes > 1024 * 1024
        || !bytes.is_multiple_of(2)
    {
        return Err(fail(EXIT_FAILURE, "Pending deletion ownership is invalid."));
    }
    let capacity = bytes;
    let mut data = vec![0_u16; bytes as usize / 2];
    let mut actual = bytes;
    let mut actual_type = 0_u32;
    if unsafe {
        RegQueryValueExW(
            key,
            wide(OsStr::new(VALUE)).as_ptr(),
            ptr::null_mut(),
            &mut actual_type,
            data.as_mut_ptr().cast(),
            &mut actual,
        )
    } != 0
        || actual_type != REG_MULTI_SZ
        || actual > capacity
        || !actual.is_multiple_of(2)
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read pending deletion ownership.",
        ));
    }
    data.truncate(actual as usize / 2);
    decode_pending_rename_pairs(&data)
}

pub(super) fn open_session_manager(access: u32) -> Result<HKEY> {
    const SESSION_MANAGER: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(SESSION_MANAGER)).as_ptr(),
            0,
            access,
            &mut key,
        )
    } != 0
    {
        Err(fail(EXIT_FAILURE, "Cannot open pending deletion state."))
    } else {
        Ok(key)
    }
}

pub(super) fn normalized_pending_source(value: &str) -> String {
    let value = value.replace('/', "\\");
    if let Some(unc) = value.strip_prefix(r"\??\UNC\") {
        format!(r"\\{}", unc.to_ascii_lowercase())
    } else {
        value
            .strip_prefix(r"\??\")
            .or_else(|| value.strip_prefix(r"\\?\"))
            .unwrap_or(&value)
            .to_ascii_lowercase()
    }
}

pub(super) fn schedule_terminal_service_deletion(path: &Path) -> Result<()> {
    assert_plain_file(path)?;
    let expected_path = canonical(path)?;
    let before_key = open_session_manager(KEY_READ)?;
    let before = read_pending_rename_pairs(before_key)?;
    unsafe { RegCloseKey(before_key) };
    if unsafe {
        MoveFileExW(
            wide(path.as_os_str()).as_ptr(),
            ptr::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Windows could not take terminal service deletion ownership.",
        ));
    }
    let key = open_session_manager(KEY_READ | KEY_WRITE)?;
    let after = read_pending_rename_pairs(key)?;
    let valid = after.len() == before.len() + 1
        && after.get(..before.len()) == Some(before.as_slice())
        && after.last().is_some_and(|(source, destination)| {
            destination.is_empty() && normalized_pending_source(source) == expected_path
        });
    let flushed = valid && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Terminal service deletion ownership is invalid.",
        ));
    }
    Ok(())
}

pub(super) fn schedule_empty_terminal_tombstone_deletion(path: &Path) -> Result<()> {
    if !medium_launcher_directory_is_protected(path)?
        || fs::read_dir(path)
            .map_err(io_failure)?
            .next()
            .transpose()
            .map_err(io_failure)?
            .is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is invalid.",
        ));
    }
    let expected = canonical(path)?;
    let before_key = open_session_manager(KEY_READ)?;
    let before = read_pending_rename_pairs(before_key)?;
    unsafe { RegCloseKey(before_key) };
    if unsafe {
        MoveFileExW(
            wide(path.as_os_str()).as_ptr(),
            ptr::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Windows could not own tombstone deletion.",
        ));
    }
    let key = open_session_manager(KEY_READ | KEY_WRITE)?;
    let after = read_pending_rename_pairs(key)?;
    let valid = after.len() == before.len() + 1
        && after.get(..before.len()) == Some(before.as_slice())
        && after.last().is_some_and(|(source, destination)| {
            destination.is_empty() && normalized_pending_source(source) == expected
        });
    let flushed = valid && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Tombstone deletion ownership is invalid.",
        ))
    }
}

pub(super) fn remove_uninstall_finalizer_residue(paths: &Paths) -> Result<()> {
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let pending_suffix = name.strip_prefix(UNINSTALL_FINALIZER_PENDING_PREFIX);
        let published_suffix = name.strip_prefix(UNINSTALL_FINALIZER_PREFIX);
        if pending_suffix.is_none() && published_suffix.is_none() {
            continue;
        }
        let pending =
            pending_suffix.is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        let published =
            published_suffix.is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        if !pending && !published {
            return Err(fail(EXIT_REJECTED, "Finalizer namespace entry is invalid."));
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !medium_launcher_directory_is_protected(&path)?
        {
            continue;
        }
        let identity =
            owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let marker = path.join("finalizer-tree-identity-v1");
        if pending || fs::read_to_string(marker).is_ok_and(|value| value == identity) {
            let executable = path.join(UNINSTALL_FINALIZER_NAME);
            if path_present(&executable)? {
                arm_mapped_image_deletion(&executable)?;
            }
            remove_owned_tree(&path, &identity)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        }
    }
    Ok(())
}
