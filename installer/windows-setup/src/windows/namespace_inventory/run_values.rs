//! Machine, loaded-profile, and offline-profile Run owner inventory.
use super::*;

pub(in super::super) fn hive_has_owned_run_value(hive: HKEY) -> Result<bool> {
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

pub(in super::super) fn no_owned_run_values() -> Result<bool> {
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
