//! Retire legacy relaunch records in Windows user profiles.
use super::*;

pub(super) fn enumerate_registry_subkeys(key: *mut c_void) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut index = 0;
    loop {
        let mut name = [0_u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
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
            return Err(fail(EXIT_FAILURE, "Cannot enumerate user registry hives."));
        }
        values.push(String::from_utf16_lossy(&name[..length as usize]));
        index += 1;
    }
    Ok(values)
}

pub(super) fn clear_legacy_relaunch_values_in_hive(hive: *mut c_void, paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const PREFIX: &str = "Talking Quill Update Relaunch ";
    let mut run = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut run,
        )
    };
    if opened == 2 {
        return Ok(());
    }
    if opened != 0 {
        return Ok(());
    }
    let launcher = paths.recovery_launcher.clone();
    let mut index = 0;
    loop {
        let mut name = [0_u16; 512];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                run,
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
            break;
        }
        let value_name = String::from_utf16_lossy(&name[..length as usize]);
        let Some(generation) = value_name.strip_prefix(PREFIX) else {
            index += 1;
            continue;
        };
        if validate_machine_lock_suffix(generation).is_err() {
            index += 1;
            continue;
        }
        let expected = format!(
            "\"{}\" --windows-update-relaunch-v1={generation}",
            launcher.display()
        );
        if read_registry_value(run, &value_name, 2048)
            .ok()
            .flatten()
            .as_deref()
            != Some(expected.as_str())
        {
            index += 1;
            continue;
        }
        if unsafe { RegDeleteValueW(run, wide(OsStr::new(&value_name)).as_ptr()) } != 0 {
            index += 1;
        }
    }
    let _ = unsafe { RegFlushKey(run) };
    unsafe { RegCloseKey(run) };
    Ok(())
}

pub(super) fn read_profile_image_path(key: *mut c_void) -> Result<Option<PathBuf>> {
    let name = wide(OsStr::new("ProfileImagePath"));
    let mut kind = 0;
    let mut bytes = 0;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 || !matches!(kind, REG_SZ | REG_EXPAND_SZ) || bytes > 32768 {
        return Err(fail(EXIT_REJECTED, "Profile path is invalid."));
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot read profile path."));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    let text =
        String::from_utf16(&value).map_err(|_| fail(EXIT_REJECTED, "Profile path is invalid."))?;
    let source = wide(OsStr::new(&text));
    let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), ptr::null_mut(), 0) };
    if needed == 0 || needed > 32768 {
        return Err(fail(EXIT_REJECTED, "Profile path expansion failed."));
    }
    let mut expanded = vec![0_u16; needed as usize];
    if unsafe { ExpandEnvironmentStringsW(source.as_ptr(), expanded.as_mut_ptr(), needed) }
        != needed
    {
        return Err(fail(EXIT_REJECTED, "Profile path expansion failed."));
    }
    if expanded.last() == Some(&0) {
        expanded.pop();
    }
    let expanded = String::from_utf16(&expanded)
        .map_err(|_| fail(EXIT_REJECTED, "Profile path is invalid."))?;
    let path = PathBuf::from(&expanded);
    if expanded.contains('%')
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(fail(EXIT_REJECTED, "Profile path is not canonical."));
    }
    Ok(Some(path))
}

pub(super) fn remove_legacy_profile_relaunch_records(root: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let generation = entry.file_name().to_string_lossy().into_owned();
        if validate_machine_lock_suffix(&generation).is_err() {
            continue;
        }
        let directory = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&directory) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        let record_path = directory.join("relaunch-record-v1.json");
        let Ok(bytes) = fs::read(&record_path) else {
            continue;
        };
        if bytes.is_empty() || bytes.len() > 64 * 1024 {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let exact = matches!(
            value
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64),
            Some(1 | 2)
        ) && value.get("generation").and_then(serde_json::Value::as_str)
            == Some(generation.as_str())
            && value
                .get("request")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|request| request.starts_with("--windows-update-bootstrap-v2="));
        if exact {
            let _ = remove_plain_tree(&directory);
        }
    }
    let _ = fs::remove_dir(root);
    Ok(())
}

pub(super) fn clear_legacy_profile_relaunch_owners(paths: &Paths) -> Result<()> {
    let loaded = enumerate_registry_subkeys(HKEY_USERS).unwrap_or_default();
    for sid in loaded
        .iter()
        .filter(|sid| sid.starts_with("S-1-5-") && !sid.ends_with("_Classes"))
    {
        let mut hive = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                wide(OsStr::new(sid)).as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut hive,
            )
        } != 0
        {
            continue;
        }
        let _ = clear_legacy_relaunch_values_in_hive(hive, paths);
        unsafe { RegCloseKey(hive) };
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
        return Err(fail(EXIT_FAILURE, "Cannot open the profile inventory."));
    }
    for sid in enumerate_registry_subkeys(profiles).unwrap_or_default() {
        if !sid.starts_with("S-1-5-") {
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
            continue;
        }
        let profile_path = read_profile_image_path(profile).ok().flatten();
        unsafe { RegCloseKey(profile) };
        let Some(profile_path) = profile_path else {
            continue;
        };
        let legacy_records =
            profile_path.join("AppData/Local/Talking Quill/Windows Update Recovery");
        remove_legacy_profile_relaunch_records(&legacy_records)?;
        if loaded.iter().any(|value| value == &sid) {
            continue;
        }
        let ntuser = profile_path.join("NTUSER.DAT");
        if fs::symlink_metadata(&ntuser).is_err() {
            continue;
        }
        let mut offline = ptr::null_mut();
        if unsafe {
            RegLoadAppKeyW(
                wide(ntuser.as_os_str()).as_ptr(),
                &mut offline,
                KEY_READ | KEY_WRITE,
                0,
                0,
            )
        } != 0
        {
            continue;
        }
        let _ = clear_legacy_relaunch_values_in_hive(offline, paths);
        let _ = unsafe { RegFlushKey(offline) };
        unsafe { RegCloseKey(offline) };
    }
    unsafe { RegCloseKey(profiles) };
    Ok(())
}
