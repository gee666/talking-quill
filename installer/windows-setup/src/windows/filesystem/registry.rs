//! Durable registry deletion and registry presence checks.
use super::*;

pub(in super::super) fn unregister_uninstall() -> Result<()> {
    delete_registry_tree_durable(
        UNINSTALL_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "native uninstall registration",
    )
}

pub(in super::super) fn delete_machine_lock_registry_durable(
    path: &str,
    parent: &str,
    label: &str,
) -> Result<()> {
    delete_registry_tree_durable_in_hive(machine_lock_registry_hive(), path, parent, label)
}

pub(in super::super) fn delete_registry_tree_durable(
    path: &str,
    parent: &str,
    label: &str,
) -> Result<()> {
    delete_registry_tree_durable_in_hive(HKEY_LOCAL_MACHINE, path, parent, label)
}

pub(in super::super) fn delete_registry_tree_durable_in_hive(
    hive: HKEY,
    path: &str,
    parent: &str,
    label: &str,
) -> Result<()> {
    let status = unsafe { RegDeleteTreeW(hive, wide(OsStr::new(path)).as_ptr()) };
    if status != 0 && status != 2 {
        return Err(fail(EXIT_FAILURE, format!("Cannot remove the {label}.")));
    }
    let mut deleted = ptr::null_mut();
    let observed = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(path)).as_ptr(),
            0,
            KEY_READ,
            &mut deleted,
        )
    };
    if observed == 0 {
        unsafe { RegCloseKey(deleted) };
        return Err(fail(EXIT_FAILURE, format!("Windows retained the {label}.")));
    }
    if observed != 2 {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot verify removal of the {label}."),
        ));
    }
    let mut parent_key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(parent)).as_ptr(),
            0,
            KEY_READ,
            &mut parent_key,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot open the {label} parent."),
        ));
    }
    let flushed = unsafe { RegFlushKey(parent_key) } == 0;
    unsafe { RegCloseKey(parent_key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            format!("Cannot flush removal of the {label}."),
        ))
    }
}

#[cfg(any(test, feature = "stale-schema2-cleanup"))]
pub(in super::super) fn registry_key_present(root: HKEY, path: &str) -> Result<bool> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 0 {
        unsafe { RegCloseKey(key) };
        Ok(true)
    } else if status == 2 {
        Ok(false)
    } else {
        Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale coordination registry state.",
        ))
    }
}
