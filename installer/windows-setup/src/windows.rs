use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use std::{mem, ptr};

use hmac::{Hmac, Mac};
use p256::ecdh::diffie_hellman;
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::{PublicKey, SecretKey};
use sha2::{Digest, Sha256};
use sha2_10::Sha256 as Sha256V10;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{IExecAction, ITaskService, TaskScheduler};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};
#[cfg(all(
    feature = "stale-schema2-cleanup",
    any(test, feature = "machine-lock-test-namespace")
))]
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_CANCELLED, ERROR_IO_PENDING,
    ERROR_PIPE_CONNECTED, ERROR_SERVICE_SPECIFIC_ERROR, GetHandleInformation, GetLastError,
    INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount,
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    SECURITY_ATTRIBUTES, SetFileSecurityW, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TOKEN_STATISTICS, TOKEN_USER, TokenElevation, TokenIntegrityLevel, TokenSessionId,
    TokenStatistics, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_RENAME_INFO,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo,
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, FlushFileBuffers,
    GetFileInformationByHandle, GetFileInformationByHandleEx, MOVEFILE_DELAY_UNTIL_REBOOT,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SYNCHRONIZE, SetFileInformationByHandle, WriteFile,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_MULTI_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW,
    RegEnumKeyExW, RegEnumValueW, RegFlushKey, RegLoadAppKeyW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfig2W, QueryServiceConfigW,
    QueryServiceObjectSecurity, QueryServiceStatus, RegisterServiceCtrlHandlerW, SC_ACTION,
    SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_FAILURE_ACTIONS,
    SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, SERVICE_CONTROL_STOP, SERVICE_ERROR_NORMAL,
    SERVICE_FAILURE_ACTIONS_FLAG, SERVICE_FAILURE_ACTIONSW, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STOP,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceObjectSecurity,
    SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcess,
    OpenProcessToken, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, ReleaseMutex, TerminateProcess, WaitForMultipleObjects,
    WaitForSingleObject,
};
#[cfg(all(
    feature = "stale-schema2-cleanup",
    any(test, feature = "machine-lock-test-namespace")
))]
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetProcessTimes};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_RoamingAppData, FOLDERID_System,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDOK, IDYES, MB_ICONQUESTION, MB_OKCANCEL, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
};

use crate::owned_tree::{owned_tree_identity, remove_owned_tree};
use crate::package::{self, ParsedPackage};

#[cfg(any(
    not(any(test, feature = "machine-lock-test-namespace")),
    feature = "stale-schema2-cleanup"
))]
const MACHINE_LOCK_DIRECTORY_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const TEST_MACHINE_LOCK_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;AU)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const TEST_MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;AU)";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_directory_sddl() -> &'static str {
    TEST_MACHINE_LOCK_DIRECTORY_SDDL
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_directory_sddl() -> &'static str {
    MACHINE_LOCK_DIRECTORY_SDDL
}
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_file_sddl() -> &'static str {
    TEST_MACHINE_LOCK_FILE_SDDL
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_file_sddl() -> &'static str {
    MACHINE_LOCK_FILE_SDDL
}

const MACHINE_LOCK_RETIRED_PREFIX: &str = "retired:";
const MACHINE_LOCK_REGISTRY_KEY: &str = r"Software\Talking Quill\RecoveryStateLockV1";
const MACHINE_LOCK_REGISTRY_VALUE: &str = "DirectorySuffix";
const MACHINE_LOCK_DIRECTORY_PREFIX: &str = ".Talking Quill.machine-lock-";
const MACHINE_LOCK_PENDING_PREFIX: &str = ".Talking Quill.machine-lock-pending-";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
const MACHINE_LOCK_TEST_ID_ENV: &str = "TQ_MACHINE_LOCK_TEST_NAMESPACE_ID";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_test_id() -> Result<&'static str> {
    static ID: OnceLock<String> = OnceLock::new();
    let value = ID.get_or_init(|| {
        std::env::var(MACHINE_LOCK_TEST_ID_ENV)
            .expect("machine-lock tests require the wrapper namespace environment")
    });
    validate_machine_lock_suffix(value)?;
    Ok(value)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_registry_hive() -> HKEY {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_some() {
        HKEY_CURRENT_USER
    } else {
        HKEY_LOCAL_MACHINE
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_registry_hive() -> HKEY {
    HKEY_LOCAL_MACHINE
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_registry_key() -> Result<String> {
    match std::env::var_os(MACHINE_LOCK_TEST_ID_ENV) {
        Some(_) => Ok(format!(
            r"Software\Talking Quill Tests\{}\RecoveryStateLockV1",
            machine_lock_test_id()?
        )),
        None => Ok(MACHINE_LOCK_REGISTRY_KEY.to_owned()),
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_registry_key() -> Result<String> {
    Ok(MACHINE_LOCK_REGISTRY_KEY.to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_registry_parent() -> Result<String> {
    match std::env::var_os(MACHINE_LOCK_TEST_ID_ENV) {
        Some(_) => Ok(format!(
            r"Software\Talking Quill Tests\{}",
            machine_lock_test_id()?
        )),
        None => Ok(r"Software\Talking Quill".to_owned()),
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_registry_parent() -> Result<String> {
    Ok(r"Software\Talking Quill".to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_program_data(production: &Path) -> Result<PathBuf> {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_none() {
        return Ok(production.to_owned());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp/machine-lock-tests/windows-setup")
        .join(machine_lock_test_id()?);
    if !root.is_dir() {
        return Err(fail(EXIT_FAILURE, "machine-lock test outer root is absent"));
    }
    Ok(root)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_program_data(production: &Path) -> Result<PathBuf> {
    Ok(production.to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
fn machine_lock_mutex_names() -> Result<[String; 2]> {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_none() {
        return Ok([
            r"Global\TalkingQuill.NativeSetup.V2".to_owned(),
            r"Global\TalkingQuill.UpdateRecovery.State.V1".to_owned(),
        ]);
    }
    let id = machine_lock_test_id()?;
    Ok([
        format!(r"Local\TalkingQuill.Tests.{id}.NativeSetup.V2"),
        format!(r"Local\TalkingQuill.Tests.{id}.UpdateRecovery.State.V1"),
    ])
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
fn machine_lock_mutex_names() -> Result<[String; 2]> {
    Ok([
        r"Global\TalkingQuill.NativeSetup.V2".to_owned(),
        r"Global\TalkingQuill.UpdateRecovery.State.V1".to_owned(),
    ])
}

const TERMINAL_UNINSTALL_RECORD_NAME: &str = "terminal-uninstall-record-v1.json";
const TERMINAL_RECOVERY_TOMBSTONE_PREFIX: &str = ".Talking Quill.recovery-tombstone-";
const TERMINAL_FINAL_LAUNCHER_PREFIX: &str = ".Talking Quill Terminal Relaunch-";
const TERMINAL_SERVICE_PREFIX: &str = "TalkingQuillTerminalCleanup-";
const TERMINAL_SERVICE_IMAGE_PREFIX: &str = ".Talking Quill Terminal Cleanup-";
const TERMINAL_SERVICE_PENDING_PREFIX: &str = ".Talking Quill.terminal-cleanup-pending-";
const TERMINAL_SERVICE_FILE_SDDL: &str = MACHINE_LOCK_FILE_SDDL;
const TERMINAL_SERVICE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;LC;;;AU)";
const UNINSTALL_FINALIZER_PENDING_PREFIX: &str = ".Talking Quill.uninstall-finalizer-pending-";
const UNINSTALL_FINALIZER_PREFIX: &str = ".Talking Quill.uninstall-finalizer-";
const UNINSTALL_FINALIZER_NAME: &str = "Talking Quill Uninstall Finalizer.exe";
const MEDIUM_FINALIZER_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
const MEDIUM_FINALIZER_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;AU)";
const MEDIUM_LAUNCHER_DIRECTORY_SDDL: &str = MEDIUM_FINALIZER_DIRECTORY_SDDL;
const MEDIUM_LAUNCHER_FILE_SDDL: &str = MEDIUM_FINALIZER_FILE_SDDL;
const LEGACY_LOCK_RETIREMENT_EPOCH: u8 = 3;

const EXIT_USAGE: i32 = 64;
const EXIT_FAILURE: i32 = 70;
const EXIT_REJECTED: i32 = 78;
const EXIT_ELEVATION: i32 = 79;
const TRANSACTION_SCHEMA: u8 = 2;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: std::os::windows::io::RawHandle) -> i32;
    fn NtResumeProcess(process: std::os::windows::io::RawHandle) -> i32;
}

static TERMINAL_SERVICE_GENERATION: OnceLock<String> = OnceLock::new();

pub fn run() -> i32 {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1
        && let Some(generation) = arguments[0]
            .to_str()
            .and_then(|value| value.strip_prefix("/TQ-TERMINAL-SERVICE="))
    {
        return run_terminal_service_dispatcher(generation).unwrap_or_else(|error| error.code);
    }
    // Internal relocated and elevated roles never own UI. Authentication still
    // determines operation authority inside run_inner.
    #[cfg(feature = "stale-schema2-cleanup")]
    let stale_schema2_silent = arguments
        .iter()
        .any(|value| value == "/TQ-CLEAN-STALE-SCHEMA2" || value == "/TQ-DIAGNOSE-STALE-SCHEMA2");
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let stale_schema2_silent = false;
    let silent = stale_schema2_silent
        || arguments.iter().any(|value| {
            value == "/S" || value == "/TQ-RELOCATED" || value.to_string_lossy().starts_with("/TQ-")
        })
        || arguments
            .iter()
            .any(|value| value.to_string_lossy().starts_with("/TQUPDATE="));
    match run_inner() {
        Ok(code) => code,
        Err(error) => {
            if !silent {
                report(&error.message);
            }
            error.code
        }
    }
}

#[derive(Debug)]
struct SetupError {
    code: i32,
    message: String,
}
type Result<T> = std::result::Result<T, SetupError>;
fn fail(code: i32, message: impl Into<String>) -> SetupError {
    SetupError {
        code,
        message: message.into(),
    }
}

#[cfg(feature = "acceptance-faults")]
fn take_terminal_acceptance_fault(phase: &str) -> Result<bool> {
    const KEY: &str = r"Software\Talking Quill\AcceptanceTerminalFault";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(false);
    }
    if opened != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal acceptance fault.",
        ));
    }
    let selected = read_registry_value(key, "Phase", 128)?;
    unsafe { RegCloseKey(key) };
    if selected.as_deref() != Some(phase) {
        return Ok(false);
    }
    let deleted = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(OsStr::new(KEY)).as_ptr()) };
    let mut parent = ptr::null_mut();
    let flushed = deleted == 0
        && unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                wide(OsStr::new(r"Software")).as_ptr(),
                0,
                KEY_READ,
                &mut parent,
            )
        } == 0
        && unsafe { RegFlushKey(parent) } == 0;
    if !parent.is_null() {
        unsafe { RegCloseKey(parent) };
    }
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot consume terminal acceptance fault.",
        ));
    }
    Ok(true)
}

#[cfg(feature = "acceptance-faults")]
fn terminal_maintenance_crash_at(phase: &str) {
    if std::env::var("TQ_TERMINAL_FAULT").as_deref() == Ok(phase)
        || take_terminal_acceptance_fault(phase).unwrap_or(false)
    {
        std::process::exit(197);
    }
}

#[cfg(not(feature = "acceptance-faults"))]
fn terminal_maintenance_crash_at(_phase: &str) {}

#[cfg(feature = "acceptance-faults")]
fn terminal_force_pending_delete() -> bool {
    std::env::var("TQ_TERMINAL_FAULT").as_deref() == Ok("reboot-pending-delete")
        || take_terminal_acceptance_fault("reboot-pending-delete").unwrap_or(false)
}

#[cfg(not(feature = "acceptance-faults"))]
fn terminal_force_pending_delete() -> bool {
    false
}

#[cfg(feature = "acceptance-faults")]
fn terminal_service_fail_once(phase: &str) -> Result<()> {
    if take_terminal_acceptance_fault(phase)? {
        Err(fail(EXIT_FAILURE, "Injected terminal service failure."))
    } else {
        Ok(())
    }
}

#[cfg(not(feature = "acceptance-faults"))]
fn terminal_service_fail_once(_phase: &str) -> Result<()> {
    Ok(())
}

fn terminal_service_name(generation: &str) -> Result<String> {
    validate_machine_lock_suffix(generation)?;
    Ok(format!("{TERMINAL_SERVICE_PREFIX}{generation}"))
}

fn run_terminal_service_dispatcher(generation: &str) -> Result<i32> {
    validate_machine_lock_suffix(generation)?;
    TERMINAL_SERVICE_GENERATION
        .set(generation.to_owned())
        .map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Terminal service generation was already set.",
            )
        })?;
    let mut service_name = wide(OsStr::new(&terminal_service_name(generation)?));
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_mut_ptr(),
            lpServiceProc: Some(terminal_service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Terminal cleanup service dispatcher failed.",
        ));
    }
    Ok(0)
}

unsafe extern "system" fn terminal_service_control(_control: u32) {}

unsafe extern "system" fn terminal_service_main(_argc: u32, _argv: *mut *mut u16) {
    let Some(generation) = TERMINAL_SERVICE_GENERATION.get() else {
        return;
    };
    let service_name = match terminal_service_name(generation) {
        Ok(value) => value,
        Err(_) => return,
    };
    let handle = unsafe {
        RegisterServiceCtrlHandlerW(
            wide(OsStr::new(&service_name)).as_ptr(),
            Some(terminal_service_control),
        )
    };
    if handle.is_null() {
        return;
    }
    let mut status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_START_PENDING,
        dwControlsAccepted: 0,
        dwWin32ExitCode: 0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 1,
        dwWaitHint: 120_000,
    };
    unsafe { SetServiceStatus(handle, &status) };
    status.dwCurrentState = SERVICE_RUNNING;
    status.dwCheckPoint = 0;
    status.dwWaitHint = 0;
    unsafe { SetServiceStatus(handle, &status) };
    let result = run_terminal_cleanup_service(generation);
    status.dwCurrentState = SERVICE_STOPPED;
    match result {
        Ok(()) => {
            status.dwWin32ExitCode = 0;
            status.dwServiceSpecificExitCode = 0;
        }
        Err(error) => {
            status.dwWin32ExitCode = ERROR_SERVICE_SPECIFIC_ERROR;
            status.dwServiceSpecificExitCode = error.code as u32;
        }
    }
    unsafe { SetServiceStatus(handle, &status) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Install,
    Update,
    Repair,
    Uninstall,
    #[cfg(feature = "stale-schema2-cleanup")]
    CleanStaleSchema2,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Transaction {
    schema_version: u8,
    phase: String,
    action: String,
    had_predecessor: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
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

struct Paths {
    install: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
    transaction: PathBuf,
    maintenance_generation_record: PathBuf,
    maintenance_uninstaller: PathBuf,
    recovery_launcher: PathBuf,
    profile: PathBuf,
    legacy_authority: PathBuf,
    legacy_quarantine: PathBuf,
    legacy_task_file: PathBuf,
    program_data: PathBuf,
}

#[cfg(feature = "stale-schema2-cleanup")]
fn direct_cleanup_arguments(arguments: &[OsString], elevated: bool) -> bool {
    elevated && arguments.len() == 1 && arguments[0] == "/TQ-CLEAN-STALE-SCHEMA2"
}

#[cfg(feature = "stale-schema2-cleanup")]
fn direct_diagnostic_arguments(arguments: &[OsString]) -> bool {
    arguments.len() == 1 && arguments[0] == "/TQ-DIAGNOSE-STALE-SCHEMA2"
}

#[cfg(feature = "stale-schema2-cleanup")]
const fn direct_cleanup_token_is_authorized(elevated: bool, integrity_rid: u32) -> bool {
    const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;
    elevated && integrity_rid >= SECURITY_MANDATORY_HIGH_RID
}

mod controller;
use controller::*;
mod completion;
use completion::{COMPLETION_BYTES, decode_completion, encode_completion};

mod worker;
use worker::*;

mod marker_security;
use marker_security::marker_security_is_exact;

mod machine_lock;
use machine_lock::*;

mod channel;
use channel::*;

mod pipe_io;
#[cfg(test)]
use pipe_io::deadline_ms;
use pipe_io::{pipe_connect, pipe_read, pipe_write};
mod peer_identity;
use peer_identity::verify_peer_claims;

mod authorization;
use authorization::*;

mod installation;
use installation::*;

mod recovery;
use recovery::*;

mod registration;
use registration::*;

mod update_recovery;
use update_recovery::*;

mod terminal_record;
use terminal_record::*;

mod terminal_service;
use terminal_service::*;

mod legacy_profiles;
use legacy_profiles::*;

mod terminal_finalizer;
use terminal_finalizer::*;

mod pending_deletion;
use pending_deletion::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod namespace_inventory;
#[cfg(feature = "stale-schema2-cleanup")]
use namespace_inventory::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod registry_security;
#[cfg(feature = "stale-schema2-cleanup")]
use registry_security::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod stale_objects;
#[cfg(feature = "stale-schema2-cleanup")]
use stale_objects::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod stale_audit;
#[cfg(feature = "stale-schema2-cleanup")]
use stale_audit::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod stale_diagnostic;
#[cfg(feature = "stale-schema2-cleanup")]
use stale_diagnostic::*;

#[cfg(feature = "stale-schema2-cleanup")]
mod stale_cleanup;
#[cfg(feature = "stale-schema2-cleanup")]
use stale_cleanup::*;

mod filesystem;
use filesystem::*;

#[cfg(test)]
mod tests;

mod runtime_process;
use runtime_process::{process_image_from_handle, request_runtime_exit, terminate_tree_processes};

mod legacy_cleanup;
use legacy_cleanup::retire_legacy_authority;
#[cfg(test)]
use legacy_cleanup::{owned_legacy_executable, service_executable};

#[cfg(feature = "stale-schema2-cleanup")]
use std::io::{Seek, SeekFrom};

#[cfg(feature = "stale-schema2-cleanup")]
use windows_sys::Win32::Security::Authorization::SE_REGISTRY_KEY;

#[cfg(feature = "stale-schema2-cleanup")]
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorGroup, GetSecurityDescriptorOwner,
};

#[cfg(feature = "stale-schema2-cleanup")]
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_WRITE_THROUGH, WRITE_DAC, WRITE_OWNER};

#[cfg(feature = "stale-schema2-cleanup")]
use windows_sys::Win32::System::Registry::{REG_OPTION_OPEN_LINK, RegSetKeySecurity};

#[cfg(feature = "stale-schema2-cleanup")]
use crate::owned_tree::retained_directory_names;

#[cfg(all(test, feature = "stale-schema2-cleanup"))]
mod stale_tests;

#[cfg(feature = "stale-schema2-cleanup")]
use windows_sys::Win32::Security::GROUP_SECURITY_INFORMATION;
