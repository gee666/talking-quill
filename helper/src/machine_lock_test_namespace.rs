#![cfg(all(windows, feature = "machine-lock-test-namespace"))]

use crate::owned_tree::flush_owned_directory;
use serde::{Deserialize, Serialize};
use std::{
    io,
    mem::zeroed,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_FILE_OBJECT, SE_REGISTRY_KEY,
        },
        DACL_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CommitTransaction, CreateDirectoryW, CreateFileW,
        CreateTransaction, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileDispositionInfo, FlushFileBuffers, GetFileInformationByHandle,
        OPEN_EXISTING, READ_CONTROL, ReadFile, SetFileInformationByHandle, WriteFile,
    },
    System::{
        Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_CREATED_NEW_KEY,
            REG_OPTION_NON_VOLATILE, REG_OPTION_OPEN_LINK, RegCloseKey, RegCreateKeyExW,
            RegDeleteKeyExW, RegDeleteKeyTransactedW, RegDeleteValueW, RegEnumKeyExW,
            RegEnumValueW, RegFlushKey, RegOpenKeyExW, RegOpenKeyTransactedW, RegQueryInfoKeyW,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const OWNERSHIP_STREAM: &str = "TalkingQuill.TestOwnership.V1";
const ERROR_SUCCESS: u32 = 0;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
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

pub fn delete_empty_registry_root() -> io::Result<()> {
    let Some(software) =
        open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ | KEY_WRITE)?
    else {
        return Ok(());
    };
    let Some(test_root) =
        open_relative_key(&software, "Talking Quill Tests", KEY_READ | KEY_WRITE)?
    else {
        return Ok(());
    };
    let inventory = inventory_key(&test_root)?;
    if !inventory.subkeys.is_empty() || !inventory.values.is_empty() {
        return Err(io::Error::other("test registry root is not empty"));
    }
    if unsafe {
        RegDeleteKeyExW(
            software.0,
            wide(Path::new("Talking Quill Tests"))?.as_ptr(),
            0,
            0,
        )
    } != ERROR_SUCCESS
    {
        return Err(io::Error::last_os_error());
    }
    if unsafe { RegFlushKey(software.0) } != ERROR_SUCCESS {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn registry_root_inventory() -> io::Result<Option<KeyInventory>> {
    let Some(software) = open_relative_key_raw(HKEY_CURRENT_USER, "Software", KEY_READ)? else {
        return Ok(None);
    };
    let Some(test_root) = open_relative_key(&software, "Talking Quill Tests", KEY_READ)? else {
        return Ok(None);
    };
    Ok(Some(inventory_key(&test_root)?))
}

pub fn create_protected_root(path: &Path, ownership_prefix: &str) -> io::Result<String> {
    validate_prefix(ownership_prefix)?;
    let directory_descriptor = exact_descriptor(true)?;
    let attributes = security_attributes(&directory_descriptor);
    let path_wide = wide(path)?;
    if unsafe { CreateDirectoryW(path_wide.as_ptr(), &attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let root = open_directory(path)?;
    validate_directory(root.0)?;
    validate_exact_security(root.0, true)?;
    if std::env::var_os("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref()
        == Some(std::ffi::OsStr::new("create-before-record"))
    {
        std::process::exit(197);
    }
    let identity = identity(root.0)?;
    drop(root);
    let record = format!("{ownership_prefix}:{identity}");
    let file_descriptor = exact_descriptor(false)?;
    let file_attributes = security_attributes(&file_descriptor);
    let stream = ownership_stream(path)?;
    let stream_wide = wide(&stream)?;
    let file = Handle(unsafe {
        CreateFileW(
            stream_wide.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_READ
                | windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &file_attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    });
    if file.0 == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let bytes = record.as_bytes();
    let mut written = 0;
    if unsafe {
        WriteFile(
            file.0,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut written,
            null_mut(),
        )
    } == 0
        || written as usize != bytes.len()
        || unsafe { FlushFileBuffers(file.0) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    flush_owned_directory(path).map_err(io::Error::other)?;
    Ok(identity)
}

pub fn remove_interrupted_root(path: &Path, ownership_prefix: &str) -> io::Result<()> {
    validate_prefix(ownership_prefix)?;
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
    let stream = ownership_stream(path)?;
    match read_stream(&stream) {
        Ok(value) if value == format!("{ownership_prefix}:{identity}") => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(value) => {
            return Err(io::Error::other(format!(
                "interrupted ownership record is invalid: {value}"
            )));
        }
        Err(error) => return Err(error),
    }
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

pub fn create_registry_namespace(namespace_id: &str) -> io::Result<()> {
    validate_namespace_id(namespace_id)?;
    let descriptor = exact_descriptor(false)?;
    let attributes = security_attributes(&descriptor);
    let path = wide(Path::new(&format!(
        "Software\\Talking Quill Tests\\{namespace_id}"
    )))?;
    let mut key = null_mut();
    let mut disposition = 0;
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            &attributes,
            &mut key,
            &mut disposition,
        )
    };
    let key = Key(key);
    if status != ERROR_SUCCESS || disposition != REG_CREATED_NEW_KEY {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    if unsafe { RegFlushKey(key.0) } != ERROR_SUCCESS {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
    if unsafe { CommitTransaction(transaction.0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if registry_inventory(namespace_id)?.present {
        return Err(io::Error::other("registry namespace remains"));
    }
    Ok(())
}

fn registry_delete_test_pause() -> io::Result<()> {
    let Some(path) = std::env::var_os("TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE") else {
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
                "registry race seam timed out",
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
    Ok(Some((software, test_root, namespace)))
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
