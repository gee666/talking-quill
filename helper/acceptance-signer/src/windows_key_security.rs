use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{addr_of, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetSecurityDescriptorControl, GetTokenInformation, INHERITED_ACE, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE,
    SetKernelObjectSecurity, TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL,
    SetFileInformationByHandle, WRITE_DAC,
};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub struct ValidatedPrivateKey {
    pub file: File,
    _ancestors: Vec<File>,
}

pub fn open_validated_private_key(path: &Path) -> Result<ValidatedPrivateKey, &'static str> {
    if !path.is_absolute() {
        return Err("private key path absolute");
    }
    let ancestors = retain_ancestors(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| "private key open")?;
    validate_private_key_handle(&file)?;
    Ok(ValidatedPrivateKey {
        file,
        _ancestors: ancestors,
    })
}

pub fn create_protected_private_key(path: &Path, bytes: &[u8]) -> Result<(), &'static str> {
    if !path.is_absolute() || bytes.is_empty() || bytes.len() > 512 {
        return Err("private key creation input");
    }
    let _creation_ancestors =
        create_protected_directories(path.parent().ok_or("private key parent")?)?;
    let sid = current_user_sid_string()?;
    let creation_descriptor = SecurityDescriptor::from_sddl(&format!(
        "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;FA;;;{sid})"
    ))?;
    let final_descriptor = SecurityDescriptor::from_sddl(&format!(
        "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;0x120089;;;{sid})"
    ))?;
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let attributes = creation_descriptor.attributes();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | WRITE_DAC,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err("private key create");
    }
    let mut file = unsafe { File::from_raw_handle(handle) };
    let result = file
        .write_all(bytes)
        .map_err(|_| "private key write")
        .and_then(|()| {
            if unsafe {
                SetKernelObjectSecurity(
                    file.as_raw_handle(),
                    DACL_SECURITY_INFORMATION,
                    final_descriptor.0,
                )
            } == 0
            {
                return Err("private key final acl");
            }
            if unsafe { FlushFileBuffers(file.as_raw_handle()) } == 0 {
                Err("private key flush")
            } else {
                Ok(())
            }
        });
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(path);
        if path.exists() {
            return Err("private key failed creation cleanup");
        }
        return Err(error);
    }
    if let Err(error) = open_validated_private_key(path) {
        let _ = std::fs::remove_file(path);
        if path.exists() {
            return Err("private key failed validation cleanup");
        }
        return Err(error);
    }
    Ok(())
}

pub fn delete_validated_private_key(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute() {
        return Err("private key delete path");
    }
    let _ancestors = retain_ancestors(path)?;
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            DELETE | READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err("private key delete open");
    }
    let file = unsafe { File::from_raw_handle(handle) };
    validate_private_key_handle(&file)?;
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            &disposition as *const _ as *const c_void,
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err("private key handle delete");
    }
    drop(file);
    if path.exists() {
        return Err("private key handle delete verification");
    }
    Ok(())
}

pub fn validate_private_key_handle(file: &File) -> Result<(), &'static str> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.nNumberOfLinks != 1
        || info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
    {
        return Err("private key file identity");
    }
    let bytes = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    if !(1..=512).contains(&bytes) {
        return Err("private key size");
    }
    validate_security(file.as_raw_handle())
}

fn create_protected_directories(path: &Path) -> Result<Vec<File>, &'static str> {
    let mut missing = Vec::new();
    let mut retained = Vec::new();
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            retained = retain_ancestors(&candidate.join(".talking-quill-retained-parent"))?;
            break;
        }
        missing.push(candidate.to_path_buf());
        current = candidate.parent();
    }
    let sid = current_user_sid_string()?;
    let descriptor = SecurityDescriptor::from_sddl(&format!(
        "O:{sid}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{sid})"
    ))?;
    let attributes = descriptor.attributes();
    for directory in missing.iter().rev() {
        let mut wide = directory.as_os_str().encode_wide().collect::<Vec<_>>();
        wide.push(0);
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
            return Err("private key directory create");
        }
        retained.push(open_directory(directory)?);
    }
    Ok(retained)
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
impl SecurityDescriptor {
    fn from_sddl(value: &str) -> Result<Self, &'static str> {
        let mut wide = value.encode_utf16().collect::<Vec<_>>();
        wide.push(0);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
            || descriptor.is_null()
        {
            return Err("security descriptor create");
        }
        Ok(Self(descriptor))
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0 as _) };
    }
}

fn retain_ancestors(path: &Path) -> Result<Vec<File>, &'static str> {
    let mut paths = Vec::<PathBuf>::new();
    let mut current = path.parent();
    while let Some(parent) = current {
        paths.push(parent.to_path_buf());
        current = parent.parent();
    }
    paths.reverse();
    paths
        .iter()
        .map(|path| open_directory(path))
        .collect::<Result<Vec<_>, _>>()
}

fn open_directory(path: &Path) -> Result<File, &'static str> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err("private key ancestor open");
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("private key ancestor identity");
    }
    Ok(file)
}

mod acl;
use acl::{current_user_sid_string, validate_security};

#[cfg(test)]
mod tests;
