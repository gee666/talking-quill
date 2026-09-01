#![cfg(windows)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
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
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_MULTI_SZ,
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
const MACHINE_LOCK_DIRECTORY_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
const MACHINE_LOCK_RETIRED_PREFIX: &str = "retired:";
const MACHINE_LOCK_REGISTRY_KEY: &str = r"Software\Talking Quill\RecoveryStateLockV1";
const MACHINE_LOCK_REGISTRY_VALUE: &str = "DirectorySuffix";
const MACHINE_LOCK_DIRECTORY_PREFIX: &str = ".Talking Quill.machine-lock-";
const MACHINE_LOCK_PENDING_PREFIX: &str = ".Talking Quill.machine-lock-pending-";
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

fn run_recovery_launcher_argument_inner(argument: &std::ffi::OsStr) -> Result<u32, i32> {
    let argument = argument.to_str().ok_or(EXIT_INVALID_REQUEST)?;
    if argument.starts_with("--windows-update-bootstrap-v2=") {
        authorize_public_update_bootstrap(argument)?;
    }
    if let Some(generation) = argument.strip_prefix("--windows-update-relaunch-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        validate_generation(generation)?;
        return retire_stale_legacy_relaunch(generation);
    }
    if argument == "--windows-update-relaunch-owner-v1" {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return run_machine_relaunch_owner();
    }
    if argument == "--windows-update-relaunch-owner-retire-v1" {
        if !is_elevated() {
            return Err(EXIT_NOT_ELEVATED);
        }
        return retire_no_work_machine_relaunch_owner();
    }
    let generation = if let Some(generation) = argument.strip_prefix("--windows-update-resume-v2=")
    {
        validate_generation(generation)?;
        generation
    } else if let Some(binding) = argument.strip_prefix("--windows-update-cleanup-v1=") {
        let (_, generation) = binding.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
        validate_generation(generation)?;
        generation
    } else {
        return Err(EXIT_INVALID_REQUEST);
    };
    let cleanup_binding = argument.strip_prefix("--windows-update-cleanup-v1=");
    if !is_elevated() {
        let directory = medium_recovery_directory(generation)?;
        if cleanup_binding.is_some() && !directory.exists() {
            if !recovery_command_present(generation)? {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
        } else {
            let attempt = begin_protected_visible_retry(&directory, generation)?;
            if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
                show_visible_retry_paused();
                return Err(EXIT_LAUNCH_FAILED);
            }
        }
        launch_elevated_bootstrap(argument)?;
        if cleanup_binding.is_none() {
            launch_program_files_application_for_generation(generation)?;
        }
        return Ok(0);
    }
    if let Some(binding) = cleanup_binding {
        let _state = RecoveryStateLock::acquire()?;
        let directory = medium_recovery_directory(generation)?;
        if directory.exists() {
            let directory = find_recovery_directory(generation)?;
            let attempt =
                protected_visible_attempt(&directory, generation).ok_or(EXIT_IDENTITY_MISMATCH)?;
            if attempt == 0 {
                begin_protected_visible_retry(&directory, generation)?;
            } else if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
        } else if !recovery_command_present(generation)? {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let generation = run_native_cleanup(binding)?;
        clear_restart_recovery(&generation)?;
        return Ok(0);
    }
    let state = RecoveryStateLock::acquire()?;
    let directory = find_recovery_directory(generation)?;
    let attempt =
        protected_visible_attempt(&directory, generation).ok_or(EXIT_IDENTITY_MISMATCH)?;
    if attempt == 0 {
        begin_protected_visible_retry(&directory, generation)?;
    } else if attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(state);
    let staged = directory.join("talking-quill-update-bootstrap.exe");
    let _retained = open_locked(&staged).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let status = std::process::Command::new(&staged)
        .arg(argument)
        .status()
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let code = status.code().ok_or(EXIT_LAUNCH_FAILED)?;
    if code == 0 { Ok(0) } else { Err(code) }
}

fn run_from_argument_inner(argument: &std::ffi::OsStr) -> Result<u32, i32> {
    let argument = argument.to_str().ok_or(EXIT_INVALID_REQUEST)?;
    if argument.starts_with("--windows-update-bootstrap-v2=") {
        authorize_public_update_bootstrap(argument)?;
    }
    if let Some(generation) = argument.strip_prefix("--windows-update-relaunch-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        validate_generation(generation)?;
        return retire_stale_legacy_relaunch(generation);
    }
    if argument == "--windows-update-relaunch-owner-install-v1" {
        if !is_elevated() {
            return Err(EXIT_NOT_ELEVATED);
        }
        return install_machine_relaunch_owner().map(|()| 0);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-launch-after-parent-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return launch_after_parent_exit(encoded);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-app-ready-v1=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return acknowledge_app_ready(encoded);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-bootstrap-v3=") {
        if is_elevated() {
            return Err(EXIT_INVALID_REQUEST);
        }
        return run_relaunch_wrapper(encoded);
    }
    if (argument.starts_with("--windows-update-bootstrap-v2=")
        || argument.starts_with("--windows-update-resume-v2=")
        || argument.starts_with("--windows-update-cleanup-v1="))
        && !is_elevated()
    {
        return launch_elevated_bootstrap(argument).map(|()| 0);
    }
    if !is_elevated() {
        return Err(EXIT_NOT_ELEVATED);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-cleanup-v1=") {
        let _state = RecoveryStateLock::acquire()?;
        let generation = run_native_cleanup(encoded)?;
        clear_restart_recovery(&generation)?;
        return Ok(0);
    }
    if argument.starts_with("--windows-update-bootstrap-v2=")
        || argument.starts_with("--windows-update-bootstrap-bound-v1=")
    {
        return stage_bootstrap(argument).map(|()| 0);
    }
    let (encoded, previous_generation, resuming) =
        if let Some(generation) = argument.strip_prefix("--windows-update-resume-v2=") {
            validate_generation(generation)?;
            let directory = recovery_directory()?;
            if read_active_generation(&directory)? != generation {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            if protected_visible_attempt(&directory, generation)
                .is_none_or(|attempt| attempt == 0 || attempt > MAX_VISIBLE_RECOVERY_ATTEMPTS)
            {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            (read_persisted_request()?, Some(generation.to_owned()), true)
        } else {
            let staged = argument
                .strip_prefix("--windows-update-bootstrap-staged-v2=")
                .ok_or(EXIT_INVALID_REQUEST)?;
            let (generation, encoded) = staged.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
            validate_generation(generation)?;
            (encoded.to_owned(), Some(generation.to_owned()), false)
        };
    let execution = execute_staged_request(&encoded, previous_generation.as_deref(), resuming);
    let result = execution
        .as_ref()
        .map(|(code, _)| *code)
        .map_err(|code| *code);
    // Only a truthful successful setup exit proves that protected retry ownership can move
    // to cleanup. UAC, launch, and installer failures retain the same generation and counter.
    if result == Ok(0) {
        let generation = execution
            .as_ref()
            .map(|(_, generation)| generation.as_str())
            .map_err(|code| *code)?;
        schedule_staged_cleanup(Some(generation))?;
    } else if resuming
        && previous_generation.as_deref().is_some_and(|generation| {
            recovery_directory().ok().is_some_and(|directory| {
                protected_visible_attempt(&directory, generation)
                    == Some(MAX_VISIBLE_RECOVERY_ATTEMPTS)
            })
        })
    {
        show_visible_retry_paused();
    }
    result
}

const RELAUNCH_RUN_VALUE: &str = "Talking Quill Update Relaunch";
const RELAUNCH_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RELAUNCH_ROOT_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;;0x00000025;;;AU)";
const RELAUNCH_RECORD_NAME: &str = "relaunch-record-v1.json";
const RELAUNCH_MARKER_NAME: &str = "relaunch-record-marker-v1";
const TERMINAL_UNINSTALL_RECORD_NAME: &str = "terminal-uninstall-record-v1.json";

fn relaunch_record_sddl(identity: &RelaunchIdentity) -> String {
    if identity.user_sid == identity.logon_sid {
        format!(
            "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})",
            identity.user_sid
        )
    } else {
        format!(
            "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})(A;OICI;FA;;;{})",
            identity.user_sid, identity.logon_sid
        )
    }
}

fn run_relaunch_wrapper(encoded: &str) -> Result<u32, i32> {
    if encoded.len() > 16_384 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let wrapper: RelaunchWrapper =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    let suffix = wrapper
        .request
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    if wrapper.request.len() > 12_288 || !valid_nonce(&wrapper.nonce) {
        return Err(EXIT_INVALID_REQUEST);
    }
    let request = parse_and_authorize_request(suffix)?;
    // Install under the shared lifecycle lock, then reacquire in the global order before
    // taking any user intent or generation lock.
    launch_elevated_installed_helper("--windows-update-relaunch-owner-install-v1")?;
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    let intent_path = PathBuf::from(&wrapper.intent_path);
    let _intent_lock = acquire_relaunch_intent_lock(&intent_path)?;
    let intent = read_relaunch_intent(&intent_path, &wrapper.nonce)?;
    if intent.source_version != request.candidate.predecessor.version
        || intent.target_version != request.candidate.version
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = current_relaunch_identity()?;
    let predecessor = installed_manifest()?;
    if predecessor.version != request.candidate.predecessor.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let generation = new_recovery_generation()?;
    let record = PersistedRelaunchRecord {
        schema_version: 3,
        generation: generation.clone(),
        user_sid: identity.user_sid,
        logon_sid: identity.logon_sid,
        request: wrapper.request,
        nonce: wrapper.nonce,
        source_version: intent.source_version,
        target_version: intent.target_version,
        phase: "armed".into(),
        completed_version: None,
        recovery_generation: generation.clone(),
        predecessor,
    };
    publish_relaunch_record(&record)?;
    std::fs::remove_file(&intent_path).map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(_intent_lock);
    drop(machine_lifecycle);
    let result = run_persisted_relaunch(&generation);
    let _ =
        std::fs::remove_file(intent_path.with_file_name("windows-update-relaunch-intent-v1.lock"));
    result
}

fn retire_stale_legacy_relaunch(generation: &str) -> Result<u32, i32> {
    let legacy_root =
        known_folder(&FOLDERID_LocalAppData)?.join("Talking Quill/Windows Update Recovery");
    let legacy_directory = legacy_root.join(generation);
    clear_legacy_relaunch_owner(generation)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&legacy_directory)
        && metadata.is_dir()
        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && stale_local_relaunch_record_is_exact(&legacy_directory, generation)
    {
        let _ = remove_relaunch_record_directory(&legacy_directory);
        let _ = std::fs::remove_dir(&legacy_root);
    }
    Ok(0)
}

fn stale_local_relaunch_record_is_exact(directory: &Path, generation: &str) -> bool {
    let Ok(bytes) = std::fs::read(directory.join(RELAUNCH_RECORD_NAME)) else {
        return false;
    };
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    matches!(
        value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64),
        Some(1 | 2)
    ) && value.get("generation").and_then(serde_json::Value::as_str) == Some(generation)
        && value
            .get("request")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|request| request.starts_with("--windows-update-bootstrap-v2="))
}

fn clear_legacy_relaunch_owner(generation: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened != 0 {
        return Ok(());
    }
    let name = format!("Talking Quill Update Relaunch {generation}");
    let Ok(launcher) = medium_launcher_path() else {
        unsafe { RegCloseKey(key) };
        return Ok(());
    };
    let expected = format!(
        "\"{}\" --windows-update-relaunch-v1={generation}",
        launcher.display()
    );
    let Ok(actual) = read_registry_string(key, &name) else {
        unsafe { RegCloseKey(key) };
        return Ok(());
    };
    if actual.as_deref().is_some_and(|value| value != expected) {
        unsafe { RegCloseKey(key) };
        return Ok(());
    }
    if actual.is_some() {
        let _ = unsafe { RegDeleteValueW(key, wide_nul(Path::new(&name))?.as_ptr()) };
        let _ = unsafe { RegFlushKey(key) };
    }
    unsafe { RegCloseKey(key) };
    Ok(())
}

fn maintenance_path_from_command(command: &str) -> Result<PathBuf, i32> {
    let path = command
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(PathBuf::from)
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let valid_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix("Talking Quill Maintenance-"))
        .and_then(|value| value.strip_suffix(".exe"))
        .is_some_and(|generation| validate_generation(generation).is_ok());
    if path.parent() != Some(program_files.as_path()) || !valid_name {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(path)
}

fn terminal_uninstall_record() -> Result<Option<TerminalUninstallRecord>, i32> {
    let root = medium_launcher_directory()?;
    let path = root.join(TERMINAL_UNINSTALL_RECORD_NAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(EXIT_IDENTITY_MISMATCH),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(EXIT_IDENTITY_MISMATCH),
    }
    let file = File::open(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let record_identity = file_identity_text(&file)?;
    drop(file);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if bytes.is_empty()
        || bytes.len() > 4096
        || !has_exact_security(&path, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let record: TerminalUninstallRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let maintenance = maintenance_path_from_command(&record.uninstall_command)?;
    let uninstall_command = format!("\"{}\"", maintenance.display());
    if record.schema_version != 3
        || validate_generation(&record.generation).is_err()
        || !matches!(
            record.phase.as_str(),
            "armed"
                | "machine-retired"
                | "cleanup-complete"
                | "final-launcher-owned"
                | "maintenance-deletion-owned"
                | "maintenance-deleted"
                | "uninstall-unregistered"
                | "journal-removed"
        )
        || decode_hash(&record.maintenance_sha256).is_none()
        || record.uninstall_command != uninstall_command
        || record.quiet_uninstall_command != format!("{uninstall_command} /S")
        || record.service_name != format!("TalkingQuillTerminalCleanup-{}", record.generation)
        || !record.service_image.ends_with(&format!(
            ".Talking Quill Terminal Cleanup-{}.exe",
            record.generation
        ))
        || decode_hash(&record.service_sha256).is_none()
        || record.service_file_identity.is_empty()
        || record.record_file_identity != record_identity
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(Some(record))
}

fn journal_owned_terminal_maintenance() -> Result<Option<PathBuf>, i32> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let transaction_path = program_files.join(".Talking Quill.native-transaction-v2.json");
    let bytes = match std::fs::read(&transaction_path) {
        Ok(bytes) if !bytes.is_empty() && bytes.len() <= 4096 => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(EXIT_IDENTITY_MISMATCH),
    };
    let transaction: SetupTransaction =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if transaction.schema_version != 2
        || transaction.action != "uninstall"
        || transaction.phase != "uninstall-cleanup-complete"
        || !transaction.had_predecessor
    {
        return Ok(None);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill",
            ))?
            .as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let quiet = read_registry_string(key, "QuietUninstallString")?;
    unsafe { RegCloseKey(key) };
    let maintenance = quiet
        .as_deref()
        .and_then(|value| value.strip_suffix(" /S"))
        .ok_or(EXIT_IDENTITY_MISMATCH)
        .and_then(maintenance_path_from_command)?;
    let retained = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    drop(retained);
    Ok(Some(maintenance))
}

fn run_machine_relaunch_owner() -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    if let Some(record) = terminal_uninstall_record()? {
        let maintenance = maintenance_path_from_command(&record.uninstall_command)?;
        if !maintenance.exists()
            && matches!(
                record.phase.as_str(),
                "final-launcher-owned"
                    | "maintenance-deletion-owned"
                    | "uninstall-unregistered"
                    | "journal-removed"
            )
            && std::env::current_exe()
                .ok()
                .and_then(|path| path.file_name().map(|name| name.to_owned()))
                .and_then(|name| name.to_str().map(str::to_owned))
                .is_some_and(|name| name.starts_with(".Talking Quill Terminal Relaunch-"))
        {
            let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
            drop(machine_lifecycle);
            launch_elevated_executable(&current, "--windows-update-relaunch-owner-retire-v1")?;
            return Ok(0);
        }
        let mut retained = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&record.maintenance_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(retained);
        drop(machine_lifecycle);
        launch_elevated_executable(&maintenance, "/S")?;
        return Ok(0);
    }
    if let Some(maintenance) = journal_owned_terminal_maintenance()? {
        drop(machine_lifecycle);
        launch_elevated_executable(&maintenance, "/S")?;
        return Ok(0);
    }
    let identity = current_relaunch_identity()?;
    let generations = relaunch_generations()?;
    if generations.is_empty()
        && !known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill")
            .exists()
    {
        let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
        drop(machine_lifecycle);
        launch_elevated_executable(&current, "--windows-update-relaunch-owner-retire-v1")?;
        return Ok(0);
    }
    drop(machine_lifecycle);
    for generation in generations {
        let Ok(record) = read_persisted_relaunch_record(&generation) else {
            let _ = retire_stale_schema2_relaunch_record(&generation);
            continue;
        };
        if record.user_sid == identity.user_sid && record.logon_sid == identity.logon_sid {
            let _ = run_persisted_relaunch(&generation);
        }
    }
    Ok(0)
}

fn run_persisted_relaunch(generation: &str) -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(generation)?;
    let _lock = acquire_relaunch_record_lock(&directory)?;
    let mut record = read_persisted_relaunch_record(generation)?;
    let identity = current_relaunch_identity()?;
    if record.user_sid != identity.user_sid || record.logon_sid != identity.logon_sid {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if record.phase == "app-ready" {
        drop(_lock);
        remove_relaunch_record_directory(&directory)?;
        return Ok(0);
    }
    let suffix = record
        .request
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    let request = parse_request_envelope(suffix)?;
    verify_update_authorization(&request.candidate)?;
    if record.source_version != request.candidate.predecessor.version
        || record.target_version != request.candidate.version
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if matches!(record.phase.as_str(), "setup-complete" | "launch-started") {
        let surviving = verified_surviving_version(&request.candidate, &record.predecessor)?;
        if record.completed_version.as_deref() != Some(surviving.as_str()) {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if record.phase == "setup-complete" {
            record.phase = "launch-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        defer_launch_until_parent_exit(&surviving, generation)?;
        return Ok(0);
    }
    let target_committed = verify_post_install_request(&request).is_ok();
    let recovering_setup = matches!(record.phase.as_str(), "armed" | "setup-started");
    if recovering_setup
        && native_setup_transaction_present()?
        && verified_surviving_version(&request.candidate, &record.predecessor).is_err()
    {
        let recovery_directory = medium_recovery_directory(&record.recovery_generation)?;
        if !recovery_directory.exists()
            || read_active_generation(&recovery_directory)? != record.recovery_generation
            || !recovery_command_present(&record.recovery_generation)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if record.phase == "armed" {
            record.phase = "setup-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        let resume = format!("--windows-update-resume-v2={}", record.recovery_generation);
        drop(_lock);
        drop(machine_lifecycle);
        launch_elevated_bootstrap(&resume)?;
        return run_persisted_relaunch(generation);
    }
    let recovered_survivor = recovering_setup
        && verified_surviving_version(&request.candidate, &record.predecessor).is_ok();
    if !target_committed && !recovered_survivor {
        verify_update_relation_against_snapshot(
            &request.candidate,
            &request.sha256,
            &record.predecessor,
        )?;
        if record.phase == "armed" {
            record.phase = "setup-started".into();
            write_persisted_relaunch_record(&directory, &record)?;
        }
        drop(_lock);
        drop(machine_lifecycle);
        let setup_result = launch_elevated_installed_helper(&format!(
            "--windows-update-bootstrap-bound-v1={}:{}",
            record.recovery_generation,
            record
                .request
                .strip_prefix("--windows-update-bootstrap-v2=")
                .ok_or(EXIT_INVALID_REQUEST)?
        ));
        let _machine_lifecycle = RecoveryStateLock::acquire()?;
        let _lock = acquire_relaunch_record_lock(&directory)?;
        record = read_persisted_relaunch_record(generation)?;
        if setup_result == Err(1223) {
            // Login recovery cancellation is not terminal ownership evidence. Keep the exact
            // armed generation for a later logon or maintenance recovery attempt.
            return Err(1223);
        }
    }
    let surviving = verified_surviving_version(&request.candidate, &record.predecessor)?;
    record.phase = "setup-complete".into();
    record.completed_version = Some(surviving.clone());
    write_persisted_relaunch_record(&directory, &record)?;
    record.phase = "launch-started".into();
    write_persisted_relaunch_record(&directory, &record)?;
    defer_launch_until_parent_exit(&surviving, generation)?;
    Ok(0)
}

fn acknowledge_app_ready(encoded: &str) -> Result<u32, i32> {
    let request: AppReadyRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if !valid_version(&request.version) || validate_generation(&request.generation).is_err() {
        return Err(EXIT_INVALID_REQUEST);
    }
    verify_app_ready_parent()?;
    if installed_manifest_version()? != request.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(&request.generation)?;
    let lock = acquire_relaunch_record_lock(&directory)?;
    let mut record = read_persisted_relaunch_record(&request.generation)?;
    if record.completed_version.as_deref() != Some(request.version.as_str())
        || !matches!(record.phase.as_str(), "launch-started" | "app-ready")
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if record.phase != "app-ready" {
        record.phase = "app-ready".into();
        write_persisted_relaunch_record(&directory, &record)?;
    }
    drop(lock);
    remove_relaunch_record_directory(&directory)?;
    Ok(0)
}

#[cfg(test)]
static TEST_RELAUNCH_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

fn relaunch_root() -> Result<PathBuf, i32> {
    #[cfg(test)]
    if let Some(root) = TEST_RELAUNCH_ROOT
        .lock()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .clone()
    {
        return Ok(root);
    }
    Ok(known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery/Relaunch Records"))
}

fn relaunch_identity_key(identity: &RelaunchIdentity) -> String {
    hex_digest(&Sha256::digest(identity.user_sid.as_bytes()))[..16].to_owned()
}

fn relaunch_generation_directory(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    let identity = current_relaunch_identity()?;
    Ok(relaunch_root()?.join(format!("{}-{generation}", relaunch_identity_key(&identity))))
}

fn relaunch_marker_value(record: &PersistedRelaunchRecord) -> String {
    format!(
        "{}:{}:{}",
        record.generation,
        record.nonce,
        hex_digest(&Sha256::digest(record.request.as_bytes()))
    )
}

fn publish_relaunch_record(record: &PersistedRelaunchRecord) -> Result<(), i32> {
    let root = relaunch_root()?;
    if !root.exists() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = current_relaunch_identity()?;
    if record.user_sid != identity.user_sid || record.logon_sid != identity.logon_sid {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let directory = relaunch_generation_directory(&record.generation)?;
    std::fs::create_dir(&directory).map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&directory, &identity)?;
    write_persisted_relaunch_record(&directory, record)?;
    write_protected_relaunch_file(
        &directory.join(RELAUNCH_MARKER_NAME),
        relaunch_marker_value(record).as_bytes(),
    )?;
    flush_directory(&directory)?;
    Ok(())
}

fn write_persisted_relaunch_record(
    directory: &Path,
    record: &PersistedRelaunchRecord,
) -> Result<(), i32> {
    let bytes = serde_json::to_vec(record).map_err(|_| EXIT_LAUNCH_FAILED)?;
    write_protected_relaunch_file(&directory.join(RELAUNCH_RECORD_NAME), &bytes)
}

fn write_protected_relaunch_file(path: &Path, bytes: &[u8]) -> Result<(), i32> {
    let parent = path.parent().ok_or(EXIT_INVALID_REQUEST)?;
    let temporary = parent.join(format!(".relaunch-pending-{}", new_recovery_generation()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&temporary, &current_relaunch_identity()?)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(file);
    verify_protected_relaunch_file(&temporary, bytes)?;
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(path)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let _ = std::fs::remove_file(temporary);
        return Err(EXIT_LAUNCH_FAILED);
    }
    flush_directory(parent)?;
    verify_protected_relaunch_file(path, bytes)
}

fn verify_protected_relaunch_file(path: &Path, expected: &[u8]) -> Result<(), i32> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut actual = Vec::new();
    file.read_to_end(&mut actual)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if actual == expected && has_relaunch_dacl(path, &current_relaunch_identity()?)? {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn read_persisted_relaunch_record(generation: &str) -> Result<PersistedRelaunchRecord, i32> {
    let directory = relaunch_generation_directory(generation)?;
    let identity = current_relaunch_identity()?;
    if !has_relaunch_dacl(&directory, &identity)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let path = directory.join(RELAUNCH_RECORD_NAME);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verify_protected_relaunch_file(&path, &bytes)?;
    let record: PersistedRelaunchRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if record.schema_version != 3
        || record.generation != generation
        || record.user_sid != identity.user_sid
        || validate_generation(&record.recovery_generation).is_err()
        || record.logon_sid != identity.logon_sid
        || !valid_nonce(&record.nonce)
        || !valid_version(&record.source_version)
        || !valid_version(&record.target_version)
        || !matches!(
            record.phase.as_str(),
            "armed" | "setup-started" | "setup-complete" | "launch-started" | "app-ready"
        )
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let marker = directory.join(RELAUNCH_MARKER_NAME);
    verify_protected_relaunch_file(&marker, relaunch_marker_value(&record).as_bytes())?;
    Ok(record)
}

fn retire_stale_schema2_relaunch_record(generation: &str) -> Result<(), i32> {
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    let directory = relaunch_generation_directory(generation)?;
    let _lock = acquire_relaunch_record_lock(&directory)?;
    let identity = current_relaunch_identity()?;
    if !has_relaunch_dacl(&directory, &identity)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let path = directory.join(RELAUNCH_RECORD_NAME);
    let bytes = std::fs::read(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verify_protected_relaunch_file(&path, &bytes)?;
    let record: PersistedRelaunchRecord =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if record.schema_version != 2
        || record.generation != generation
        || record.user_sid != identity.user_sid
        || record.logon_sid != identity.logon_sid
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_protected_relaunch_file(
        &directory.join(RELAUNCH_MARKER_NAME),
        relaunch_marker_value(&record).as_bytes(),
    )?;
    drop(_lock);
    remove_relaunch_record_directory(&directory)
}

fn acquire_relaunch_record_lock(directory: &Path) -> Result<std::fs::File, i32> {
    let path = directory.join("relaunch-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_relaunch_dacl(&path, &current_relaunch_identity()?)?;
    Ok(file)
}

fn relaunch_generations() -> Result<Vec<String>, i32> {
    let root = relaunch_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    let prefix = format!("{}-", relaunch_identity_key(&current_relaunch_identity()?));
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(&prefix) else {
            continue;
        };
        if entry.file_type().map_err(|_| EXIT_LAUNCH_FAILED)?.is_dir()
            && validate_generation(generation).is_ok()
        {
            values.push(generation.to_owned());
        }
    }
    Ok(values)
}

fn remove_relaunch_record_directory(directory: &Path) -> Result<(), i32> {
    for name in [
        RELAUNCH_RECORD_NAME,
        RELAUNCH_MARKER_NAME,
        "relaunch-state-v1.lock",
    ] {
        let path = directory.join(name);
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    std::fs::remove_dir(directory).map_err(|_| EXIT_LAUNCH_FAILED)
}

fn launch_elevated_installed_helper(argument: &str) -> Result<(), i32> {
    let helper = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill/resources/helper/talking-quill-helper.exe");
    launch_elevated_executable(&helper, argument)
}

fn defer_launch_until_parent_exit(version: &str, generation: &str) -> Result<(), i32> {
    let parent_pid = current_parent_process_id()?;
    if verify_installed_application_process(parent_pid).is_err() {
        return launch_program_files_application_for_generation(generation);
    }
    let request = DeferredLaunchRequest {
        parent_pid,
        generation: generation.into(),
        version: version.into(),
    };
    let encoded = base64_encode(&serde_json::to_vec(&request).map_err(|_| EXIT_LAUNCH_FAILED)?);
    std::process::Command::new(
        known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill/resources/helper/talking-quill-helper.exe"),
    )
    .arg(format!("--windows-update-launch-after-parent-v1={encoded}"))
    .spawn()
    .map(|_| ())
    .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn launch_after_parent_exit(encoded: &str) -> Result<u32, i32> {
    let request: DeferredLaunchRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if request.parent_pid == 0
        || !valid_version(&request.version)
        || validate_generation(&request.generation).is_err()
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    let raw = unsafe { OpenProcess(SYNCHRONIZE, 0, request.parent_pid) };
    if !raw.is_null() {
        let parent = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { WaitForSingleObject(parent.as_raw_handle(), 120_000) } != WAIT_OBJECT_0 {
            return Err(EXIT_LAUNCH_FAILED);
        }
    }
    if installed_manifest_version()? != request.version {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    launch_program_files_application_for_generation(&request.generation)?;
    Ok(0)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(TABLE[((value >> 18) & 63) as usize] as char);
        output.push(TABLE[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn current_parent_process_id() -> Result<u32, i32> {
    let snapshot_raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot_raw == -1_isize as HANDLE {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot_raw) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let current = unsafe { GetCurrentProcessId() };
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == current && entry.th32ParentProcessID != 0 {
                return Ok(entry.th32ParentProcessID);
            }
            if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
                break;
            }
        }
    }
    Err(EXIT_IDENTITY_MISMATCH)
}

fn verify_app_ready_parent() -> Result<(), i32> {
    verify_installed_application_process(current_parent_process_id()?)
}

fn verify_installed_application_process(process_id: u32) -> Result<(), i32> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if raw.is_null() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut image = vec![0_u16; 32_768];
    let mut length = image.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, image.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    image.truncate(length as usize);
    let actual = PathBuf::from(String::from_utf16(&image).map_err(|_| EXIT_IDENTITY_MISMATCH)?);
    let expected = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill/Talking Quill.exe");
    if paths_equal(&actual, &expected) {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn native_setup_transaction_present() -> Result<bool, i32> {
    let path =
        known_folder(&FOLDERID_ProgramFiles)?.join(".Talking Quill.native-transaction-v2.json");
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(
            metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(EXIT_IDENTITY_MISMATCH),
    }
}

fn authorize_public_update_bootstrap(argument: &str) -> Result<(), i32> {
    let encoded = argument
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
    let request = parse_and_authorize_request(encoded)?;
    let installed = installed_manifest_version()?;
    if request.candidate.predecessor.version != installed
        || !version_at_least(&installed, PUBLIC_UPDATE_TRUST_ROOT)
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

fn version_at_least(value: &str, minimum: &str) -> bool {
    let parse = |version: &str| -> Option<(u64, u64, u64)> {
        let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
        let result = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(result)
    };
    parse(value)
        .zip(parse(minimum))
        .is_some_and(|(value, minimum)| value >= minimum)
}

fn valid_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part == &"0" || !part.starts_with('0'))
        })
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn acquire_relaunch_intent_lock(path: &Path) -> Result<std::fs::File, i32> {
    let lock = path.with_file_name("windows-update-relaunch-intent-v1.lock");
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(lock)
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn read_relaunch_intent(path: &Path, expected_nonce: &str) -> Result<RelaunchIntent, i32> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if !path.is_absolute()
        || path.file_name().and_then(|value| value.to_str())
            != Some("windows-update-relaunch-intent-v1.json")
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case("Talking Quill"))
        || !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() > 4096
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let intent: RelaunchIntent =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?)
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if intent.schema_version != 1
        || intent.nonce != expected_nonce
        || !valid_version(&intent.source_version)
        || !valid_version(&intent.target_version)
        || intent.source_version == intent.target_version
        || !matches!(
            intent.phase.as_str(),
            "armed" | "setup-started" | "setup-complete" | "launch-started" | "app-ready"
        )
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(intent)
}

fn installed_manifest() -> Result<InstalledManifest, i32> {
    let path = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill/resources/keyboard-owner-release-v1.json");
    serde_json::from_slice(&std::fs::read(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}

fn installed_manifest_version() -> Result<String, i32> {
    Ok(installed_manifest()?.version)
}

fn verified_surviving_version(
    candidate: &UpdateCandidate,
    stored_predecessor: &InstalledManifest,
) -> Result<String, i32> {
    if installed_candidate_committed(candidate) {
        verify_installed_candidate_files(candidate)?;
        return Ok(candidate.version.clone());
    }
    let installed = installed_manifest()?;
    let predecessor = &candidate.predecessor;
    if stored_predecessor.version != predecessor.version
        || stored_predecessor.platform != predecessor.platform
        || stored_predecessor.architecture != predecessor.architecture
        || stored_predecessor.release_build_digest != predecessor.release_build_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let role_hash = |role: &str| {
        installed
            .roles
            .iter()
            .find(|value| value.role == role)
            .map(|value| value.sha256.as_str())
    };
    if installed.version == predecessor.version
        && installed.platform == predecessor.platform
        && installed.architecture == predecessor.architecture
        && installed.release_build_digest == predecessor.release_build_digest
        && role_hash("gateway") == Some(predecessor.gateway_sha256.as_str())
        && role_hash("owner") == Some(predecessor.owner_sha256.as_str())
    {
        verify_installed_snapshot(stored_predecessor)?;
        Ok(predecessor.version.clone())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn verify_installed_candidate_files(candidate: &UpdateCandidate) -> Result<(), i32> {
    let root = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill");
    for role in &candidate.roles {
        let mut file = open_locked(&root.join(&role.path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&role.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    Ok(())
}

fn relaunch_run_command(launcher: &Path) -> Result<String, i32> {
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    if command.encode_utf16().count() > 260 {
        Err(EXIT_LAUNCH_FAILED)
    } else {
        Ok(command)
    }
}

fn install_machine_relaunch_owner() -> Result<(), i32> {
    // Fixed global order: verified legacy mutex pair, then protected machine file lock.
    let _machine_lifecycle = RecoveryStateLock::acquire()?;
    if terminal_uninstall_record()?.is_some() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let launcher = ensure_medium_launcher(&current)?;
    let root = relaunch_root()?;
    if !root.exists() {
        create_directory_with_sddl(&root, RELAUNCH_ROOT_SDDL)?;
        apply_restricted_dacl(&root, RELAUNCH_ROOT_SDDL)?;
        flush_directory(root.parent().ok_or(EXIT_IDENTITY_MISMATCH)?)?;
    }
    let command = relaunch_run_command(&launcher)?;
    set_machine_relaunch_run_value(&command)
}

fn verify_machine_relaunch_owner() -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let final_launcher = current.parent() == Some(program_data.as_path())
        && current
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(".Talking Quill Terminal Relaunch-"))
            .and_then(|suffix| suffix.strip_suffix(".exe"))
            .is_some_and(|generation| validate_generation(generation).is_ok());
    let expected_path = if final_launcher {
        current
    } else {
        medium_launcher_path()?
    };
    let expected = relaunch_run_command(&expected_path)?;
    let actual = read_registry_string(key, RELAUNCH_RUN_VALUE)?;
    unsafe { RegCloseKey(key) };
    if actual.as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn finish_terminal_without_maintenance() -> Result<(), i32> {
    let Some(record) = terminal_uninstall_record()? else {
        return Ok(());
    };
    if !matches!(
        record.phase.as_str(),
        "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "uninstall-unregistered"
            | "journal-removed"
    ) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let generation = current
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".Talking Quill Terminal Relaunch-"))
        .and_then(|suffix| suffix.strip_suffix(".exe"))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    if generation != record.generation {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let uninstall = wide_nul(Path::new(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill",
    ))?;
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, uninstall.as_ptr()) };
    if status != 0 && status != 2 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let transaction = program_files.join(".Talking Quill.native-transaction-v2.json");
    if transaction.exists() {
        std::fs::remove_file(transaction).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let root = known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery");
    if root.exists() {
        let tombstone = known_folder(&FOLDERID_ProgramData)?
            .join(format!(".Talking Quill.recovery-tombstone-{generation}"));
        if tombstone.exists() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        std::fs::rename(root, tombstone).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

fn cleanup_terminal_recovery_tombstones() -> Result<Vec<PathBuf>, i32> {
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let mut tombstones = Vec::new();
    let mut maintenance_images = Vec::new();
    for entry in std::fs::read_dir(&program_data).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(".Talking Quill.recovery-tombstone-") else {
            continue;
        };
        validate_generation(generation)?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, RELAUNCH_ROOT_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let marker = path.join("launcher-tree-identity-v1");
        if marker.exists() {
            let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
            if !std::fs::read_to_string(&marker).is_ok_and(|value| value == identity) {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            let record_path = path.join("terminal-uninstall-record-v1.json");
            let mut inventory = std::fs::read_dir(&path)
                .map_err(|_| EXIT_LAUNCH_FAILED)?
                .map(|entry| {
                    entry
                        .map(|value| value.file_name().to_string_lossy().into_owned())
                        .map_err(|_| EXIT_LAUNCH_FAILED)
                })
                .collect::<Result<Vec<_>, _>>()?;
            inventory.sort();
            if record_path.exists() {
                let launcher_names = inventory
                    .iter()
                    .filter(|name| {
                        name.strip_prefix(RECOVERY_LAUNCHER_PUBLISHED_PREFIX)
                            .and_then(|value| value.strip_suffix(".exe"))
                            .is_some_and(|value| validate_generation(value).is_ok())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut expected = vec![
                    "launcher-tree-identity-v1".to_owned(),
                    "terminal-uninstall-record-v1.json".to_owned(),
                ];
                expected.extend(launcher_names.iter().cloned());
                expected.sort();
                if inventory != expected {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                let record: TerminalUninstallRecord = serde_json::from_slice(
                    &std::fs::read(&record_path).map_err(|_| EXIT_IDENTITY_MISMATCH)?,
                )
                .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
                if record.generation != generation
                    || !matches!(
                        record.phase.as_str(),
                        "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
                {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                maintenance_images.push((
                    maintenance_path_from_command(&record.uninstall_command)?,
                    decode_hash(&record.maintenance_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?,
                ));
                for launcher_name in launcher_names {
                    std::fs::remove_file(path.join(launcher_name))
                        .map_err(|_| EXIT_LAUNCH_FAILED)?;
                }
                std::fs::remove_file(&record_path).map_err(|_| EXIT_LAUNCH_FAILED)?;
            } else {
                let current_generation = std::env::current_exe()
                    .ok()
                    .and_then(|value| value.file_name().map(|name| name.to_owned()))
                    .and_then(|name| name.to_str().map(str::to_owned))
                    .and_then(|name| {
                        name.strip_prefix(".Talking Quill Terminal Relaunch-")
                            .and_then(|suffix| suffix.strip_suffix(".exe"))
                            .map(str::to_owned)
                    });
                if inventory != ["launcher-tree-identity-v1"]
                    || current_generation.as_deref() != Some(generation)
                {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
            }
            std::fs::remove_file(&marker).map_err(|_| EXIT_LAUNCH_FAILED)?;
            tombstones.push(path.clone());
        } else if std::fs::read_dir(&path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?
            .next()
            .is_none()
        {
            tombstones.push(path.clone());
        } else {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    for (maintenance, expected) in maintenance_images {
        if !maintenance.exists() {
            continue;
        }
        let mut file = open_locked(&maintenance).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(file);
        let metadata = std::fs::symlink_metadata(&maintenance).map_err(|_| EXIT_LAUNCH_FAILED)?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        std::fs::remove_file(maintenance).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(tombstones)
}

fn normalized_pending_delete_source(value: &str) -> String {
    let replaced = value.replace('/', "\\");
    replaced
        .strip_prefix(r"\??\")
        .or_else(|| replaced.strip_prefix(r"\\?\"))
        .unwrap_or(&replaced)
        .to_ascii_lowercase()
}

fn decode_pending_delete_pairs(data: &[u16]) -> Result<Vec<(String, String)>, i32> {
    let mut pairs = Vec::new();
    let mut cursor = 0;
    let mut terminated = false;
    while cursor < data.len() {
        let source_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        if cursor == source_start {
            let required = if pairs.is_empty() { 2 } else { 1 };
            if data.len() - cursor < required || data[cursor..].iter().any(|value| *value != 0) {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            terminated = true;
            break;
        }
        let source =
            String::from_utf16(&data[source_start..cursor]).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        cursor += 1;
        let destination_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let destination = String::from_utf16(&data[destination_start..cursor])
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        cursor += 1;
        pairs.push((source, destination));
    }
    if terminated {
        Ok(pairs)
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn pending_delete_owned(path: &Path) -> Result<bool, i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(
                r"SYSTEM\CurrentControlSet\Control\Session Manager",
            ))?
            .as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new("PendingFileRenameOperations"))?;
    let mut bytes = 0_u32;
    let mut value_type = 0_u32;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if queried == 2 {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    if queried != 0
        || value_type != REG_MULTI_SZ
        || bytes == 0
        || bytes > 1024 * 1024
        || !bytes.is_multiple_of(2)
    {
        unsafe { RegCloseKey(key) };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let capacity = bytes;
    let mut actual = bytes;
    let mut actual_type = 0_u32;
    let mut data = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut actual_type,
            data.as_mut_ptr().cast(),
            &mut actual,
        )
    } != 0
        || actual_type != REG_MULTI_SZ
        || actual > capacity
        || !actual.is_multiple_of(2)
    {
        unsafe { RegCloseKey(key) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    unsafe { RegCloseKey(key) };
    data.truncate(actual as usize / 2);
    let pairs = decode_pending_delete_pairs(&data)?;
    let expected = path
        .to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .to_ascii_lowercase();
    Ok(pairs.iter().any(|(source, destination)| {
        destination.is_empty() && normalized_pending_delete_source(source) == expected
    }))
}

fn schedule_and_verify_pending_delete(path: &Path) -> Result<(), i32> {
    if !pending_delete_owned(path)?
        && unsafe {
            MoveFileExW(
                wide_nul(path)?.as_ptr(),
                std::ptr::null(),
                MOVEFILE_DELAY_UNTIL_REBOOT,
            )
        } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if pending_delete_owned(path)? {
        Ok(())
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn retire_no_work_machine_relaunch_owner() -> Result<u32, i32> {
    let machine_lifecycle = RecoveryStateLock::acquire()?;
    verify_machine_relaunch_owner()?;
    finish_terminal_without_maintenance()?;
    let tombstones = cleanup_terminal_recovery_tombstones()?;
    if terminal_uninstall_record()?.is_some()
        || journal_owned_terminal_maintenance()?.is_some()
        || !relaunch_generations()?.is_empty()
        || known_folder(&FOLDERID_ProgramFiles)?
            .join("Talking Quill")
            .exists()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    schedule_and_verify_pending_delete(&current)?;
    for tombstone in &tombstones {
        schedule_and_verify_pending_delete(tombstone)?;
    }
    machine_lifecycle.retire()?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let deleted =
        unsafe { RegDeleteValueW(key, wide_nul(Path::new(RELAUNCH_RUN_VALUE))?.as_ptr()) };
    let flushed = deleted == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(EXIT_LAUNCH_FAILED);
    }
    for tombstone in tombstones {
        if tombstone.exists() {
            std::fs::remove_dir(tombstone).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    Ok(0)
}

fn set_machine_relaunch_run_value(command: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RELAUNCH_RUN_KEY))?.as_ptr(),
            0,
            std::ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new(RELAUNCH_RUN_VALUE))?;
    let value = wide_nul(Path::new(command))?;
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = unsafe { RegFlushKey(key) };
    unsafe { RegCloseKey(key) };
    if status == 0 && flushed == 0 {
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

fn launch_program_files_application_for_generation(generation: &str) -> Result<(), i32> {
    validate_generation(generation)?;
    let application = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill/Talking Quill.exe");
    let metadata = std::fs::symlink_metadata(&application).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    std::process::Command::new(application)
        .arg(format!(
            "--windows-update-relaunch-generation-v1={generation}"
        ))
        .spawn()
        .map(|_| ())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn launch_elevated_bootstrap(argument: &str) -> Result<(), i32> {
    let executable = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    launch_elevated_executable(&executable, argument)
}

fn launch_elevated_executable(executable: &Path, argument: &str) -> Result<(), i32> {
    let verb = wide_nul(Path::new("runas"))?;
    let file = wide_nul(executable)?;
    let parameters = wide_nul(Path::new(argument))?;
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: 1,
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { ShellExecuteExW(&mut execute) } == 0 || execute.hProcess.is_null() {
        let error = unsafe { GetLastError() } as i32;
        return Err(if error == 1223 {
            1223
        } else {
            EXIT_LAUNCH_FAILED
        });
    }
    let process = unsafe { OwnedHandle::from_raw_handle(execute.hProcess) };
    let wait =
        unsafe { WaitForSingleObject(process.as_raw_handle(), INSTALLER_SUPERVISION_TIMEOUT_MS) };
    if wait == WAIT_TIMEOUT {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_INSTALLER_STILL_RUNNING as u32) };
        unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(EXIT_INSTALLER_STILL_RUNNING);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if code != 0 {
        return Err(code as i32);
    }
    Ok(())
}

fn run_native_cleanup(binding: &str) -> Result<String, i32> {
    let (suffix, generation) = binding.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    validate_generation(generation)?;
    let path = known_folder(&FOLDERID_ProgramData)?
        .join(format!(".Talking Quill.update-bootstrap-{suffix}"));
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(generation.to_owned());
        }
        Err(_) => return Err(EXIT_LAUNCH_FAILED),
        Ok(_) => {}
    }
    let expected_identity = std::fs::read_to_string(path.join("cleanup-tree-identity-v1"))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if expected_identity.is_empty() || expected_identity.len() > 256 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    match read_active_generation(&path) {
        Ok(active) if active != generation => return Ok(generation.to_owned()),
        Ok(_) => {}
        Err(_) if !active_generation_path(&path).exists() => {}
        Err(error) => return Err(error),
    }
    for _ in 0..120 {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || metadata.file_attributes()
                        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                        != 0 =>
            {
                return Err(EXIT_IDENTITY_MISMATCH);
            }
            Ok(_) => match remove_owned_tree(&path, &expected_identity) {
                Ok(()) => return Ok(generation.to_owned()),
                Err(crate::owned_tree::OwnedTreeError::IdentityMismatch) => {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                Err(_) => {}
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(generation.to_owned());
            }
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(EXIT_LAUNCH_FAILED)
}

fn execute_staged_request(
    encoded: &str,
    previous_generation: Option<&str>,
    resuming: bool,
) -> Result<(u32, String), i32> {
    let request = parse_and_authorize_request(encoded)?;
    let recovery_generation = previous_generation.ok_or(EXIT_INVALID_REQUEST)?.to_owned();
    if resuming && installed_candidate_committed(&request.candidate) {
        return Ok((0, recovery_generation));
    }
    let expected_hash = decode_hash(&request.sha256).ok_or(EXIT_INVALID_REQUEST)?;
    let staged = copy_verified_installer(Path::new(&request.installer_path), expected_hash)?;
    persist_request(encoded)?;
    if !resuming {
        let _state = RecoveryStateLock::acquire()?;
        persist_restart_recovery(&recovery_directory()?, &recovery_generation)?;
    }
    acknowledge_recovery_ownership()?;
    let code = launch_verified_installer(&staged, expected_hash, &request.candidate)?;
    Ok((code, recovery_generation))
}

fn installed_candidate_committed(candidate: &UpdateCandidate) -> bool {
    let Ok(program_files) = known_folder(&FOLDERID_ProgramFiles) else {
        return false;
    };
    if program_files
        .join(".Talking Quill.native-transaction-v2.json")
        .exists()
    {
        return false;
    }
    let manifest_path = program_files
        .join("Talking Quill")
        .join("resources")
        .join("keyboard-owner-release-v1.json");
    let Ok(bytes) = std::fs::read(manifest_path) else {
        return false;
    };
    let Ok(installed) = serde_json::from_slice::<InstalledManifest>(&bytes) else {
        return false;
    };
    installed.version == candidate.version
        && installed.platform == candidate.platform
        && installed.architecture == candidate.architecture
        && installed.source_commit == candidate.source_commit
        && installed.source_tree == candidate.source_tree
        && installed.release_build_digest == candidate.release_build_digest
        && candidate.roles.len() == installed.roles.len()
        && candidate.roles.iter().all(|expected| {
            installed.roles.iter().any(|actual| {
                actual.role == expected.role
                    && actual.path == expected.path
                    && actual.sha256 == expected.sha256
                    && actual.suppression_capable == expected.suppression_capable
            })
        })
}

fn stage_bootstrap(argument: &str) -> Result<(), i32> {
    let (bound_generation, suffix) =
        if let Some(value) = argument.strip_prefix("--windows-update-bootstrap-bound-v1=") {
            let (generation, suffix) = value.split_once(':').ok_or(EXIT_INVALID_REQUEST)?;
            validate_generation(generation)?;
            (Some(generation), suffix)
        } else {
            (
                None,
                argument
                    .strip_prefix("--windows-update-bootstrap-v2=")
                    .ok_or(EXIT_INVALID_REQUEST)?,
            )
        };
    // The installed helper authorizes the exact outer installer and candidate
    // policy before it resumes any staged elevated executable.
    let request = parse_and_authorize_request(suffix)?;
    let authorized_package = decode_hash(&request.sha256).ok_or(EXIT_INVALID_REQUEST)?;
    let mut installer =
        open_locked(Path::new(&request.installer_path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut installer).map_err(|_| EXIT_IDENTITY_MISMATCH)? != authorized_package {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let (current, mut trusted, trusted_identity, expected_hash) = trusted_installed_bootstrap()?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let recovery_state = RecoveryStateLock::acquire()?;
    let recovery_launcher = ensure_medium_launcher(&current)?;
    reclaim_incomplete_recovery_directories(&program_data)?;
    let final_suffix = getrandom::u64().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let pending_token = new_recovery_generation()?;
    let pending = program_data.join(format!(
        ".Talking Quill.update-bootstrap-pending-{pending_token}"
    ));
    let directory = program_data.join(format!(
        ".Talking Quill.update-bootstrap-{final_suffix:016x}"
    ));
    create_restricted_directory(&pending)?;
    let mut directory_guard = match StagedDirectoryGuard::new(pending.clone()) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = std::fs::remove_dir(&pending);
            return Err(error);
        }
    };
    directory_guard.persist_identity_marker()?;
    let pending_staged = pending.join("talking-quill-update-bootstrap.exe");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&pending_staged)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&pending_staged, RESTRICTED_FILE_SDDL)?;
    trusted
        .seek(SeekFrom::Start(0))
        .and_then(|_| std::io::copy(&mut trusted, &mut output).map(|_| ()))
        .and_then(|_| output.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    stage_predecessor_evidence(&current, &pending)?;
    let mut retained = open_locked(&pending_staged).map_err(|_| EXIT_LAUNCH_FAILED)?;
    let expected_identity = file_identity(&retained).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut retained).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash
        || file_identity(&trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != trusted_identity
        || hash_file(&mut trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(retained);
    directory_guard.publish(directory.clone())?;
    directory_guard.register_prelaunch_cleanup(&recovery_launcher, bound_generation)?;
    let cleanup_generation = directory_guard
        .cleanup_generation
        .as_deref()
        .ok_or(EXIT_LAUNCH_FAILED)?
        .to_owned();
    drop(recovery_state);
    let staged = directory.join("talking-quill-update-bootstrap.exe");
    let mut retained = open_locked(&staged).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut retained).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let staged_argument =
        format!("--windows-update-bootstrap-staged-v2={cleanup_generation}:{suffix}");
    let mut application = wide_nul(&staged)?;
    let mut command = wide_nul(&PathBuf::from(format!(
        "\"{}\" {}",
        staged.display(),
        staged_argument
    )))?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_mut_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(process.hThread) };
    if file_identity(&trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != trusted_identity
        || hash_file(&mut trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash
        || !paths_equal(
            &current,
            &std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?,
        )
        || verify_suspended_process(
            process_handle.as_raw_handle(),
            &staged,
            expected_identity,
            expected_hash,
        )
        .is_err()
    {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_IDENTITY_MISMATCH as u32,
            )
        };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if unsafe { ResumeThread(thread_handle.as_raw_handle()) } == u32::MAX {
        unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    let ownership_marker = directory.join("recovery-owned-v2");
    let ownership_deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if std::fs::read(&ownership_marker).is_ok_and(|value| value == b"owned-v2") {
            directory_guard.transfer_to_launched_recovery();
            break;
        }
        if unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 0) } == WAIT_OBJECT_0
            || std::time::Instant::now() >= ownership_deadline
        {
            unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
            unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
            return Err(EXIT_LAUNCH_FAILED);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let wait = unsafe {
        WaitForSingleObject(
            process_handle.as_raw_handle(),
            INSTALLER_SUPERVISION_TIMEOUT_MS,
        )
    };
    if wait == WAIT_TIMEOUT {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_INSTALLER_STILL_RUNNING as u32,
            )
        };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_INSTALLER_STILL_RUNNING);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process_handle.as_raw_handle(), &mut code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if code != 0 {
        return Err(code as i32);
    }
    Ok(())
}

fn copy_restricted_snapshot(source: &Path, target: &Path) -> Result<(), i32> {
    let mut input = open_locked(source).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(target)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(target, RESTRICTED_FILE_SDDL)?;
    std::io::copy(&mut input, &mut output)
        .and_then(|_| output.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn stage_predecessor_evidence(installed_gateway: &Path, directory: &Path) -> Result<(), i32> {
    let helper_directory = installed_gateway.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let resources = helper_directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    copy_restricted_snapshot(
        &resources.join("keyboard-owner-release-v1.json"),
        &directory.join("predecessor-release.json"),
    )?;
    copy_restricted_snapshot(
        &helper_directory.join("talking-quill-keyboard-owner.exe"),
        &directory.join("predecessor-owner.exe"),
    )?;
    copy_restricted_snapshot(
        &helper_directory.join("talking-quill-update-recovery-launcher.exe"),
        &directory.join("predecessor-recovery-launcher.exe"),
    )
}

fn parse_request_envelope(encoded: &str) -> Result<UpdateRequest, i32> {
    let request: UpdateRequest =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if request.version != 2
        || request.installer_path.contains('\0')
        || request.sha256.len() != 64
        || !request
            .sha256
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    Ok(request)
}

fn parse_and_authorize_request(encoded: &str) -> Result<UpdateRequest, i32> {
    let request = parse_request_envelope(encoded)?;
    verify_update_relation(&request.candidate, &request.sha256)?;
    verify_update_authorization(&request.candidate)?;
    Ok(request)
}

fn verify_post_install_request(request: &UpdateRequest) -> Result<(), i32> {
    if request.candidate.package_sha256 != request.sha256
        || request.candidate.release_build_digest != request.candidate.package_layout_digest
        || canonical_candidate_layout(&request.candidate)?
            != request.candidate.package_layout_digest
        || !installed_candidate_committed(&request.candidate)
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_update_authorization(&request.candidate)
}

fn known_folder(folder: &windows_sys::core::GUID) -> Result<PathBuf, i32> {
    let mut value = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut value) } < 0
        || value.is_null()
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut length = 0usize;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    let path = PathBuf::from(
        String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| EXIT_LAUNCH_FAILED)?,
    );
    unsafe { CoTaskMemFree(value.cast()) };
    Ok(path)
}

fn trusted_installed_bootstrap() -> Result<(PathBuf, File, FileIdentity, [u8; 32]), i32> {
    let current =
        std::fs::canonicalize(std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?)
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let expected = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill")
        .join("resources")
        .join("helper")
        .join("talking-quill-helper.exe");
    if !paths_equal(&current, &expected) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut retained = open_locked(&current).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let identity = file_identity(&retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let hash = hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    Ok((current, retained, identity, hash))
}

fn verify_update_relation(candidate: &UpdateCandidate, package_sha256: &str) -> Result<(), i32> {
    let current = std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let staged = current
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("talking-quill-update-bootstrap.exe"));
    let resources = current
        .parent()
        .and_then(|parent| {
            if staged {
                Some(parent.to_owned())
            } else {
                parent.parent().map(Path::to_owned)
            }
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let manifest_name = if staged {
        "predecessor-release.json"
    } else {
        "keyboard-owner-release-v1.json"
    };
    let bytes = std::fs::read(resources.join(manifest_name)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let installed: InstalledManifest =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let digest = |value: &str| decode_hash(value).is_some();
    if candidate.platform != "win"
        || installed.platform != "win"
        || candidate.architecture != installed.architecture
        || !matches!(candidate.architecture.as_str(), "x64" | "arm64")
        || candidate.owner_mode != "local-unsigned-enabled"
        || candidate.package_mode != "update"
        || candidate.package_sha256.len() != 64
        || candidate.channel != format!("latest-{}", candidate.architecture)
        || candidate.transaction_binding != "source-target-package-sha256-v1"
        || candidate.version.is_empty()
        || !valid_source_identity(&candidate.source_commit)
        || !valid_source_identity(&candidate.source_tree)
        || !valid_source_identity(&installed.source_commit)
        || !valid_source_identity(&installed.source_tree)
        || candidate.release_build_digest == installed.release_build_digest
        || !digest(&candidate.release_build_digest)
        || !digest(&candidate.package_layout_digest)
        || !digest(&candidate.package_sha256)
        || candidate.roles.len() != 3
        || installed.roles.len() != 3
        || candidate.predecessor.platform != "win"
        || candidate.predecessor.architecture != installed.architecture
        || candidate.predecessor.version != installed.version
        || !digest(&installed.release_build_digest)
        || candidate.predecessor.release_build_digest != installed.release_build_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if candidate.package_sha256 != package_sha256
        || candidate.release_build_digest != candidate.package_layout_digest
        || canonical_candidate_layout(candidate)? != candidate.package_layout_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let candidate_gateway = update_role(&candidate.roles, "gateway")?;
    let candidate_owner = update_role(&candidate.roles, "owner")?;
    let candidate_recovery_launcher = update_role(&candidate.roles, "recovery-launcher")?;
    let installed_gateway = update_role(&installed.roles, "gateway")?;
    let installed_owner = update_role(&installed.roles, "owner")?;
    let installed_recovery_launcher = update_role(&installed.roles, "recovery-launcher")?;
    if installed_gateway.path != "resources/helper/talking-quill-helper.exe"
        || installed_gateway.suppression_capable
        || installed_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !installed_owner.suppression_capable
        || installed_recovery_launcher.path
            != "resources/helper/talking-quill-update-recovery-launcher.exe"
        || installed_recovery_launcher.suppression_capable
        || !digest(&installed_gateway.sha256)
        || !digest(&installed_owner.sha256)
        || !digest(&installed_recovery_launcher.sha256)
        || candidate_gateway.path != "resources/helper/talking-quill-helper.exe"
        || candidate_gateway.suppression_capable
        || !digest(&candidate_gateway.sha256)
        || candidate_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !candidate_owner.suppression_capable
        || !digest(&candidate_owner.sha256)
        || candidate_recovery_launcher.path
            != "resources/helper/talking-quill-update-recovery-launcher.exe"
        || candidate_recovery_launcher.suppression_capable
        || !digest(&candidate_recovery_launcher.sha256)
        || candidate.predecessor.gateway_sha256 != installed_gateway.sha256
        || candidate.predecessor.owner_sha256 != installed_owner.sha256
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut gateway_file = open_locked(&current).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let owner_name = if staged {
        "predecessor-owner.exe"
    } else {
        "talking-quill-keyboard-owner.exe"
    };
    let role_directory = current.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let mut owner_file =
        open_locked(&role_directory.join(owner_name)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let recovery_launcher_name = if staged {
        "predecessor-recovery-launcher.exe"
    } else {
        "talking-quill-update-recovery-launcher.exe"
    };
    let mut recovery_launcher_file = open_locked(&role_directory.join(recovery_launcher_name))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut gateway_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
        != decode_hash(&installed_gateway.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        || hash_file(&mut owner_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&installed_owner.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        || hash_file(&mut recovery_launcher_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&installed_recovery_launcher.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

fn verify_update_relation_against_snapshot(
    candidate: &UpdateCandidate,
    package_sha256: &str,
    predecessor: &InstalledManifest,
) -> Result<(), i32> {
    let gateway = update_role(&predecessor.roles, "gateway")?;
    let owner = update_role(&predecessor.roles, "owner")?;
    if candidate.package_sha256 != package_sha256
        || candidate.predecessor.version != predecessor.version
        || candidate.predecessor.platform != predecessor.platform
        || candidate.predecessor.architecture != predecessor.architecture
        || candidate.predecessor.release_build_digest != predecessor.release_build_digest
        || candidate.predecessor.gateway_sha256 != gateway.sha256
        || candidate.predecessor.owner_sha256 != owner.sha256
        || canonical_candidate_layout(candidate)? != candidate.package_layout_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_installed_snapshot(predecessor)
}

fn verify_installed_snapshot(snapshot: &InstalledManifest) -> Result<(), i32> {
    let installed = installed_manifest()?;
    if installed.version != snapshot.version
        || installed.platform != snapshot.platform
        || installed.architecture != snapshot.architecture
        || installed.source_commit != snapshot.source_commit
        || installed.source_tree != snapshot.source_tree
        || installed.release_build_digest != snapshot.release_build_digest
        || installed.roles.len() != snapshot.roles.len()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let root = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill");
    for role in &snapshot.roles {
        let current = update_role(&installed.roles, &role.role)?;
        if current.path != role.path
            || current.sha256 != role.sha256
            || current.suppression_capable != role.suppression_capable
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let mut file = open_locked(&root.join(&role.path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&role.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    Ok(())
}

fn verify_update_authorization(candidate: &UpdateCandidate) -> Result<(), i32> {
    let primary = env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1");
    let primary_digest = hex_digest(&Sha256::digest(decode_hex_bytes(primary, 65)?));
    if candidate.authorization.verification_key_sha256.as_deref() != Some(primary_digest.as_str()) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_update_authorization_with_key(candidate, primary)
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn verify_update_authorization_with_key(
    candidate: &UpdateCandidate,
    public_hex: &str,
) -> Result<(), i32> {
    if candidate.authorization.scheme != "p256-sha256-v1" {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let public = decode_hex_bytes(public_hex, 65)?;
    let verifying_key =
        VerifyingKey::from_sec1_bytes(&public).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let signature_bytes = decode_base64(&candidate.authorization.signature)?;
    let signature = Signature::from_der(&signature_bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verifying_key
        .verify(&authorization_transcript(candidate)?, &signature)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}

fn authorization_transcript(candidate: &UpdateCandidate) -> Result<Vec<u8>, i32> {
    let package = decode_hash(&candidate.package_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?;
    let layout = decode_hash(&candidate.package_layout_digest).ok_or(EXIT_IDENTITY_MISMATCH)?;
    let mut transcript = Vec::with_capacity(110);
    transcript.extend_from_slice(b"talking-quill/windows-update-authorization/v1\0");
    transcript.extend_from_slice(&package);
    transcript.extend_from_slice(&layout);
    Ok(transcript)
}

fn decode_hex_bytes(value: &str, expected: usize) -> Result<Vec<u8>, i32> {
    if value.len() != expected.saturating_mul(2) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    (0..expected)
        .map(|index| {
            u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|_| EXIT_IDENTITY_MISMATCH)
        })
        .collect()
}

fn valid_source_identity(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_candidate_layout(candidate: &UpdateCandidate) -> Result<String, i32> {
    let mut hash = Sha256::new();
    hash.update(b"talking-quill/package-layout/v1\0");
    for (name, value) in [
        ("version", candidate.version.as_str()),
        ("platform", candidate.platform.as_str()),
        ("architecture", candidate.architecture.as_str()),
        ("ownerMode", candidate.owner_mode.as_str()),
        ("packageMode", candidate.package_mode.as_str()),
        ("sourceCommit", candidate.source_commit.as_str()),
        ("sourceTree", candidate.source_tree.as_str()),
    ] {
        hash_identity_field(&mut hash, name, value)?;
    }
    for role in &candidate.roles {
        hash_identity_field(
            &mut hash,
            "role",
            &format!(
                "{}\0{}\0{}\0{}",
                role.role, role.path, role.sha256, role.suppression_capable
            ),
        )?;
    }
    hash_identity_field(&mut hash, "predecessorPresent", "true")?;
    for (name, value) in [
        (
            "predecessorPlatform",
            candidate.predecessor.platform.as_str(),
        ),
        (
            "predecessorArchitecture",
            candidate.predecessor.architecture.as_str(),
        ),
        ("predecessorVersion", candidate.predecessor.version.as_str()),
        (
            "predecessorReleaseBuildDigest",
            candidate.predecessor.release_build_digest.as_str(),
        ),
        (
            "predecessorGatewaySha256",
            candidate.predecessor.gateway_sha256.as_str(),
        ),
        (
            "predecessorOwnerSha256",
            candidate.predecessor.owner_sha256.as_str(),
        ),
    ] {
        hash_identity_field(&mut hash, name, value)?;
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hash_identity_field(hash: &mut Sha256, name: &str, value: &str) -> Result<(), i32> {
    let name_length = u16::try_from(name.len()).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let value_length = u32::try_from(value.len()).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    hash.update(name_length.to_be_bytes());
    hash.update(value_length.to_be_bytes());
    hash.update(name.as_bytes());
    hash.update(value.as_bytes());
    Ok(())
}

fn update_role<'a>(roles: &'a [UpdateRole], name: &str) -> Result<&'a UpdateRole, i32> {
    roles
        .iter()
        .find(|value| value.role == name)
        .ok_or(EXIT_IDENTITY_MISMATCH)
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn restricted(sddl: &str) -> Result<Self, i32> {
        let mut descriptor = std::ptr::null_mut();
        let sddl = wide_nul(Path::new(sddl))?;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        Ok(Self(descriptor))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0.cast()) };
    }
}

fn create_restricted_directory(path: &Path) -> Result<(), i32> {
    create_directory_with_sddl(path, RESTRICTED_STAGING_SDDL)
}

fn create_directory_with_sddl(path: &Path, sddl: &str) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(sddl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let path = wide_nul(path)?;
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

fn security_descriptor_text(descriptor: PSECURITY_DESCRIPTOR) -> Result<String, i32> {
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut text = std::ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            information,
            &mut text,
            std::ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(value)
}

fn has_exact_security(path: &Path, sddl: &str) -> Result<bool, i32> {
    let expected = SecurityDescriptor::restricted(sddl)?;
    let expected = security_descriptor_text(expected.0)?;
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let path = wide_nul(path)?;
    let mut needed = 0_u32;
    unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            information,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            information,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(security_descriptor_text(actual.as_mut_ptr().cast())?.eq_ignore_ascii_case(&expected))
}

fn apply_relaunch_dacl(path: &Path, identity: &RelaunchIdentity) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(&relaunch_record_sddl(identity))?;
    let path = wide_nul(path)?;
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        Err(EXIT_LAUNCH_FAILED)
    } else {
        Ok(())
    }
}

fn has_relaunch_dacl(path: &Path, identity: &RelaunchIdentity) -> Result<bool, i32> {
    let expected = SecurityDescriptor::restricted(&relaunch_record_sddl(identity))?;
    let expected = security_descriptor_text(expected.0)?;
    let path = wide_nul(path)?;
    let mut needed = 0_u32;
    unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(security_descriptor_text(actual.as_mut_ptr().cast())?.eq_ignore_ascii_case(&expected))
}

fn apply_restricted_dacl(path: &Path, sddl: &str) -> Result<(), i32> {
    let descriptor = SecurityDescriptor::restricted(sddl)?;
    let path = wide_nul(path)?;
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

fn published_recovery_launcher_name(source: &File, source_hash: [u8; 32]) -> Result<String, i32> {
    let mut digest = Sha256::new();
    digest.update(source_hash);
    digest.update(file_identity_text(source)?.as_bytes());
    let hash = digest.finalize();
    let generation = hash[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{RECOVERY_LAUNCHER_PUBLISHED_PREFIX}{generation}.exe"
    ))
}

fn ensure_medium_launcher(installed_helper: &Path) -> Result<PathBuf, i32> {
    let source = installed_helper
        .parent()
        .ok_or(EXIT_IDENTITY_MISMATCH)?
        .join(RECOVERY_LAUNCHER_NAME);
    let mut source_file = open_locked(&source).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let source_hash = hash_file(&mut source_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let published_name = published_recovery_launcher_name(&source_file, source_hash)?;
    let directory = medium_launcher_directory()?;
    reclaim_incomplete_launcher_directories(directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?)?;
    if !directory.exists() {
        publish_medium_launcher_directory(
            &directory,
            &mut source_file,
            source_hash,
            &published_name,
        )?;
        return Ok(directory.join(&published_name));
    }
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !has_exact_security(&directory, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let directory_identity = owned_tree_identity(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let identity_path = directory.join(RECOVERY_LAUNCHER_IDENTITY_NAME);
    if identity_path.exists() {
        if std::fs::read_to_string(&identity_path).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != directory_identity
            || !has_exact_security(&identity_path, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut identity_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&identity_path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&identity_path, MEDIUM_LAUNCHER_FILE_SDDL)?;
        identity_file
            .write_all(directory_identity.as_bytes())
            .and_then(|_| identity_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let target = directory.join(&published_name);
    if target.exists() {
        let mut existing = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut existing).map_err(|_| EXIT_IDENTITY_MISMATCH)? == source_hash
            && has_exact_security(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Ok(target);
        }
    }
    let temporary = directory.join(format!(".{published_name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    std::io::copy(&mut source_file, &mut output).map_err(|_| EXIT_LAUNCH_FAILED)?;
    output.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    let mut copied = open_locked(&temporary).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut copied).map_err(|_| EXIT_LAUNCH_FAILED)? != source_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(copied);
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(&target)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut published = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut published).map_err(|_| EXIT_IDENTITY_MISMATCH)? != source_hash
        || !has_exact_security(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(target)
}

fn publish_medium_launcher_directory(
    directory: &Path,
    source_file: &mut File,
    source_hash: [u8; 32],
    published_name: &str,
) -> Result<(), i32> {
    let parent = directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let token = new_recovery_generation()?;
    let pending = parent.join(format!("{RECOVERY_LAUNCHER_PENDING_PREFIX}{token}"));
    create_directory_with_sddl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    apply_restricted_dacl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    let identity = owned_tree_identity(&pending).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let result = (|| {
        let marker = pending.join(RECOVERY_LAUNCHER_IDENTITY_NAME);
        let mut marker_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&marker)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&marker, MEDIUM_LAUNCHER_FILE_SDDL)?;
        marker_file
            .write_all(identity.as_bytes())
            .and_then(|_| marker_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        let launcher = pending.join(published_name);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&launcher)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&launcher, MEDIUM_LAUNCHER_FILE_SDDL)?;
        source_file
            .seek(SeekFrom::Start(0))
            .and_then(|_| std::io::copy(source_file, &mut output).map(|_| ()))
            .and_then(|_| output.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        drop(output);
        let mut copied = open_locked(&launcher).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut copied).map_err(|_| EXIT_IDENTITY_MISMATCH)? != source_hash {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(copied);
        if unsafe {
            MoveFileExW(
                wide_nul(&pending)?.as_ptr(),
                wide_nul(directory)?.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        if owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)? != identity {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        Ok(())
    })();
    if result.is_err() && pending.exists() {
        let _ = remove_owned_tree(&pending, &identity);
    }
    result
}

fn reclaim_incomplete_launcher_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let pending = name
            .to_str()
            .and_then(|value| value.strip_prefix(RECOVERY_LAUNCHER_PENDING_PREFIX))
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !pending {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

// A normal Run value is the durable retry record. Windows must not consume recovery ownership
// before the elevated child reports success, as RunOnce does even when UAC is cancelled.
const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_ONCE_VALUE_PREFIX: &str = "Talking Quill Update Recovery ";
const MAX_VISIBLE_RECOVERY_ATTEMPTS: u8 = 3;
const RECOVERY_REQUEST_FILE: &str = "update-recovery-request-v2.txt";

fn recovery_directory() -> Result<PathBuf, i32> {
    std::env::current_exe()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .parent()
        .map(Path::to_owned)
        .ok_or(EXIT_LAUNCH_FAILED)
}

fn recovery_request_path() -> Result<PathBuf, i32> {
    Ok(recovery_directory()?.join(RECOVERY_REQUEST_FILE))
}

fn medium_launcher_directory() -> Result<PathBuf, i32> {
    Ok(known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery"))
}

fn valid_published_recovery_launcher(path: &Path, directory: &Path) -> bool {
    path.parent() == Some(directory)
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix(RECOVERY_LAUNCHER_PUBLISHED_PREFIX))
            .and_then(|value| value.strip_suffix(".exe"))
            .is_some_and(|generation| {
                generation.len() == 32
                    && generation
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

fn medium_launcher_path() -> Result<PathBuf, i32> {
    let directory = medium_launcher_directory()?;
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    if valid_published_recovery_launcher(&current, &directory) {
        return Ok(current);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let command = read_registry_string(key, RELAUNCH_RUN_VALUE)?;
    unsafe { RegCloseKey(key) };
    let Some(path) = command
        .as_deref()
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix("\" --windows-update-relaunch-owner-v1"))
        .map(PathBuf::from)
    else {
        return Err(EXIT_IDENTITY_MISMATCH);
    };
    if valid_published_recovery_launcher(&path, &directory) {
        Ok(path)
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn recovery_binding_path(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    Ok(medium_launcher_directory()?.join(format!("recovery-binding-v1-{generation}")))
}

fn medium_recovery_directory(generation: &str) -> Result<PathBuf, i32> {
    let launcher_directory = medium_launcher_directory()?;
    if !has_exact_security(&launcher_directory, MEDIUM_LAUNCHER_DIRECTORY_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let binding = recovery_binding_path(generation)?;
    if !has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let suffix = std::fs::read_to_string(binding).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(known_folder(&FOLDERID_ProgramData)?
        .join(format!(".Talking Quill.update-bootstrap-{suffix}")))
}

fn recovery_command_present(generation: &str) -> Result<bool, i32> {
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(false);
    }
    if opened != 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new(&recovery_value_name(generation)?))?;
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    if first != 0 || kind != REG_SZ || !(2..=2048).contains(&bytes) || !bytes.is_multiple_of(2) {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    let second = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    unsafe { RegCloseKey(key) };
    if second != 0 || kind != REG_SZ {
        return Ok(false);
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    let command = String::from_utf16(&value).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let launcher = medium_launcher_path()?;
    let resume = format!(
        "\"{}\" --windows-update-resume-v2={generation}",
        launcher.display()
    );
    let cleanup = std::fs::read_to_string(recovery_binding_path(generation)?)
        .ok()
        .map(|suffix| {
            format!(
                "\"{}\" --windows-update-cleanup-v1={suffix}:{generation}",
                launcher.display()
            )
        });
    Ok(command == resume || cleanup.as_ref().is_some_and(|cleanup| cleanup == &command))
}

fn owned_recovery_generations() -> Result<std::collections::BTreeSet<String>, i32> {
    let mut generations = std::collections::BTreeSet::new();
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if opened == 0 {
        let mut index = 0_u32;
        loop {
            let mut name = [0_u16; 128];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == 259 {
                break;
            }
            if status != 0 {
                unsafe { RegCloseKey(key) };
                return Err(EXIT_LAUNCH_FAILED);
            }
            let value = String::from_utf16_lossy(&name[..length as usize]);
            if let Some(generation) = value.strip_prefix(RUN_ONCE_VALUE_PREFIX)
                && validate_generation(generation).is_ok()
            {
                generations.insert(generation.to_owned());
            }
            index += 1;
        }
        unsafe { RegCloseKey(key) };
    } else if opened != 2 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if let Ok(entries) = std::fs::read_dir(medium_launcher_directory()?) {
        for entry in entries.flatten() {
            if let Some(generation) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("recovery-binding-v1-"))
                && validate_generation(generation).is_ok()
            {
                generations.insert(generation.to_owned());
            }
        }
    }
    Ok(generations)
}

fn reclaim_incomplete_recovery_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let pending = name
            .strip_prefix(".Talking Quill.update-bootstrap-pending-")
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        let published = name
            .strip_prefix(".Talking Quill.update-bootstrap-")
            .is_some_and(|suffix| {
                suffix.len() == 16
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !pending && !published {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, RESTRICTED_STAGING_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let marker = std::fs::read_to_string(path.join("cleanup-tree-identity-v1"));
        if marker.as_ref().is_ok_and(|value| value != &identity) {
            continue;
        }
        if published && marker.is_err() {
            continue;
        }
        let generation = read_active_generation(&path).ok();
        let complete = generation.as_deref().is_some_and(|generation| {
            protected_visible_attempt(&path, generation).is_some()
                && recovery_binding_path(generation).is_ok_and(|binding| {
                    has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL).unwrap_or(false)
                        && std::fs::read_to_string(binding)
                            .is_ok_and(|suffix| Some(suffix.as_str()) == name.rsplit('-').next())
                })
                && recovery_command_present(generation).unwrap_or(false)
        });
        if pending || !complete {
            if let Some(generation) = generation.as_deref() {
                let _ = clear_restart_recovery(generation);
            }
            remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    for generation in owned_recovery_generations()? {
        let complete = medium_recovery_directory(&generation).is_ok_and(|directory| {
            directory.exists()
                && read_active_generation(&directory).is_ok_and(|active| active == generation)
                && protected_visible_attempt(&directory, &generation).is_some()
                && recovery_command_present(&generation).unwrap_or(false)
        });
        if !complete {
            clear_restart_recovery(&generation)?;
        }
    }
    Ok(())
}

fn find_recovery_directory(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    let root = known_folder(&FOLDERID_ProgramData)?;
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        else {
            continue;
        };
        if suffix.len() != 16
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            continue;
        }
        let path = entry.path();
        let valid = std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        }) && has_exact_security(&path, RESTRICTED_STAGING_SDDL).unwrap_or(false)
            && std::fs::read_to_string(path.join("cleanup-tree-identity-v1"))
                .ok()
                .is_some_and(|identity| {
                    !identity.is_empty()
                        && identity.len() <= 256
                        && owned_tree_identity(&path).is_ok_and(|actual| actual == identity)
                });
        if valid && read_active_generation(&path).is_ok_and(|active| active == generation) {
            matches.push(path);
        }
    }
    if matches.len() == 1 {
        Ok(matches.remove(0))
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

fn visible_retry_path(directory: &Path, generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    Ok(directory.join(format!("visible-attempt-v1-{generation}")))
}

struct RecoveryStateLock {
    _legacy: Option<LegacyMutexPair>,
    file: File,
    path: PathBuf,
}

impl RecoveryStateLock {
    fn acquire() -> Result<Self, i32> {
        Self::acquire_for_epoch(installed_recovery_policy_epoch()?)
    }

    fn acquire_for_epoch(predecessor_policy_epoch: u8) -> Result<Self, i32> {
        let legacy = if predecessor_policy_epoch < LEGACY_LOCK_RETIREMENT_EPOCH {
            Some(LegacyMutexPair::acquire()?)
        } else {
            None
        };
        let path = machine_lock_file(predecessor_policy_epoch)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
            {
                Ok(file) => {
                    if !has_exact_security(&path, MACHINE_LOCK_FILE_SDDL)?
                        || file_identity_text(&file)?
                            != std::fs::read_to_string(path.with_extension("identity-v1"))
                                .map_err(|_| EXIT_IDENTITY_MISMATCH)?
                    {
                        return Err(EXIT_IDENTITY_MISMATCH);
                    }
                    validate_acquired_machine_lock_state(&path)?;
                    return Ok(Self {
                        _legacy: legacy,
                        file,
                        path,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(EXIT_LAUNCH_FAILED),
            }
        }
    }

    fn retire(self) -> Result<(), i32> {
        let directory = self
            .path
            .parent()
            .ok_or(EXIT_IDENTITY_MISMATCH)?
            .to_path_buf();
        let suffix = directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
            .ok_or(EXIT_IDENTITY_MISMATCH)?;
        let identity = owned_tree_identity(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let mut key = std::ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_KEY))?.as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut key,
            )
        } != 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        let value = wide_nul(Path::new(&format!("{MACHINE_LOCK_RETIRED_PREFIX}{suffix}")))?;
        let status = unsafe {
            RegSetValueExW(
                key,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
        unsafe { RegCloseKey(key) };
        if !flushed {
            return Err(EXIT_LAUNCH_FAILED);
        }
        drop(self);
        remove_owned_tree(&directory, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        let deleted = unsafe {
            RegDeleteTreeW(
                HKEY_LOCAL_MACHINE,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_KEY))?.as_ptr(),
            )
        };
        if deleted == 0 || deleted == 2 {
            Ok(())
        } else {
            Err(EXIT_LAUNCH_FAILED)
        }
    }
}

fn validate_acquired_machine_lock_state(lock: &Path) -> Result<(), i32> {
    let suffix = lock
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(MACHINE_LOCK_REGISTRY_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let publication = read_registry_string(key, MACHINE_LOCK_REGISTRY_VALUE)?;
    unsafe { RegCloseKey(key) };
    match publication.as_deref() {
        Some(value) if value == suffix => Ok(()),
        Some(value)
            if value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX) == Some(suffix)
                && terminal_uninstall_record()?.is_some() =>
        {
            Ok(())
        }
        _ => Err(EXIT_IDENTITY_MISMATCH),
    }
}

impl Drop for RecoveryStateLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
    }
}

fn installed_recovery_policy_epoch() -> Result<u8, i32> {
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let bytes = std::fs::read(current).map_err(|_| EXIT_LAUNCH_FAILED)?;
    const PREFIX: &[u8] = b"TALKING_QUILL_WINDOWS_RECOVERY_POLICY_EPOCH=";
    let matches = bytes
        .windows(PREFIX.len() + 1)
        .filter_map(|window| {
            window
                .strip_prefix(PREFIX)
                .map(|value| value[0])
                .filter(u8::is_ascii_digit)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(1);
    }
    if matches.iter().any(|value| *value != matches[0]) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(matches[0] - b'0')
}

struct LegacyMutexPair([OwnedHandle; 2]);

impl LegacyMutexPair {
    fn acquire() -> Result<Self, i32> {
        let first = acquire_verified_legacy_mutex("Global\\TalkingQuill.NativeSetup.V2")?;
        let second = acquire_verified_legacy_mutex("Global\\TalkingQuill.UpdateRecovery.State.V1")?;
        Ok(Self([first, second]))
    }
}

impl Drop for LegacyMutexPair {
    fn drop(&mut self) {
        for handle in self.0.iter().rev() {
            unsafe { ReleaseMutex(handle.as_raw_handle()) };
        }
    }
}

fn acquire_verified_legacy_mutex(name: &str) -> Result<OwnedHandle, i32> {
    let descriptor = SecurityDescriptor::restricted("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let raw = unsafe { CreateMutexW(&attributes, 0, wide_nul(Path::new(name))?.as_ptr()) };
    if raw.is_null() {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if !matches!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) },
        WAIT_OBJECT_0 | 0x80
    ) || !legacy_mutex_security_is_exact(handle.as_raw_handle())?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(handle)
}

fn legacy_mutex_security_is_exact(handle: HANDLE) -> Result<bool, i32> {
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let text = security_descriptor_text(descriptor)?;
    unsafe { LocalFree(descriptor.cast()) };
    let normalized = text.to_ascii_uppercase();
    Ok(normalized.starts_with("O:BA")
        && (normalized.contains("(A;;GA;;;SY)") || normalized.contains("(A;;0X1F0001;;;SY)"))
        && (normalized.contains("(A;;GA;;;BA)") || normalized.contains("(A;;0X1F0001;;;BA)"))
        && normalized.matches("(A;;").count() == 2
        && !normalized.contains(";;;AU)"))
}

fn machine_lock_file(predecessor_policy_epoch: u8) -> Result<PathBuf, i32> {
    let root = known_folder(&FOLDERID_ProgramData)?;
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(MACHINE_LOCK_REGISTRY_KEY))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    reclaim_machine_lock_pending(&root)?;
    let terminal_owner_present = terminal_uninstall_record()?.is_some();
    let published = read_registry_string(key, MACHINE_LOCK_REGISTRY_VALUE)?;
    let published = if let Some(retired) = published
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_generation(retired)?;
        let retired_directory = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{retired}"));
        if terminal_owner_present && retired_directory.exists() {
            Some(retired.to_owned())
        } else {
            if unsafe {
                RegDeleteValueW(
                    key,
                    wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                )
            } != 0
                || unsafe { RegFlushKey(key) } != 0
            {
                unsafe { RegCloseKey(key) };
                return Err(EXIT_LAUNCH_FAILED);
            }
            if retired_directory.exists() {
                reclaim_retired_machine_lock_directory(&retired_directory)?;
            }
            None
        }
    } else {
        published
    };
    let directory = if let Some(suffix) = published {
        validate_generation(&suffix)?;
        root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH && !terminal_owner_present {
            unsafe { RegCloseKey(key) };
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        reclaim_unpublished_machine_lock_directories(&root)?;
        let suffix = new_recovery_generation()?;
        let token = new_recovery_generation()?;
        let pending = root.join(format!("{MACHINE_LOCK_PENDING_PREFIX}{token}"));
        let published = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_directory_with_sddl(&pending, MACHINE_LOCK_DIRECTORY_SDDL)?;
        apply_restricted_dacl(&pending, MACHINE_LOCK_DIRECTORY_SDDL)?;
        let identity = owned_tree_identity(&pending).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        create_or_verify_marker(
            &pending.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
            MACHINE_LOCK_FILE_SDDL,
        )?;
        initialize_machine_lock_tree(&pending, &identity)?;
        flush_directory(&pending)?;
        if unsafe {
            MoveFileExW(
                wide_nul(&pending)?.as_ptr(),
                wide_nul(&published)?.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        flush_directory(&root)?;
        let value = wide_nul(Path::new(&suffix))?;
        if unsafe {
            RegSetValueExW(
                key,
                wide_nul(Path::new(MACHINE_LOCK_REGISTRY_VALUE))?.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } != 0
            || unsafe { RegFlushKey(key) } != 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        published
    };
    unsafe { RegCloseKey(key) };
    verify_machine_lock_tree(&directory)
}

fn initialize_machine_lock_tree(directory: &Path, directory_identity: &str) -> Result<(), i32> {
    let lock = directory.join("recovery-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&lock)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&lock, MACHINE_LOCK_FILE_SDDL)?;
    file.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    create_or_verify_marker(
        &lock.with_extension("identity-v1"),
        &identity,
        MACHINE_LOCK_FILE_SDDL,
    )?;
    create_or_verify_marker(
        &directory.join("lock-tree-identity-v1"),
        directory_identity,
        MACHINE_LOCK_FILE_SDDL,
    )
}

fn verify_machine_lock_tree(directory: &Path) -> Result<PathBuf, i32> {
    if !has_exact_security(directory, MACHINE_LOCK_DIRECTORY_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let identity = owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if std::fs::read_to_string(directory.join("lock-tree-identity-v1"))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?
        != identity
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let lock = directory.join("recovery-state-v1.lock");
    if !has_exact_security(&lock, MACHINE_LOCK_FILE_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(lock)
}

fn reclaim_retired_machine_lock_directory(directory: &Path) -> Result<(), i32> {
    verify_machine_lock_tree(directory)?;
    let identity = owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    remove_owned_tree(directory, &identity).map_err(|_| EXIT_LAUNCH_FAILED)
}

fn reclaim_unpublished_machine_lock_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        else {
            continue;
        };
        if validate_generation(suffix).is_err() {
            continue;
        }
        let path = entry.path();
        if !has_exact_security(&path, MACHINE_LOCK_DIRECTORY_SDDL)? {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let marker = path.join("publication-pending-v1");
        let expected = format!("{suffix}:{identity}");
        if std::fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    Ok(())
}

fn reclaim_machine_lock_pending(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_PENDING_PREFIX))
            .is_some_and(|suffix| validate_generation(suffix).is_ok())
        {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, MACHINE_LOCK_DIRECTORY_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

fn flush_directory(path: &Path) -> Result<(), i32> {
    let handle = unsafe {
        CreateFileW(
            wide_nul(path)?.as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

fn create_or_verify_marker(path: &Path, value: &str, sddl: &str) -> Result<(), i32> {
    if path.exists() {
        return verify_marker(path, value, sddl, None);
    }
    let parent = path.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let name = path
        .file_name()
        .ok_or(EXIT_IDENTITY_MISMATCH)?
        .to_string_lossy();
    let temporary = parent.join(format!("{name}.tmp-{}", new_recovery_generation()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, sddl)?;
    file.write_all(value.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    verify_marker(&temporary, value, sddl, Some(&identity))?;
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(path)?.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    flush_directory(parent)?;
    verify_marker(path, value, sddl, Some(&identity))
}

fn verify_marker(
    path: &Path,
    value: &str,
    sddl: &str,
    expected_identity: Option<&str>,
) -> Result<(), i32> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let identity = file_identity_text(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || content != value
        || !has_exact_security(path, sddl)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

fn file_identity_text(file: &File) -> Result<String, i32> {
    let identity = file_identity(file).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    Ok(format!(
        "{}:{}",
        identity.volume,
        (u64::from(identity.index_high) << 32) | u64::from(identity.index_low)
    ))
}

fn read_registry_string(key: *mut c_void, name: &str) -> Result<Option<String>, i32> {
    let name = wide_nul(Path::new(name))?;
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || !(2..=1024).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}

fn protected_visible_attempt(directory: &Path, generation: &str) -> Option<u8> {
    let path = visible_retry_path(directory, generation).ok()?;
    if !has_exact_security(&path, RETRY_COUNTER_SDDL).ok()? {
        return None;
    }
    let length = std::fs::metadata(path).ok()?.len();
    u8::try_from(length).ok()
}

fn begin_protected_visible_retry(directory: &Path, generation: &str) -> Result<u8, i32> {
    let path = visible_retry_path(directory, generation)?;
    if !has_exact_security(&path, RETRY_COUNTER_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .share_mode(0)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let attempt = u8::try_from(file.metadata().map_err(|_| EXIT_LAUNCH_FAILED)?.len())
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if attempt >= MAX_VISIBLE_RECOVERY_ATTEMPTS {
        return Ok(attempt.saturating_add(1));
    }
    file.write_all(&[1])
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    Ok(attempt + 1)
}

fn show_visible_retry_paused() {
    let text = wide_nul(Path::new(
        "Talking Quill could not obtain administrator approval after three attempts. Automatic update prompts are paused and the recovery generation is retained. Open Apps > Installed apps and choose Uninstall for Talking Quill to run maintenance. Maintenance recovers the installed state before continuing.",
    ));
    let title = wide_nul(Path::new("Talking Quill recovery paused"));
    if let (Ok(text), Ok(title)) = (text, title) {
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONWARNING,
            )
        };
    }
}

fn persist_request(encoded: &str) -> Result<(), i32> {
    let path = recovery_request_path()?;
    if path.exists() {
        return if std::fs::read_to_string(path).map_err(|_| EXIT_LAUNCH_FAILED)? == encoded {
            Ok(())
        } else {
            Err(EXIT_IDENTITY_MISMATCH)
        };
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&path, RESTRICTED_FILE_SDDL)?;
    file.write_all(encoded.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn acknowledge_recovery_ownership() -> Result<(), i32> {
    let path = recovery_directory()?.join("recovery-owned-v2");
    if path.exists() {
        return if std::fs::read(&path).map_err(|_| EXIT_LAUNCH_FAILED)? == b"owned-v2" {
            Ok(())
        } else {
            Err(EXIT_IDENTITY_MISMATCH)
        };
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&path, RESTRICTED_FILE_SDDL)?;
    file.write_all(b"owned-v2")
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn read_persisted_request() -> Result<String, i32> {
    let value =
        std::fs::read_to_string(recovery_request_path()?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if value.is_empty() || value.len() > 64 * 1024 {
        return Err(EXIT_INVALID_REQUEST);
    }
    Ok(value)
}

fn validate_generation(generation: &str) -> Result<(), i32> {
    if generation.len() == 32
        && generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(EXIT_INVALID_REQUEST)
    }
}

fn new_recovery_generation() -> Result<String, i32> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| EXIT_LAUNCH_FAILED)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn recovery_value_name(generation: &str) -> Result<String, i32> {
    validate_generation(generation)?;
    Ok(format!("{RUN_ONCE_VALUE_PREFIX}{generation}"))
}

fn active_generation_path(directory: &Path) -> PathBuf {
    directory.join("active-recovery-generation-v1")
}

fn persist_active_generation(directory: &Path, generation: &str) -> Result<(), i32> {
    validate_generation(generation)?;
    let counter = visible_retry_path(directory, generation)?;
    if counter.exists() {
        if !has_exact_security(&counter, RETRY_COUNTER_SDDL)? {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&counter)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&counter, RETRY_COUNTER_SDDL)?;
        file.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let suffix = published_recovery_suffix(directory)?;
    let binding = recovery_binding_path(generation)?;
    if binding.exists() {
        if std::fs::read_to_string(&binding).map_err(|_| EXIT_IDENTITY_MISMATCH)? != suffix
            || !has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&binding)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&binding, MEDIUM_LAUNCHER_FILE_SDDL)?;
        file.write_all(suffix.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let target = active_generation_path(directory);
    let temporary = directory.join(format!(
        ".active-recovery-generation-v1.tmp-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, RESTRICTED_FILE_SDDL)?;
    file.write_all(generation.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(file);
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(&target)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(())
}

fn published_recovery_suffix(directory: &Path) -> Result<&str, i32> {
    directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .filter(|value| {
            value.len() == 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)
}

fn read_active_generation(directory: &Path) -> Result<String, i32> {
    let generation = std::fs::read_to_string(active_generation_path(directory))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    validate_generation(&generation)?;
    Ok(generation)
}

fn persist_run_once(command: &str, generation: &str) -> Result<(), i32> {
    if command.encode_utf16().count() + 1 > 260 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let command = wide_nul(Path::new(command))?;
    let status = unsafe {
        RegSetValueExW(
            key,
            wide_nul(Path::new(&recovery_value_name(generation)?))?.as_ptr(),
            0,
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

fn persist_restart_recovery(directory: &Path, generation: &str) -> Result<(), i32> {
    if read_active_generation(directory)? != generation
        || protected_visible_attempt(directory, generation).is_none()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let launcher = medium_launcher_path()?;
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-resume-v2={generation}",
            launcher.display()
        ),
        generation,
    )
}

fn persist_prelaunch_cleanup(
    installed_helper: &Path,
    directory: &Path,
    identity: &str,
    generation: &str,
) -> Result<(), i32> {
    let suffix = directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .filter(|value| {
            value.len() == 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let identity_path = directory.join("cleanup-tree-identity-v1");
    if identity_path.exists() {
        if std::fs::read_to_string(&identity_path).map_err(|_| EXIT_LAUNCH_FAILED)? != identity {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut identity_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&identity_path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&identity_path, RESTRICTED_FILE_SDDL)?;
        identity_file
            .write_all(identity.as_bytes())
            .and_then(|_| identity_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    persist_active_generation(directory, generation)?;
    let binding = format!("{suffix}:{generation}");
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-cleanup-v1={binding}",
            installed_helper.display()
        ),
        generation,
    )
}

fn clear_restart_recovery(generation: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let status = unsafe {
        RegDeleteValueW(
            key,
            wide_nul(Path::new(&recovery_value_name(generation)?))?.as_ptr(),
        )
    };
    let flushed = (status == 0 || status == 2) && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        let _ = std::fs::remove_file(recovery_binding_path(generation)?);
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

fn schedule_staged_cleanup(previous_generation: Option<&str>) -> Result<(), i32> {
    let _state = RecoveryStateLock::acquire()?;
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let directory = current
        .parent()
        .map(Path::to_owned)
        .ok_or(EXIT_LAUNCH_FAILED)?;
    if directory
        .file_name()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.starts_with(".Talking Quill.update-bootstrap-"))
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let generation = previous_generation.ok_or(EXIT_INVALID_REQUEST)?;
    if read_active_generation(&directory)? != generation {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let launcher = medium_launcher_path()?;
    let suffix = published_recovery_suffix(&directory)?;
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-cleanup-v1={suffix}:{generation}",
            launcher.display()
        ),
        generation,
    )?;
    spawn_staged_cleanup(&directory, generation)
}

#[derive(Clone, Eq, PartialEq)]
struct RelaunchIdentity {
    user_sid: String,
    logon_sid: String,
}

fn sid_string(sid: *mut core::ffi::c_void) -> Result<String, i32> {
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let length = unsafe { (0..).take_while(|&index| *text.add(index) != 0).count() };
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| EXIT_IDENTITY_MISMATCH);
    unsafe { LocalFree(text.cast()) };
    result
}

fn token_information(class: i32) -> Result<Vec<u8>, i32> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut needed = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut value = vec![0_u8; needed as usize];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            value.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(value)
}

fn current_relaunch_identity() -> Result<RelaunchIdentity, i32> {
    let user = token_information(TokenUser)?;
    let user = unsafe { &*(user.as_ptr().cast::<TOKEN_USER>()) };
    let user_sid = sid_string(user.User.Sid)?;
    // TokenUser is the durable SID of the real interactive logon identity. Unlike the
    // per-session logon-group SID, it remains stable across reboot/login recovery.
    Ok(RelaunchIdentity {
        logon_sid: user_sid.clone(),
        user_sid,
    })
}

fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        ) != 0
            && returned == size_of::<TOKEN_ELEVATION>() as u32
            && elevation.TokenIsElevated == 1
    }
}

fn copy_verified_installer(path: &Path, expected_hash: [u8; 32]) -> Result<PathBuf, i32> {
    if !path.is_absolute() {
        return Err(EXIT_INVALID_REQUEST);
    }
    let directory = std::env::current_exe()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .parent()
        .ok_or(EXIT_LAUNCH_FAILED)?
        .to_owned();
    let target = directory.join("verified-update-installer.exe");
    if target.exists() {
        let mut retained = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)? == expected_hash {
            return Ok(target);
        }
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut source = open_locked(&canonical).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut source).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&target)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&target, RESTRICTED_FILE_SDDL)?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    std::io::copy(&mut source, &mut output).map_err(|_| EXIT_LAUNCH_FAILED)?;
    output.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    let mut copied = open_locked(&target).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut copied).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(target)
}

fn launch_verified_installer(
    path: &Path,
    expected_hash: [u8; 32],
    candidate: &UpdateCandidate,
) -> Result<u32, i32> {
    if !path.is_absolute() {
        return Err(EXIT_INVALID_REQUEST);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let mut retained = open_locked(&canonical).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let expected_identity = file_identity(&retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }

    let mut application = wide_nul(&canonical)?;
    // The candidate derives update mode and predecessor identity from its
    // authenticated TQPKG2 manifest and installed machine state. Arguments
    // carry no install authority.
    let mut command = wide_nul(&PathBuf::from(format!(
        "\"{}\" /S /TQUPDATE={} /TQGATEWAYHASH={} /TQOWNERHASH={} /TQLAYOUT={}",
        canonical.display(),
        candidate.package_sha256,
        candidate.predecessor.gateway_sha256,
        candidate.predecessor.owner_sha256,
        candidate.predecessor.release_build_digest,
    )))?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_mut_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(process.hThread) };

    let verified = verify_suspended_process(
        process_handle.as_raw_handle(),
        &canonical,
        expected_identity,
        expected_hash,
    );
    if verified.is_err() {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_IDENTITY_MISMATCH as u32,
            )
        };
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if unsafe { ResumeThread(thread_handle.as_raw_handle()) } == u32::MAX {
        unsafe { TerminateProcess(process_handle.as_raw_handle(), EXIT_LAUNCH_FAILED as u32) };
        return Err(EXIT_LAUNCH_FAILED);
    }
    // The verified copy lives in the elevated bootstrap's protected directory,
    // so same-user code cannot replace it after this point. Release the file
    // lock because native setup removes its own executable during normal teardown.
    drop(retained);
    let wait = unsafe {
        WaitForSingleObject(
            process_handle.as_raw_handle(),
            INSTALLER_SUPERVISION_TIMEOUT_MS,
        )
    };
    if wait == WAIT_TIMEOUT {
        unsafe {
            TerminateProcess(
                process_handle.as_raw_handle(),
                EXIT_INSTALLER_STILL_RUNNING as u32,
            )
        };
        unsafe { WaitForSingleObject(process_handle.as_raw_handle(), 30_000) };
        return Err(EXIT_INSTALLER_STILL_RUNNING);
    }
    if wait != WAIT_OBJECT_0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process_handle.as_raw_handle(), &mut exit_code) } == 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    Ok(exit_code)
}

fn spawn_staged_cleanup(directory: &Path, generation: &str) -> Result<(), i32> {
    let installed = medium_launcher_path()?;
    let suffix = directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    std::process::Command::new(installed)
        .arg(format!("--windows-update-cleanup-v1={suffix}:{generation}"))
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn verify_suspended_process(
    process: HANDLE,
    expected_path: &Path,
    expected_identity: FileIdentity,
    expected_hash: [u8; 32],
) -> Result<(), ()> {
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err(());
    }
    path.truncate(length as usize);
    let launched = PathBuf::from(String::from_utf16(&path).map_err(|_| ())?);
    if !paths_equal(&launched, expected_path) {
        return Err(());
    }
    let mut reopened = open_locked(&launched).map_err(|_| ())?;
    if file_identity(&reopened).map_err(|_| ())? != expected_identity
        || hash_file(&mut reopened).map_err(|_| ())? != expected_hash
    {
        return Err(());
    }
    Ok(())
}

fn open_locked(path: &Path) -> std::io::Result<File> {
    let raw = unsafe {
        CreateFileW(
            wide_nul(path)
                .map_err(|_| std::io::Error::other("invalid installer path"))?
                .as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::other(
            "installer is not a plain regular file",
        ));
    }
    Ok(file)
}

fn file_identity(file: &File) -> std::io::Result<FileIdentity> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        index_high: info.nFileIndexHigh,
        index_low: info.nFileIndexLow,
    })
}

fn hash_file(file: &mut File) -> std::io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(digest.finalize().into())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let text = path.as_os_str().to_string_lossy();
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
    normalized(left).eq_ignore_ascii_case(&normalized(right))
}

fn wide_nul(path: &Path) -> Result<Vec<u16>, i32> {
    use std::os::windows::ffi::OsStrExt;
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.is_empty() || value.contains(&0) {
        return Err(EXIT_INVALID_REQUEST);
    }
    value.push(0);
    Ok(value)
}

fn decode_hash(value: &str) -> Option<[u8; 32]> {
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(output)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, i32> {
    if value.is_empty() || !value.len().is_multiple_of(4) || value.len() > 64 * 1024 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    for chunk in value.as_bytes().chunks_exact(4) {
        let a = base64_value(chunk[0]).ok_or(EXIT_INVALID_REQUEST)?;
        let b = base64_value(chunk[1]).ok_or(EXIT_INVALID_REQUEST)?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2]).ok_or(EXIT_INVALID_REQUEST)?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3]).ok_or(EXIT_INVALID_REQUEST)?
        };
        if chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(EXIT_INVALID_REQUEST);
        }
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

fn base64_value(value: u8) -> Option<u8> {
    match value {
        b'A'..=b'Z' => Some(value - b'A'),
        b'a'..=b'z' => Some(value - b'a' + 26),
        b'0'..=b'9' => Some(value - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MEDIUM_LAUNCHER_DIRECTORY_SDDL, PersistedRelaunchRecord, RECOVERY_LAUNCHER_PENDING_PREFIX,
        RUN_ONCE_VALUE_PREFIX, StagedDirectoryGuard, UpdateAuthorization, UpdateCandidate,
        UpdatePredecessor, UpdateRole, authorization_transcript, canonical_candidate_layout,
        create_directory_with_sddl, create_restricted_directory, decode_base64,
        decode_pending_delete_pairs, publish_relaunch_record, read_persisted_relaunch_record,
        reclaim_incomplete_launcher_directories, reclaim_incomplete_recovery_directories,
        recovery_value_name, remove_relaunch_record_directory, validate_generation,
        write_persisted_relaunch_record,
    };
    #[test]
    fn pending_delete_pairs_preserve_final_empty_destinations_and_terminators() {
        let encode = |values: &[&str], final_terminator: bool| {
            let mut data = Vec::new();
            for value in values {
                data.extend(value.encode_utf16());
                data.push(0);
            }
            if final_terminator {
                data.push(0);
            }
            data
        };
        assert_eq!(
            decode_pending_delete_pairs(&encode(&[r"\??\C:\old.exe", ""], true)).unwrap(),
            vec![(r"\??\C:\old.exe".into(), String::new())]
        );
        assert_eq!(
            decode_pending_delete_pairs(&encode(
                &[r"\??\C:\a.exe", "", r"\??\C:\b.exe", r"\??\C:\c.exe"],
                true,
            ))
            .unwrap(),
            vec![
                (r"\??\C:\a.exe".into(), String::new()),
                (r"\??\C:\b.exe".into(), r"\??\C:\c.exe".into()),
            ]
        );
        assert!(decode_pending_delete_pairs(&encode(&[r"\??\C:\old.exe", ""], false)).is_err());
        assert!(decode_pending_delete_pairs(&[0]).is_err());
        assert_eq!(decode_pending_delete_pairs(&[0, 0]).unwrap(), Vec::new());
    }

    #[test]
    fn relaunch_record_marker_and_each_phase_are_power_loss_safe() {
        let generation = super::new_recovery_generation().unwrap();
        let identity = super::current_relaunch_identity().unwrap();
        let root =
            std::env::temp_dir().join(format!("tq-relaunch-record-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        *super::TEST_RELAUNCH_ROOT.lock().unwrap() = Some(root.clone());
        let mut record = PersistedRelaunchRecord {
            schema_version: 3,
            generation: generation.clone(),
            user_sid: identity.user_sid,
            logon_sid: identity.logon_sid,
            request: "--windows-update-bootstrap-v2=dGVzdA==".into(),
            nonce: "11".repeat(16),
            source_version: "0.0.69".into(),
            target_version: "0.0.70".into(),
            phase: "armed".into(),
            completed_version: None,
            recovery_generation: generation.clone(),
            predecessor: super::InstalledManifest {
                version: "0.0.69".into(),
                platform: "win32".into(),
                architecture: "x64".into(),
                source_commit: "a".repeat(40),
                source_tree: "b".repeat(40),
                release_build_digest: "c".repeat(64),
                roles: Vec::new(),
            },
        };
        publish_relaunch_record(&record).unwrap();
        for phase in [
            "setup-started",
            "setup-complete",
            "launch-started",
            "app-ready",
        ] {
            record.phase = phase.into();
            if phase == "setup-complete" {
                record.completed_version = Some("0.0.69".into());
            }
            write_persisted_relaunch_record(
                &super::relaunch_generation_directory(&generation).unwrap(),
                &record,
            )
            .unwrap();
            assert_eq!(
                read_persisted_relaunch_record(&generation).unwrap().phase,
                phase
            );
        }
        record.schema_version = 2;
        write_persisted_relaunch_record(
            &super::relaunch_generation_directory(&generation).unwrap(),
            &record,
        )
        .unwrap();
        assert!(read_persisted_relaunch_record(&generation).is_err());
        record.schema_version = 3;
        write_persisted_relaunch_record(
            &super::relaunch_generation_directory(&generation).unwrap(),
            &record,
        )
        .unwrap();
        remove_relaunch_record_directory(
            &super::relaunch_generation_directory(&generation).unwrap(),
        )
        .unwrap();
        *super::TEST_RELAUNCH_ROOT.lock().unwrap() = None;
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn terminated_bootstrap_publish_seams_leave_only_reclaimable_owned_residue() {
        let root_variable = "TQ_TERMINATED_BOOTSTRAP_TEST_ROOT";
        let seam_variable = "TQ_TERMINATED_BOOTSTRAP_TEST_SEAM";
        if let (Some(root), Some(seam)) = (
            std::env::var_os(root_variable),
            std::env::var_os(seam_variable),
        ) {
            let root = std::path::PathBuf::from(root);
            let seam = seam.to_string_lossy();
            let pending = root.join(format!(
                ".Talking Quill.update-bootstrap-pending-{}",
                "11".repeat(16)
            ));
            create_restricted_directory(&pending).unwrap();
            let mut guard = StagedDirectoryGuard::new(pending.clone()).unwrap();
            if seam != "directory-created" {
                guard.persist_identity_marker().unwrap();
            }
            if matches!(seam.as_ref(), "payload-durable" | "directory-published") {
                let mut payload = std::fs::File::create(pending.join("payload")).unwrap();
                std::io::Write::write_all(&mut payload, b"durable payload").unwrap();
                payload.sync_all().unwrap();
            }
            if seam == "directory-published" {
                guard
                    .publish(root.join(".Talking Quill.update-bootstrap-1122334455667788"))
                    .unwrap();
            }
            let mut marker = std::fs::File::create(root.join(format!("{seam}.durable"))).unwrap();
            std::io::Write::write_all(&mut marker, seam.as_bytes()).unwrap();
            marker.sync_all().unwrap();
            std::mem::forget(guard);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        for seam in [
            "directory-created",
            "identity-durable",
            "payload-durable",
            "directory-published",
        ] {
            let root = std::env::temp_dir().join(format!(
                "tq-terminated-bootstrap-recovery-{}-{seam}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir(&root).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("terminated_bootstrap_publish_seams_leave_only_reclaimable_owned_residue")
                .arg("--nocapture")
                .env(root_variable, &root)
                .env(seam_variable, seam)
                .spawn()
                .unwrap();
            let durable = root.join(format!("{seam}.durable"));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !durable.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(durable.exists(), "{seam}");
            child.kill().unwrap();
            child.wait().unwrap();
            reclaim_incomplete_recovery_directories(&root).unwrap();
            assert_eq!(
                std::fs::read_dir(&root)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".Talking Quill.update-bootstrap-"))
                    .count(),
                0,
                "{seam}"
            );
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn launcher_pending_directories_are_reclaimed_before_the_identity_marker() {
        let root = std::env::temp_dir().join(format!(
            "tq-launcher-pending-recovery-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        for token in ["11".repeat(16), "22".repeat(16)] {
            let pending = root.join(format!("{RECOVERY_LAUNCHER_PENDING_PREFIX}{token}"));
            create_directory_with_sddl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap();
            super::apply_restricted_dacl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap();
            assert!(super::has_exact_security(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap());
        }
        reclaim_incomplete_launcher_directories(&root).unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminated_generation_commit_seams_never_publish_an_incomplete_run_record() {
        let root_variable = "TQ_GENERATION_COMMIT_TEST_ROOT";
        let seam_variable = "TQ_GENERATION_COMMIT_TEST_SEAM";
        let operations = ["retry", "binding", "active", "run"];
        if let (Some(root), Some(seam)) = (
            std::env::var_os(root_variable),
            std::env::var_os(seam_variable),
        ) {
            let root = std::path::PathBuf::from(root);
            let seam = seam.to_string_lossy();
            for operation in operations {
                let mut file = std::fs::File::create(root.join(operation)).unwrap();
                std::io::Write::write_all(&mut file, operation.as_bytes()).unwrap();
                file.sync_all().unwrap();
                if operation == seam {
                    std::fs::File::create(root.join("durable"))
                        .unwrap()
                        .sync_all()
                        .unwrap();
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            }
            unreachable!();
        }
        for seam in operations {
            let root = std::env::temp_dir().join(format!(
                "tq-generation-commit-{}-{seam}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir(&root).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("terminated_generation_commit_seams_never_publish_an_incomplete_run_record")
                .arg("--nocapture")
                .env(root_variable, &root)
                .env(seam_variable, seam)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !root.join("durable").exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(root.join("durable").exists(), "{seam}");
            child.kill().unwrap();
            child.wait().unwrap();
            if root.join("run").exists() {
                for prerequisite in ["retry", "binding", "active"] {
                    assert!(root.join(prerequisite).exists(), "{seam}: {prerequisite}");
                }
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn every_generation_commit_interruption_preserves_publish_order() {
        let operations = ["retry", "binding", "active", "run"];
        for interrupted_after in 0..=operations.len() {
            let durable = &operations[..interrupted_after];
            if durable.contains(&"active") {
                assert!(durable.contains(&"retry"));
                assert!(durable.contains(&"binding"));
            }
            if durable.contains(&"run") {
                assert_eq!(durable, operations);
            }
        }
        let generation = "00112233445566778899aabbccddeeff";
        let cleanup = format!("--windows-update-cleanup-v1=1122334455667788:{generation}");
        let resume = format!("--windows-update-resume-v2={generation}");
        assert!(cleanup.ends_with(generation));
        assert!(resume.ends_with(generation));
    }

    #[test]
    fn run_once_generations_are_unique_and_stale_deletion_cannot_name_a_successor() {
        let first = "00112233445566778899aabbccddeeff";
        let second = "ffeeddccbbaa99887766554433221100";
        let first_name = recovery_value_name(first).unwrap();
        let second_name = recovery_value_name(second).unwrap();
        assert_ne!(first_name, second_name);
        assert!(first_name.starts_with(RUN_ONCE_VALUE_PREFIX));
        assert!(second_name.starts_with(RUN_ONCE_VALUE_PREFIX));
        let mut simulated_registry = std::collections::BTreeMap::from([
            (first_name.clone(), "running"),
            (second_name.clone(), "successor"),
        ]);
        simulated_registry.remove(&first_name);
        assert_eq!(simulated_registry.get(&second_name), Some(&"successor"));
        assert!(validate_generation(first).is_ok());
        assert!(validate_generation("0011").is_err());
    }

    #[test]
    fn prelaunch_guard_removes_the_exact_abandoned_tree() {
        let root =
            std::env::temp_dir().join(format!("tq-staged-bootstrap-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        {
            let _guard = StagedDirectoryGuard::new(root.clone()).unwrap();
            std::fs::write(root.join("partial"), b"partial").unwrap();
        }
        assert!(!root.exists());
    }

    fn candidate() -> UpdateCandidate {
        UpdateCandidate {
            version: "1.2.3".into(),
            platform: "win".into(),
            architecture: "x64".into(),
            owner_mode: "local-unsigned-enabled".into(),
            package_mode: "update".into(),
            source_commit: "66".repeat(20),
            source_tree: "77".repeat(20),
            release_build_digest: String::new(),
            package_layout_digest: String::new(),
            package_sha256: "aa".repeat(32),
            channel: "latest-x64".into(),
            transaction_binding: "source-target-package-sha256-v1".into(),
            roles: vec![
                UpdateRole {
                    role: "gateway".into(),
                    path: "resources/helper/talking-quill-helper.exe".into(),
                    sha256: "11".repeat(32),
                    suppression_capable: false,
                },
                UpdateRole {
                    role: "owner".into(),
                    path: "resources/helper/talking-quill-keyboard-owner.exe".into(),
                    sha256: "22".repeat(32),
                    suppression_capable: true,
                },
                UpdateRole {
                    role: "recovery-launcher".into(),
                    path: "resources/helper/talking-quill-update-recovery-launcher.exe".into(),
                    sha256: "33".repeat(32),
                    suppression_capable: false,
                },
            ],
            predecessor: UpdatePredecessor {
                platform: "win".into(),
                architecture: "x64".into(),
                version: "1.2.2".into(),
                release_build_digest: "33".repeat(32),
                gateway_sha256: "44".repeat(32),
                owner_sha256: "55".repeat(32),
            },
            authorization: UpdateAuthorization {
                scheme: "p256-sha256-v1".into(),
                signature: String::new(),
                verification_key_sha256: None,
            },
        }
    }

    #[test]
    fn candidate_role_and_predecessor_layout_matches_the_javascript_contract() {
        assert_eq!(
            canonical_candidate_layout(&candidate()).unwrap(),
            "edd8c82885e70192cf07b0fbf006e74734ed8009df89a8fe0fe8529369156d30"
        );
    }

    #[test]
    fn signed_candidate_authorization_rejects_mutated_outer_bytes() {
        use p256::ecdsa::{SigningKey, signature::Signer, signature::Verifier};

        let mut value = candidate();
        let signing_key = SigningKey::from_bytes((&[7_u8; 32]).into()).unwrap();
        let signature: p256::ecdsa::Signature =
            signing_key.sign(&authorization_transcript(&value).unwrap());
        signing_key
            .verifying_key()
            .verify(&authorization_transcript(&value).unwrap(), &signature)
            .unwrap();
        value.package_sha256 = "ab".repeat(32);
        assert!(
            signing_key
                .verifying_key()
                .verify(&authorization_transcript(&value).unwrap(), &signature)
                .is_err()
        );
    }

    #[test]
    fn base64_request_decoder_is_strict() {
        assert_eq!(
            decode_base64("eyJ2ZXJzaW9uIjoxfQ==").unwrap(),
            br#"{"version":1}"#
        );
        assert!(decode_base64("abc").is_err());
        assert!(decode_base64("AA=A").is_err());
    }
}
