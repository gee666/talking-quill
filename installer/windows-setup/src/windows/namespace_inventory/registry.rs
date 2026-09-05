//! Exact registry value and subkey name inventories.
use super::*;

pub(in super::super) fn enumerate_registry_value_names(key: HKEY) -> Result<Vec<String>> {
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

pub(in super::super) fn registry_value_names(
    root: HKEY,
    path: &str,
) -> Result<Option<Vec<String>>> {
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

pub(in super::super) fn registry_subkeys(root: HKEY, path: &str) -> Result<Option<Vec<String>>> {
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
