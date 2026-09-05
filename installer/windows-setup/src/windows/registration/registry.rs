//! Native uninstall and application registry operations.
use super::*;

pub(in super::super) fn register_uninstall_executable(executable: &Path) -> Result<()> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open uninstall recovery registration.",
        ));
    }
    let values = [
        ("UninstallString", format!("\"{}\"", executable.display())),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", executable.display()),
        ),
    ];
    for (name, value) in values {
        let bytes = wide(OsStr::new(&value));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        } != 0
        {
            unsafe { RegCloseKey(key) };
            return Err(fail(EXIT_FAILURE, "Cannot register uninstall recovery."));
        }
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(EXIT_FAILURE, "Cannot flush uninstall recovery."))
    }
}

pub(in super::super) fn registered_uninstall_executable() -> Result<Option<PathBuf>> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read uninstall recovery registration.",
        ));
    }
    let value = read_registry_value(key, "UninstallString", 4096)?;
    unsafe { RegCloseKey(key) };
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    let path = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'));
    Ok(path.map(PathBuf::from))
}

pub(in super::super) fn read_registry_value(
    key: *mut c_void,
    name: &str,
    maximum: u32,
) -> Result<Option<String>> {
    let name = wide(OsStr::new(name));
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || bytes < 2 || bytes > maximum || !bytes.is_multiple_of(2) {
        return Err(fail(EXIT_REJECTED, "Registry string is invalid."));
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
        return Err(fail(EXIT_FAILURE, "Cannot read registry string."));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Registry string is invalid."))
}

pub(in super::super) fn register_uninstall(paths: &Paths, version: &str) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native uninstall registration.",
        ));
    }
    let values = [
        ("DisplayName", "Talking Quill".to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "Talking Quill contributors".to_string()),
        ("InstallLocation", paths.install.display().to_string()),
        (
            "UninstallString",
            format!("\"{}\"", paths.maintenance_uninstaller.display()),
        ),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", paths.maintenance_uninstaller.display()),
        ),
    ];
    let result = values.iter().try_for_each(|(name, value)| {
        let bytes = wide(OsStr::new(value));
        let status = unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(fail(
                EXIT_FAILURE,
                "Cannot write the native uninstall registration.",
            ))
        }
    });
    let flushed = result.is_ok() && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        result
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot durably write the native uninstall registration.",
        ))
    }
}

pub(in super::super) fn register_app_path(paths: &Paths) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(APP_PATH_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native application registration.",
        ));
    }
    let executable = wide(paths.install.join("Talking Quill.exe").as_os_str());
    let directory = wide(paths.install.as_os_str());
    let first = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("")).as_ptr(),
            0,
            REG_SZ,
            executable.as_ptr().cast(),
            (executable.len() * 2) as u32,
        )
    };
    let second = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Path")).as_ptr(),
            0,
            REG_SZ,
            directory.as_ptr().cast(),
            (directory.len() * 2) as u32,
        )
    };
    let flushed = first == 0 && second == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot write the native application registration.",
        ))
    }
}

pub(in super::super) fn unregister_app_path() -> Result<()> {
    delete_registry_tree_durable(
        APP_PATH_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        "native application registration",
    )
}
