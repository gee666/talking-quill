#![cfg(windows)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::owned_tree::{owned_tree_identity, remove_owned_tree};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SetFileSecurityW, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers,
    GetFileInformationByHandle, MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_MULTI_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW,
    RegEnumValueW, RegFlushKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateMutexW, CreateProcessW, GetCurrentProcess, GetCurrentProcessId,
    GetExitCodeProcess, OpenProcess, OpenProcessToken, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex, ResumeThread,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};

#[used]
static WINDOWS_UPDATE_PRIMARY_KEY_MARKER: &str = concat!(
    "TALKING_QUILL_WINDOWS_UPDATE_PRIMARY_KEY_V1=",
    env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1")
);
#[used]
static RELEASE_MANIFEST_KEY_MARKER: &str = concat!(
    "TALKING_QUILL_RELEASE_MANIFEST_KEY_V1=",
    env!("TALKING_QUILL_RELEASE_MANIFEST_PUBLIC_KEY_SEC1")
);
const RESTRICTED_STAGING_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const RESTRICTED_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
const MEDIUM_LAUNCHER_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
const MEDIUM_LAUNCHER_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;AU)";
// Authenticated users may only read and append. They can exhaust retries (safe denial), but
// cannot erase attempts or obtain more than the machine-owned bound.
const RETRY_COUNTER_SDDL: &str =
    "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x120089;;;AU)(A;;0x00000004;;;AU)";
const RECOVERY_LAUNCHER_NAME: &str = "talking-quill-update-recovery-launcher.exe";
const RECOVERY_LAUNCHER_PUBLISHED_PREFIX: &str = "talking-quill-update-recovery-launcher-";
const RECOVERY_LAUNCHER_IDENTITY_NAME: &str = "launcher-tree-identity-v1";
const RECOVERY_LAUNCHER_PENDING_PREFIX: &str = ".Talking Quill.update-launcher-pending-";
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
const MACHINE_LOCK_DIRECTORY_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const MACHINE_LOCK_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;AU)";
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;AU)";
const MACHINE_LOCK_RETIRED_PREFIX: &str = "retired:";
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
const MACHINE_LOCK_REGISTRY_KEY: &str = r"Software\Talking Quill\RecoveryStateLockV1";
const MACHINE_LOCK_REGISTRY_VALUE: &str = "DirectorySuffix";
const MACHINE_LOCK_DIRECTORY_PREFIX: &str = ".Talking Quill.machine-lock-";
const MACHINE_LOCK_PENDING_PREFIX: &str = ".Talking Quill.machine-lock-pending-";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const MACHINE_LOCK_TEST_ID_ENV: &str = "TQ_MACHINE_LOCK_TEST_NAMESPACE_ID";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_test_id() -> Result<&'static str, i32> {
    static ID: OnceLock<String> = OnceLock::new();
    let value = ID.get_or_init(|| {
        std::env::var(MACHINE_LOCK_TEST_ID_ENV)
            .expect("machine-lock tests require the wrapper namespace environment")
    });
    validate_generation(value)?;
    Ok(value)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_registry_hive() -> HKEY {
    HKEY_CURRENT_USER
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_registry_hive() -> HKEY {
    HKEY_LOCAL_MACHINE
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_registry_key() -> Result<String, i32> {
    Ok(format!(
        r"Software\Talking Quill Tests\{}\RecoveryStateLockV1",
        machine_lock_test_id()?
    ))
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_registry_key() -> Result<String, i32> {
    Ok(MACHINE_LOCK_REGISTRY_KEY.to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_program_data() -> Result<PathBuf, i32> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tmp/machine-lock-tests/helper")
        .join(machine_lock_test_id()?);
    if !root.is_dir() {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(root)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_program_data() -> Result<PathBuf, i32> {
    known_folder(&FOLDERID_ProgramData)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_mutex_names() -> Result<[String; 2], i32> {
    let id = machine_lock_test_id()?;
    Ok([
        format!(r"Local\TalkingQuill.Tests.{id}.NativeSetup.V2"),
        format!(r"Local\TalkingQuill.Tests.{id}.UpdateRecovery.State.V1"),
    ])
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_mutex_names() -> Result<[String; 2], i32> {
    Ok([
        r"Global\TalkingQuill.NativeSetup.V2".to_owned(),
        r"Global\TalkingQuill.UpdateRecovery.State.V1".to_owned(),
    ])
}

const LEGACY_LOCK_RETIREMENT_EPOCH: u8 = 3;
#[used]
static WINDOWS_RECOVERY_POLICY_EPOCH_MARKER: &str = "TALKING_QUILL_WINDOWS_RECOVERY_POLICY_EPOCH=2";

const EXIT_INVALID_REQUEST: i32 = 64;
const EXIT_NOT_ELEVATED: i32 = 77;
const EXIT_IDENTITY_MISMATCH: i32 = 78;
const EXIT_LAUNCH_FAILED: i32 = 79;
const EXIT_INSTALLER_STILL_RUNNING: i32 = 80;
const INSTALLER_SUPERVISION_TIMEOUT_MS: u32 = 15 * 60 * 1_000;
const PUBLIC_UPDATE_TRUST_ROOT: &str = "0.0.69";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateRequest {
    version: u8,
    installer_path: String,
    sha256: String,
    candidate: UpdateCandidate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RelaunchWrapper {
    request: String,
    intent_path: String,
    nonce: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RelaunchIntent {
    schema_version: u8,
    nonce: String,
    source_version: String,
    target_version: String,
    phase: String,
    completed_version: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AppReadyRequest {
    generation: String,
    version: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PersistedRelaunchRecord {
    schema_version: u8,
    generation: String,
    user_sid: String,
    logon_sid: String,
    request: String,
    nonce: String,
    source_version: String,
    target_version: String,
    phase: String,
    completed_version: Option<String>,
    recovery_generation: String,
    predecessor: InstalledManifest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TerminalUninstallRecord {
    schema_version: u8,
    generation: String,
    phase: String,
    maintenance_sha256: String,
    uninstall_command: String,
    quiet_uninstall_command: String,
    service_name: String,
    service_image: String,
    service_sha256: String,
    service_file_identity: String,
    record_file_identity: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetupTransaction {
    schema_version: u8,
    phase: String,
    action: String,
    had_predecessor: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeferredLaunchRequest {
    parent_pid: u32,
    generation: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateCandidate {
    version: String,
    platform: String,
    architecture: String,
    owner_mode: String,
    package_mode: String,
    source_commit: String,
    source_tree: String,
    release_build_digest: String,
    package_layout_digest: String,
    package_sha256: String,
    channel: String,
    transaction_binding: String,
    roles: Vec<UpdateRole>,
    predecessor: UpdatePredecessor,
    authorization: UpdateAuthorization,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateAuthorization {
    scheme: String,
    signature: String,
    #[serde(default)]
    verification_key_sha256: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateRole {
    role: String,
    path: String,
    sha256: String,
    suppression_capable: bool,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdatePredecessor {
    platform: String,
    architecture: String,
    version: String,
    release_build_digest: String,
    gateway_sha256: String,
    owner_sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledManifest {
    version: String,
    platform: String,
    architecture: String,
    source_commit: String,
    source_tree: String,
    release_build_digest: String,
    roles: Vec<UpdateRole>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    volume: u32,
    index_high: u32,
    index_low: u32,
}

struct StagedDirectoryGuard {
    path: PathBuf,
    identity: String,
    launched: bool,
    cleanup_generation: Option<String>,
}

impl StagedDirectoryGuard {
    fn new(path: PathBuf) -> Result<Self, i32> {
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        Ok(Self {
            path,
            identity,
            launched: false,
            cleanup_generation: None,
        })
    }

    fn persist_identity_marker(&self) -> Result<(), i32> {
        let path = self.path.join("cleanup-tree-identity-v1");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&path, RESTRICTED_FILE_SDDL)?;
        file.write_all(self.identity.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)
    }

    fn publish(&mut self, published: PathBuf) -> Result<(), i32> {
        if published.exists()
            || unsafe {
                MoveFileExW(
                    wide_nul(&self.path)?.as_ptr(),
                    wide_nul(&published)?.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        self.path = published;
        if owned_tree_identity(&self.path).map_err(|_| EXIT_IDENTITY_MISMATCH)? != self.identity {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        Ok(())
    }

    fn register_prelaunch_cleanup(
        &mut self,
        installed_helper: &Path,
        bound_generation: Option<&str>,
    ) -> Result<(), i32> {
        let generation = bound_generation
            .map(str::to_owned)
            .map_or_else(new_recovery_generation, Ok)?;
        validate_generation(&generation)?;
        self.cleanup_generation = Some(generation.clone());
        persist_prelaunch_cleanup(installed_helper, &self.path, &self.identity, &generation)
    }

    fn transfer_to_launched_recovery(&mut self) {
        self.launched = true;
    }
}

impl Drop for StagedDirectoryGuard {
    fn drop(&mut self) {
        if !self.launched {
            let Ok(_state) = RecoveryStateLock::acquire() else {
                return;
            };
            let removed = remove_owned_tree(&self.path, &self.identity).is_ok();
            if removed && let Some(generation) = &self.cleanup_generation {
                let _ = clear_restart_recovery(generation);
            }
        }
    }
}

pub fn run_from_argument(argument: &std::ffi::OsStr) -> i32 {
    match run_from_argument_inner(argument) {
        Ok(code) => code as i32,
        Err(code) => code,
    }
}

pub fn run_recovery_launcher_argument(argument: &std::ffi::OsStr) -> i32 {
    match run_recovery_launcher_argument_inner(argument) {
        Ok(code) => code as i32,
        Err(code) => code,
    }
}

mod relaunch;
use relaunch::*;

mod relaunch_record;
use relaunch_record::*;

mod request;
use request::*;

mod terminal;
use terminal::*;

mod launch;
use launch::*;

mod staging;
use staging::*;

mod authorization;
use authorization::*;

mod security;
use security::*;

mod launcher;
use launcher::*;

mod machine_lock;
use machine_lock::*;

mod persistence;
use persistence::*;

mod installer_process;
use installer_process::*;

mod filesystem;
use filesystem::*;

#[cfg(test)]
mod tests;
