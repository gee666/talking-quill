//! Legacy mutex acquisition and exact security verification.
use super::*;

pub(in super::super) struct LegacyMutexPair([OwnedHandle; 2]);
impl LegacyMutexPair {
    pub(in super::super) fn acquire() -> Result<Self> {
        let names = machine_lock_mutex_names()?;
        Ok(Self([
            acquire_verified_legacy_mutex(&names[0])?,
            acquire_verified_legacy_mutex(&names[1])?,
        ]))
    }
}
impl Drop for LegacyMutexPair {
    fn drop(&mut self) {
        for handle in self.0.iter().rev() {
            unsafe { ReleaseMutex(handle.as_raw_handle()) };
        }
    }
}

pub(in super::super) fn acquire_verified_legacy_mutex(name: &str) -> Result<OwnedHandle> {
    let sddl = wide(OsStr::new("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create the legacy lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let raw = unsafe { CreateMutexW(&attributes, 0, wide(OsStr::new(name)).as_ptr()) };
    unsafe { LocalFree(descriptor) };
    if raw.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open the legacy machine lock."));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if !matches!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) },
        0 | 0x80
    ) || !legacy_mutex_security_is_exact(handle.as_raw_handle())?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Legacy machine lock identity is invalid.",
        ));
    }
    Ok(handle)
}

pub(in super::super) fn legacy_mutex_security_is_exact(handle: *mut c_void) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
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
            "Cannot inspect the legacy machine lock.",
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
            "Cannot encode the legacy machine lock ACL.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
        .to_ascii_uppercase();
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.starts_with("O:BA")
        && (value.contains("(A;;GA;;;SY)") || value.contains("(A;;0X1F0001;;;SY)"))
        && (value.contains("(A;;GA;;;BA)") || value.contains("(A;;0X1F0001;;;BA)"))
        && value.matches("(A;;").count() == 2
        && !value.contains(";;;AU)"))
}
