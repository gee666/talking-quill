//! Registry security descriptors and exact lock publication handles.
use super::*;

pub(super) struct LocalSecurityDescriptor(*mut c_void);

impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RegistryAceShape {
    pub(super) kind: u8,
    pub(super) flags: u8,
    pub(super) mask: u32,
    pub(super) sid: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RegistryDescriptorShape {
    pub(super) owner: Vec<u8>,
    pub(super) group: Vec<u8>,
    pub(super) control: u16,
    pub(super) acl_revision: u8,
    pub(super) aces: Vec<RegistryAceShape>,
}

pub(super) fn descriptor_from_sddl(sddl: &str, exit_code: i32) -> Result<LocalSecurityDescriptor> {
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
        return Err(fail(exit_code, "Cannot create stale registry cleanup ACL."));
    }
    Ok(LocalSecurityDescriptor(descriptor))
}

pub(super) fn query_registry_descriptor(key: HKEY) -> Result<LocalSecurityDescriptor> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            key,
            SE_REGISTRY_KEY,
            REGISTRY_DESCRIPTOR_INFORMATION,
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
            "Cannot inspect machine lock registry ACL.",
        ));
    }
    Ok(LocalSecurityDescriptor(descriptor))
}

pub(super) fn descriptor_sddl(descriptor: &LocalSecurityDescriptor) -> Result<String> {
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            REGISTRY_DESCRIPTOR_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode machine lock registry ACL.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(value.to_ascii_uppercase())
}

pub(super) fn descriptor_sid_bytes(sid: *mut c_void) -> Option<Vec<u8>> {
    if sid.is_null() {
        return None;
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    (length > 0).then(|| unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec())
}

pub(super) fn registry_descriptor_shape(
    descriptor: &LocalSecurityDescriptor,
) -> Option<RegistryDescriptorShape> {
    let mut owner = ptr::null_mut();
    let mut group = ptr::null_mut();
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted) } == 0
        || unsafe { GetSecurityDescriptorGroup(descriptor.0, &mut group, &mut defaulted) } == 0
    {
        return None;
    }
    let mut present = 0;
    let mut dacl = ptr::null_mut();
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) }
        == 0
        || present == 0
        || dacl.is_null()
    {
        return None;
    }
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return None;
    }
    let mut aces = Vec::with_capacity(unsafe { (*dacl).AceCount } as usize);
    let mut used_acl_bytes = mem::size_of_val(unsafe { &*dacl });
    for index in 0..unsafe { (*dacl).AceCount } as u32 {
        let mut raw = ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut raw) } == 0 || raw.is_null() {
            return None;
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        let sid_offset = mem::size_of::<ACCESS_ALLOWED_ACE>() - mem::size_of::<u32>();
        if ace.Header.AceType != 0 || (ace.Header.AceSize as usize) < sid_offset + 8 {
            return None;
        }
        let sid_pointer = ptr::addr_of!(ace.SidStart).cast::<u8>();
        let sub_authorities = unsafe { *sid_pointer.add(1) } as usize;
        let sid_length = 8_usize.checked_add(4_usize.checked_mul(sub_authorities)?)?;
        if ace.Header.AceSize as usize != sid_offset + sid_length {
            return None;
        }
        let sid = unsafe { std::slice::from_raw_parts(sid_pointer, sid_length) }.to_vec();
        used_acl_bytes = used_acl_bytes.checked_add(ace.Header.AceSize as usize)?;
        aces.push(RegistryAceShape {
            kind: ace.Header.AceType,
            flags: ace.Header.AceFlags,
            mask: ace.Mask,
            sid,
        });
    }
    if used_acl_bytes != unsafe { (*dacl).AclSize } as usize {
        return None;
    }
    Some(RegistryDescriptorShape {
        owner: descriptor_sid_bytes(owner)?,
        group: descriptor_sid_bytes(group)?,
        control,
        acl_revision: unsafe { (*dacl).AclRevision },
        aces,
    })
}

pub(super) fn hardened_registry_descriptor_is_exact(
    descriptor: &LocalSecurityDescriptor,
) -> Result<bool> {
    let expected = descriptor_from_sddl(STALE_REGISTRY_HARDENED_SDDL, EXIT_FAILURE)?;
    Ok(registry_descriptor_shape(descriptor).is_some()
        && registry_descriptor_shape(descriptor) == registry_descriptor_shape(&expected))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StaleRegistryAclAdmission {
    LegacyExactParent,
    Hardened,
}

pub(super) fn stale_registry_acl_admission(
    parent_sddl: &str,
    child_sddl: &str,
    parent_descriptor: &LocalSecurityDescriptor,
    child_descriptor: &LocalSecurityDescriptor,
) -> Result<Option<StaleRegistryAclAdmission>> {
    let legacy = STALE_REGISTRY_LEGACY_SDDL.to_ascii_uppercase();
    if child_sddl == legacy && parent_sddl == legacy && child_sddl == parent_sddl {
        return Ok(Some(StaleRegistryAclAdmission::LegacyExactParent));
    }
    if hardened_registry_descriptor_is_exact(parent_descriptor)?
        && hardened_registry_descriptor_is_exact(child_descriptor)?
    {
        Ok(Some(StaleRegistryAclAdmission::Hardened))
    } else {
        Ok(None)
    }
}

pub(super) fn protect_stale_registry_key(key: HKEY, path: &str) -> Result<()> {
    let descriptor = descriptor_from_sddl(STALE_REGISTRY_HARDENED_SDDL, EXIT_FAILURE)?;
    let status = unsafe {
        RegSetKeySecurity(
            key,
            REGISTRY_DESCRIPTOR_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot protect stale registry cleanup state.",
        ));
    }

    let mut reopened = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(path)).as_ptr(),
            REG_OPTION_OPEN_LINK,
            KEY_READ,
            &mut reopened,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot reopen protected stale registry state.",
        ));
    }
    let observed = query_registry_descriptor(reopened);
    unsafe { RegCloseKey(reopened) };
    if !hardened_registry_descriptor_is_exact(&observed?)? {
        return Err(fail(
            EXIT_FAILURE,
            "Protected stale registry descriptor did not verify structurally.",
        ));
    }
    Ok(())
}

pub(super) struct ExactMachineLockPublication {
    pub(super) suffix: String,
    pub(super) parent: HKEY,
    pub(super) child: HKEY,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(super) acl_admission: StaleRegistryAclAdmission,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(super) parent_sddl: String,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(super) child_sddl: String,
}

impl Drop for ExactMachineLockPublication {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.child);
            RegCloseKey(self.parent);
        }
    }
}

pub(super) fn exact_machine_lock_publication() -> Result<Option<ExactMachineLockPublication>> {
    exact_machine_lock_publication_with_access(KEY_READ)
}

pub(super) fn exact_machine_lock_publication_for_mutation()
-> Result<Option<ExactMachineLockPublication>> {
    exact_machine_lock_publication_with_access(KEY_READ | KEY_WRITE | WRITE_DAC | WRITE_OWNER)
}

pub(super) fn exact_machine_lock_publication_with_access(
    access: u32,
) -> Result<Option<ExactMachineLockPublication>> {
    let mut parent = ptr::null_mut();
    let parent_status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(r"Software\Talking Quill")).as_ptr(),
            REG_OPTION_OPEN_LINK,
            access,
            &mut parent,
        )
    };
    if parent_status == 2 {
        return Ok(None);
    }
    if parent_status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect machine lock registry parent.",
        ));
    }
    let mut child = ptr::null_mut();
    let child_status = unsafe {
        RegOpenKeyExW(
            parent,
            wide(OsStr::new("RecoveryStateLockV1")).as_ptr(),
            REG_OPTION_OPEN_LINK,
            access,
            &mut child,
        )
    };
    if child_status != 0 {
        unsafe { RegCloseKey(parent) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect machine lock publication.",
        ));
    }

    let inspected = (|| {
        let suffix = read_machine_lock_registry_string(child)?
            .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock registry value is missing."))?;
        validate_machine_lock_suffix(&suffix)?;
        let child_descriptor = query_registry_descriptor(child)?;
        let parent_descriptor = query_registry_descriptor(parent)?;
        let child_sddl = descriptor_sddl(&child_descriptor)?;
        let parent_sddl = descriptor_sddl(&parent_descriptor)?;
        let acl_admission = stale_registry_acl_admission(
            &parent_sddl,
            &child_sddl,
            &parent_descriptor,
            &child_descriptor,
        )?
        .ok_or_else(|| {
            fail(
                EXIT_REJECTED,
                "Machine lock registry ACL is not the recognized fixture ACL.",
            )
        })?;
        #[cfg(not(feature = "stale-schema2-cleanup"))]
        let _ = acl_admission;

        let parent_subkeys = enumerate_registry_subkeys(parent)?;
        let parent_values = enumerate_registry_value_names(parent)?;
        let child_values = enumerate_registry_value_names(child)?;
        let child_subkeys = enumerate_registry_subkeys(child)?;
        if parent_subkeys != ["RecoveryStateLockV1"]
            || !parent_values.is_empty()
            || !child_subkeys.is_empty()
            || child_values != [MACHINE_LOCK_REGISTRY_VALUE]
        {
            return Err(fail(
                EXIT_REJECTED,
                "Machine lock registry inventory is not exact.",
            ));
        }
        Ok(ExactMachineLockPublication {
            suffix,
            parent,
            child,
            #[cfg(feature = "stale-schema2-cleanup")]
            acl_admission,
            #[cfg(feature = "stale-schema2-cleanup")]
            parent_sddl,
            #[cfg(feature = "stale-schema2-cleanup")]
            child_sddl,
        })
    })();
    match inspected {
        Ok(publication) => Ok(Some(publication)),
        Err(error) => {
            unsafe {
                RegCloseKey(child);
                RegCloseKey(parent);
            }
            Err(error)
        }
    }
}

pub(super) fn protected_file_handle_acl_is_exact(file: &File) -> Result<bool> {
    protected_handle_acl_is_exact(file, false)
}

pub(super) fn protected_handle_acl_is_exact(file: &File, directory: bool) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
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
        return Err(fail(EXIT_REJECTED, "Cannot inspect protected file ACL."));
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
        return Err(fail(EXIT_REJECTED, "Cannot encode protected file ACL."));
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
