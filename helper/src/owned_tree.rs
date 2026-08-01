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

pub fn remove_owned_tree(path: &Path, expected_identity: &str) -> Result<(), OwnedTreeError> {
    let (device, inode) = parse_identity(expected_identity)?;
    platform::remove(path, device, inode)
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
    use super::OwnedTreeError;
    use std::{
        ffi::c_void, io, mem::zeroed, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandle, OPEN_EXISTING,
            SYNCHRONIZE, SetFileInformationByHandle,
        },
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
        let device = u64::from(info.dwVolumeSerialNumber);
        let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        if device != expected_device || inode != expected_inode {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
        {
            return Err(OwnedTreeError::IdentityMismatch);
        }
        remove_directory_contents(root.0, device)?;
        mark_deleted(root.0)?;
        drop(root);
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn identity_for_test(path: &Path) -> Result<String, OwnedTreeError> {
        let handle = open_root(path)?;
        let info = information(handle.0)?;
        Ok(format!(
            "{}:{}",
            info.dwVolumeSerialNumber,
            (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
        ))
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
            if directory && !reparse {
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

    fn open_relative(parent: HANDLE, name: &mut [u16]) -> Result<Handle, OwnedTreeError> {
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
                DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA | FILE_LIST_DIRECTORY | SYNCHRONIZE,
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

    fn open_root(path: &Path) -> Result<Handle, OwnedTreeError> {
        let wide = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                DELETE | FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
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
        Ok(Handle(handle))
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
        platform::identity_for_test(path).unwrap()
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
