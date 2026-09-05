//! Security descriptors for retained update files and directories.
use super::*;

pub(super) struct SecurityDescriptor(pub(super) PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    pub(super) fn restricted(sddl: &str) -> Result<Self, i32> {
        let mut descriptor = std::ptr::null_mut();
        let sddl = wide_nul(Path::new(sddl))?;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast()) };
    }
}

pub(super) fn create_restricted_directory(path: &Path) -> Result<(), i32> {
    create_directory_with_sddl(path, RESTRICTED_STAGING_SDDL)
}

pub(super) fn create_directory_with_sddl(path: &Path, sddl: &str) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(sddl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let path = wide_nul(path)?;
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

pub(super) fn security_descriptor_text(descriptor: PSECURITY_DESCRIPTOR) -> Result<String, i32> {
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut text = std::ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            information,
            &mut text,
            std::ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(value)
}

pub(super) fn has_exact_security(path: &Path, sddl: &str) -> Result<bool, i32> {
    let expected = SecurityDescriptor::restricted(sddl)?;
    let expected = security_descriptor_text(expected.0)?;
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let path = wide_nul(path)?;
    let mut needed = 0_u32;
    unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            information,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            information,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(security_descriptor_text(actual.as_mut_ptr().cast())?.eq_ignore_ascii_case(&expected))
}

pub(super) fn apply_relaunch_dacl(path: &Path, identity: &RelaunchIdentity) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(&relaunch_record_sddl(identity))?;
    let path = wide_nul(path)?;
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        Err(EXIT_LAUNCH_FAILED)
    } else {
        Ok(())
    }
}

pub(super) fn has_relaunch_dacl(path: &Path, identity: &RelaunchIdentity) -> Result<bool, i32> {
    let expected = SecurityDescriptor::restricted(&relaunch_record_sddl(identity))?;
    let expected = security_descriptor_text(expected.0)?;
    let path = wide_nul(path)?;
    let mut needed = 0_u32;
    unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(security_descriptor_text(actual.as_mut_ptr().cast())?.eq_ignore_ascii_case(&expected))
}

pub(super) fn apply_restricted_dacl(path: &Path, sddl: &str) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(sddl)?;
    let path = wide_nul(path)?;
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}
