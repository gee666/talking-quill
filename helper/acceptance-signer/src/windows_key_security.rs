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

fn validate_security(handle: HANDLE) -> Result<(), &'static str> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() || owner.is_null() || dacl.is_null() {
        return Err("private key security query");
    }
    let result = validate_descriptor(descriptor, owner, dacl);
    unsafe { LocalFree(descriptor as _) };
    result
}

fn validate_descriptor(
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
) -> Result<(), &'static str> {
    let current = current_user_sid()?;
    let system = well_known_sid(WinLocalSystemSid)?;
    let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
    if unsafe { EqualSid(owner, current.as_ptr() as PSID) } == 0 {
        return Err("private key owner");
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
        || unsafe { (*dacl).AceCount } != 3
    {
        return Err("private key dacl policy");
    }
    let expected = [current, system, administrators];
    let mut seen = [false; 3];
    for index in 0..3u32 {
        let mut raw: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, index, &mut raw) } == 0 || raw.is_null() {
            return Err("private key ace query");
        }
        let ace = unsafe { &*(raw as *const ACCESS_ALLOWED_ACE) };
        if u32::from(ace.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || u32::from(ace.Header.AceFlags) & INHERITED_ACE != 0
            || ace.Mask != FILE_GENERIC_READ
        {
            return Err("private key ace policy");
        }
        let sid = addr_of!(ace.SidStart) as PSID;
        let Some(position) = expected
            .iter()
            .position(|candidate| unsafe { EqualSid(sid, candidate.as_ptr() as PSID) } != 0)
        else {
            return Err("private key principal");
        };
        if seen[position] {
            return Err("private key duplicate principal");
        }
        seen[position] = true;
    }
    if !seen.into_iter().all(|value| value) {
        return Err("private key principal set");
    }
    Ok(())
}

fn current_user_sid_string() -> Result<String, &'static str> {
    let sid = current_user_sid()?;
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr() as PSID, &mut text) } == 0 || text.is_null() {
        return Err("current user sid string");
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| "current user sid string")?;
    unsafe { LocalFree(text as _) };
    Ok(value)
}

fn current_user_sid() -> Result<Vec<u8>, &'static str> {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("current token");
    }
    let mut bytes = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes) };
    if bytes < size_of::<TOKEN_USER>() as u32 {
        unsafe { CloseHandle(token) };
        return Err("current user sid size");
    }
    let mut buffer = vec![0u8; bytes as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut c_void,
            bytes,
            &mut bytes,
        )
    };
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err("current user sid");
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    copy_sid(user.User.Sid)
}

fn well_known_sid(kind: i32) -> Result<Vec<u8>, &'static str> {
    let mut buffer = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut bytes = buffer.len() as u32;
    if unsafe { CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr() as PSID, &mut bytes) } == 0
    {
        return Err("well known sid");
    }
    buffer.truncate(bytes as usize);
    Ok(buffer)
}

fn copy_sid(sid: PSID) -> Result<Vec<u8>, &'static str> {
    use windows_sys::Win32::Security::GetLengthSid;
    let bytes = unsafe { GetLengthSid(sid) };
    if bytes == 0 || bytes > SECURITY_MAX_SID_SIZE {
        return Err("sid length");
    }
    let mut output = vec![0u8; bytes as usize];
    unsafe { std::ptr::copy_nonoverlapping(sid as *const u8, output.as_mut_ptr(), bytes as usize) };
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "talking-quill-key-security-{}-{name}",
            std::process::id()
        ))
    }

    fn create_hard_linked_exact_acl_key(key: &Path, linked: &Path) -> File {
        let ancestors =
            create_protected_directories(key.parent().expect("key parent")).expect("create parent");
        let sid = current_user_sid_string().expect("current user sid");
        let creation_descriptor = SecurityDescriptor::from_sddl(&format!(
            "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;FA;;;{sid})"
        ))
        .expect("create writable descriptor");
        let final_descriptor = SecurityDescriptor::from_sddl(&format!(
            "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;0x120089;;;{sid})"
        ))
        .expect("create final descriptor");
        let mut wide = key.as_os_str().encode_wide().collect::<Vec<_>>();
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
        assert_ne!(handle, INVALID_HANDLE_VALUE, "create writable key");
        let mut writable = unsafe { File::from_raw_handle(handle) };
        writable.write_all(&[1, 2, 3]).expect("write key");
        assert_ne!(
            unsafe { FlushFileBuffers(writable.as_raw_handle()) },
            0,
            "flush writable key"
        );
        drop(writable);
        drop(ancestors);
        std::fs::hard_link(key, linked).expect("create hard link while permitted");

        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | WRITE_DAC,
                FILE_SHARE_READ,
                null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE, "reopen writable key");
        let file = unsafe { File::from_raw_handle(handle) };
        assert_ne!(
            unsafe {
                SetKernelObjectSecurity(
                    file.as_raw_handle(),
                    DACL_SECURITY_INFORMATION,
                    final_descriptor.0,
                )
            },
            0,
            "apply final key ACL"
        );
        file
    }

    #[test]
    fn exact_acl_key_is_admitted_and_hard_link_is_rejected() {
        let directory = test_path("exact");
        let key = directory.join("key.der");
        let linked = directory.join("linked.der");
        let _ = std::fs::remove_dir_all(&directory);
        let retained = create_hard_linked_exact_acl_key(&key, &linked);
        validate_security(retained.as_raw_handle()).expect("validate exact protected ACL");
        assert_eq!(
            validate_private_key_handle(&retained),
            Err("private key file identity")
        );
        drop(retained);
        assert_eq!(
            open_validated_private_key(&key).err(),
            Some("private key file identity")
        );
        std::fs::remove_file(&linked).expect("remove hard link");
        let admitted = open_validated_private_key(&key).expect("admit single-link key");
        drop(admitted);
        delete_validated_private_key(&key).expect("handle-delete key");
        assert!(!key.exists());
        std::fs::remove_dir(&directory).expect("remove directory");
    }
}
