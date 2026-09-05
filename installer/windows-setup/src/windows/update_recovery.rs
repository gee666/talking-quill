//! Retire obsolete update recovery launchers.
use super::*;

pub(super) fn clear_update_recovery(paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const PREFIX: &str = "Talking Quill Update Recovery ";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 0 {
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
                break;
            }
            if status != 0 {
                unsafe { RegCloseKey(key) };
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot enumerate update recovery values.",
                ));
            }
            let value = String::from_utf16_lossy(&name[..length as usize]);
            let owned = value.strip_prefix(PREFIX).is_some_and(|generation| {
                generation.len() == 32
                    && generation
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
            if owned {
                if unsafe { RegDeleteValueW(key, name.as_ptr()) } != 0 {
                    unsafe { RegCloseKey(key) };
                    return Err(fail(EXIT_FAILURE, "Cannot remove update recovery value."));
                }
            } else {
                index += 1;
            }
        }
        if unsafe { RegFlushKey(key) } != 0 {
            unsafe { RegCloseKey(key) };
            return Err(fail(EXIT_FAILURE, "Cannot flush update recovery cleanup."));
        }
        unsafe { RegCloseKey(key) };
    } else if opened != 2 {
        return Err(fail(EXIT_FAILURE, "Cannot open update recovery values."));
    }
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let pending = name
            .strip_prefix(".Talking Quill.update-bootstrap-pending-")
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        let published = name
            .strip_prefix(".Talking Quill.update-bootstrap-")
            .is_some_and(|suffix| {
                suffix.len() == 16
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        let launcher_pending = name
            .strip_prefix(".Talking Quill.update-launcher-pending-")
            .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        if pending || published || launcher_pending {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
            if !metadata.is_dir()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || !(if launcher_pending {
                    medium_launcher_directory_is_protected(&path)?
                } else {
                    staged_path_is_protected(&path, true)?
                })
            {
                continue;
            }
            let identity = owned_tree_identity(&path)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
            let recorded = if launcher_pending {
                fs::read_to_string(path.join("launcher-tree-identity-v1"))
            } else {
                fs::read_to_string(path.join("cleanup-tree-identity-v1"))
            };
            if pending
                || launcher_pending
                || (published && recorded.is_ok_and(|value| value == identity))
            {
                remove_owned_tree(&path, &identity)
                    .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
            }
        }
    }
    // The protected launcher is a stable machine component. Per-user relaunch
    // ownership may live in a different HKCU hive under over-the-shoulder UAC,
    // so update cleanup must not infer that no relaunch owner exists.
    Ok(())
}
