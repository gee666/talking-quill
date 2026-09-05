use super::OwnedTreeError;
use std::{ffi::c_void, io, mem::zeroed, os::windows::ffi::OsStrExt, path::Path, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo,
        FlushFileBuffers, GetFileInformationByHandle, OPEN_EXISTING, SYNCHRONIZE,
        SetFileInformationByHandle,
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

pub(super) fn identity(path: &Path) -> Result<String, OwnedTreeError> {
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

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn retained_names(handle: HANDLE) -> Result<Vec<String>, OwnedTreeError> {
    directory_names(handle)?
        .into_iter()
        .map(|name| String::from_utf16(&name).map_err(|_| OwnedTreeError::InvalidPath))
        .collect()
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
            if name.as_slice() != [b'.' as u16] && name.as_slice() != [b'.' as u16, b'.' as u16] {
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
            FILE_OPEN_REPARSE_POINT | FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT,
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
