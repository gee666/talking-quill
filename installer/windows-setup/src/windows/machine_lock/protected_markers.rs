//! Protected directory creation and atomic identity markers.
use super::*;

pub(in super::super) fn flush_setup_directory(path: &Path) -> Result<()> {
    let handle = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open a durable machine lock directory.",
        ));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        return Err(fail(EXIT_FAILURE, "Cannot flush a machine lock directory."));
    }
    Ok(())
}

pub(in super::super) fn create_restricted_lock_directory(path: &Path) -> Result<()> {
    create_directory_with_security(path, machine_lock_directory_sddl())
}

pub(in super::super) fn create_directory_with_security(
    path: &Path,
    descriptor_sddl: &str,
) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
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
        return Err(fail(EXIT_FAILURE, "Cannot create the machine lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) };
    unsafe { LocalFree(descriptor) };
    if created == 0 && !path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock directory.",
        ));
    }
    Ok(())
}

pub(in super::super) fn apply_lock_dacl(path: &Path, descriptor_sddl: &str) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
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
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock file ACL.",
        ));
    }
    let status = unsafe {
        SetFileSecurityW(
            wide(path.as_os_str()).as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor) };
    if status == 0 {
        Err(fail(EXIT_FAILURE, "Cannot protect the machine lock file."))
    } else {
        Ok(())
    }
}

pub(in super::super) fn create_or_verify_lock_marker(path: &Path, value: &str) -> Result<()> {
    create_atomic_marker(path, value, machine_lock_file_sddl())
}

pub(in super::super) fn create_atomic_marker(path: &Path, value: &str, sddl: &str) -> Result<()> {
    if path_present(path)? {
        return verify_atomic_marker(path, value, sddl, None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no parent."))?;
    let name = path
        .file_name()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no name."))?
        .to_string_lossy();
    let temporary = parent.join(format!("{name}.tmp-{}", new_machine_lock_suffix()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(io_failure)?;
    apply_lock_dacl(&temporary, sddl)?;
    file.write_all(value.as_bytes()).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    verify_atomic_marker(&temporary, value, sddl, Some(&identity))?;
    if unsafe {
        MoveFileExW(
            wide(temporary.as_os_str()).as_ptr(),
            wide(path.as_os_str()).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot publish the protected marker."));
    }
    flush_setup_directory(parent)?;
    verify_atomic_marker(path, value, sddl, Some(&identity))
}

pub(in super::super) fn verify_atomic_marker(
    path: &Path,
    value: &str,
    sddl: &str,
    expected_identity: Option<&str>,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(io_failure)?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || content != value
        || !marker_security_is_exact(path, sddl)?
    {
        return Err(fail(EXIT_REJECTED, "Protected marker identity changed."));
    }
    Ok(())
}

pub(in super::super) fn file_identity_text(file: &File) -> Result<String> {
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock file identity is unavailable.",
        ));
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(format!("{}:{index}", information.dwVolumeSerialNumber))
}
