//! Durable machine lock registry publication and retirement.
use super::*;

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(in super::super) fn machine_lock_terminal_owner_present(_paths: &Paths) -> Result<bool> {
    Ok(false)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(in super::super) fn machine_lock_terminal_owner_present(paths: &Paths) -> Result<bool> {
    Ok(read_terminal_uninstall_record(paths)?.is_some())
}

pub(in super::super) fn machine_lock_file(
    paths: &Paths,
    predecessor_policy_epoch: u8,
) -> Result<PathBuf> {
    let program_data = machine_lock_program_data(&paths.program_data)?;
    let registry_key = machine_lock_registry_key()?;
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            machine_lock_registry_hive(),
            wide(OsStr::new(&registry_key)).as_ptr(),
            0,
            ptr::null(),
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
            "Cannot open the machine lock registry key.",
        ));
    }
    reclaim_machine_lock_pending(&program_data)?;
    let terminal_owner_present = machine_lock_terminal_owner_present(paths)?;
    let publication = read_machine_lock_registry_string(key)?;
    let publication = if let Some(retired) = publication
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_machine_lock_suffix(retired)?;
        let retired_directory =
            program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{retired}"));
        if terminal_owner_present && path_present(&retired_directory)? {
            Some(retired.to_owned())
        } else {
            if unsafe {
                RegDeleteValueW(key, wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr())
            } != 0
                || unsafe { RegFlushKey(key) } != 0
            {
                unsafe { RegCloseKey(key) };
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot clear retired machine lock publication.",
                ));
            }
            if path_present(&retired_directory)? {
                reclaim_retired_machine_lock_directory(&program_data, retired)?;
            }
            None
        }
    } else {
        publication
    };
    let directory = if let Some(suffix) = publication {
        validate_machine_lock_suffix(&suffix)?;
        program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH && !terminal_owner_present {
            unsafe { RegCloseKey(key) };
            return Err(fail(
                EXIT_REJECTED,
                "A retired recovery policy cannot republish the machine lock.",
            ));
        }
        reclaim_unpublished_machine_lock_directories(&program_data)?;
        let suffix = new_machine_lock_suffix()?;
        let token = new_machine_lock_suffix()?;
        let pending = program_data.join(format!("{MACHINE_LOCK_PENDING_PREFIX}{token}"));
        let published = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_restricted_lock_directory(&pending)?;
        apply_lock_dacl(&pending, machine_lock_directory_sddl())?;
        let identity = owned_tree_identity(&pending).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Machine lock directory identity is unavailable.",
            )
        })?;
        create_or_verify_lock_marker(
            &pending.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
        )?;
        initialize_machine_lock_tree(&pending, &identity)?;
        flush_setup_directory(&pending)?;
        if unsafe {
            MoveFileExW(
                wide(pending.as_os_str()).as_ptr(),
                wide(published.as_os_str()).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot publish the machine lock directory.",
            ));
        }
        flush_setup_directory(&program_data)?;
        let value = wide(OsStr::new(&suffix));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } != 0
            || unsafe { RegFlushKey(key) } != 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot persist the machine lock identity.",
            ));
        }
        published
    };
    unsafe { RegCloseKey(key) };
    verify_machine_lock_tree(&directory)
}

pub(in super::super) fn retire_machine_lock_publication(_paths: &Paths) -> Result<String> {
    let registry_key = machine_lock_registry_key()?;
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            machine_lock_registry_hive(),
            wide(OsStr::new(&registry_key)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retire the machine lock publication.",
        ));
    }
    let publication = read_machine_lock_registry_string(key)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock publication is missing."))?;
    unsafe { RegCloseKey(key) };
    let suffix = publication
        .strip_prefix(MACHINE_LOCK_RETIRED_PREFIX)
        .unwrap_or(&publication)
        .to_owned();
    validate_machine_lock_suffix(&suffix)?;
    delete_machine_lock_registry_durable(
        &registry_key,
        &machine_lock_registry_parent()?,
        "machine lock publication",
    )?;
    Ok(suffix)
}

pub(in super::super) fn read_machine_lock_registry_string(
    key: *mut c_void,
) -> Result<Option<String>> {
    let name = wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE));
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
    if first != 0 || kind != REG_SZ || !(2..=256).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock registry value is invalid.",
        ));
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
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read the machine lock registry value.",
        ));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lock registry value is invalid."))
}
