use std::{io, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OwnedTreeError {
    #[error("invalid expected directory identity")]
    InvalidIdentity,
    #[error("identity-bound deletion is unsupported on this platform")]
    Unsupported,
    #[error("owned directory identity did not match")]
    IdentityMismatch,
    #[error("owned directory crosses a filesystem or mount boundary")]
    MountBoundary,
    #[error("owned directory path is invalid")]
    InvalidPath,
    #[error("identity-bound deletion failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(windows)]
pub fn owned_tree_identity(path: &Path) -> Result<String, OwnedTreeError> {
    platform::identity(path)
}

#[cfg(windows)]
pub fn flush_owned_directory(path: &Path) -> Result<(), OwnedTreeError> {
    platform::flush_directory(path)
}

pub fn remove_owned_tree(path: &Path, expected_identity: &str) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove(path, device, inode)
}

#[derive(Clone, Debug)]
pub struct ExactOwnedTreeEntry {
    pub relative_path: String,
    pub directory: bool,
    pub identity: String,
}

#[cfg(windows)]
pub fn remove_exact_owned_tree(
    path: &Path,
    expected_identity: &str,
    expected_entries: &[ExactOwnedTreeEntry],
) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove_exact(path, device, inode, expected_entries, false)
}

#[cfg(windows)]
pub fn inventory_exact_owned_tree_from_handle(
    root: windows_sys::Win32::Foundation::HANDLE,
) -> Result<Vec<ExactOwnedTreeEntry>, OwnedTreeError> {
    platform::inventory_from_handle(root)
}

#[cfg(windows)]
pub fn remove_exact_owned_tree_from_handles(
    parent: windows_sys::Win32::Foundation::HANDLE,
    root: windows_sys::Win32::Foundation::HANDLE,
    expected_identity: &str,
    expected_entries: &[ExactOwnedTreeEntry],
) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove_exact_from_handles(parent, root, device, inode, expected_entries)
}

#[cfg(windows)]
pub fn remove_remaining_exact_owned_tree(
    path: &Path,
    expected_identity: &str,
    expected_entries: &[ExactOwnedTreeEntry],
) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove_exact(path, device, inode, expected_entries, true)
}

fn parse_identity(value: &str) -> Result<(u64, u64), OwnedTreeError> {
    let (device, inode) = value
        .split_once(':')
        .ok_or(OwnedTreeError::InvalidIdentity)?;
    if device.is_empty() || inode.is_empty() || value.matches(':').count() != 1 {
        return Err(OwnedTreeError::InvalidIdentity);
    }
    Ok((
        device
            .parse()
            .map_err(|_| OwnedTreeError::InvalidIdentity)?,
        inode.parse().map_err(|_| OwnedTreeError::InvalidIdentity)?,
    ))
}

#[cfg(windows)]
mod platform {
    use super::{ExactOwnedTreeEntry, OwnedTreeError, parse_identity};
    use std::{
        collections::{BTreeMap, BTreeSet},
        ffi::{OsString, c_void},
        io,
        mem::zeroed,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::Path,
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GENERIC_WRITE,
            HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                SE_FILE_OBJECT, SetSecurityInfo,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandle, OPEN_EXISTING,
            READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle, WRITE_DAC,
        },
        System::Threading::GetCurrentProcess,
    };

    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x0000_4000;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
    const FILE_NAMES_INFORMATION: u32 = 12;
    const STATUS_NO_MORE_FILES: i32 = 0x8000_0006_u32 as i32;
    const DIRECTORY_BUFFER_SIZE: usize = 64 * 1024;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: HANDLE,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtOpenFile(
            file_handle: *mut HANDLE,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            share_access: u32,
            open_options: u32,
        ) -> i32;
        fn NtQueryDirectoryFile(
            file_handle: HANDLE,
            event: HANDLE,
            apc_routine: *mut c_void,
            apc_context: *mut c_void,
            io_status_block: *mut IoStatusBlock,
            file_information: *mut c_void,
            length: u32,
            file_information_class: u32,
            return_single_entry: u8,
            file_name: *mut UnicodeString,
            restart_scan: u8,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct VerifiedNode {
        handle: Handle,
        children: Vec<VerifiedNode>,
    }

    pub(super) fn inventory_from_handle(
        root: HANDLE,
    ) -> Result<Vec<ExactOwnedTreeEntry>, OwnedTreeError> {
        let info = information(root)?;
        let root_device = u64::from(info.dwVolumeSerialNumber);
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
            || info.nNumberOfLinks != 1
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        let mut entries = Vec::new();
        inventory_directory(root, "", root_device, &mut entries)?;
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(entries)
    }

    fn inventory_directory(
        directory_handle: HANDLE,
        prefix: &str,
        root_device: u64,
        entries: &mut Vec<ExactOwnedTreeEntry>,
    ) -> Result<(), OwnedTreeError> {
        for mut name in directory_names(directory_handle)? {
            let component = OsString::from_wide(&name)
                .into_string()
                .map_err(|_| OwnedTreeError::InvalidPath)?;
            if component.contains('/')
                || component.contains('\\')
                || component == "."
                || component == ".."
            {
                return Err(OwnedTreeError::InvalidPath);
            }
            let relative_path = if prefix.is_empty() {
                component
            } else {
                format!("{prefix}/{component}")
            };
            let child = open_relative(directory_handle, &mut name)?;
            let info = information(child.0)?;
            let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            let device = u64::from(info.dwVolumeSerialNumber);
            if device != root_device || reparse || (!directory && info.nNumberOfLinks != 1) {
                return Err(OwnedTreeError::IdentityMismatch);
            }
            entries.push(ExactOwnedTreeEntry {
                relative_path: relative_path.clone(),
                directory,
                identity: identity_from_handle(child.0)?,
            });
            if directory {
                inventory_directory(child.0, &relative_path, root_device, entries)?;
            }
        }
        Ok(())
    }

    pub(super) fn remove_exact_from_handles(
        parent: HANDLE,
        root: HANDLE,
        expected_device: u64,
        expected_inode: u64,
        expected_entries: &[ExactOwnedTreeEntry],
    ) -> Result<(), OwnedTreeError> {
        let expected = expected_entries
            .iter()
            .map(|entry| {
                parse_identity(&entry.identity).map(|identity| {
                    (
                        entry.relative_path.clone(),
                        (entry.directory, identity.0, identity.1),
                    )
                })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        if expected.len() != expected_entries.len() {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        let mut duplicate = null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                root,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let duplicate = Handle(duplicate);
        let info = information(duplicate.0)?;
        validate_root(&info, expected_device, expected_inode)?;
        let mut seen = BTreeSet::new();
        let tree = verify_exact_directory(duplicate, "", expected_device, &expected, &mut seen)?;
        if seen.len() != expected.len() {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        delete_verified(tree)?;
        if unsafe { FlushFileBuffers(parent) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub fn remove_exact(
        path: &Path,
        expected_device: u64,
        expected_inode: u64,
        expected_entries: &[ExactOwnedTreeEntry],
        allow_missing: bool,
    ) -> Result<(), OwnedTreeError> {
        let expected = expected_entries
            .iter()
            .map(|entry| {
                parse_identity(&entry.identity).map(|identity| {
                    (
                        entry.relative_path.clone(),
                        (entry.directory, identity.0, identity.1),
                    )
                })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        if expected.len() != expected_entries.len() {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        let parent_path = path.parent().ok_or(OwnedTreeError::InvalidPath)?;
        let parent = open_barrier_directory(parent_path)?;
        let root = open_exact_root(path, expected_device, expected_inode)?;
        let info = information(root.0)?;
        validate_root(&info, expected_device, expected_inode)?;
        let mut seen = BTreeSet::new();
        let tree = verify_exact_directory(root, "", expected_device, &expected, &mut seen)?;
        if !allow_missing && seen.len() != expected.len() {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        delete_verified(tree)?;
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub fn remove(
        path: &Path,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<(), OwnedTreeError> {
        let parent_path = path.parent().ok_or(OwnedTreeError::InvalidPath)?;
        let parent = open_barrier_directory(parent_path)?;
        // This is the sole pathname open. Omitting FILE_SHARE_DELETE prevents replacement of the
        // verified root while every descendant is enumerated/opened relative to this handle.
        let root = open_root(path)?;
        let info = information(root.0)?;
        validate_root(&info, expected_device, expected_inode)?;
        remove_directory_contents(root.0, expected_device)?;
        mark_deleted(root.0)?;
        drop(root);
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(super) fn flush_directory(path: &Path) -> Result<(), OwnedTreeError> {
        let handle = open_barrier_directory(path)?;
        if unsafe { FlushFileBuffers(handle.0) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(super) fn identity(path: &Path) -> Result<String, OwnedTreeError> {
        let handle = open_identity(path)?;
        identity_from_handle(handle.0)
    }

    #[cfg(test)]
    pub(super) fn reparse_identity(path: &Path) -> Result<String, OwnedTreeError> {
        let wide = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        let handle = Handle(handle);
        identity_from_handle(handle.0)
    }

    fn identity_from_handle(handle: HANDLE) -> Result<String, OwnedTreeError> {
        let info = information(handle)?;
        Ok(format!(
            "{}:{}",
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
        ))
    }

    fn validate_root(
        info: &BY_HANDLE_FILE_INFORMATION,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<(), OwnedTreeError> {
        let device = u64::from(info.dwVolumeSerialNumber);
        let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        if device != expected_device || inode != expected_inode {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
            || info.nNumberOfLinks != 1
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(())
    }

    fn verify_exact_directory(
        handle: Handle,
        prefix: &str,
        root_device: u64,
        expected: &BTreeMap<String, (bool, u64, u64)>,
        seen: &mut BTreeSet<String>,
    ) -> Result<VerifiedNode, OwnedTreeError> {
        let mut children = Vec::new();
        for mut name in directory_names(handle.0)? {
            let component = OsString::from_wide(&name)
                .into_string()
                .map_err(|_| OwnedTreeError::InvalidPath)?;
            if component.contains('/')
                || component.contains('\\')
                || component == "."
                || component == ".."
            {
                return Err(OwnedTreeError::InvalidPath);
            }
            let relative = if prefix.is_empty() {
                component
            } else {
                format!("{prefix}/{component}")
            };
            let Some(&(expected_directory, expected_device, expected_inode)) =
                expected.get(&relative)
            else {
                return Err(OwnedTreeError::IdentityMismatch);
            };
            let child = open_exact_relative(
                handle.0,
                &mut name,
                expected_directory,
                expected_device,
                expected_inode,
            )?;
            let info = information(child.0)?;
            let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let device = u64::from(info.dwVolumeSerialNumber);
            let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
            if expected_directory != directory
                || expected_device != device
                || expected_inode != inode
                || device != root_device
                || !seen.insert(relative.clone())
            {
                return Err(OwnedTreeError::IdentityMismatch);
            }
            let node = if directory {
                verify_exact_directory(child, &relative, root_device, expected, seen)?
            } else {
                VerifiedNode {
                    handle: child,
                    children: Vec::new(),
                }
            };
            children.push(node);
        }
        Ok(VerifiedNode { handle, children })
    }

    fn delete_verified(node: VerifiedNode) -> Result<(), OwnedTreeError> {
        for child in node.children {
            delete_verified(child)?;
        }
        mark_deleted(node.handle.0)
    }

    fn remove_directory_contents(handle: HANDLE, root_device: u64) -> Result<(), OwnedTreeError> {
        let info = information(handle)?;
        if u64::from(info.dwVolumeSerialNumber) != root_device {
            return Err(OwnedTreeError::MountBoundary);
        }
        let names = directory_names(handle)?;
        for mut name in names {
            let child = open_relative(handle, &mut name)?;
            let child_info = information(child.0)?;
            if u64::from(child_info.dwVolumeSerialNumber) != root_device {
                return Err(OwnedTreeError::MountBoundary);
            }
            let directory = child_info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let reparse = child_info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            if reparse || (!directory && child_info.nNumberOfLinks != 1) {
                return Err(OwnedTreeError::IdentityMismatch);
            }
            if directory {
                remove_directory_contents(child.0, root_device)?;
            }
            mark_deleted(child.0)?;
        }
        Ok(())
    }

    fn directory_names(handle: HANDLE) -> Result<Vec<Vec<u16>>, OwnedTreeError> {
        let mut names = Vec::new();
        let mut restart = 1_u8;
        loop {
            let mut buffer = vec![0_u8; DIRECTORY_BUFFER_SIZE];
            let mut io_status = IoStatusBlock {
                status_or_pointer: 0,
                information: 0,
            };
            let status = unsafe {
                NtQueryDirectoryFile(
                    handle,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    &mut io_status,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    FILE_NAMES_INFORMATION,
                    0,
                    null_mut(),
                    restart,
                )
            };
            restart = 0;
            if status == STATUS_NO_MORE_FILES {
                break;
            }
            nt_success(status)?;
            let used = io_status.information.min(buffer.len());
            let mut offset = 0_usize;
            loop {
                if offset + 12 > used {
                    return Err(OwnedTreeError::IdentityMismatch);
                }
                let next = read_u32(&buffer, offset) as usize;
                let name_bytes = read_u32(&buffer, offset + 8) as usize;
                if !name_bytes.is_multiple_of(2) || offset + 12 + name_bytes > used {
                    return Err(OwnedTreeError::IdentityMismatch);
                }
                let mut name = Vec::with_capacity(name_bytes / 2);
                for index in (offset + 12..offset + 12 + name_bytes).step_by(2) {
                    name.push(u16::from_le_bytes([buffer[index], buffer[index + 1]]));
                }
                if name.as_slice() != [b'.' as u16] && name.as_slice() != [b'.' as u16, b'.' as u16]
                {
                    names.push(name);
                }
                if next == 0 {
                    break;
                }
                if next < 12 || offset + next >= used {
                    return Err(OwnedTreeError::IdentityMismatch);
                }
                offset += next;
            }
        }
        Ok(names)
    }

    fn open_exact_relative(
        parent: HANDLE,
        name: &mut [u16],
        expected_directory: bool,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<Handle, OwnedTreeError> {
        let desired =
            DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | FILE_LIST_DIRECTORY | SYNCHRONIZE;
        if let Ok(child) = open_relative_access(parent, name, desired) {
            validate_exact_entry(
                &information(child.0)?,
                expected_directory,
                expected_device,
                expected_inode,
            )?;
            return Ok(child);
        }
        let repair = open_relative_access(
            parent,
            name,
            WRITE_DAC | READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        )?;
        validate_exact_entry(
            &information(repair.0)?,
            expected_directory,
            expected_device,
            expected_inode,
        )?;
        repair_exact_dacl(repair.0)?;
        let child = open_relative_access(parent, name, desired)?;
        let repaired = information(child.0)?;
        validate_exact_entry(
            &repaired,
            expected_directory,
            expected_device,
            expected_inode,
        )?;
        drop(repair);
        Ok(child)
    }

    fn validate_exact_entry(
        info: &BY_HANDLE_FILE_INFORMATION,
        expected_directory: bool,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<(), OwnedTreeError> {
        let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        let device = u64::from(info.dwVolumeSerialNumber);
        let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        if directory != expected_directory
            || reparse
            || info.nNumberOfLinks != 1
            || device != expected_device
            || inode != expected_inode
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(())
    }

    fn repair_exact_dacl(handle: HANDLE) -> Result<(), OwnedTreeError> {
        let sddl = wide(Path::new("D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)"))?;
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
            || descriptor.is_null()
        {
            return Err(io::Error::last_os_error().into());
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = null_mut();
        let read = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
        };
        let status = if read == 0 || present == 0 || dacl.is_null() {
            1
        } else {
            unsafe {
                SetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null_mut(),
                )
            }
        };
        unsafe { LocalFree(descriptor) };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32).into());
        }
        Ok(())
    }

    fn open_relative(parent: HANDLE, name: &mut [u16]) -> Result<Handle, OwnedTreeError> {
        let child = open_relative_access(
            parent,
            name,
            DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | FILE_LIST_DIRECTORY | SYNCHRONIZE,
        )?;
        let info = information(child.0)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks != 1 {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(child)
    }

    fn open_relative_access(
        parent: HANDLE,
        name: &mut [u16],
        desired_access: u32,
    ) -> Result<Handle, OwnedTreeError> {
        let byte_length = name
            .len()
            .checked_mul(2)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or(OwnedTreeError::InvalidPath)?;
        let mut unicode = UnicodeString {
            length: byte_length,
            maximum_length: byte_length,
            buffer: name.as_mut_ptr(),
        };
        let mut attributes = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: parent,
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: null_mut(),
            security_quality_of_service: null_mut(),
        };
        let mut io_status = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut child = null_mut();
        let status = unsafe {
            NtOpenFile(
                &mut child,
                desired_access,
                &mut attributes,
                &mut io_status,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_OPEN_REPARSE_POINT
                    | FILE_OPEN_FOR_BACKUP_INTENT
                    | FILE_SYNCHRONOUS_IO_NONALERT,
            )
        };
        nt_success(status)?;
        if child.is_null() {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(Handle(child))
    }

    fn open_identity(path: &Path) -> Result<Handle, OwnedTreeError> {
        let wide = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        let handle = Handle(handle);
        let info = information(handle.0)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks != 1 {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(handle)
    }

    fn open_exact_root(
        path: &Path,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<Handle, OwnedTreeError> {
        let desired = DELETE | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY;
        if let Ok(root) = open_root_access(path, desired) {
            validate_root(&information(root.0)?, expected_device, expected_inode)?;
            return Ok(root);
        }
        let repair = open_root_access(path, WRITE_DAC | READ_CONTROL | FILE_READ_ATTRIBUTES)?;
        validate_root(&information(repair.0)?, expected_device, expected_inode)?;
        repair_exact_dacl(repair.0)?;
        let root = open_root_access(path, desired)?;
        validate_root(&information(root.0)?, expected_device, expected_inode)?;
        drop(repair);
        Ok(root)
    }

    fn open_root(path: &Path) -> Result<Handle, OwnedTreeError> {
        let root = open_root_access(path, DELETE | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)?;
        let info = information(root.0)?;
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
            || info.nNumberOfLinks != 1
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(root)
    }

    fn open_root_access(path: &Path, desired_access: u32) -> Result<Handle, OwnedTreeError> {
        let wide = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                desired_access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Handle(handle))
    }

    fn open_barrier_directory(path: &Path) -> Result<Handle, OwnedTreeError> {
        let wide = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        let handle = Handle(handle);
        let info = information(handle.0)?;
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
            || info.nNumberOfLinks != 1
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        Ok(handle)
    }

    fn information(handle: HANDLE) -> Result<BY_HANDLE_FILE_INFORMATION, OwnedTreeError> {
        let mut info = unsafe { zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(info)
    }

    fn mark_deleted(handle: HANDLE) -> Result<(), OwnedTreeError> {
        let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn nt_success(status: i32) -> Result<(), OwnedTreeError> {
        if status >= 0 {
            return Ok(());
        }
        let error = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(error as i32).into())
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("bounded directory entry"),
        )
    }

    fn wide(path: &Path) -> Result<Vec<u16>, OwnedTreeError> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(OwnedTreeError::InvalidPath);
        }
        value.push(0);
        Ok(value)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::OwnedTreeError;
    use std::{
        ffi::{CStr, CString, OsStr},
        io,
        mem::zeroed,
        os::unix::ffi::OsStrExt,
        path::Path,
    };

    struct Fd(libc::c_int);
    impl Drop for Fd {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.0);
            }
        }
    }
    struct Dir(*mut libc::DIR);
    impl Drop for Dir {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }

    pub fn remove(
        path: &Path,
        expected_device: u64,
        expected_inode: u64,
    ) -> Result<(), OwnedTreeError> {
        let parent = path.parent().ok_or(OwnedTreeError::InvalidPath)?;
        let name = path.file_name().ok_or(OwnedTreeError::InvalidPath)?;
        let parent_fd = open_path(parent)?;
        let name = c_name(name)?;
        let root_fd = open_at(parent_fd.0, &name)?;
        let root = stat_fd(root_fd.0)?;
        if root.st_dev as u64 != expected_device || root.st_ino != expected_inode {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        remove_contents(root_fd.0, root.st_dev)?;
        if unsafe { libc::unlinkat(parent_fd.0, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { libc::fsync(parent_fd.0) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn remove_contents(fd: libc::c_int, root_device: libc::dev_t) -> Result<(), OwnedTreeError> {
        let duplicate = unsafe { libc::dup(fd) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let directory = unsafe { libc::fdopendir(duplicate) };
        if directory.is_null() {
            unsafe {
                libc::close(duplicate);
            }
            return Err(io::Error::last_os_error().into());
        }
        let directory = Dir(directory);
        loop {
            unsafe {
                *libc::__error() = 0;
            }
            let entry = unsafe { libc::readdir(directory.0) };
            if entry.is_null() {
                let error = unsafe { *libc::__error() };
                if error == 0 {
                    break;
                }
                return Err(io::Error::from_raw_os_error(error).into());
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let mut metadata: libc::stat = unsafe { zeroed() };
            if unsafe { libc::fstatat(fd, name.as_ptr(), &mut metadata, libc::AT_SYMLINK_NOFOLLOW) }
                != 0
            {
                return Err(io::Error::last_os_error().into());
            }
            let kind = metadata.st_mode & libc::S_IFMT;
            if kind == libc::S_IFDIR {
                if metadata.st_dev != root_device {
                    return Err(OwnedTreeError::MountBoundary);
                }
                let child = open_at(fd, name)?;
                let opened = stat_fd(child.0)?;
                if opened.st_dev != metadata.st_dev || opened.st_ino != metadata.st_ino {
                    return Err(OwnedTreeError::IdentityMismatch);
                }
                remove_contents(child.0, root_device)?;
                if unsafe { libc::unlinkat(fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                    return Err(io::Error::last_os_error().into());
                }
            } else if unsafe { libc::unlinkat(fd, name.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(())
    }

    fn open_path(path: &Path) -> Result<Fd, OwnedTreeError> {
        let path =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| OwnedTreeError::InvalidPath)?;
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Fd(fd))
    }
    fn open_at(parent: libc::c_int, name: &CStr) -> Result<Fd, OwnedTreeError> {
        let fd = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Fd(fd))
    }
    fn stat_fd(fd: libc::c_int) -> Result<libc::stat, OwnedTreeError> {
        let mut metadata = unsafe { zeroed() };
        if unsafe { libc::fstat(fd, &mut metadata) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(metadata)
    }
    fn c_name(name: &OsStr) -> Result<CString, OwnedTreeError> {
        CString::new(name.as_bytes()).map_err(|_| OwnedTreeError::InvalidPath)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn identity(path: &Path) -> String {
        platform::identity(path).unwrap()
    }

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "talking-quill-owned-tree-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn rejects_a_directory_replacement_and_preserves_unrelated_data() {
        let parent = temporary("replacement");
        let root = parent.join("owned");
        let moved = parent.join("moved-owned");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/private"), b"private").unwrap();
        let expected = identity(&root);
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("unrelated"), b"preserve").unwrap();

        assert!(matches!(
            remove_owned_tree(&root, &expected),
            Err(OwnedTreeError::IdentityMismatch)
        ));
        assert_eq!(fs::read(root.join("unrelated")).unwrap(), b"preserve");
        assert_eq!(fs::read(moved.join("nested/private")).unwrap(), b"private");
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn exact_open_rejects_a_recorded_hardlink_before_deletion() {
        let parent = temporary("exact-hardlink");
        let root = parent.join("owned");
        let outside = parent.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, b"preserve").unwrap();
        let file_identity = identity(&outside);
        fs::hard_link(&outside, root.join("linked")).unwrap();
        let expected = identity(&root);
        let entries = [ExactOwnedTreeEntry {
            relative_path: "linked".into(),
            directory: false,
            identity: file_identity,
        }];

        assert!(remove_exact_owned_tree(&root, &expected, &entries).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"preserve");
        assert!(root.join("linked").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn exact_open_rejects_a_recorded_reparse_before_deletion() {
        let parent = temporary("exact-reparse");
        let root = parent.join("owned");
        let outside = parent.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"preserve").unwrap();
        let link = root.join("linked");
        if std::os::windows::fs::symlink_dir(&outside, &link).is_err() {
            fs::remove_dir_all(parent).unwrap();
            return;
        }
        let expected = identity(&root);
        let entries = [ExactOwnedTreeEntry {
            relative_path: "linked".into(),
            directory: true,
            identity: platform::reparse_identity(&link).unwrap(),
        }];

        assert!(remove_exact_owned_tree(&root, &expected, &entries).is_err());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn removes_only_the_identity_bound_tree() {
        let parent = temporary("remove");
        let root = parent.join("owned");
        let outside = parent.join("outside");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("nested/private"), b"private").unwrap();
        fs::write(outside.join("sentinel"), b"preserve").unwrap();
        let expected = identity(&root);

        remove_owned_tree(&root, &expected).unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
        fs::remove_dir_all(parent).unwrap();
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::OwnedTreeError;
    use std::path::Path;
    pub fn remove(_: &Path, _: u64, _: u64) -> Result<(), OwnedTreeError> {
        Err(OwnedTreeError::Unsupported)
    }
}
