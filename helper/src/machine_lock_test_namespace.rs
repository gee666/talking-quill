#![cfg(all(windows, feature = "machine-lock-test-namespace"))]

use crate::owned_tree::flush_owned_directory;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::c_void,
    io,
    mem::zeroed,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SHARING_VIOLATION, FILETIME, HANDLE, LocalFree,
    },
    Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_REGISTRY_KEY,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        SetFileSecurityW, SetKernelObjectSecurity, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        BACKUP_ALTERNATE_DATA, BACKUP_DATA, BY_HANDLE_FILE_INFORMATION, BackupRead, CREATE_NEW,
        CommitTransaction, CreateFileW, CreateTransaction, DELETE, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
        FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileDispositionInfo, FileIdInfo, FlushFileBuffers, GetFileInformationByHandle,
        GetFileInformationByHandleEx, LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, OPEN_EXISTING,
        READ_CONTROL, ReadFile, SetFileInformationByHandle, WriteFile,
    },
    System::{
        IO::OVERLAPPED,
        Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_CREATED_NEW_KEY,
            REG_OPTION_NON_VOLATILE, REG_OPTION_OPEN_LINK, RegCloseKey, RegCreateKeyExW,
            RegDeleteKeyTransactedW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegFlushKey,
            RegOpenKeyExW, RegOpenKeyTransactedW, RegQueryInfoKeyW, RegQueryValueExW,
            RegSetValueExW,
        },
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcessToken,
            QueryFullProcessImageNameW,
        },
    },
};

const OWNERSHIP_STREAM: &str = "TalkingQuill.TestOwnership.V1";
const REG_LINK_TYPE: u32 = 6;
const REG_OPTION_CREATE_LINK: u32 = 2;
const KEY_CREATE_LINK: u32 = 0x0020;
const ERROR_SUCCESS: u32 = 0;
const KEY_NAME_INFORMATION: i32 = 3;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const FILE_CREATE: u32 = 2;
const FILE_OPEN: u32 = 1;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;
const FILE_OPEN_REPARSE_POINT_NT: u32 = 0x0020_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;

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
    fn NtCreateFile(
        file_handle: *mut HANDLE,
        desired_access: u32,
        object_attributes: *mut ObjectAttributes,
        io_status_block: *mut IoStatusBlock,
        allocation_size: *mut i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *mut c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file_handle: HANDLE,
        io_status_block: *mut IoStatusBlock,
        file_information: *mut c_void,
        length: u32,
        file_information_class: u32,
    ) -> i32;
    fn NtDeleteKey(key_handle: HANDLE) -> i32;
    fn NtQueryKey(
        key_handle: HANDLE,
        key_information_class: i32,
        key_information: *mut c_void,
        length: u32,
        result_length: *mut u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupBinding {
    version: u32,
    nonce: String,
    ownership_prefix: String,
    root_identity: String,
    ads_sha256: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreationIntent {
    version: u32,
    nonce: String,
    ownership_prefix: String,
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

pub struct RetainedNamespaceHandles {
    _parent: Handle,
    _root: Handle,
}

pub struct ProtectedRootSession {
    parent_path: PathBuf,
    ownership_prefix: String,
    binding_path: PathBuf,
    binding_nonce: String,
    identity: String,
    ads_sha256: String,
    parent: Handle,
    root: Handle,
}

impl ProtectedRootSession {
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn ads_sha256(&self) -> &str {
        &self.ads_sha256
    }

    pub fn retain_low_access_guards(&mut self) -> io::Result<()> {
        self.parent = open_directory_low_guard(&self.parent_path)?;
        Ok(())
    }

    pub fn restore_root_for_teardown(&mut self) -> io::Result<()> {
        validate_directory(self.root.0)?;
        if identity(self.root.0)? != self.identity {
            return Err(io::Error::other(
                "outer namespace root identity changed before teardown",
            ));
        }
        Ok(())
    }

    pub fn inventory(&self) -> io::Result<Vec<crate::owned_tree::ExactOwnedTreeEntry>> {
        let stream = open_relative_ownership_stream(self.root.0)?;
        validate_exact_security(stream.0, true)?;
        let bytes = read_handle(stream.0)?;
        let expected = format!("{}:{}", self.ownership_prefix, self.identity);
        if hex_digest(&bytes) != self.ads_sha256 || bytes != expected.as_bytes() {
            return Err(io::Error::other("protected root ownership stream changed"));
        }
        crate::owned_tree::inventory_exact_owned_tree_from_handle(self.root.0)
            .map_err(io::Error::other)
    }

    pub fn delete_handle_bound(
        self,
        expected: &[crate::owned_tree::ExactOwnedTreeEntry],
    ) -> io::Result<()> {
        crate::owned_tree::remove_exact_owned_tree_from_handles(
            self.parent.0,
            self.root.0,
            &self.identity,
            expected,
        )
        .map_err(io::Error::other)?;
        let Self {
            parent_path: _,
            ownership_prefix,
            binding_path,
            binding_nonce,
            identity,
            ads_sha256,
            root,
            parent,
        } = self;
        drop(root);
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        drop(parent);
        delete_cleanup_binding(
            &binding_path,
            &ownership_prefix,
            &binding_nonce,
            &identity,
            &ads_sha256,
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorClaim<'a> {
    pid: u32,
    creation_time: u64,
    image_path: &'a str,
    image_identity: String,
    image_sha256: String,
}

pub struct ExactFileSecurity {
    descriptor: Descriptor,
}

impl ExactFileSecurity {
    pub fn inheritable_attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 1,
        }
    }
}

pub fn exact_inheritable_file_security() -> io::Result<ExactFileSecurity> {
    Ok(ExactFileSecurity {
        descriptor: exact_descriptor(false)?,
    })
}

pub struct SupervisorClaimGuard {
    claim: String,
    _image: Handle,
}

impl SupervisorClaimGuard {
    pub fn claim(&self) -> &str {
        &self.claim
    }
}

pub fn retain_supervisor_claim() -> io::Result<SupervisorClaimGuard> {
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(GetCurrentProcess(), 0, path.as_mut_ptr(), &mut length) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    path.truncate(length as usize);
    let path_text = String::from_utf16(&path)
        .map_err(|_| io::Error::other("supervisor image path is not valid UTF-16"))?;
    path.push(0);
    let image = Handle(unsafe {
        CreateFileW(
            path.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if image.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut image_information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(image.0, &mut image_information) } == 0
        || image_information.dwFileAttributes
            & (windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
                | FILE_ATTRIBUTE_REPARSE_POINT)
            != 0
    {
        return Err(io::Error::other(
            "supervisor image is not an exact regular file",
        ));
    }
    let identity = identity(image.0)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        if unsafe {
            ReadFile(
                image.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read as usize]);
    }
    let claim = serde_json::to_string(&SupervisorClaim {
        pid: unsafe { GetCurrentProcessId() },
        creation_time: ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64,
        image_path: &path_text,
        image_identity: identity,
        image_sha256: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
    .map_err(io::Error::other)?;
    Ok(SupervisorClaimGuard {
        claim,
        _image: image,
    })
}

pub struct NativeRecordPending {
    parent: Handle,
    file: Handle,
    destination: Vec<u16>,
    name: String,
}

impl NativeRecordPending {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut offset = 0usize;
        while offset < bytes.len() {
            let mut written = 0u32;
            if unsafe {
                WriteFile(
                    self.file.0,
                    bytes[offset..].as_ptr().cast(),
                    (bytes.len() - offset).min(u32::MAX as usize) as u32,
                    &mut written,
                    null_mut(),
                )
            } == 0
                || written == 0
            {
                return Err(io::Error::last_os_error());
            }
            offset += written as usize;
        }
        Ok(())
    }

    pub fn flush(&self) -> io::Result<()> {
        if unsafe { FlushFileBuffers(self.file.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn flush_parent(&self) -> io::Result<()> {
        if unsafe { FlushFileBuffers(self.parent.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn rename(&self, replace: bool) -> io::Result<()> {
        let name_bytes = self.destination.len() * size_of::<u16>();
        let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let mut buffer = vec![0_u8; size_of::<FILE_RENAME_INFO>() + name_bytes];
        let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*information).Anonymous.ReplaceIfExists = replace;
            (*information).RootDirectory = self.parent.0;
            (*information).FileNameLength = name_bytes as u32;
            std::ptr::copy_nonoverlapping(
                self.destination.as_ptr(),
                buffer.as_mut_ptr().add(header).cast(),
                self.destination.len(),
            );
        }
        let mut status_block = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let status = unsafe {
            NtSetInformationFile(
                self.file.0,
                &mut status_block,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                10,
            )
        };
        if status < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(status) } as i32,
            ));
        }
        Ok(())
    }

    pub fn replace(self) -> io::Result<()> {
        self.rename(true)?;
        self.flush_parent()
    }
}

fn record_crash_at(phase: &str) {
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok(phase) {
        std::process::exit(197);
    }
}

fn cleanup_record_value(bytes: &[u8]) -> io::Result<serde_json::Value> {
    serde_json::from_slice(bytes).map_err(|_| io::Error::other("cleanup record is not valid JSON"))
}

fn cleanup_record_revision(value: &serde_json::Value) -> io::Result<Option<u64>> {
    match value.get("revision") {
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| io::Error::other("cleanup record revision is invalid")),
        None => Ok(None),
    }
}

fn validate_record_bytes_transition(
    record_id: &str,
    current_bytes: Option<&[u8]>,
    candidate_bytes: &[u8],
) -> io::Result<serde_json::Value> {
    let candidate = cleanup_record_value(candidate_bytes)?;
    validate_namespace_id(record_id)?;
    if candidate["recordId"].as_str() != Some(record_id) {
        return Err(io::Error::other("cleanup record identity changed"));
    }
    let candidate_revision = cleanup_record_revision(&candidate)?
        .ok_or_else(|| io::Error::other("new cleanup record revision is absent"))?;
    if let Some(current_bytes) = current_bytes {
        let current = cleanup_record_value(current_bytes)?;
        if current["recordId"].as_str() != Some(record_id) {
            return Err(io::Error::other("current cleanup record identity changed"));
        }
        let expected = cleanup_record_revision(&current)?
            .map_or(Some(0), |value| value.checked_add(1))
            .ok_or_else(|| io::Error::other("cleanup record revision exhausted"))?;
        if candidate_revision != expected {
            return Err(io::Error::other(
                "cleanup record revision is not contiguous",
            ));
        }
        if let Some(nonce) = current.get("controlNonce").and_then(|value| value.as_str())
            && candidate["controlNonce"].as_str() != Some(nonce)
        {
            return Err(io::Error::other("cleanup record control nonce changed"));
        }
    } else if candidate_revision != 0 {
        return Err(io::Error::other(
            "initial cleanup record revision is not zero",
        ));
    }
    Ok(candidate)
}

fn validate_record_transition(
    path: &Path,
    candidate_bytes: &[u8],
    expected_current_sha256: Option<&str>,
) -> io::Result<serde_json::Value> {
    let record_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("cleanup record path has no identity"))?;
    match read_protected_record(path)? {
        Some(current_bytes) => {
            let current_sha256 = Sha256::digest(&current_bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if expected_current_sha256 != Some(current_sha256.as_str()) {
                return Err(io::Error::other("current cleanup record digest changed"));
            }
            validate_record_bytes_transition(record_id, Some(&current_bytes), candidate_bytes)
        }
        None if expected_current_sha256.is_some() => {
            Err(io::Error::other("expected cleanup record is absent"))
        }
        None => validate_record_bytes_transition(record_id, None, candidate_bytes),
    }
}

fn read_protected_record(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ | READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(None);
        }
        return Err(error);
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    read_handle(file.0).map(Some)
}

fn cleanup_record_backup_path(path: &Path) -> io::Result<PathBuf> {
    let record_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("cleanup record path has no identity"))?;
    validate_namespace_id(record_id)?;
    Ok(path.with_file_name(format!("{record_id}.previous-v1")))
}

fn open_record_frozen(path: &Path, expected_sha256: &str) -> io::Result<(Handle, Vec<u8>)> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | DELETE
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
            FILE_SHARE_READ,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    let bytes = read_handle(file.0)?;
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected_sha256 {
        return Err(io::Error::other("current cleanup record digest changed"));
    }
    Ok((file, bytes))
}

fn open_record_for_swap(path: &Path, expected_sha256: &str) -> io::Result<(Handle, Vec<u8>)> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | DELETE
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    let bytes = read_handle(file.0)?;
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != expected_sha256 {
        return Err(io::Error::other("current cleanup record digest changed"));
    }
    Ok((file, bytes))
}

pub fn publish_cleanup_record(
    path: &Path,
    bytes: &[u8],
    expected_current_sha256: Option<&str>,
) -> io::Result<()> {
    let candidate = validate_record_transition(path, bytes, expected_current_sha256)?;
    let crash_at = |seam: &str| {
        let phase_matches = std::env::var("TQ_MACHINE_LOCK_TEST_RECORD_CRASH_PHASE")
            .map(|phase| candidate["phase"].as_str() == Some(&phase))
            .unwrap_or(true);
        if phase_matches {
            record_crash_at(seam);
        }
    };
    let pending = create_native_record_pending(path)?;
    crash_at("native-record-temp-created");
    pending.flush_parent()?;
    crash_at("native-record-temp-parent-flushed");
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref()
        == Ok("native-record-temp-partial-written")
        && std::env::var("TQ_MACHINE_LOCK_TEST_RECORD_CRASH_PHASE")
            .map(|phase| candidate["phase"].as_str() == Some(&phase))
            .unwrap_or(true)
    {
        pending.write(&bytes[..bytes.len() / 2])?;
        pending.flush()?;
        std::process::exit(197);
    }
    pending.write(bytes)?;
    crash_at("native-record-temp-written");
    pending.flush()?;
    crash_at("native-record-temp-file-flushed");
    if let Some(expected_sha256) = expected_current_sha256 {
        let backup_path = cleanup_record_backup_path(path)?;
        if backup_path.exists() {
            return Err(io::Error::other(
                "cleanup record previous file already exists",
            ));
        }
        let (current, current_bytes) = open_record_for_swap(path, expected_sha256)?;
        let record_id = path.file_stem().and_then(|value| value.to_str()).unwrap();
        validate_record_bytes_transition(record_id, Some(&current_bytes), bytes)?;
        let backup_name = backup_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap();
        rename_file_handle(current.0, pending.parent.0, backup_name, false)?;
        crash_at("native-record-current-renamed");
        pending.flush_parent()?;
        crash_at("native-record-current-rename-parent-flushed");
        pending.rename(false)?;
        crash_at("native-record-temp-renamed");
        pending.flush_parent()?;
        crash_at("native-record-destination-parent-flushed");
        delete_file_handle(current.0)?;
        crash_at("native-record-previous-retired");
        pending.flush_parent()?;
        crash_at("native-record-previous-retirement-parent-flushed");
    } else {
        validate_record_transition(path, bytes, None)?;
        pending.rename(false)?;
        crash_at("native-record-temp-renamed");
        pending.flush_parent()?;
        crash_at("native-record-destination-parent-flushed");
    }
    Ok(())
}

pub fn recover_cleanup_record_backup(path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("cleanup record previous name is invalid"))?;
    let record_id = name
        .strip_suffix(".previous-v1")
        .ok_or_else(|| io::Error::other("cleanup record previous suffix is invalid"))?;
    validate_namespace_id(record_id)?;
    let destination = path.with_file_name(format!("{record_id}.json"));
    let parent = open_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("cleanup record previous has no parent"))?,
    )?;
    let previous = open_exact_protected_file(path)?;
    let previous_bytes = read_handle(previous.0)?;
    if let Some(destination_bytes) = read_protected_record(&destination)? {
        let destination_sha256 = Sha256::digest(&destination_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (_destination, frozen_bytes) = open_record_frozen(&destination, &destination_sha256)?;
        validate_record_bytes_transition(record_id, Some(&previous_bytes), &frozen_bytes)?;
        delete_file_handle(previous.0)?;
    } else {
        rename_file_handle(previous.0, parent.0, &format!("{record_id}.json"), false)?;
    }
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn inspect_cleanup_record_pending(path: &Path) -> io::Result<Vec<u8>> {
    validate_cleanup_record_pending_name(path)?;
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    read_handle(file.0)
}

fn validate_cleanup_record_pending_name(path: &Path) -> io::Result<(&str, &str)> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("cleanup record temporary name is invalid"))?;
    let captures = name
        .strip_suffix(".pending-v1")
        .and_then(|value| value.rsplit_once(".native-"))
        .ok_or_else(|| io::Error::other("cleanup record temporary name is invalid"))?;
    validate_namespace_id(captures.0)?;
    if captures.1.len() != 32 || !captures.1.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other(
            "cleanup record temporary nonce is invalid",
        ));
    }
    Ok(captures)
}

pub fn recover_cleanup_record_pending(
    path: &Path,
    expected_candidate_sha256: &str,
) -> io::Result<bool> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("cleanup record temporary has no parent"))?;
    let captures = validate_cleanup_record_pending_name(path)?;
    let destination = parent_path.join(format!("{}.json", captures.0));
    let parent = open_directory(parent_path)?;
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | DELETE
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    let bytes = read_handle(file.0)?;
    let candidate_sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if candidate_sha256 != expected_candidate_sha256 {
        return Err(io::Error::other("cleanup record temporary digest changed"));
    }
    let current_bytes = read_protected_record(&destination)?;
    let expected_sha256 = current_bytes.as_ref().map(|bytes| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    if validate_record_transition(&destination, &bytes, expected_sha256.as_deref()).is_err() {
        if cleanup_record_value(&bytes).is_ok() {
            return Err(io::Error::other(
                "complete cleanup record temporary failed transition validation",
            ));
        }
        delete_file_handle(file.0)?;
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(false);
    }
    if let Some(expected_sha256) = expected_sha256.as_deref() {
        let backup_path = cleanup_record_backup_path(&destination)?;
        if backup_path.exists() {
            return Err(io::Error::other(
                "cleanup record previous file already exists",
            ));
        }
        let (current, current_bytes) = open_record_for_swap(&destination, expected_sha256)?;
        validate_record_bytes_transition(captures.0, Some(&current_bytes), &bytes)?;
        let backup_name = backup_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap();
        rename_file_handle(current.0, parent.0, backup_name, false)?;
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        rename_file_handle(file.0, parent.0, &format!("{}.json", captures.0), false)?;
        if unsafe { FlushFileBuffers(parent.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        delete_file_handle(current.0)?;
    } else {
        validate_record_transition(&destination, &bytes, None)?;
        rename_file_handle(file.0, parent.0, &format!("{}.json", captures.0), false)?;
    }
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(true)
}

fn rename_file_handle(
    file: HANDLE,
    destination_parent: HANDLE,
    destination: &str,
    replace: bool,
) -> io::Result<()> {
    let destination = destination.encode_utf16().collect::<Vec<_>>();
    let name_bytes = destination.len() * size_of::<u16>();
    let header = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let mut buffer = vec![0_u8; size_of::<FILE_RENAME_INFO>() + name_bytes];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*information).Anonymous.ReplaceIfExists = replace;
        (*information).RootDirectory = destination_parent;
        (*information).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            destination.as_ptr(),
            buffer.as_mut_ptr().add(header).cast(),
            destination.len(),
        );
    }
    let mut status_block = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    let status = unsafe {
        NtSetInformationFile(
            file,
            &mut status_block,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            10,
        )
    };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredCleanupLog {
    pub byte_length: u64,
    pub sha256: String,
    pub content_prefix: String,
    pub prefix_byte_length: u64,
    pub truncated: bool,
    pub file_identity: String,
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn streamed_log_evidence(handle: HANDLE, prefix_limit: usize) -> io::Result<RecoveredCleanupLog> {
    let mut digest = Sha256::new();
    let mut prefix = Vec::with_capacity(prefix_limit);
    let mut byte_length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let mut read = 0;
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            break;
        }
        let bytes = &buffer[..read as usize];
        digest.update(bytes);
        byte_length = byte_length
            .checked_add(read.into())
            .ok_or_else(|| io::Error::other("cleanup log length overflow"))?;
        let remaining = prefix_limit.saturating_sub(prefix.len());
        prefix.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
    }
    Ok(RecoveredCleanupLog {
        byte_length,
        sha256: digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        content_prefix: base64_encode(&prefix),
        prefix_byte_length: prefix.len() as u64,
        truncated: byte_length > prefix.len() as u64,
        file_identity: file_id_128(handle)?,
    })
}

fn open_exact_protected_file(path: &Path) -> io::Result<Handle> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | DELETE
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    Ok(file)
}

fn validate_cleanup_log_path(path: &Path, record_id: &str, stream: &str) -> io::Result<()> {
    validate_namespace_id(record_id)?;
    if path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        != Some(".cleanup-records-v1")
    {
        return Err(io::Error::other("cleanup log parent is invalid"));
    }
    let suffix = match stream {
        "stdout" => ".stdout.log",
        "stderr" => ".stderr.log",
        "combined" => ".log",
        _ => return Err(io::Error::other("cleanup log stream is invalid")),
    };
    if path.file_name().and_then(|value| value.to_str()) != Some(&format!("{record_id}{suffix}")) {
        return Err(io::Error::other("cleanup log path is not record-bound"));
    }
    Ok(())
}

pub fn inspect_cleanup_log(
    path: &Path,
    record_id: &str,
    stream: &str,
    expected_parent_identity: &str,
) -> io::Result<Option<RecoveredCleanupLog>> {
    validate_cleanup_log_path(path, record_id, stream)?;
    let parent = open_directory(path.parent().unwrap())?;
    validate_directory(parent.0)?;
    if file_id_128(parent.0)? != expected_parent_identity {
        return Err(io::Error::other("cleanup log parent identity changed"));
    }
    let file = match open_exact_protected_file(path) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Ok(None),
        Err(error) => return Err(error),
    };
    streamed_log_evidence(file.0, 64 * 1024).map(Some)
}

pub fn delete_cleanup_log(
    path: &Path,
    record_id: &str,
    expected_sha256: Option<&str>,
    expected_byte_length: Option<u64>,
    expected_file_identity: Option<&str>,
    stream: &str,
    expected_parent_identity: &str,
) -> io::Result<()> {
    validate_cleanup_log_path(path, record_id, stream)?;
    let parent = open_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("cleanup log has no parent"))?,
    )?;
    validate_directory(parent.0)?;
    if file_id_128(parent.0)? != expected_parent_identity {
        return Err(io::Error::other("cleanup log parent identity changed"));
    }
    match open_exact_protected_file(path) {
        Ok(file) => {
            let actual = streamed_log_evidence(file.0, 0)?;
            if expected_sha256 != Some(actual.sha256.as_str())
                || expected_byte_length != Some(actual.byte_length)
                || expected_file_identity != Some(actual.file_identity.as_str())
            {
                return Err(io::Error::other(
                    "cleanup log identity or content changed before deletion",
                ));
            }
            delete_file_handle(file.0)?;
            record_crash_at(&format!("recovered-log-retired:{stream}"));
        }
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            if expected_sha256.is_some()
                || expected_byte_length.is_some()
                || expected_file_identity.is_some()
            {
                return Err(io::Error::other("authenticated cleanup log disappeared"));
            }
        }
        Err(error) => return Err(error),
    }
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    record_crash_at(&format!(
        "recovered-log-retirement-directory-flushed:{stream}"
    ));
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyEvidenceRoot {
    pub identity: String,
    pub names: Vec<String>,
}

fn validate_legacy_evidence_root_path(path: &Path) -> io::Result<()> {
    if path.file_name().and_then(|value| value.to_str()) != Some("machine-lock-log-evidence-v1")
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            != Some("tmp")
    {
        return Err(io::Error::other("legacy evidence root path is invalid"));
    }
    Ok(())
}

fn open_legacy_evidence_root(path: &Path, expected_identity: Option<&str>) -> io::Result<Handle> {
    validate_legacy_evidence_root_path(path)?;
    let root = open_directory(path)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    if let Some(expected) = expected_identity
        && identity(root.0)? != expected
    {
        return Err(io::Error::other("legacy evidence root identity changed"));
    }
    Ok(root)
}

fn validate_legacy_evidence_path(
    path: &Path,
    record_id: &str,
    stream: &str,
    pending: bool,
) -> io::Result<()> {
    validate_namespace_id(record_id)?;
    validate_legacy_evidence_root_path(
        path.parent()
            .ok_or_else(|| io::Error::other("legacy evidence file has no parent"))?,
    )?;
    if !matches!(stream, "stdout" | "stderr" | "combined") {
        return Err(io::Error::other("legacy evidence stream is invalid"));
    }
    let pending_suffix = if pending { ".pending-v1" } else { "" };
    let expected = format!("{record_id}.{stream}.evidence-v1{pending_suffix}");
    if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
        return Err(io::Error::other("legacy evidence path is not record-bound"));
    }
    Ok(())
}

pub fn protect_legacy_evidence_root(path: &Path) -> io::Result<()> {
    validate_legacy_evidence_root_path(path)?;
    let descriptor = exact_descriptor(true)?;
    if unsafe {
        SetFileSecurityW(
            wide(path)?.as_ptr(),
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let root = open_legacy_evidence_root(path, None)?;
    if unsafe { FlushFileBuffers(root.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn create_legacy_evidence_fixture(
    path: &Path,
    record_id: &str,
    stream: &str,
    pending: bool,
    bytes: &[u8],
) -> io::Result<()> {
    validate_legacy_evidence_path(path, record_id, stream, pending)?;
    let _root = open_legacy_evidence_root(path.parent().unwrap(), None)?;
    write_new_protected_file(path, bytes, "legacy-evidence-fixture")
}

pub fn inspect_legacy_evidence_root(path: &Path) -> io::Result<LegacyEvidenceRoot> {
    let root = open_legacy_evidence_root(path, None)?;
    let mut names = std::fs::read_dir(path)?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("legacy evidence directory entry name is invalid"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(LegacyEvidenceRoot {
        identity: identity(root.0)?,
        names,
    })
}

pub fn inspect_legacy_evidence(
    path: &Path,
    record_id: &str,
    stream: &str,
    pending: bool,
    expected_root_identity: &str,
) -> io::Result<Option<RecoveredCleanupLog>> {
    validate_legacy_evidence_path(path, record_id, stream, pending)?;
    let _root = open_legacy_evidence_root(path.parent().unwrap(), Some(expected_root_identity))?;
    let file = match open_exact_protected_file(path) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Ok(None),
        Err(error) => return Err(error),
    };
    streamed_log_evidence(file.0, 64 * 1024).map(Some)
}

pub struct ExpectedStreamedLog<'a> {
    pub sha256: Option<&'a str>,
    pub byte_length: Option<u64>,
    pub file_identity: Option<&'a str>,
}

pub fn delete_legacy_evidence(
    path: &Path,
    record_id: &str,
    stream: &str,
    pending: bool,
    expected_root_identity: &str,
    expected: ExpectedStreamedLog<'_>,
) -> io::Result<()> {
    validate_legacy_evidence_path(path, record_id, stream, pending)?;
    let root = open_legacy_evidence_root(path.parent().unwrap(), Some(expected_root_identity))?;
    match open_exact_protected_file(path) {
        Ok(file) => {
            let actual = streamed_log_evidence(file.0, 0)?;
            if expected.sha256 != Some(actual.sha256.as_str())
                || expected.byte_length != Some(actual.byte_length)
                || expected.file_identity != Some(actual.file_identity.as_str())
            {
                return Err(io::Error::other("legacy evidence changed before deletion"));
            }
            delete_file_handle(file.0)?;
            record_crash_at(&format!("legacy-evidence-retired:{stream}"));
        }
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            if expected.sha256.is_some()
                || expected.byte_length.is_some()
                || expected.file_identity.is_some()
            {
                return Err(io::Error::other(
                    "authenticated legacy evidence disappeared",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    if unsafe { FlushFileBuffers(root.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    record_crash_at(&format!("legacy-evidence-directory-flushed:{stream}"));
    Ok(())
}

pub fn remove_empty_legacy_evidence_root(
    path: &Path,
    expected_root_identity: &str,
) -> io::Result<()> {
    let parent = open_directory(
        path.parent()
            .ok_or_else(|| io::Error::other("legacy evidence root has no parent"))?,
    )?;
    let root = open_legacy_evidence_root(path, Some(expected_root_identity))?;
    if std::fs::read_dir(path)?.next().is_some() {
        return Err(io::Error::other("legacy evidence root is not empty"));
    }
    handle_delete_empty_root(root, parent)?;
    record_crash_at("legacy-evidence-root-retired");
    Ok(())
}

pub fn delete_cleanup_record(path: &Path) -> io::Result<()> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("cleanup record has no parent"))?;
    let parent = open_directory(parent_path)?;
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            DELETE | READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    delete_file_handle(file.0)?;
    record_crash_at("cleanup-record-retired");
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    record_crash_at("cleanup-record-retirement-directory-flushed");
    Ok(())
}

fn delete_file_handle(file: HANDLE) -> io::Result<()> {
    let information = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file,
            FileDispositionInfo,
            (&raw const information).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn create_native_record_pending(path: &Path) -> io::Result<NativeRecordPending> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("cleanup record has no parent"))?;
    let destination = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::other("cleanup record name is invalid"))?;
    let record_id = destination
        .strip_suffix(".json")
        .ok_or_else(|| io::Error::other("cleanup record suffix is invalid"))?;
    validate_namespace_id(record_id)?;
    let parent = open_directory(parent_path)?;
    let descriptor = exact_descriptor(false)?;
    for _ in 0..32 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let name = format!("{record_id}.native-{suffix}.pending-v1");
        let mut name_wide = name.encode_utf16().collect::<Vec<_>>();
        let mut unicode = UnicodeString {
            length: (name_wide.len() * 2) as u16,
            maximum_length: (name_wide.len() * 2) as u16,
            buffer: name_wide.as_mut_ptr(),
        };
        let mut attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: parent.0,
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: descriptor.0,
            security_quality_of_service: null_mut(),
        };
        let mut status_block = IoStatusBlock {
            status_or_pointer: 0,
            information: 0,
        };
        let mut handle = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                windows_sys::Win32::Foundation::GENERIC_WRITE
                    | DELETE
                    | FILE_READ_ATTRIBUTES
                    | READ_CONTROL
                    | 0x0010_0000,
                &mut attributes,
                &mut status_block,
                null_mut(),
                FILE_ATTRIBUTE_NORMAL,
                0,
                FILE_CREATE,
                FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT_NT,
                null_mut(),
                0,
            )
        };
        if status < 0 {
            let error = unsafe { RtlNtStatusToDosError(status) };
            if error == windows_sys::Win32::Foundation::ERROR_FILE_EXISTS {
                continue;
            }
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let file = Handle(handle);
        validate_exact_security(file.0, false)?;
        validate_regular_file(file.0)?;
        return Ok(NativeRecordPending {
            parent,
            file,
            destination: destination.encode_utf16().collect(),
            name,
        });
    }
    Err(io::Error::other(
        "cannot allocate a unique native cleanup-record pending name",
    ))
}

pub struct CleanupRecordPrivacyGuard {
    _handle: Handle,
    _lock: Box<OVERLAPPED>,
}

pub fn retain_private_cleanup_record(
    path: &Path,
    record_id: &str,
    control_nonce: &str,
) -> io::Result<CleanupRecordPrivacyGuard> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ | READ_CONTROL,
            0,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    validate_regular_file(file.0)?;
    let record: serde_json::Value = serde_json::from_slice(&read_handle(file.0)?)
        .map_err(|_| io::Error::other("cleanup record is not valid JSON"))?;
    let probe = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if probe.0 != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::other(
            "cleanup record privacy handle permits a second reader",
        ));
    }
    let mut lock: Box<OVERLAPPED> = Box::new(unsafe { zeroed() });
    if unsafe {
        LockFileEx(
            file.0,
            LOCKFILE_EXCLUSIVE_LOCK,
            0,
            u32::MAX,
            u32::MAX,
            lock.as_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if record["schemaVersion"].as_u64() != Some(5)
        || record["recordId"].as_str() != Some(record_id)
        || record["controlNonce"].as_str() != Some(control_nonce)
    {
        return Err(io::Error::other(
            "cleanup record private control identity changed",
        ));
    }
    Ok(CleanupRecordPrivacyGuard {
        _handle: file,
        _lock: lock,
    })
}

pub struct RegistryNamespaceSession {
    namespace_id: String,
    _software: Key,
    test_root: Key,
    namespace: Key,
}

impl RegistryNamespaceSession {
    pub fn inventory(&self) -> io::Result<RegistryInventory> {
        inventory_from_handles(&self.test_root, &self.namespace, None)
    }

    pub fn delete_handle_bound(self, expected: &RegistryInventory) -> io::Result<()> {
        if self.inventory()? != *expected {
            return Err(io::Error::other(
                "registry namespace changed after inventory seal",
            ));
        }
        delete_registry_exact_internal(
            &self.namespace_id,
            expected,
            Some((&self.test_root, &self.namespace)),
        )
    }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { RegCloseKey(self.0) };
        }
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryInventory {
    pub present: bool,
    pub namespace: Option<KeyInventory>,
    pub recovery: Option<KeyInventory>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInventory {
    pub security: String,
    pub subkeys: Vec<String>,
    pub values: Vec<RegistryValue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryValue {
    pub name: String,
    pub value_type: u32,
    pub data_hex: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInventory {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

pub fn stream_inventory(path: &Path) -> io::Result<Vec<StreamInventory>> {
    if std::env::var_os("TQ_MACHINE_LOCK_TEST_STREAM_INVENTORY_FAIL").is_some() {
        return Err(io::Error::other("forced stream inventory failure"));
    }
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut context = null_mut();
    let result = stream_inventory_inner(file.0, &mut context);
    let mut ignored = 0;
    unsafe {
        BackupRead(file.0, null_mut(), 0, &mut ignored, 1, 0, &mut context);
    }
    result
}

fn stream_inventory_inner(
    handle: HANDLE,
    context: &mut *mut c_void,
) -> io::Result<Vec<StreamInventory>> {
    const HEADER_SIZE: usize = 20;
    let mut streams = Vec::new();
    loop {
        let mut header = [0_u8; HEADER_SIZE];
        let read = backup_read(handle, &mut header, context)?;
        if read == 0 {
            break;
        }
        if read != HEADER_SIZE {
            return Err(io::Error::other("truncated backup stream header"));
        }
        let stream_id = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let size = i64::from_le_bytes(header[8..16].try_into().unwrap());
        let name_size = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
        if size < 0 || !name_size.is_multiple_of(2) || name_size > 64 * 1024 {
            return Err(io::Error::other("invalid backup stream metadata"));
        }
        let mut name_bytes = vec![0_u8; name_size];
        backup_read_exact(handle, &mut name_bytes, context)?;
        let name_units = name_bytes
            .chunks_exact(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect::<Vec<_>>();
        let mut digest = Sha256::new();
        let mut remaining = size as u64;
        let mut buffer = [0_u8; 64 * 1024];
        while remaining != 0 {
            let length = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
            backup_read_exact(handle, &mut buffer[..length], context)?;
            if stream_id == BACKUP_DATA || stream_id == BACKUP_ALTERNATE_DATA {
                digest.update(&buffer[..length]);
            }
            remaining -= length as u64;
        }
        if stream_id == BACKUP_DATA || stream_id == BACKUP_ALTERNATE_DATA {
            let name = if name_units.is_empty() {
                "::$DATA".to_owned()
            } else {
                String::from_utf16(&name_units).map_err(io::Error::other)?
            };
            streams.push(StreamInventory {
                name,
                size: size as u64,
                sha256: digest
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            });
        }
    }
    streams.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(streams)
}

fn backup_read(handle: HANDLE, buffer: &mut [u8], context: &mut *mut c_void) -> io::Result<usize> {
    let mut read = 0;
    if unsafe {
        BackupRead(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut read,
            0,
            0,
            context,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(read as usize)
}

fn backup_read_exact(
    handle: HANDLE,
    mut buffer: &mut [u8],
    context: &mut *mut c_void,
) -> io::Result<()> {
    while !buffer.is_empty() {
        let read = backup_read(handle, buffer, context)?;
        if read == 0 {
            return Err(io::Error::other("truncated backup stream"));
        }
        buffer = &mut buffer[read..];
    }
    Ok(())
}

pub fn delete_empty_registry_root() -> io::Result<()> {
    let transaction =
        Handle(unsafe { CreateTransaction(null_mut(), null_mut(), 0, 0, 0, 30_000, null()) });
    if transaction.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let Some(software) = open_relative_key_transacted(
        HKEY_CURRENT_USER,
        "Software",
        KEY_READ | KEY_WRITE,
        transaction.0,
    )?
    else {
        return Ok(());
    };
    verify_key_path(&software, &["Software"])?;
    reject_registry_link(&software)?;
    let Some(test_root) = open_relative_key_transacted(
        software.0,
        "Talking Quill Tests",
        KEY_READ | KEY_WRITE,
        transaction.0,
    )?
    else {
        return Ok(());
    };
    verify_relative_key_path(&software, &test_root, "Talking Quill Tests")?;
    reject_registry_link(&test_root)?;
    let inventory = inventory_key(&test_root)?;
    if !inventory.subkeys.is_empty() || !inventory.values.is_empty() {
        return Err(io::Error::other("test registry root is not empty"));
    }
    registry_delete_test_pause()?;
    if inventory_key(&test_root)? != inventory {
        return Err(io::Error::other(
            "test registry root changed before deletion",
        ));
    }
    let status = unsafe {
        RegDeleteKeyTransactedW(
            software.0,
            wide(Path::new("Talking Quill Tests"))?.as_ptr(),
            0,
            0,
            transaction.0,
            null(),
        )
    };
    if status != ERROR_SUCCESS || unsafe { CommitTransaction(transaction.0) } == 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

pub fn create_empty_registry_root_fixture() -> io::Result<()> {
    let software = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ | KEY_WRITE)?
        .ok_or_else(|| io::Error::other("HKCU Software is absent"))?;
    verify_key_path(&software, &["Software"])?;
    if open_relative_key(&software, "Talking Quill Tests", KEY_READ)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "test registry root exists",
        ));
    }
    let root = create_relative_key(&software, "Talking Quill Tests")?;
    verify_relative_key_path(&software, &root, "Talking Quill Tests")?;
    if unsafe { RegFlushKey(root.0) } != ERROR_SUCCESS {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn create_registry_link_fixture() -> io::Result<()> {
    let software = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ | KEY_WRITE)?
        .ok_or_else(|| io::Error::other("HKCU Software is absent"))?;
    verify_key_path(&software, &["Software"])?;
    if open_relative_key(&software, "Talking Quill Tests", KEY_READ)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "test registry root exists",
        ));
    }
    let descriptor = exact_descriptor(false)?;
    let attributes = security_attributes(&descriptor);
    let mut key = null_mut();
    let mut disposition = 0;
    let status = unsafe {
        RegCreateKeyExW(
            software.0,
            wide(Path::new("Talking Quill Tests"))?.as_ptr(),
            0,
            null(),
            REG_OPTION_CREATE_LINK,
            KEY_READ | KEY_WRITE,
            &attributes,
            &mut key,
            &mut disposition,
        )
    };
    let link = Key(key);
    if status != ERROR_SUCCESS || disposition != REG_CREATED_NEW_KEY {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let target = wide(Path::new(&format!(
        r"\REGISTRY\USER\{}\Software",
        current_user_sid()?
    )))?;
    let status = unsafe {
        RegSetValueExW(
            link.0,
            wide(Path::new("SymbolicLinkValue"))?.as_ptr(),
            0,
            REG_LINK_TYPE,
            target.as_ptr().cast(),
            (target.len() * size_of::<u16>()) as u32,
        )
    };
    if status != ERROR_SUCCESS || unsafe { RegFlushKey(link.0) } != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

pub fn remove_registry_link_fixture() -> io::Result<()> {
    let software = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ | KEY_WRITE)?
        .ok_or_else(|| io::Error::other("HKCU Software is absent"))?;
    verify_key_path(&software, &["Software"])?;
    let link = open_relative_key(
        &software,
        "Talking Quill Tests",
        KEY_READ | KEY_WRITE | KEY_CREATE_LINK | DELETE,
    )
    .map_err(|error| io::Error::other(format!("cannot open registry link fixture: {error}")))?
    .ok_or_else(|| io::Error::other("registry link fixture is absent"))?;
    verify_relative_key_path(&software, &link, "Talking Quill Tests").map_err(|error| {
        io::Error::other(format!("cannot verify registry link fixture: {error}"))
    })?;
    let mut value_type = 0;
    let mut length = 0;
    let value_name = wide(Path::new("SymbolicLinkValue"))?;
    let status = unsafe {
        RegQueryValueExW(
            link.0,
            value_name.as_ptr(),
            null(),
            &mut value_type,
            null_mut(),
            &mut length,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    if value_type != REG_LINK_TYPE || length == 0 || length > 4096 {
        return Err(io::Error::other("registry link fixture changed"));
    }
    let status = unsafe { NtDeleteKey(link.0) };
    if status < 0 {
        return Err(io::Error::other(format!(
            "cannot delete retained registry link fixture: NTSTATUS {status:#x}"
        )));
    }
    Ok(())
}

pub fn registry_root_inventory() -> io::Result<Option<KeyInventory>> {
    let Some(software) = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ)? else {
        return Ok(None);
    };
    verify_key_path(&software, &["Software"])?;
    reject_registry_link(&software)?;
    let Some(test_root) = open_relative_key(&software, "Talking Quill Tests", KEY_READ)? else {
        return Ok(None);
    };
    verify_relative_key_path(&software, &test_root, "Talking Quill Tests")?;
    reject_registry_link(&test_root)?;
    Ok(Some(inventory_key(&test_root)?))
}

pub fn harden_current_process_for_supervised_child() -> io::Result<()> {
    let descriptor_sddl = format!(
        "D:P(D;;0x000fefff;;;{0})(D;;0x000fefff;;;OW)(A;;0x001fffff;;;SY)(A;;0x00101000;;;{0})",
        current_user_sid()?
    );
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide(Path::new(&descriptor_sddl))?.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Descriptor(descriptor);
    if unsafe {
        SetKernelObjectSecurity(
            GetCurrentProcess(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut actual = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            GetCurrentProcess(),
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut actual,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let actual = Descriptor(actual);
    let expected_text = descriptor_text(descriptor.0)?;
    let actual_text = descriptor_text(actual.0)?;
    if actual_text != expected_text {
        return Err(io::Error::other(format!(
            "supervisor process DACL verification failed: {actual_text} != {expected_text}"
        )));
    }
    Ok(())
}

pub fn create_protected_root(
    path: &Path,
    expected_parent_identity: &str,
    ownership_prefix: &str,
    binding_path: &Path,
    binding_nonce: &str,
) -> io::Result<(String, String)> {
    let session = create_protected_root_retained(
        path,
        expected_parent_identity,
        ownership_prefix,
        binding_path,
        binding_nonce,
    )?;
    Ok((session.identity.clone(), session.ads_sha256.clone()))
}

pub fn create_protected_root_retained(
    path: &Path,
    expected_parent_identity: &str,
    ownership_prefix: &str,
    binding_path: &Path,
    binding_nonce: &str,
) -> io::Result<ProtectedRootSession> {
    validate_prefix(ownership_prefix)?;
    validate_nonce(binding_nonce)?;
    let intent = CreationIntent {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
    };
    write_new_protected_file(
        &intent_path(binding_path),
        &serde_json::to_vec(&intent).map_err(io::Error::other)?,
        "intent",
    )?;
    crash_at("intent-flushed-before-root");
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("protected root has no parent"))?;
    let parent = open_creation_parent(parent_path)?;
    validate_directory(parent.0)?;
    if identity(parent.0)? != expected_parent_identity {
        return Err(io::Error::other("protected root parent identity changed"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("protected root has no name"))?;
    let directory_descriptor = exact_descriptor(true)?;
    let root = create_relative_directory(parent.0, name, directory_descriptor.0)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    crash_at("create-before-binding");
    let identity = identity(root.0)?;
    let ads = format!("{ownership_prefix}:{identity}");
    let ads_sha256 = hex_digest(ads.as_bytes());
    let binding = CleanupBinding {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
        root_identity: identity.clone(),
        ads_sha256: ads_sha256.clone(),
    };
    let binding_bytes = serde_json::to_vec(&binding).map_err(io::Error::other)?;
    write_new_protected_file(binding_path, &binding_bytes, "binding")?;
    crash_at("binding-flushed-before-ads");
    root_publication_test_pause()?;
    let stream = create_relative_ownership_stream(root.0)?;
    write_and_flush_handle(stream.0, ads.as_bytes(), "ads")?;
    drop(stream);
    if unsafe { FlushFileBuffers(root.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    crash_at("ads-flushed-before-return");
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    crash_at("ads-parent-flushed");
    Ok(ProtectedRootSession {
        parent_path: parent_path.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
        binding_path: binding_path.to_owned(),
        binding_nonce: binding_nonce.to_owned(),
        identity,
        ads_sha256,
        parent,
        root,
    })
}

pub fn remove_interrupted_root(
    path: &Path,
    ownership_prefix: &str,
    binding_path: &Path,
    binding_nonce: &str,
) -> io::Result<()> {
    validate_prefix(ownership_prefix)?;
    validate_nonce(binding_nonce)?;
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("protected root has no parent"))?;
    let parent = open_directory(parent_path)?;
    let root = open_directory(path)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    if std::fs::read_dir(path)?.next().is_some() {
        return Err(io::Error::other("interrupted root is not empty"));
    }
    let identity = identity(root.0)?;
    let expected_intent = expected_intent(ownership_prefix, binding_nonce);
    if read_intent(&intent_path(binding_path))? != expected_intent {
        return Err(io::Error::other("external creation intent changed"));
    }
    let binding = read_binding(binding_path).ok().filter(|binding| {
        binding.version == 1
            && binding.nonce == binding_nonce
            && binding.ownership_prefix == ownership_prefix
            && binding.root_identity == identity
    });
    if binding.is_none() && read_stream(&ownership_stream(path)?).is_ok() {
        return Err(io::Error::other(
            "ownership ADS exists without a complete external binding",
        ));
    }
    handle_delete_empty_root(root, parent)?;
    if let Some(binding) = binding {
        delete_binding(binding_path, &binding)?;
    } else if binding_path.try_exists()? {
        delete_exact_protected_file(binding_path)?;
    }
    delete_intent(&intent_path(binding_path), &expected_intent)?;
    Ok(())
}

pub fn verify_root_binding(
    path: &Path,
    binding_path: &Path,
    ownership_prefix: &str,
    binding_nonce: &str,
    root_identity: &str,
    ads_sha256: &str,
) -> io::Result<()> {
    let root = open_directory(path)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    if identity(root.0)? != root_identity {
        return Err(io::Error::other("root binding identity changed"));
    }
    let expected = CleanupBinding {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
        root_identity: root_identity.to_owned(),
        ads_sha256: ads_sha256.to_owned(),
    };
    if read_binding(binding_path)? != expected {
        return Err(io::Error::other("external cleanup binding changed"));
    }
    if read_intent(&intent_path(binding_path))? != expected_intent(ownership_prefix, binding_nonce)
    {
        return Err(io::Error::other("external creation intent changed"));
    }
    let ads = read_stream(&ownership_stream(path)?)?;
    if hex_digest(ads.as_bytes()) != ads_sha256 {
        return Err(io::Error::other("root ownership ADS changed"));
    }
    Ok(())
}

pub fn verify_deleted_root_binding(
    binding_path: &Path,
    ownership_prefix: &str,
    binding_nonce: &str,
    root_identity: &str,
    ads_sha256: &str,
) -> io::Result<()> {
    let expected = CleanupBinding {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
        root_identity: root_identity.to_owned(),
        ads_sha256: ads_sha256.to_owned(),
    };
    let binding_present = binding_path.try_exists()?;
    let intent_path = intent_path(binding_path);
    let intent_present = intent_path.try_exists()?;
    if binding_present && !intent_present {
        return Err(io::Error::other(
            "deleted root binding exists without its intent",
        ));
    }
    if binding_present && read_binding(binding_path)? != expected {
        return Err(io::Error::other("deleted root binding changed"));
    }
    if intent_present
        && read_intent(&intent_path)? != expected_intent(ownership_prefix, binding_nonce)
    {
        return Err(io::Error::other("deleted root creation intent changed"));
    }
    Ok(())
}

pub fn delete_cleanup_binding(
    binding_path: &Path,
    ownership_prefix: &str,
    binding_nonce: &str,
    root_identity: &str,
    ads_sha256: &str,
) -> io::Result<()> {
    let expected = CleanupBinding {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
        root_identity: root_identity.to_owned(),
        ads_sha256: ads_sha256.to_owned(),
    };
    if binding_path.try_exists()? {
        delete_binding(binding_path, &expected)?;
        crash_at("binding-deleted-before-intent");
    }
    let intent = expected_intent(ownership_prefix, binding_nonce);
    let intent_path = intent_path(binding_path);
    if intent_path.try_exists()? {
        delete_intent(&intent_path, &intent)?;
    }
    Ok(())
}

pub fn delete_interrupted_creation_artifacts(
    binding_path: &Path,
    ownership_prefix: &str,
    binding_nonce: &str,
) -> io::Result<()> {
    let intent_path = intent_path(binding_path);
    if binding_path.try_exists()? {
        delete_exact_protected_file(binding_path)?;
    }
    if intent_path.try_exists()? {
        match read_intent(&intent_path) {
            Ok(actual) if actual == expected_intent(ownership_prefix, binding_nonce) => {
                delete_intent(&intent_path, &actual)?;
            }
            Err(_) => delete_exact_protected_file(&intent_path)?,
            Ok(_) => return Err(io::Error::other("external creation intent changed")),
        }
    }
    Ok(())
}

pub fn create_registry_namespace(namespace_id: &str) -> io::Result<()> {
    create_registry_namespace_owned(namespace_id).map(drop)
}

pub fn create_registry_namespace_owned(namespace_id: &str) -> io::Result<RegistryNamespaceSession> {
    validate_namespace_id(namespace_id)?;
    let software = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ | KEY_WRITE)?
        .ok_or_else(|| io::Error::other("HKCU Software is absent"))?;
    verify_key_path(&software, &["Software"])?;
    reject_registry_link(&software)?;
    let test_root = match open_relative_key(&software, "Talking Quill Tests", KEY_READ | KEY_WRITE)?
    {
        Some(key) => key,
        None => create_relative_key(&software, "Talking Quill Tests")?,
    };
    verify_key_path(&test_root, &["Software", "Talking Quill Tests"])?;
    reject_registry_link(&test_root)?;
    if open_relative_key(&test_root, namespace_id, KEY_READ)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "registry namespace exists",
        ));
    }
    let namespace = create_relative_key(&test_root, namespace_id)?;
    verify_key_path(
        &namespace,
        &["Software", "Talking Quill Tests", namespace_id],
    )?;
    reject_registry_link(&namespace)?;
    if unsafe { RegFlushKey(namespace.0) } != ERROR_SUCCESS
        || unsafe { RegFlushKey(test_root.0) } != ERROR_SUCCESS
    {
        return Err(io::Error::last_os_error());
    }
    Ok(RegistryNamespaceSession {
        namespace_id: namespace_id.to_owned(),
        _software: software,
        test_root,
        namespace,
    })
}

pub fn registry_inventory(namespace_id: &str) -> io::Result<RegistryInventory> {
    validate_namespace_id(namespace_id)?;
    let Some((_, test_root, namespace)) = open_registry_chain(namespace_id, KEY_READ)? else {
        return Ok(RegistryInventory {
            present: false,
            namespace: None,
            recovery: None,
        });
    };
    inventory_from_handles(&test_root, &namespace, None)
}

pub fn delete_registry_exact(namespace_id: &str, expected: &RegistryInventory) -> io::Result<()> {
    delete_registry_exact_internal(namespace_id, expected, None)
}

fn delete_registry_exact_internal(
    namespace_id: &str,
    expected: &RegistryInventory,
    retained: Option<(&Key, &Key)>,
) -> io::Result<()> {
    validate_namespace_id(namespace_id)?;
    if !expected.present {
        return if registry_inventory(namespace_id)? == *expected {
            Ok(())
        } else {
            Err(io::Error::other("registry inventory changed"))
        };
    }
    let transaction =
        Handle(unsafe { CreateTransaction(null_mut(), null_mut(), 0, 0, 0, 30_000, null()) });
    if transaction.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let Some((software, test_root, namespace)) =
        open_registry_chain_transacted(namespace_id, KEY_READ | KEY_WRITE, transaction.0)?
    else {
        return Err(io::Error::other("registry namespace disappeared"));
    };
    let actual = inventory_from_handles(&test_root, &namespace, Some(transaction.0))?;
    if actual != *expected {
        return Err(io::Error::other("registry inventory changed"));
    }
    let expected_namespace = expected
        .namespace
        .as_ref()
        .ok_or_else(|| io::Error::other("missing namespace inventory"))?;
    if expected_namespace
        .subkeys
        .iter()
        .any(|name| name != "RecoveryStateLockV1")
    {
        return Err(io::Error::other("unexpected registry subkey"));
    }
    if let Some(recovery_expected) = &expected.recovery {
        if !recovery_expected.subkeys.is_empty() {
            return Err(io::Error::other("recovery registry key has descendants"));
        }
        let recovery = open_relative_key_transacted(
            namespace.0,
            "RecoveryStateLockV1",
            KEY_READ | KEY_WRITE,
            transaction.0,
        )?
        .ok_or_else(|| io::Error::other("recovery registry key disappeared"))?;
        verify_relative_key_path(&namespace, &recovery, "RecoveryStateLockV1")?;
        if inventory_key(&recovery)? != *recovery_expected {
            return Err(io::Error::other("recovery registry key changed"));
        }
        delete_recorded_values(&recovery, &recovery_expected.values)?;
        let remaining = inventory_key(&recovery)?;
        if !remaining.subkeys.is_empty()
            || !remaining.values.is_empty()
            || remaining.security != recovery_expected.security
        {
            return Err(io::Error::other(
                "recovery registry key changed during deletion",
            ));
        }
        if inventory_key(&recovery)? != remaining {
            return Err(io::Error::other(
                "recovery registry key changed immediately before deletion",
            ));
        }
        if unsafe {
            RegDeleteKeyTransactedW(
                namespace.0,
                wide(Path::new("RecoveryStateLockV1"))?.as_ptr(),
                0,
                0,
                transaction.0,
                null(),
            )
        } != ERROR_SUCCESS
        {
            return Err(io::Error::last_os_error());
        }
    }
    delete_recorded_values(&namespace, &expected_namespace.values)?;
    let remaining = inventory_key(&namespace)?;
    if !remaining.subkeys.is_empty()
        || !remaining.values.is_empty()
        || remaining.security != expected_namespace.security
    {
        return Err(io::Error::other(
            "registry namespace changed during deletion",
        ));
    }
    registry_delete_test_pause()?;
    if inventory_key(&namespace)? != remaining {
        return Err(io::Error::other(
            "registry namespace changed immediately before deletion",
        ));
    }
    if unsafe {
        RegDeleteKeyTransactedW(
            test_root.0,
            wide(Path::new(namespace_id))?.as_ptr(),
            0,
            0,
            transaction.0,
            null(),
        )
    } != ERROR_SUCCESS
    {
        return Err(io::Error::last_os_error());
    }
    let test_inventory = inventory_key(&test_root)?;
    if test_inventory.subkeys.is_empty() && test_inventory.values.is_empty() {
        let status = unsafe {
            RegDeleteKeyTransactedW(
                software.0,
                wide(Path::new("Talking Quill Tests"))?.as_ptr(),
                0,
                0,
                transaction.0,
                null(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    if let Some((retained_test_root, retained_namespace)) = retained {
        verify_key_path(retained_test_root, &["Software", "Talking Quill Tests"])?;
        verify_key_path(
            retained_namespace,
            &["Software", "Talking Quill Tests", namespace_id],
        )?;
        if inventory_from_handles(retained_test_root, retained_namespace, None)? != *expected {
            return Err(io::Error::other(
                "retained registry namespace changed before transaction commit",
            ));
        }
    }
    if matches!(
        std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref(),
        Ok("registry-values-deleted")
            | Ok("registry-recovery-deleted")
            | Ok("registry-namespace-values-deleted")
            | Ok("registry-namespace-deleted")
    ) {
        std::process::exit(197);
    }
    if unsafe { CommitTransaction(transaction.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if registry_inventory(namespace_id)?.present {
        return Err(io::Error::other("registry namespace remains"));
    }
    Ok(())
}

fn registry_delete_test_pause() -> io::Result<()> {
    test_pause(
        "TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE",
        "registry race",
    )
}

fn root_publication_test_pause() -> io::Result<()> {
    test_pause(
        "TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE",
        "root publication race",
    )
}

fn test_pause(variable: &str, description: &str) -> io::Result<()> {
    let Some(path) = std::env::var_os(variable) else {
        return Ok(());
    };
    let ready = PathBuf::from(format!("{}.ready", path.to_string_lossy()));
    let release = PathBuf::from(format!("{}.continue", path.to_string_lossy()));
    let file = std::fs::File::create(&ready)?;
    file.sync_all()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !release.exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{description} seam timed out"),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

fn open_registry_chain(namespace_id: &str, access: u32) -> io::Result<Option<(Key, Key, Key)>> {
    let Some(software) = open_relative_key_raw(HKEY_CURRENT_USER, "Software", access)? else {
        return Ok(None);
    };
    let Some(test_root) = open_relative_key(&software, "Talking Quill Tests", access)? else {
        return Ok(None);
    };
    let Some(namespace) = open_relative_key(&test_root, namespace_id, access)? else {
        return Ok(None);
    };
    verify_registry_chain(&software, &test_root, &namespace, namespace_id)?;
    Ok(Some((software, test_root, namespace)))
}

fn open_registry_chain_transacted(
    namespace_id: &str,
    access: u32,
    transaction: HANDLE,
) -> io::Result<Option<(Key, Key, Key)>> {
    let Some(software) =
        open_relative_key_transacted(HKEY_CURRENT_USER, "Software", access, transaction)?
    else {
        return Ok(None);
    };
    let Some(test_root) =
        open_relative_key_transacted(software.0, "Talking Quill Tests", access, transaction)?
    else {
        return Ok(None);
    };
    let Some(namespace) =
        open_relative_key_transacted(test_root.0, namespace_id, access, transaction)?
    else {
        return Ok(None);
    };
    verify_registry_chain(&software, &test_root, &namespace, namespace_id)?;
    Ok(Some((software, test_root, namespace)))
}

fn verify_registry_chain(
    software: &Key,
    test_root: &Key,
    namespace: &Key,
    namespace_id: &str,
) -> io::Result<()> {
    verify_key_path(software, &["Software"])?;
    verify_key_path(test_root, &["Software", "Talking Quill Tests"])?;
    verify_key_path(
        namespace,
        &["Software", "Talking Quill Tests", namespace_id],
    )?;
    reject_registry_link(software)?;
    reject_registry_link(test_root)?;
    reject_registry_link(namespace)
}

fn open_relative_key_transacted(
    parent: HKEY,
    name: &str,
    access: u32,
    transaction: HANDLE,
) -> io::Result<Option<Key>> {
    let mut key = null_mut();
    let status = unsafe {
        RegOpenKeyTransactedW(
            parent,
            wide(Path::new(name))?.as_ptr(),
            REG_OPTION_OPEN_LINK,
            access,
            &mut key,
            transaction,
            null(),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(Some(Key(key)))
}

fn inventory_from_handles(
    test_root: &Key,
    namespace: &Key,
    transaction: Option<HANDLE>,
) -> io::Result<RegistryInventory> {
    let namespace_inventory = inventory_key(namespace)?;
    if namespace_inventory
        .subkeys
        .iter()
        .any(|name| name != "RecoveryStateLockV1")
    {
        return Err(io::Error::other("unexpected registry namespace descendant"));
    }
    let recovery = if namespace_inventory
        .subkeys
        .iter()
        .any(|name| name == "RecoveryStateLockV1")
    {
        let key = if let Some(transaction) = transaction {
            open_relative_key_transacted(namespace.0, "RecoveryStateLockV1", KEY_READ, transaction)?
        } else {
            open_relative_key(namespace, "RecoveryStateLockV1", KEY_READ)?
        }
        .ok_or_else(|| io::Error::other("recovery key disappeared"))?;
        verify_relative_key_path(namespace, &key, "RecoveryStateLockV1")?;
        Some(inventory_key(&key)?)
    } else {
        None
    };
    let test = inventory_key(test_root)?;
    if !test.values.is_empty() {
        return Err(io::Error::other("test registry root has values"));
    }
    Ok(RegistryInventory {
        present: true,
        namespace: Some(namespace_inventory),
        recovery,
    })
}

fn inventory_key(key: &Key) -> io::Result<KeyInventory> {
    let inventory = inventory_key_allow_link(key)?;
    if inventory
        .values
        .iter()
        .any(|value| value.value_type == REG_LINK_TYPE)
    {
        return Err(io::Error::other("registry link value is forbidden"));
    }
    Ok(inventory)
}

fn inventory_key_allow_link(key: &Key) -> io::Result<KeyInventory> {
    let mut subkey_count = 0;
    let mut max_subkey = 0;
    let mut value_count = 0;
    let mut max_value_name = 0;
    let mut max_value_data = 0;
    let status = unsafe {
        RegQueryInfoKeyW(
            key.0,
            null_mut(),
            null_mut(),
            null(),
            &mut subkey_count,
            &mut max_subkey,
            null_mut(),
            &mut value_count,
            &mut max_value_name,
            &mut max_value_data,
            null_mut(),
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let mut subkeys = Vec::with_capacity(subkey_count as usize);
    for index in 0..subkey_count {
        let mut name = vec![0_u16; max_subkey as usize + 1];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut length,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        subkeys.push(String::from_utf16(&name[..length as usize]).map_err(io::Error::other)?);
    }
    let mut values = Vec::with_capacity(value_count as usize);
    for index in 0..value_count {
        let mut name = vec![0_u16; max_value_name as usize + 1];
        let mut name_length = name.len() as u32;
        let mut data = vec![0_u8; max_value_data as usize];
        let mut data_length = data.len() as u32;
        let mut value_type = 0;
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut name_length,
                null(),
                &mut value_type,
                data.as_mut_ptr(),
                &mut data_length,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        data.truncate(data_length as usize);
        values.push(RegistryValue {
            name: String::from_utf16(&name[..name_length as usize]).map_err(io::Error::other)?,
            value_type,
            data_hex: data.iter().map(|byte| format!("{byte:02x}")).collect(),
        });
    }
    subkeys.sort();
    values.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(KeyInventory {
        security: key_security(key)?,
        subkeys,
        values,
    })
}

fn key_security(key: &Key) -> io::Result<String> {
    let mut descriptor = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            key.0,
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    let descriptor = Descriptor(descriptor);
    if status != ERROR_SUCCESS || descriptor.0.is_null() {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    descriptor_text(descriptor.0)
}

fn delete_recorded_values(key: &Key, values: &[RegistryValue]) -> io::Result<()> {
    for value in values {
        let current = inventory_key(key)?;
        let Some(actual) = current.values.iter().find(|entry| entry.name == value.name) else {
            return Err(io::Error::other("recorded registry value disappeared"));
        };
        if actual != value {
            return Err(io::Error::other("recorded registry value changed"));
        }
        if unsafe { RegDeleteValueW(key.0, wide(Path::new(&value.name))?.as_ptr()) }
            != ERROR_SUCCESS
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn create_relative_key(parent: &Key, name: &str) -> io::Result<Key> {
    if name.contains('\\') || name.contains('/') {
        return Err(io::Error::other("registry component is not relative"));
    }
    let descriptor = exact_descriptor(false)?;
    let attributes = security_attributes(&descriptor);
    let mut key = null_mut();
    let mut disposition = 0;
    let status = unsafe {
        RegCreateKeyExW(
            parent.0,
            wide(Path::new(name))?.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | DELETE,
            &attributes,
            &mut key,
            &mut disposition,
        )
    };
    if status != ERROR_SUCCESS || disposition != REG_CREATED_NEW_KEY {
        if !key.is_null() {
            unsafe { RegCloseKey(key) };
        }
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(Key(key))
}

fn reject_registry_link(key: &Key) -> io::Result<()> {
    if inventory_key(key)?
        .values
        .iter()
        .any(|value| value.value_type == REG_LINK_TYPE)
    {
        return Err(io::Error::other("registry link is forbidden"));
    }
    Ok(())
}

fn verify_relative_key_path(parent: &Key, child: &Key, name: &str) -> io::Result<()> {
    let expected = format!("{}\\{name}", canonical_key_path(parent)?);
    if !canonical_key_path(child)?.eq_ignore_ascii_case(&expected) {
        return Err(io::Error::other(
            "registry child redirected from retained parent",
        ));
    }
    Ok(())
}

fn canonical_key_path(key: &Key) -> io::Result<String> {
    let mut buffer = vec![0_u8; 4096];
    let mut needed = 0;
    let status = unsafe {
        NtQueryKey(
            key.0,
            KEY_NAME_INFORMATION,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut needed,
        )
    };
    if status < 0 || needed as usize > buffer.len() || needed < 4 {
        return Err(io::Error::other("cannot query canonical registry path"));
    }
    let byte_length = u32::from_le_bytes(buffer[..4].try_into().unwrap()) as usize;
    if !byte_length.is_multiple_of(2) || byte_length + 4 > needed as usize {
        return Err(io::Error::other("canonical registry path is malformed"));
    }
    let mut units = Vec::with_capacity(byte_length / 2);
    for offset in (4..4 + byte_length).step_by(2) {
        units.push(u16::from_le_bytes([buffer[offset], buffer[offset + 1]]));
    }
    String::from_utf16(&units).map_err(io::Error::other)
}

fn verify_key_path(key: &Key, components: &[&str]) -> io::Result<()> {
    let actual = canonical_key_path(key)?;
    let mut expected = format!(r"\REGISTRY\USER\{}", current_user_sid()?);
    for component in components {
        expected.push('\\');
        expected.push_str(component);
    }
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(io::Error::other(
            "registry key redirected from its canonical hive path",
        ));
    }
    Ok(())
}

fn open_relative_key(parent: &Key, name: &str, access: u32) -> io::Result<Option<Key>> {
    open_relative_key_raw(parent.0, name, access)
}

fn open_relative_key_raw(parent: HKEY, name: &str, access: u32) -> io::Result<Option<Key>> {
    let mut key = null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            parent,
            wide(Path::new(name))?.as_ptr(),
            REG_OPTION_OPEN_LINK,
            access,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(Some(Key(key)))
}

pub fn directory_replacement_is_blocked(source: &Path, target: &Path) -> io::Result<bool> {
    let source_parent = source
        .parent()
        .ok_or_else(|| io::Error::other("replacement source has no parent"))?;
    if target.parent() != Some(source_parent) {
        return Err(io::Error::other(
            "replacement source and target do not share a parent",
        ));
    }
    let parent = open_directory_for_replacement(source_parent)?;
    validate_directory(parent.0)?;
    let source_name = source
        .file_name()
        .ok_or_else(|| io::Error::other("replacement source has no name"))?;
    let target_name = target
        .file_name()
        .ok_or_else(|| io::Error::other("replacement target has no name"))?;
    let source = open_relative_directory(parent.0, source_name)?;
    validate_directory(source.0)?;
    match open_relative_directory_for_delete(parent.0, target_name) {
        Ok(target) => {
            validate_directory(target.0)?;
            let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
            if unsafe {
                SetFileInformationByHandle(
                    target.0,
                    FileDispositionInfo,
                    (&raw mut disposition).cast(),
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            } != 0
            {
                return Ok(false);
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error().map(|value| value as u32) {
                Some(ERROR_SHARING_VIOLATION) => Ok(true),
                _ => Err(error),
            }
        }
        Err(error) => match error.raw_os_error().map(|value| value as u32) {
            Some(ERROR_SHARING_VIOLATION) => Ok(true),
            _ => Err(error),
        },
    }
}

pub fn retain_namespace_handles(
    path: &Path,
    expected_root_identity: &str,
    expected_parent_identity: &str,
) -> io::Result<RetainedNamespaceHandles> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("namespace root has no parent"))?;
    let parent = open_directory_guard(parent_path)?;
    validate_directory(parent.0)?;
    if identity(parent.0)? != expected_parent_identity {
        return Err(io::Error::other("namespace parent identity changed"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("namespace root has no name"))?;
    let root = open_relative_directory_for_validation(parent.0, name)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    if identity(root.0)? != expected_root_identity {
        return Err(io::Error::other("namespace root identity changed"));
    }
    Ok(RetainedNamespaceHandles {
        _parent: parent,
        _root: root,
    })
}

fn create_relative_directory(
    parent: HANDLE,
    name: &std::ffi::OsStr,
    security_descriptor: PSECURITY_DESCRIPTOR,
) -> io::Result<Handle> {
    nt_create_relative(
        parent,
        name,
        DELETE
            | FILE_LIST_DIRECTORY
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | SYNCHRONIZE
            | windows_sys::Win32::Foundation::GENERIC_WRITE,
        security_descriptor.cast(),
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_CREATE,
        FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn open_relative_directory(parent: HANDLE, name: &std::ffi::OsStr) -> io::Result<Handle> {
    nt_create_relative(
        parent,
        name,
        DELETE
            | FILE_LIST_DIRECTORY
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | SYNCHRONIZE
            | windows_sys::Win32::Foundation::GENERIC_WRITE,
        null_mut(),
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_OPEN,
        FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn open_relative_directory_for_validation(
    parent: HANDLE,
    name: &std::ffi::OsStr,
) -> io::Result<Handle> {
    nt_create_relative(
        parent,
        name,
        DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        null_mut(),
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_OPEN,
        FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn open_relative_directory_for_delete(
    parent: HANDLE,
    name: &std::ffi::OsStr,
) -> io::Result<Handle> {
    nt_create_relative(
        parent,
        name,
        DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        null_mut(),
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        FILE_OPEN,
        FILE_DIRECTORY_FILE
            | FILE_SYNCHRONOUS_IO_NONALERT
            | FILE_OPEN_FOR_BACKUP_INTENT
            | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn open_relative_ownership_stream(root: HANDLE) -> io::Result<Handle> {
    nt_create_relative(
        root,
        std::ffi::OsStr::new(&format!(":{OWNERSHIP_STREAM}")),
        windows_sys::Win32::Foundation::GENERIC_READ
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | SYNCHRONIZE,
        null_mut(),
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn create_relative_ownership_stream(root: HANDLE) -> io::Result<Handle> {
    let descriptor = exact_descriptor(false)?;
    nt_create_relative(
        root,
        std::ffi::OsStr::new(&format!(":{OWNERSHIP_STREAM}")),
        windows_sys::Win32::Foundation::GENERIC_READ
            | windows_sys::Win32::Foundation::GENERIC_WRITE
            | READ_CONTROL
            | SYNCHRONIZE,
        descriptor.0.cast(),
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT_NT,
    )
}

fn nt_create_relative(
    parent: HANDLE,
    name: &std::ffi::OsStr,
    desired_access: u32,
    security_descriptor: *mut c_void,
    share_access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<Handle> {
    let mut name_wide = name.encode_wide().collect::<Vec<_>>();
    if name_wide.is_empty()
        || name_wide
            .iter()
            .any(|unit| *unit == b'\\' as u16 || *unit == b'/' as u16)
        || name_wide.len() > u16::MAX as usize / 2
    {
        return Err(io::Error::other("invalid handle-relative name"));
    }
    let mut name = UnicodeString {
        length: (name_wide.len() * size_of::<u16>()) as u16,
        maximum_length: (name_wide.len() * size_of::<u16>()) as u16,
        buffer: name_wide.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: parent,
        object_name: &mut name,
        attributes: OBJ_CASE_INSENSITIVE,
        security_descriptor,
        security_quality_of_service: null_mut(),
    };
    let mut io_status = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    let mut handle = null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &mut attributes,
            &mut io_status,
            null_mut(),
            FILE_ATTRIBUTE_NORMAL,
            share_access,
            disposition,
            options,
            null_mut(),
            0,
        )
    };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(Handle(handle))
}

fn open_directory_low_guard(path: &Path) -> io::Result<Handle> {
    let handle = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            SYNCHRONIZE | windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if handle.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn open_directory_guard(path: &Path) -> io::Result<Handle> {
    let handle = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if handle.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn open_directory_for_replacement(path: &Path) -> io::Result<Handle> {
    let handle = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if handle.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn open_creation_parent(path: &Path) -> io::Result<Handle> {
    let handle = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            FILE_LIST_DIRECTORY
                | FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if handle.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn open_directory(path: &Path) -> io::Result<Handle> {
    let handle = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            DELETE
                | FILE_LIST_DIRECTORY
                | FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if handle.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

fn validate_directory(handle: HANDLE) -> io::Result<()> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes
        & (windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
            | FILE_ATTRIBUTE_REPARSE_POINT)
        != windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY
        || info.nNumberOfLinks != 1
    {
        return Err(io::Error::other("protected root identity is invalid"));
    }
    Ok(())
}

fn file_id_128(handle: HANDLE) -> io::Result<String> {
    let mut info: FILE_ID_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(info
        .FileId
        .Identifier
        .iter()
        .rev()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn identity(handle: HANDLE) -> io::Result<String> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        "{}:{}",
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
    ))
}

fn write_and_flush_handle(handle: HANDLE, bytes: &[u8], label: &str) -> io::Result<()> {
    let split = bytes.len().div_ceil(2);
    write_all_handle(handle, &bytes[..split])?;
    crash_at(&format!("{label}-partial-write"));
    write_all_handle(handle, &bytes[split..])?;
    crash_at(&format!("{label}-written-before-flush"));
    if unsafe { FlushFileBuffers(handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    crash_at(&format!("{label}-file-flushed"));
    Ok(())
}

fn write_new_protected_file(path: &Path, bytes: &[u8], label: &str) -> io::Result<()> {
    let descriptor = exact_descriptor(false)?;
    let attributes = security_attributes(&descriptor);
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | windows_sys::Win32::Foundation::GENERIC_WRITE
                | DELETE
                | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    write_and_flush_handle(file.0, bytes, label)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("protected file has no parent"))?;
    flush_owned_directory(parent).map_err(io::Error::other)?;
    crash_at(&format!("{label}-parent-flushed"));
    Ok(())
}

fn write_all_handle(handle: HANDLE, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut written = 0;
    if unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut written,
            null_mut(),
        )
    } == 0
        || written as usize != bytes.len()
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn intent_path(binding_path: &Path) -> PathBuf {
    let mut value = binding_path.as_os_str().to_owned();
    value.push(".intent-v1");
    PathBuf::from(value)
}

fn expected_intent(ownership_prefix: &str, binding_nonce: &str) -> CreationIntent {
    CreationIntent {
        version: 1,
        nonce: binding_nonce.to_owned(),
        ownership_prefix: ownership_prefix.to_owned(),
    }
}

fn read_intent(path: &Path) -> io::Result<CreationIntent> {
    let file = open_protected_file(path)?;
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    serde_json::from_slice(&read_handle(file.0)?).map_err(io::Error::other)
}

fn read_binding(path: &Path) -> io::Result<CleanupBinding> {
    let file = open_protected_file(path)?;
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    let bytes = read_handle(file.0)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn delete_intent(path: &Path, expected: &CreationIntent) -> io::Result<()> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("creation intent has no parent"))?;
    let parent = open_directory(parent_path)?;
    let file = open_protected_file(path)?;
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    if serde_json::from_slice::<CreationIntent>(&read_handle(file.0)?).map_err(io::Error::other)?
        != *expected
    {
        return Err(io::Error::other("external creation intent changed"));
    }
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.0,
            FileDispositionInfo,
            (&raw mut disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    drop(file);
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn delete_binding(path: &Path, expected: &CleanupBinding) -> io::Result<()> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("cleanup binding has no parent"))?;
    let parent = open_directory(parent_path)?;
    let file = open_protected_file(path)?;
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    if serde_json::from_slice::<CleanupBinding>(&read_handle(file.0)?).map_err(io::Error::other)?
        != *expected
    {
        return Err(io::Error::other("cleanup binding changed"));
    }
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.0,
            FileDispositionInfo,
            (&raw mut disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    drop(file);
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn delete_exact_protected_file(path: &Path) -> io::Result<()> {
    let parent_path = path
        .parent()
        .ok_or_else(|| io::Error::other("protected file has no parent"))?;
    let parent = open_directory(parent_path)?;
    let file = open_protected_file(path)?;
    validate_exact_security(file.0, false)?;
    validate_regular_file(file.0)?;
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.0,
            FileDispositionInfo,
            (&raw mut disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    drop(file);
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn validate_regular_file(handle: HANDLE) -> io::Result<()> {
    let mut information = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | 0x10) != 0
        || information.nNumberOfLinks != 1
    {
        return Err(io::Error::other(
            "protected file is not exact non-reparse single-link data",
        ));
    }
    Ok(())
}

fn open_protected_file(path: &Path) -> io::Result<Handle> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ | DELETE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

fn read_handle(handle: HANDLE) -> io::Result<Vec<u8>> {
    const MAX_BYTES: usize = 128 * 1024 * 1024;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let mut read = 0;
        if unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Ok(bytes);
        }
        if bytes.len() + read as usize > MAX_BYTES {
            return Err(io::Error::other(
                "protected file exceeds the native read limit",
            ));
        }
        bytes.extend_from_slice(&buffer[..read as usize]);
    }
}

fn handle_delete_empty_root(root: Handle, parent: Handle) -> io::Result<()> {
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            root.0,
            FileDispositionInfo,
            (&raw mut disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    drop(root);
    if unsafe { FlushFileBuffers(parent.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn crash_at(phase: &str) {
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok(phase) {
        std::process::exit(197);
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_stream(path: &Path) -> io::Result<String> {
    let file = Handle(unsafe {
        CreateFileW(
            wide(path)?.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = vec![0_u8; 4096];
    let mut read = 0;
    if unsafe {
        ReadFile(
            file.0,
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut read,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    bytes.truncate(read as usize);
    String::from_utf8(bytes).map_err(io::Error::other)
}

fn ownership_stream(path: &Path) -> io::Result<PathBuf> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::other("non-Unicode protected root path"))?;
    Ok(PathBuf::from(format!("{text}:{OWNERSHIP_STREAM}")))
}

fn exact_descriptor(directory: bool) -> io::Result<Descriptor> {
    let user = current_user_sid()?;
    let inherit = if directory { "OICI" } else { "" };
    let sddl = format!(
        "O:{user}G:{user}D:P(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)(A;{inherit};FA;;;{user})"
    );
    descriptor_from_sddl(&sddl)
}

fn validate_exact_security(handle: HANDLE, directory: bool) -> io::Result<()> {
    let expected = exact_descriptor(directory)?;
    let expected_text = descriptor_text(expected.0)?;
    let mut actual = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut actual,
        )
    };
    let actual = Descriptor(actual);
    let actual_text = if actual.0.is_null() {
        String::new()
    } else {
        descriptor_text(actual.0)?
    };
    if status != ERROR_SUCCESS
        || normalized_descriptor(&actual_text) != normalized_descriptor(&expected_text)
    {
        return Err(io::Error::other(format!(
            "protected root security is not exact: expected {expected_text}, actual {actual_text}"
        )));
    }
    Ok(())
}

fn normalized_descriptor(value: &str) -> Option<(String, Vec<String>)> {
    let dacl = value.find("D:P")?;
    let prefix = value[..dacl + 3].to_owned();
    let mut aces = Vec::new();
    let mut remaining = &value[dacl + 3..];
    while !remaining.is_empty() {
        if !remaining.starts_with('(') {
            return None;
        }
        let end = remaining.find(')')? + 1;
        aces.push(remaining[..end].to_owned());
        remaining = &remaining[end..];
    }
    aces.sort();
    Some((prefix, aces))
}

fn descriptor_from_sddl(sddl: &str) -> io::Result<Descriptor> {
    let mut descriptor = null_mut();
    let wide = wide(Path::new(sddl))?;
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
        return Err(io::Error::last_os_error());
    }
    Ok(Descriptor(descriptor))
}

fn descriptor_text(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<String> {
    let mut text = null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            null_mut(),
        )
    } == 0
        || text.is_null()
    {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(result.to_ascii_uppercase())
}

fn current_user_sid() -> io::Result<String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut needed = 0;
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0_u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    sid_text(user.User.Sid)
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(result)
}

fn security_attributes(descriptor: &Descriptor) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

fn validate_prefix(value: &str) -> io::Result<()> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(io::Error::other("invalid ownership prefix"));
    }
    Ok(())
}

fn validate_nonce(value: &str) -> io::Result<()> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid cleanup binding nonce"));
    }
    Ok(())
}

fn validate_namespace_id(value: &str) -> io::Result<()> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid namespace id"));
    }
    Ok(())
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if value.contains(&0) {
        return Err(io::Error::other("embedded NUL"));
    }
    value.push(0);
    Ok(value)
}
