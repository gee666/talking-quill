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
use serde::Deserialize;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{
    GetLastError, HANDLE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SetFileSecurityW, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, FlushFileBuffers,
    GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegEnumValueW, RegFlushKey, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateMutexW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess,
    OpenProcessToken, PROCESS_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex, ResumeThread,
    STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHGetKnownFolderPath, ShellExecuteExW,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RelaunchIntent {
    schema_version: u8,
    nonce: String,
    source_version: String,
    target_version: String,
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

#[derive(Deserialize)]
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

#[derive(Deserialize)]
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

    fn register_prelaunch_cleanup(&mut self, installed_helper: &Path) -> Result<(), i32> {
        let generation = new_recovery_generation()?;
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
            launch_program_files_application()?;
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
    if argument.starts_with("--windows-update-bootstrap-v2=") {
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

fn run_relaunch_wrapper(encoded: &str) -> Result<u32, i32> {
    if encoded.len() > 16_384 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let wrapper: RelaunchWrapper =
        serde_json::from_slice(&decode_base64(encoded)?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if !wrapper
        .request
        .starts_with("--windows-update-bootstrap-v2=")
        || wrapper.request.len() > 12_288
        || !valid_nonce(&wrapper.nonce)
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    let intent_path = PathBuf::from(&wrapper.intent_path);
    if !intent_path.is_absolute()
        || intent_path.file_name().and_then(|value| value.to_str())
            != Some("windows-update-relaunch-intent-v1.json")
    {
        return Err(EXIT_INVALID_REQUEST);
    }
    match launch_elevated_bootstrap(&wrapper.request) {
        Ok(()) => {
            launch_updated_application()?;
            Ok(0)
        }
        Err(code) => {
            if code == 1223 {
                let _ = consume_relaunch_intent(&intent_path, &wrapper.nonce);
            }
            Err(code)
        }
    }
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

fn consume_relaunch_intent(path: &Path, expected_nonce: &str) -> Result<(), i32> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if !metadata.is_file()
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
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    std::fs::remove_file(path).map_err(|_| EXIT_LAUNCH_FAILED)
}

fn launch_updated_application() -> Result<(), i32> {
    let helper = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    if helper
        .file_name()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("talking-quill-helper.exe"))
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let root = helper
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    launch_application(root.join("Talking Quill.exe"))
}

fn launch_program_files_application() -> Result<(), i32> {
    launch_application(
        known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill/Talking Quill.exe"),
    )
}

fn launch_application(application: PathBuf) -> Result<(), i32> {
    let metadata = std::fs::symlink_metadata(&application).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    std::process::Command::new(application)
        .spawn()
        .map(|_| ())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

fn launch_elevated_bootstrap(argument: &str) -> Result<(), i32> {
    let executable = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let verb = wide_nul(Path::new("runas"))?;
    let file = wide_nul(&executable)?;
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
    let suffix = argument
        .strip_prefix("--windows-update-bootstrap-v2=")
        .ok_or(EXIT_INVALID_REQUEST)?;
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
    directory_guard.register_prelaunch_cleanup(&recovery_launcher)?;
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
    )
}

fn parse_and_authorize_request(encoded: &str) -> Result<UpdateRequest, i32> {
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
    verify_update_relation(&request.candidate, &request.sha256)?;
    verify_update_authorization(&request.candidate)?;
    Ok(request)
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
        || candidate.roles.len() != 2
        || installed.roles.len() != 2
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
    let installed_gateway = update_role(&installed.roles, "gateway")?;
    let installed_owner = update_role(&installed.roles, "owner")?;
    if installed_gateway.path != "resources/helper/talking-quill-helper.exe"
        || installed_gateway.suppression_capable
        || installed_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !installed_owner.suppression_capable
        || !digest(&installed_gateway.sha256)
        || !digest(&installed_owner.sha256)
        || candidate_gateway.path != "resources/helper/talking-quill-helper.exe"
        || candidate_gateway.suppression_capable
        || !digest(&candidate_gateway.sha256)
        || candidate_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !candidate_owner.suppression_capable
        || !digest(&candidate_owner.sha256)
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
    let mut owner_file = open_locked(
        &current
            .parent()
            .ok_or(EXIT_IDENTITY_MISMATCH)?
            .join(owner_name),
    )
    .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut gateway_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
        != decode_hash(&installed_gateway.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        || hash_file(&mut owner_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&installed_owner.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
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

fn ensure_medium_launcher(installed_helper: &Path) -> Result<PathBuf, i32> {
    let source = installed_helper
        .parent()
        .ok_or(EXIT_IDENTITY_MISMATCH)?
        .join(RECOVERY_LAUNCHER_NAME);
    let mut source_file = open_locked(&source).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let source_hash = hash_file(&mut source_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let directory = medium_launcher_directory()?;
    reclaim_incomplete_launcher_directories(directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?)?;
    if !directory.exists() {
        publish_medium_launcher_directory(&directory, &mut source_file, source_hash)?;
        return Ok(directory.join(RECOVERY_LAUNCHER_NAME));
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
    let target = directory.join(RECOVERY_LAUNCHER_NAME);
    if target.exists() {
        let mut existing = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut existing).map_err(|_| EXIT_IDENTITY_MISMATCH)? == source_hash
            && has_exact_security(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Ok(target);
        }
    }
    let temporary = directory.join(format!(
        ".{RECOVERY_LAUNCHER_NAME}.tmp-{}",
        std::process::id()
    ));
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
        let launcher = pending.join(RECOVERY_LAUNCHER_NAME);
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

fn medium_launcher_path() -> Result<PathBuf, i32> {
    Ok(medium_launcher_directory()?.join(RECOVERY_LAUNCHER_NAME))
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
                    return Ok(Self {
                        _legacy: legacy,
                        file,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(EXIT_LAUNCH_FAILED),
            }
        }
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
    let published = read_registry_string(key, MACHINE_LOCK_REGISTRY_VALUE)?;
    let published = if let Some(retired) = published
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_generation(retired)?;
        reclaim_retired_machine_lock_directory(&root, retired)?;
        None
    } else {
        published
    };
    let directory = if let Some(suffix) = published {
        validate_generation(&suffix)?;
        root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH {
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

fn reclaim_retired_machine_lock_directory(root: &Path, suffix: &str) -> Result<(), i32> {
    let path = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path.exists() {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match remove_owned_tree(&path, &identity) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return Err(EXIT_LAUNCH_FAILED),
        }
    }
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
    if first != 0 || kind != REG_SZ || !(2..=256).contains(&bytes) || !bytes.is_multiple_of(2) {
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
        MEDIUM_LAUNCHER_DIRECTORY_SDDL, RECOVERY_LAUNCHER_PENDING_PREFIX, RUN_ONCE_VALUE_PREFIX,
        StagedDirectoryGuard, UpdateAuthorization, UpdateCandidate, UpdatePredecessor, UpdateRole,
        authorization_transcript, canonical_candidate_layout, consume_relaunch_intent,
        create_directory_with_sddl, create_restricted_directory, decode_base64,
        reclaim_incomplete_launcher_directories, reclaim_incomplete_recovery_directories,
        recovery_value_name, validate_generation,
    };
    #[test]
    fn relaunch_intent_is_nonce_bound_and_consumed_once() {
        let path =
            std::env::temp_dir().join(format!("tq-relaunch-intent-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let nonce = "11".repeat(16);
        std::fs::write(
            &path,
            format!(
                "{{\"schemaVersion\":1,\"nonce\":\"{nonce}\",\"sourceVersion\":\"0.0.69\",\"targetVersion\":\"0.0.70\"}}"
            ),
        )
        .unwrap();
        assert!(consume_relaunch_intent(&path, &"22".repeat(16)).is_err());
        assert!(path.exists());
        consume_relaunch_intent(&path, &nonce).unwrap();
        assert!(!path.exists());
        assert!(consume_relaunch_intent(&path, &nonce).is_err());
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
            "77976155fb7b3d468eafa5bd2d5efc8419a969f09df0fe17e69ba08dde2d4892"
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
