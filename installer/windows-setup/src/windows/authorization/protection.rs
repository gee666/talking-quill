//! Staged helper and medium launcher protection checks.
use super::*;

pub(in super::super) fn staged_path_is_protected(path: &Path, directory: bool) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect staged helper protection.",
        ));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode staged helper protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    let expected = if directory {
        [
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)",
        ]
    } else {
        [
            "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)",
        ]
    };
    Ok(expected
        .iter()
        .any(|value| sddl.eq_ignore_ascii_case(value)))
}

pub(in super::super) fn medium_launcher_directory_is_protected(path: &Path) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect launcher staging protection.",
        ));
    }
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode launcher staging protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.eq_ignore_ascii_case("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)"))
}
