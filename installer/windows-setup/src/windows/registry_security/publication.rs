//! Exact registry publication ownership and admission.
use super::*;

pub(in super::super) struct ExactMachineLockPublication {
    pub(in super::super) suffix: String,
    pub(in super::super) parent: HKEY,
    pub(in super::super) child: HKEY,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(in super::super) acl_admission: StaleRegistryAclAdmission,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(in super::super) parent_sddl: String,
    #[cfg(feature = "stale-schema2-cleanup")]
    pub(in super::super) child_sddl: String,
}

impl Drop for ExactMachineLockPublication {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.child);
            RegCloseKey(self.parent);
        }
    }
}

pub(in super::super) fn exact_machine_lock_publication()
-> Result<Option<ExactMachineLockPublication>> {
    exact_machine_lock_publication_with_access(KEY_READ)
}

pub(in super::super) fn exact_machine_lock_publication_for_mutation()
-> Result<Option<ExactMachineLockPublication>> {
    exact_machine_lock_publication_with_access(KEY_READ | KEY_WRITE | WRITE_DAC | WRITE_OWNER)
}

pub(in super::super) fn exact_machine_lock_publication_with_access(
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
