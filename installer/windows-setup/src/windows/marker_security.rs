//! Compare retained paths against the permissions required for that specific path.
use super::*;

struct Descriptor(*mut c_void);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // Windows allocates both parsed and queried descriptors with LocalAlloc.
        unsafe { LocalFree(self.0) };
    }
}

impl Descriptor {
    fn parse(sddl: &str) -> Result<Self> {
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(OsStr::new(sddl)).as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
            || descriptor.is_null()
        {
            return Err(fail(EXIT_FAILURE, "Cannot parse protected marker ACL."));
        }
        Ok(Self(descriptor))
    }

    fn for_path(path: &Path) -> Result<Self> {
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
            return Err(fail(EXIT_REJECTED, "Cannot inspect protected marker ACL."));
        }
        Ok(Self(descriptor))
    }

    fn owner_and_dacl(&self) -> Result<String> {
        let mut text = ptr::null_mut();
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                self.0,
                SDDL_REVISION_1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            )
        } == 0
            || text.is_null()
        {
            return Err(fail(EXIT_REJECTED, "Cannot encode protected marker ACL."));
        }
        let mut length = 0;
        // A successful conversion returns a null-terminated UTF-16 string.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        unsafe { LocalFree(text.cast()) };
        Ok(value)
    }
}

pub(super) fn marker_security_is_exact(path: &Path, sddl: &str) -> Result<bool> {
    if sddl == MACHINE_LOCK_FILE_SDDL {
        // Retain compatibility with either ordering of the two privileged ACEs.
        return staged_path_is_protected(path, false);
    }
    let expected = Descriptor::parse(sddl)?.owner_and_dacl()?;
    let actual = Descriptor::for_path(path)?.owner_and_dacl()?;
    Ok(actual.eq_ignore_ascii_case(&expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_path_is_checked_against_its_requested_permissions() {
        let root = std::env::temp_dir().join(format!("tq-specific-acl-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let original = Descriptor::for_path(&root)
            .unwrap()
            .owner_and_dacl()
            .unwrap();
        let sid = original
            .strip_prefix("O:")
            .unwrap()
            .split("D:")
            .next()
            .unwrap();
        let directory_acl = format!("O:{sid}D:P(A;OICI;FA;;;{sid})");
        let file_acl = format!("O:{sid}D:P(A;;FA;;;{sid})");
        apply_lock_dacl(&root, &directory_acl).unwrap();
        let file = root.join("marker");
        fs::write(&file, "test").unwrap();
        apply_lock_dacl(&file, &file_acl).unwrap();
        assert!(marker_security_is_exact(&root, &directory_acl).unwrap());
        assert!(marker_security_is_exact(&file, &file_acl).unwrap());
        assert!(!marker_security_is_exact(&root, &file_acl).unwrap());
        assert!(!marker_security_is_exact(&file, &directory_acl).unwrap());
        assert!(!marker_security_is_exact(&file, MEDIUM_LAUNCHER_FILE_SDDL).unwrap());
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
