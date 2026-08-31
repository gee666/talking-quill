#![cfg(windows)]

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::owned_tree::{owned_tree_identity, remove_owned_tree};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{HANDLE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SetFileSecurityW, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_READ, GetFileInformationByHandle,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteValueW, RegFlushKey, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    PROCESS_INFORMATION, QueryFullProcessImageNameW, ResumeThread, STARTUPINFOW, TerminateProcess,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHGetKnownFolderPath, ShellExecuteExW,
};

#[used]
static WINDOWS_UPDATE_PRIMARY_KEY_MARKER: &str = concat!(
    "TALKING_QUILL_WINDOWS_UPDATE_PRIMARY_KEY_V1=",
    env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1")
);
const RESTRICTED_STAGING_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const RESTRICTED_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";

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
}

impl StagedDirectoryGuard {
    fn new(path: PathBuf) -> Result<Self, i32> {
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_LAUNCH_FAILED)?;
        Ok(Self {
            path,
            identity,
            launched: false,
        })
    }

    fn transfer_to_launched_recovery(&mut self) {
        self.launched = true;
    }
}

impl Drop for StagedDirectoryGuard {
    fn drop(&mut self) {
        if !self.launched {
            let _ = remove_owned_tree(&self.path, &self.identity);
        }
    }
}

pub fn run_from_argument(argument: &std::ffi::OsStr) -> i32 {
    match run_from_argument_inner(argument) {
        Ok(code) => code as i32,
        Err(code) => code,
    }
}

fn run_from_argument_inner(argument: &std::ffi::OsStr) -> Result<u32, i32> {
    let argument = argument.to_str().ok_or(EXIT_INVALID_REQUEST)?;
    if (argument.starts_with("--windows-update-bootstrap-v2=")
        || argument == "--windows-update-resume-v2")
        && !is_elevated()
    {
        return launch_elevated_bootstrap(argument).map(|()| 0);
    }
    if !is_elevated() {
        return Err(EXIT_NOT_ELEVATED);
    }
    if let Some(encoded) = argument.strip_prefix("--windows-update-cleanup-v1=") {
        return run_native_cleanup(encoded).map(|()| 0);
    }
    if argument.starts_with("--windows-update-bootstrap-v2=") {
        return stage_bootstrap(argument).map(|()| 0);
    }
    let encoded = if argument == "--windows-update-resume-v2" {
        read_persisted_request()?
    } else {
        argument
            .strip_prefix("--windows-update-bootstrap-staged-v2=")
            .ok_or(EXIT_INVALID_REQUEST)?
            .to_owned()
    };
    let result = execute_staged_request(&encoded);
    // A failed or interrupted replacement may have moved the predecessor and must retain
    // this protected recovery owner across restart. Only a truthful successful setup exit
    // proves that staging is no longer needed.
    if result == Ok(0) || native_transaction_absent()? {
        clear_restart_recovery()?;
        schedule_staged_cleanup();
    }
    result
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
        return Err(EXIT_LAUNCH_FAILED);
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

fn run_native_cleanup(encoded: &str) -> Result<(), i32> {
    let bytes = decode_base64(encoded)?;
    let binding = String::from_utf8(bytes).map_err(|_| EXIT_INVALID_REQUEST)?;
    let (path, expected_identity) = binding.split_once('\0').ok_or(EXIT_INVALID_REQUEST)?;
    let path = PathBuf::from(path);
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    if path
        .parent()
        .is_none_or(|parent| !paths_equal(parent, &program_data))
        || path
            .file_name()
            .and_then(|value| value.to_str())
            .is_none_or(|name| {
                !name.starts_with(".Talking Quill.update-bootstrap-")
                    || name.len() != ".Talking Quill.update-bootstrap-".len() + 16
            })
    {
        return Err(EXIT_IDENTITY_MISMATCH);
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
            Ok(_) => match remove_owned_tree(&path, expected_identity) {
                Ok(()) => return Ok(()),
                Err(crate::owned_tree::OwnedTreeError::IdentityMismatch) => {
                    return Err(EXIT_IDENTITY_MISMATCH);
                }
                Err(_) => {}
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(EXIT_LAUNCH_FAILED)
}

fn execute_staged_request(encoded: &str) -> Result<u32, i32> {
    let request = parse_and_authorize_request(encoded)?;
    let expected_hash = decode_hash(&request.sha256).ok_or(EXIT_INVALID_REQUEST)?;
    let staged = copy_verified_installer(Path::new(&request.installer_path), expected_hash)?;
    persist_request(encoded)?;
    persist_restart_recovery()?;
    launch_verified_installer(&staged, expected_hash, &request.candidate)
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
    let random = getrandom::u64().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let directory = program_data.join(format!(".Talking Quill.update-bootstrap-{random:016x}"));
    create_restricted_directory(&directory)?;
    let mut directory_guard = StagedDirectoryGuard::new(directory.clone())?;
    let staged = directory.join("talking-quill-update-bootstrap.exe");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&staged)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&staged, RESTRICTED_FILE_SDDL)?;
    trusted
        .seek(SeekFrom::Start(0))
        .and_then(|_| std::io::copy(&mut trusted, &mut output).map(|_| ()))
        .and_then(|_| output.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    stage_predecessor_evidence(&current, &directory)?;
    let mut retained = open_locked(&staged).map_err(|_| EXIT_LAUNCH_FAILED)?;
    let expected_identity = file_identity(&retained).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut retained).map_err(|_| EXIT_LAUNCH_FAILED)? != expected_hash
        || file_identity(&trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != trusted_identity
        || hash_file(&mut trusted).map_err(|_| EXIT_IDENTITY_MISMATCH)? != expected_hash
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let staged_argument = format!("--windows-update-bootstrap-staged-v2={suffix}");
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
    directory_guard.transfer_to_launched_recovery();
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
    let descriptor = SecurityDescriptor::restricted(RESTRICTED_STAGING_SDDL)?;
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

fn native_transaction_absent() -> Result<bool, i32> {
    let path =
        known_folder(&FOLDERID_ProgramFiles)?.join(".Talking Quill.native-transaction-v2.json");
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Err(EXIT_LAUNCH_FAILED),
    }
}

const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const RUN_ONCE_VALUE: &str = "!Talking Quill Update Recovery";
const RECOVERY_REQUEST_FILE: &str = "update-recovery-request-v2.txt";

fn recovery_request_path() -> Result<PathBuf, i32> {
    Ok(std::env::current_exe()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .parent()
        .ok_or(EXIT_LAUNCH_FAILED)?
        .join(RECOVERY_REQUEST_FILE))
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

fn read_persisted_request() -> Result<String, i32> {
    let value =
        std::fs::read_to_string(recovery_request_path()?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if value.is_empty() || value.len() > 64 * 1024 {
        return Err(EXIT_INVALID_REQUEST);
    }
    Ok(value)
}

fn persist_restart_recovery() -> Result<(), i32> {
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let command = format!("\"{}\" --windows-update-resume-v2", current.display());
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
    let command = wide_nul(Path::new(&command))?;
    let status = unsafe {
        RegSetValueExW(
            key,
            wide_nul(Path::new(RUN_ONCE_VALUE))?.as_ptr(),
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

fn clear_restart_recovery() -> Result<(), i32> {
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
    let status = unsafe { RegDeleteValueW(key, wide_nul(Path::new(RUN_ONCE_VALUE))?.as_ptr()) };
    let flushed = (status == 0 || status == 2) && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

fn schedule_staged_cleanup() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let Some(directory) = current.parent().map(Path::to_owned) else {
        return;
    };
    if directory
        .file_name()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.starts_with(".Talking Quill.update-bootstrap-"))
    {
        return;
    }
    spawn_staged_cleanup(&directory);
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

fn spawn_staged_cleanup(directory: &Path) {
    let Ok(installed) = known_folder(&FOLDERID_ProgramFiles).map(|program_files| {
        program_files.join("Talking Quill/resources/helper/talking-quill-helper.exe")
    }) else {
        return;
    };
    let Ok(identity) = owned_tree_identity(directory) else {
        return;
    };
    let binding = format!("{}\0{}", directory.to_string_lossy(), identity);
    let encoded = encode_base64(binding.as_bytes());
    let _ = std::process::Command::new(installed)
        .arg(format!("--windows-update-cleanup-v1={encoded}"))
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn();
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    output
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
        StagedDirectoryGuard, UpdateAuthorization, UpdateCandidate, UpdatePredecessor, UpdateRole,
        authorization_transcript, canonical_candidate_layout, decode_base64,
    };

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
