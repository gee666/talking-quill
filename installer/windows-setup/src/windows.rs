use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
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
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_CANCELLED, ERROR_IO_PENDING,
    ERROR_PIPE_CONNECTED, ERROR_SERVICE_SPECIFIC_ERROR, GetHandleInformation, GetLastError,
    INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_REGISTRY_KEY,
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
    FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo,
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, FlushFileBuffers,
    GetFileInformationByHandle, GetFileInformationByHandleEx, MOVEFILE_DELAY_UNTIL_REBOOT,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SYNCHRONIZE, SetFileInformationByHandle, WRITE_DAC, WriteFile,
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
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_MULTI_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW,
    RegEnumKeyExW, RegEnumValueW, RegFlushKey, RegLoadAppKeyW, RegOpenKeyExW, RegQueryValueExW,
    RegSetKeySecurity, RegSetValueExW,
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
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_RoamingAppData, FOLDERID_System,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGetKnownFolderPath, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDOK, IDYES, MB_ICONQUESTION, MB_OKCANCEL, MB_YESNO, MessageBoxW,
};

use crate::owned_tree::{owned_tree_identity, remove_owned_tree, retained_directory_names};
use crate::package::{self, ParsedPackage};

const MACHINE_LOCK_DIRECTORY_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
const MACHINE_LOCK_RETIRED_PREFIX: &str = "retired:";
const MACHINE_LOCK_REGISTRY_KEY: &str = r"Software\Talking Quill\RecoveryStateLockV1";
const MACHINE_LOCK_REGISTRY_VALUE: &str = "DirectorySuffix";
const MACHINE_LOCK_DIRECTORY_PREFIX: &str = ".Talking Quill.machine-lock-";
const MACHINE_LOCK_PENDING_PREFIX: &str = ".Talking Quill.machine-lock-pending-";
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

fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_diagnostic_arguments(&arguments) {
        return run_direct_stale_schema2_diagnostic(&arguments)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    let elevated = token_is_elevated()?;
    let relocated = !elevated
        && arguments.iter().any(|value| value == "/TQ-RELOCATED")
        && arguments
            .iter()
            .all(|value| value == "/TQ-RELOCATED" || value == "/S");
    let legacy_predecessor = elevated && legacy_predecessor_arguments(&arguments);
    #[cfg(feature = "stale-schema2-cleanup")]
    let cleanup_requested =
        !elevated && arguments.len() == 1 && arguments[0] == "/TQ-CLEAN-STALE-SCHEMA2";
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let cleanup_requested = false;
    #[cfg(feature = "stale-schema2-cleanup")]
    let direct_cleanup_requested = direct_cleanup_arguments(&arguments, elevated);
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let direct_cleanup_requested = false;
    if !((arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "/S"))
        || legacy_predecessor
        || relocated
        || cleanup_requested
        || direct_cleanup_requested)
    {
        return Err(fail(EXIT_USAGE, "The native setup accepts only /S."));
    }
    let mut silent = arguments.first().is_some_and(|value| value == "/S") || cleanup_requested;
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_cleanup_requested {
        return run_direct_elevated_stale_schema2_cleanup()
            .map(|()| 0)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    if !elevated {
        let current =
            std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        #[cfg(feature = "stale-schema2-cleanup")]
        if cleanup_requested {
            let retained = retain_controller_image(&current, false)?;
            let channel =
                ControllerChannel::create(Action::CleanStaleSchema2, true, std::process::id())?;
            let result = elevate(&current, true, &channel, None);
            drop(retained);
            return result;
        }
        let controller_paths = paths()?;
        let retained = retain_controller_image(&current, relocated)?;
        let mut lifecycle_parent = 0;
        let mut relocation_server = None;
        let mut relocated_finalizer = false;
        let mut relocated_identity_guard = None;
        let action = if relocated {
            let original = process_image(parent_process_id()?)?;
            let installed = controller_paths.install.join("Uninstall Talking Quill.exe");
            let expected =
                if canonical(&original)? == canonical(&controller_paths.maintenance_uninstaller)? {
                    controller_paths.maintenance_uninstaller.clone()
                } else if canonical(&original)? == canonical(&installed)? {
                    installed
                } else if is_uninstall_finalizer(&original)? {
                    relocated_finalizer = true;
                    original.clone()
                } else {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Relocated uninstall source is not an authenticated maintenance image.",
                    ));
                };
            relocated_identity_guard = Some(validate_relocated_uninstall_image(
                &current,
                &original,
                &expected,
                &controller_paths.maintenance_uninstaller,
            )?);
            let (action, server, requested_silent, requested_lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&current, Some(&expected))?;
            silent = requested_silent;
            lifecycle_parent = requested_lifecycle_parent;
            relocation_server = Some(server);
            if action != Action::Uninstall {
                return Err(fail(
                    EXIT_REJECTED,
                    "Relocation requested an invalid operation.",
                ));
            }
            Action::Uninstall
        } else {
            derive_action(&current, &controller_paths)?
        };
        if action == Action::Uninstall && !relocated {
            request_runtime_exit(&controller_paths, None)?;
            let (path, lock) = create_relocated_image(&current)?;
            let channel = ControllerChannel::create(Action::Uninstall, silent, std::process::id())?;
            let process = launch_relocated(&path, &channel)?;
            drop(retained);
            let code = channel.wait_relocated_status(&process, &current)?;
            drop(lock);
            return Ok(code);
        }
        if !silent && !confirm_controller(action)? {
            return Ok(ERROR_CANCELLED as i32);
        }
        let delete_profile = !silent
            && action == Action::Uninstall
            && message_box(
                "Also delete your Talking Quill profile?",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES;
        if action != Action::Install {
            request_runtime_exit(
                &controller_paths,
                (lifecycle_parent != 0).then_some(lifecycle_parent),
            )?;
        }
        let retained = if relocated {
            drop(retained);
            retain_controller_image(&current, true)?
        } else {
            retained
        };
        let arm_deletion = || -> Result<()> {
            let server = relocation_server.as_ref().ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                b"TQ-ARM-DELETE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
            if pipe_read::<1>(
                server.as_raw_handle(),
                None,
                Instant::now() + Duration::from_secs(30),
            )? != [1]
            {
                return Err(fail(
                    EXIT_FAILURE,
                    "Installed uninstall image deletion was not armed.",
                ));
            }
            Ok(())
        };
        let before_accept = (relocated && lifecycle_parent != 0 && !relocated_finalizer)
            .then_some(&arm_deletion as &dyn Fn() -> Result<()>);
        if relocated_finalizer {
            pipe_write(
                relocation_server
                    .as_ref()
                    .ok_or_else(|| fail(EXIT_FAILURE, "Finalizer relocation channel is missing."))?
                    .as_raw_handle(),
                b"TQ-KEEP-IMAGE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        let channel = ControllerChannel::create(action, silent, lifecycle_parent)?;
        let result = elevate(&current, silent, &channel, before_accept);
        drop(relocated_identity_guard);
        drop(retained);
        if relocated && lifecycle_parent != 0 {
            let status = result.as_ref().copied().unwrap_or_else(|error| error.code);
            let server = relocation_server.ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                &status.to_le_bytes(),
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        if result.as_ref().is_ok_and(|code| *code == 0) && delete_profile {
            remove_plain_tree(&controller_paths.profile)?;
        }
        return result;
    }
    run_worker(silent, legacy_predecessor)
}

fn request_runtime_exit(paths: &Paths, ignored_process: Option<u32>) -> Result<()> {
    let application = paths.install.join("Talking Quill.exe");
    if application.exists() {
        assert_plain_file(&application)?;
        let mut request = Command::new(&application)
            .arg("--talking-quill-request-machine-quit")
            .spawn()
            .map_err(io_failure)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if request.try_wait().map_err(io_failure)?.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if request.try_wait().map_err(io_failure)?.is_none() {
            let _ = request.kill();
            return Err(fail(EXIT_FAILURE, "Application quit request timed out."));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if !runtime_process_active(paths, ignored_process)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(fail(
        EXIT_FAILURE,
        "Talking Quill runtime did not release the installed tree.",
    ))
}

fn runtime_process_active(paths: &Paths, ignored_process: Option<u32>) -> Result<bool> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(EXIT_FAILURE, "Cannot inspect runtime processes."));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while available {
        if entry.th32ProcessID != std::process::id()
            && Some(entry.th32ProcessID) != ignored_process
            && let Ok(image) = process_image(entry.th32ProcessID)
        {
            let value = image.as_os_str().to_string_lossy().to_lowercase();
            let root = paths.install.as_os_str().to_string_lossy().to_lowercase();
            if value == root
                || value
                    .strip_prefix(&root)
                    .is_some_and(|suffix| suffix.starts_with('\\'))
            {
                return Ok(true);
            }
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(false)
}

fn legacy_predecessor_arguments(arguments: &[OsString]) -> bool {
    if arguments.len() != 5 || arguments[0] != "/S" {
        return false;
    }
    [
        "/TQUPDATE=",
        "/TQGATEWAYHASH=",
        "/TQOWNERHASH=",
        "/TQLAYOUT=",
    ]
    .iter()
    .all(|prefix| {
        arguments
            .iter()
            .skip(1)
            .any(|value| value.to_string_lossy().starts_with(prefix))
    })
}

fn confirm_controller(action: Action) -> Result<bool> {
    let text = match action {
        Action::Install => "Install Talking Quill for all users?",
        Action::Update => "Update Talking Quill for all users?",
        Action::Repair => "Repair Talking Quill for all users?",
        Action::Uninstall => {
            "Uninstall Talking Quill?\n\nYour profile is preserved unless you delete it in the application first."
        }
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => "Remove the exact stale schema-2 test residue?",
    };
    let response = message_box(text, MB_OKCANCEL | MB_ICONQUESTION);
    Ok(response == IDOK)
}

fn retain_controller_image(path: &Path, delete_on_close: bool) -> Result<OwnedHandle> {
    let access = FILE_GENERIC_READ | if delete_on_close { DELETE } else { 0 };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if delete_on_close {
            FILE_FLAG_DELETE_ON_CLOSE
        } else {
            0
        };
    let sharing = FILE_SHARE_READ | FILE_SHARE_DELETE;
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            access,
            sharing,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn rename_handle(handle: std::os::windows::io::RawHandle, destination: &Path) -> Result<()> {
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(mem::size_of::<u16>())
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let fixed = mem::offset_of!(FILE_RENAME_INFO, FileName);
    let total = fixed
        .checked_add(name_bytes)
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let mut storage = vec![0_usize; total.div_ceil(mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = u32::try_from(name_bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
        ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileRenameInfo,
            info.cast(),
            u32::try_from(total)
                .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn rename_retained(handle: &OwnedHandle, destination: &Path) -> Result<()> {
    rename_handle(handle.as_raw_handle(), destination)
}

fn arm_mapped_image_deletion(path: &Path) -> Result<()> {
    let rename_handle = open_plain_handle(path, false, true)?;
    let stream = PathBuf::from(format!(":tq-uninstall-{:08x}", std::process::id()));
    rename_retained(&rename_handle, &stream).map_err(|error| {
        fail(
            error.code,
            format!("Mapped image stream rename failed: {}", error.message),
        )
    })?;
    drop(rename_handle);
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            format!(
                "Renamed mapped image reopen failed: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let delete_handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            delete_handle.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    drop(delete_handle);
    if path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Windows did not commit mapped uninstall image deletion.",
        ));
    }
    Ok(())
}

fn validate_relocated_uninstall_image(
    relocated: &Path,
    original: &Path,
    expected: &Path,
    maintenance: &Path,
) -> Result<File> {
    let relocated_canonical = std::fs::canonicalize(relocated).map_err(io_failure)?;
    let temp_canonical = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = relocated_canonical
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Relocated uninstall path is invalid."))?;
    if relocated_canonical.parent() != Some(temp_canonical.as_path())
        || !name.starts_with(".TalkingQuill-uninstall-")
        || !name.ends_with(".exe")
        || name.len() != ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        || canonical(original)? != canonical(expected)?
    {
        return Err(fail(EXIT_REJECTED, "Relocated uninstall path is invalid."));
    }
    let original_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(original)
        .map_err(io_failure)?;
    let expected_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(expected)
        .map_err(io_failure)?;
    let mut relocated_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(relocated)
        .map_err(io_failure)?;
    if file_identity_text(&original_file)? != file_identity_text(&expected_file)?
        || file_hash(original)? != file_hash(expected)?
        || file_hash(original)? != file_hash(maintenance)?
        || hash_reader(&mut relocated_file)? != file_hash(maintenance)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Relocated uninstall source identity does not match maintenance authority.",
        ));
    }
    drop((original_file, expected_file));
    Ok(relocated_file)
}

fn create_relocated_image(source: &Path) -> Result<(PathBuf, File)> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!(".TalkingQuill-uninstall-{suffix}.exe"));
    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(&path)
        .map_err(io_failure)?;
    let mut source = File::open(source).map_err(io_failure)?;
    std::io::copy(&mut source, &mut target).map_err(io_failure)?;
    target.sync_all().map_err(io_failure)?;
    Ok((path, target))
}

fn launch_relocated(executable: &Path, channel: &ControllerChannel) -> Result<OwnedHandle> {
    let file = wide(executable.as_os_str());
    let parameters = wide(OsStr::new(if channel.silent {
        "/TQ-RELOCATED /S"
    } else {
        "/TQ-RELOCATED"
    }));
    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 || info.hProcess.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot relocate the uninstall controller.",
        ));
    }
    let shell_process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    channel.authenticate(&shell_process, executable, None)
}

fn launch_same_token_uninstall_cleanup(image: &Path) -> Result<()> {
    let channel = ControllerChannel::create(Action::Uninstall, true, 0)?;
    let mut child = Command::new(image).arg("/S").spawn().map_err(io_failure)?;
    let mut duplicate = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            child.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        let _ = child.kill();
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retain the elevated uninstall cleanup process.",
        ));
    }
    let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let process = channel.authenticate(&shell, image, None)?;
    let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), 700_000) };
    if wait != WAIT_OBJECT_0 {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_FAILURE as u32) };
        unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(fail(
            EXIT_FAILURE,
            "Elevated uninstall cleanup did not complete.",
        ));
    }
    let mut code = EXIT_FAILURE as u32;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 || code != 0 {
        return Err(fail(code as i32, "Elevated uninstall cleanup failed."));
    }
    let _ = child.wait();
    Ok(())
}

fn elevate(
    executable: &Path,
    silent: bool,
    channel: &ControllerChannel,
    before_accept: Option<&dyn Fn() -> Result<()>>,
) -> Result<i32> {
    let file = wide(executable.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let parameters = wide(OsStr::new(if silent { "/S" } else { "" }));
    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(fail(
            EXIT_ELEVATION,
            if unsafe { GetLastError() } == ERROR_CANCELLED {
                "Setup elevation was cancelled."
            } else {
                "Setup elevation failed."
            },
        ));
    }
    if info.hProcess.is_null() {
        return Err(fail(
            EXIT_ELEVATION,
            "The elevated worker handle is missing.",
        ));
    }
    let shell_process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let process = channel.authenticate(&shell_process, executable, before_accept)?;
    let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), 600_000) };
    if wait != WAIT_OBJECT_0 {
        unsafe { TerminateProcess(process.as_raw_handle(), EXIT_ELEVATION as u32) };
        let _ = unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) };
        return Err(fail(
            EXIT_ELEVATION,
            if wait == WAIT_TIMEOUT {
                "The elevated worker exceeded its absolute lifecycle deadline."
            } else {
                "Waiting for the elevated worker failed."
            },
        ));
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return Err(fail(
            EXIT_ELEVATION,
            "Cannot read the elevated worker result.",
        ));
    }
    Ok(code as i32)
}

fn run_worker(_silent: bool, legacy_predecessor: bool) -> Result<i32> {
    let current = std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
    // A normal elevated worker proves its controller before touching attacker-sized package data.
    let authenticated_controller = if legacy_predecessor {
        None
    } else {
        Some(WorkerChannel::connect_and_authenticate(&current, None)?)
    };
    let requested_action = authenticated_controller.as_ref().map(|value| value.0);
    #[cfg(feature = "stale-schema2-cleanup")]
    if requested_action == Some(Action::CleanStaleSchema2) {
        reclaim_exact_schema2_orphan(true, true)?;
        return Ok(0);
    }
    let mut image = File::open(&current).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let length = image
        .metadata()
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
        .len();
    let package = package::parse(&mut image, length).map_err(|error| {
        fail(
            EXIT_REJECTED,
            format!("TQPKG2 validation failed: {error:?}"),
        )
    })?;
    let expected_architecture = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unsupported"
    };
    if package.manifest.architecture != expected_architecture {
        return Err(fail(
            EXIT_REJECTED,
            "TQPKG2 architecture does not match the native setup image.",
        ));
    }
    if package.manifest.package_mode == "fresh" && requested_action == Some(Action::Install) {
        reclaim_exact_schema2_orphan(false, true)?;
    }
    let mut paths = paths()?;
    let predecessor_policy_epoch = installed_recovery_policy_epoch(&paths)?;
    let mut machine_lock = Some(MachineLock::acquire(
        &paths,
        120_000,
        predecessor_policy_epoch,
    )?);
    let state_root = paths
        .transaction
        .parent()
        .ok_or_else(|| fail(EXIT_FAILURE, "Installer state root is invalid."))?;
    assert_plain_directory(state_root)?;
    cleanup_transaction_residue(state_root)?;
    let finishing_existing_uninstall = pending_uninstall_transaction(&paths)?;
    // A pending uninstall is already durable machine authority. Validate the signed package and
    // predecessor arguments, but defer installed-state predecessor authentication until recovery
    // only when a predecessor still exists.
    let predecessor_authorized = if legacy_predecessor {
        validate_predecessor_arguments(&package, &current)?;
        if finishing_existing_uninstall {
            false
        } else {
            authenticate_predecessor_helper(&package, &paths)?;
            true
        }
    } else {
        false
    };
    let uninstall_authorized = if requested_action == Some(Action::Uninstall) {
        authorize_uninstall_controller(&paths, &current)?;
        true
    } else {
        false
    };
    let system = WindowsNativeSystem;
    // A relocated controller must stop mapping the installed image before finish-uninstall
    // recovery can delete it. The existing protected journal is sufficient durable authority.
    if finishing_existing_uninstall
        && let Some((Action::Uninstall, server, _, lifecycle_parent)) =
            authenticated_controller.as_ref()
        && *lifecycle_parent != 0
    {
        arm_relocated_uninstall_controller(server)?;
    }
    // Authentication and package validation happen before recovery. Once the machine lock is held,
    // every installed entry point must finish durable recovery before arming or deriving a new action.
    if finishing_existing_uninstall {
        ensure_uninstall_finalizer_registered(&current, &paths)?;
    }
    recover_with_adapter(&paths, &system)?;
    recover_terminal_recovery_tombstones(&paths)?;
    if finishing_existing_uninstall {
        complete_terminal_uninstall(&paths, &system, &current, &mut machine_lock)?;
        if requested_action == Some(Action::Uninstall) {
            return Ok(0);
        }
        // Terminal retirement removes the predecessor and its generation. Rebuild paths so a
        // continued fresh install cannot recreate an image already owned by reboot deletion.
        paths = self::paths()?;
        machine_lock = Some(MachineLock::acquire(
            &paths,
            120_000,
            installed_recovery_policy_epoch(&paths)?,
        )?);
        let recovered_action = derive_action(&current, &paths)?;
        if requested_action.is_some_and(|requested| requested != recovered_action) {
            return Err(fail(
                EXIT_REJECTED,
                "Recovered machine state does not match the authenticated setup request.",
            ));
        }
    }
    if uninstall_authorized && !path_present(&paths.transaction)? && !path_present(&paths.install)?
    {
        // Journal-absent recovery must honor the durable terminal owner before touching its Run
        // target. The exact maintenance image finishes residue, retires Run, then self-unlinks.
        if let Some(record) = read_terminal_uninstall_record(&paths)? {
            let maintenance_image =
                canonical(&current)? == canonical(&paths.maintenance_uninstaller)?;
            let authenticated_relocated_image = authenticated_controller.is_some()
                && relocated_uninstall_matches_maintenance(&current, &paths)?;
            if (!maintenance_image && !authenticated_relocated_image)
                || hex_hash(&file_hash(&current)?) != record.maintenance_sha256
            {
                return Err(fail(EXIT_REJECTED, "Terminal uninstall owner is invalid."));
            }
            if matches!(record.phase.as_str(), "armed" | "machine-retired") {
                resume_terminal_service(&paths, &record)?;
            }
            drop(machine_lock.take());
            wait_for_terminal_service_retirement(&paths, &record.generation)?;
            return Ok(0);
        }
        system.unregister_app_path()?;
        system.unregister_uninstall()?;
        clear_update_recovery(&paths)?;
        clear_legacy_profile_relaunch_owners(&paths)?;
        remove_update_recovery_launcher_residue(&paths)?;
        remove_uninstall_finalizer_residue(&paths)?;
        let legacy = retire_and_remove_machine_lock(&paths, &mut machine_lock)?;
        arm_mapped_image_deletion(&current)?;
        drop(legacy);
        clear_machine_relaunch_owner(&paths)?;
        return Ok(0);
    }
    if let Some((Action::Uninstall, server, _, lifecycle_parent)) =
        authenticated_controller.as_ref()
        && *lifecycle_parent != 0
    {
        write_transaction(&paths, "uninstall-armed", Action::Uninstall, true)?;
        arm_relocated_uninstall_controller(server)?;
    }
    let mut action = if uninstall_authorized {
        Action::Uninstall
    } else {
        let derived = derive_action(&current, &paths)?;
        if requested_action.is_some_and(|requested| requested != derived) {
            return Err(fail(
                EXIT_REJECTED,
                "Recovered machine state does not match the authenticated setup request.",
            ));
        }
        derived
    };
    if action != Action::Uninstall {
        action =
            authorize_package_mode(&package, &paths, &current, predecessor_authorized, action)?;
    }
    if action != Action::Install || paths.install.exists() {
        request_runtime_exit(
            &paths,
            authenticated_controller
                .as_ref()
                .and_then(|value| (value.3 != 0).then_some(value.3)),
        )?;
    }
    match action {
        Action::Install | Action::Update | Action::Repair => {
            install(&mut image, &package, &current, &paths, action, &system)
        }
        Action::Uninstall => {
            write_transaction(&paths, "uninstalling", Action::Uninstall, true)?;
            let original_controller = authenticated_controller
                .as_ref()
                .ok_or_else(|| {
                    fail(
                        EXIT_REJECTED,
                        "Uninstall controller authentication is missing.",
                    )
                })?
                .3;
            uninstall(&paths, &system, original_controller != 0, &mut machine_lock)
        }
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => {
            return Err(fail(
                EXIT_REJECTED,
                "Cleanup reached the installer transaction path.",
            ));
        }
    }?;
    if action == Action::Uninstall {
        complete_terminal_uninstall(&paths, &system, &current, &mut machine_lock)?;
    }
    Ok(0)
}

trait NativeSystemAdapter {
    fn register_version(&self, paths: &Paths, version: &str) -> Result<()>;
    fn register_installed(&self, paths: &Paths) -> Result<()>;
    fn unregister_app_path(&self) -> Result<()>;
    fn unregister_uninstall(&self) -> Result<()>;
    fn retire_legacy(&self, paths: &Paths) -> Result<()>;
    fn clear_update_recovery(&self, paths: &Paths) -> Result<()>;
}

struct WindowsNativeSystem;

impl NativeSystemAdapter for WindowsNativeSystem {
    fn register_version(&self, paths: &Paths, version: &str) -> Result<()> {
        register_uninstall(paths, version)?;
        register_app_path(paths)
    }
    fn register_installed(&self, paths: &Paths) -> Result<()> {
        register_installed_uninstall(paths)
    }
    fn unregister_app_path(&self) -> Result<()> {
        unregister_app_path()
    }
    fn unregister_uninstall(&self) -> Result<()> {
        unregister_uninstall()
    }
    fn retire_legacy(&self, paths: &Paths) -> Result<()> {
        retire_legacy_authority(paths)
    }
    fn clear_update_recovery(&self, paths: &Paths) -> Result<()> {
        clear_update_recovery(paths)
    }
}

fn installed_recovery_policy_epoch(paths: &Paths) -> Result<u8> {
    let helper = paths
        .install
        .join("resources/helper/talking-quill-helper.exe");
    if !path_present(&helper)? {
        return Ok(1);
    }
    assert_plain_file(&helper)?;
    let bytes = fs::read(helper).map_err(io_failure)?;
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
        return Err(fail(
            EXIT_REJECTED,
            "Installed recovery policy epoch is invalid.",
        ));
    }
    Ok(matches[0] - b'0')
}

struct MachineLock {
    _legacy: Option<LegacyMutexPair>,
    file: File,
}
impl MachineLock {
    fn acquire(paths: &Paths, timeout: u32, predecessor_policy_epoch: u8) -> Result<Self> {
        let legacy = if predecessor_policy_epoch < LEGACY_LOCK_RETIREMENT_EPOCH {
            Some(LegacyMutexPair::acquire()?)
        } else {
            None
        };
        let path = machine_lock_file(paths, predecessor_policy_epoch)?;
        let deadline = Instant::now() + Duration::from_millis(timeout.into());
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&path)
            {
                Ok(file) => {
                    let expected = fs::read_to_string(path.with_extension("identity-v1"))
                        .map_err(io_failure)?;
                    if !staged_path_is_protected(&path, false)?
                        || file_identity_text(&file)? != expected
                    {
                        return Err(fail(EXIT_REJECTED, "Machine lock identity is invalid."));
                    }
                    validate_acquired_machine_lock_state(&paths.program_data, &path)?;
                    return Ok(Self {
                        _legacy: legacy,
                        file,
                    });
                }
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(fail(EXIT_FAILURE, "Machine setup lock timed out.")),
            }
        }
    }
}
fn validate_acquired_machine_lock_state(program_data: &Path, lock: &Path) -> Result<()> {
    let suffix = lock
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock path is invalid."))?;
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(MACHINE_LOCK_REGISTRY_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_REJECTED, "Machine lock publication is missing."));
    }
    let publication = read_machine_lock_registry_string(key)?;
    unsafe { RegCloseKey(key) };
    match publication.as_deref() {
        Some(value) if value == suffix => Ok(()),
        Some(value)
            if value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX) == Some(suffix)
                && path_present(
                    &program_data
                        .join("Talking Quill Update Recovery")
                        .join(TERMINAL_UNINSTALL_RECORD_NAME),
                )? =>
        {
            Ok(())
        }
        _ => Err(fail(EXIT_REJECTED, "Machine lock publication changed.")),
    }
}

impl MachineLock {
    fn take_legacy(&mut self) -> Option<LegacyMutexPair> {
        self._legacy.take()
    }
}
impl Drop for MachineLock {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
    }
}

struct LegacyMutexPair([OwnedHandle; 2]);
impl LegacyMutexPair {
    fn acquire() -> Result<Self> {
        Ok(Self([
            acquire_verified_legacy_mutex("Global\\TalkingQuill.NativeSetup.V2")?,
            acquire_verified_legacy_mutex("Global\\TalkingQuill.UpdateRecovery.State.V1")?,
        ]))
    }
}
impl Drop for LegacyMutexPair {
    fn drop(&mut self) {
        for handle in self.0.iter().rev() {
            unsafe { ReleaseMutex(handle.as_raw_handle()) };
        }
    }
}

fn acquire_verified_legacy_mutex(name: &str) -> Result<OwnedHandle> {
    let sddl = wide(OsStr::new("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create the legacy lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let raw = unsafe { CreateMutexW(&attributes, 0, wide(OsStr::new(name)).as_ptr()) };
    unsafe { LocalFree(descriptor) };
    if raw.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open the legacy machine lock."));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if !matches!(
        unsafe { WaitForSingleObject(handle.as_raw_handle(), 30_000) },
        0 | 0x80
    ) || !legacy_mutex_security_is_exact(handle.as_raw_handle())?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Legacy machine lock identity is invalid.",
        ));
    }
    Ok(handle)
}

fn legacy_mutex_security_is_exact(handle: *mut c_void) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the legacy machine lock.",
        ));
    }
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode the legacy machine lock ACL.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
        .to_ascii_uppercase();
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.starts_with("O:BA")
        && (value.contains("(A;;GA;;;SY)") || value.contains("(A;;0X1F0001;;;SY)"))
        && (value.contains("(A;;GA;;;BA)") || value.contains("(A;;0X1F0001;;;BA)"))
        && value.matches("(A;;").count() == 2
        && !value.contains(";;;AU)"))
}

fn machine_lock_file(paths: &Paths, predecessor_policy_epoch: u8) -> Result<PathBuf> {
    let program_data = &paths.program_data;
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(MACHINE_LOCK_REGISTRY_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open the machine lock registry key.",
        ));
    }
    reclaim_machine_lock_pending(program_data)?;
    let terminal_owner_present = read_terminal_uninstall_record(paths)?.is_some();
    let publication = read_machine_lock_registry_string(key)?;
    let publication = if let Some(retired) = publication
        .as_deref()
        .and_then(|value| value.strip_prefix(MACHINE_LOCK_RETIRED_PREFIX))
    {
        validate_machine_lock_suffix(retired)?;
        let retired_directory =
            program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{retired}"));
        if terminal_owner_present && path_present(&retired_directory)? {
            Some(retired.to_owned())
        } else {
            if unsafe {
                RegDeleteValueW(key, wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr())
            } != 0
                || unsafe { RegFlushKey(key) } != 0
            {
                unsafe { RegCloseKey(key) };
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot clear retired machine lock publication.",
                ));
            }
            if path_present(&retired_directory)? {
                reclaim_retired_machine_lock_directory(program_data, retired)?;
            }
            None
        }
    } else {
        publication
    };
    let directory = if let Some(suffix) = publication {
        validate_machine_lock_suffix(&suffix)?;
        program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"))
    } else {
        if predecessor_policy_epoch >= LEGACY_LOCK_RETIREMENT_EPOCH && !terminal_owner_present {
            unsafe { RegCloseKey(key) };
            return Err(fail(
                EXIT_REJECTED,
                "A retired recovery policy cannot republish the machine lock.",
            ));
        }
        reclaim_unpublished_machine_lock_directories(program_data)?;
        let suffix = new_machine_lock_suffix()?;
        let token = new_machine_lock_suffix()?;
        let pending = program_data.join(format!("{MACHINE_LOCK_PENDING_PREFIX}{token}"));
        let published = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_restricted_lock_directory(&pending)?;
        apply_lock_dacl(&pending, MACHINE_LOCK_DIRECTORY_SDDL)?;
        let identity = owned_tree_identity(&pending).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Machine lock directory identity is unavailable.",
            )
        })?;
        create_or_verify_lock_marker(
            &pending.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
        )?;
        initialize_machine_lock_tree(&pending, &identity)?;
        flush_setup_directory(&pending)?;
        if unsafe {
            MoveFileExW(
                wide(pending.as_os_str()).as_ptr(),
                wide(published.as_os_str()).as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot publish the machine lock directory.",
            ));
        }
        flush_setup_directory(program_data)?;
        let value = wide(OsStr::new(&suffix));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE)).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        } != 0
            || unsafe { RegFlushKey(key) } != 0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot persist the machine lock identity.",
            ));
        }
        published
    };
    unsafe { RegCloseKey(key) };
    verify_machine_lock_tree(&directory)
}

fn validate_machine_lock_suffix(suffix: &str) -> Result<()> {
    if suffix.len() == 32
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(fail(
            EXIT_REJECTED,
            "Machine lock registry identity is invalid.",
        ))
    }
}

fn initialize_machine_lock_tree(directory: &Path, directory_identity: &str) -> Result<()> {
    let lock = directory.join("recovery-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&lock)
        .map_err(io_failure)?;
    apply_lock_dacl(&lock, MACHINE_LOCK_FILE_SDDL)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    create_or_verify_lock_marker(&lock.with_extension("identity-v1"), &identity)?;
    create_or_verify_lock_marker(&directory.join("lock-tree-identity-v1"), directory_identity)
}

fn verify_machine_lock_tree(directory: &Path) -> Result<PathBuf> {
    if !staged_path_is_protected(directory, true)? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory is not protected.",
        ));
    }
    let identity = owned_tree_identity(directory).map_err(|_| {
        fail(
            EXIT_REJECTED,
            "Machine lock directory identity is unavailable.",
        )
    })?;
    if fs::read_to_string(directory.join("lock-tree-identity-v1")).map_err(io_failure)? != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory identity changed.",
        ));
    }
    let lock = directory.join("recovery-state-v1.lock");
    if !staged_path_is_protected(&lock, false)? {
        return Err(fail(EXIT_REJECTED, "Machine lock file is not protected."));
    }
    Ok(lock)
}

fn reclaim_retired_machine_lock_directory(program_data: &Path, suffix: &str) -> Result<()> {
    let path = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path_present(&path)? {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity =
        owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match remove_owned_tree(&path, &identity) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(fail(EXIT_FAILURE, error.to_string())),
        }
    }
}

fn retire_machine_lock_publication(_paths: &Paths) -> Result<String> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(MACHINE_LOCK_REGISTRY_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retire the machine lock publication.",
        ));
    }
    let publication = read_machine_lock_registry_string(key)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Machine lock publication is missing."))?;
    unsafe { RegCloseKey(key) };
    let suffix = publication
        .strip_prefix(MACHINE_LOCK_RETIRED_PREFIX)
        .unwrap_or(&publication)
        .to_owned();
    validate_machine_lock_suffix(&suffix)?;
    delete_registry_tree_durable(
        MACHINE_LOCK_REGISTRY_KEY,
        r"Software\Talking Quill",
        "machine lock publication",
    )?;
    Ok(suffix)
}

fn reclaim_unpublished_machine_lock_directories(program_data: &Path) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|name| name.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        else {
            continue;
        };
        if validate_machine_lock_suffix(suffix).is_err() {
            continue;
        }
        let path = entry.path();
        if !staged_path_is_protected(&path, true)? {
            continue;
        }
        let identity =
            owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let marker = path.join("publication-pending-v1");
        let expected = format!("{suffix}:{identity}");
        if fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            remove_owned_tree(&path, &identity)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        }
    }
    Ok(())
}

fn reclaim_machine_lock_pending(program_data: &Path) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_PENDING_PREFIX))
            .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !staged_path_is_protected(&path, true)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Pending machine lock identity is unavailable.",
            )
        })?;
        remove_owned_tree(&path, &identity)
            .map_err(|_| fail(EXIT_FAILURE, "Cannot reclaim pending machine lock state."))?;
    }
    Ok(())
}

fn flush_setup_directory(path: &Path) -> Result<()> {
    let handle = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open a durable machine lock directory.",
        ));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if unsafe { FlushFileBuffers(handle.as_raw_handle()) } == 0 {
        return Err(fail(EXIT_FAILURE, "Cannot flush a machine lock directory."));
    }
    Ok(())
}

fn new_machine_lock_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(EXIT_FAILURE, "Cannot generate the machine lock identity."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_machine_lock_registry_string(key: *mut c_void) -> Result<Option<String>> {
    let name = wide(OsStr::new(MACHINE_LOCK_REGISTRY_VALUE));
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || !(2..=256).contains(&bytes) || !bytes.is_multiple_of(2) {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock registry value is invalid.",
        ));
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read the machine lock registry value.",
        ));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lock registry value is invalid."))
}

fn create_restricted_lock_directory(path: &Path) -> Result<()> {
    create_directory_with_security(path, MACHINE_LOCK_DIRECTORY_SDDL)
}

fn create_directory_with_security(path: &Path, descriptor_sddl: &str) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create the machine lock ACL."));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) };
    unsafe { LocalFree(descriptor) };
    if created == 0 && !path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock directory.",
        ));
    }
    Ok(())
}

fn apply_lock_dacl(path: &Path, descriptor_sddl: &str) -> Result<()> {
    let sddl = wide(OsStr::new(descriptor_sddl));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the machine lock file ACL.",
        ));
    }
    let status = unsafe {
        SetFileSecurityW(
            wide(path.as_os_str()).as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor) };
    if status == 0 {
        Err(fail(EXIT_FAILURE, "Cannot protect the machine lock file."))
    } else {
        Ok(())
    }
}

fn create_or_verify_lock_marker(path: &Path, value: &str) -> Result<()> {
    create_atomic_marker(path, value, MACHINE_LOCK_FILE_SDDL)
}

fn create_atomic_marker(path: &Path, value: &str, sddl: &str) -> Result<()> {
    if path_present(path)? {
        return verify_atomic_marker(path, value, sddl, None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no parent."))?;
    let name = path
        .file_name()
        .ok_or_else(|| fail(EXIT_REJECTED, "Protected marker has no name."))?
        .to_string_lossy();
    let temporary = parent.join(format!("{name}.tmp-{}", new_machine_lock_suffix()?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(io_failure)?;
    apply_lock_dacl(&temporary, sddl)?;
    file.write_all(value.as_bytes()).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    verify_atomic_marker(&temporary, value, sddl, Some(&identity))?;
    if unsafe {
        MoveFileExW(
            wide(temporary.as_os_str()).as_ptr(),
            wide(path.as_os_str()).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot publish the protected marker."));
    }
    flush_setup_directory(parent)?;
    verify_atomic_marker(path, value, sddl, Some(&identity))
}

fn verify_atomic_marker(
    path: &Path,
    value: &str,
    sddl: &str,
    expected_identity: Option<&str>,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(io_failure)?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || content != value
        || !marker_security_is_exact(path, sddl)?
    {
        return Err(fail(EXIT_REJECTED, "Protected marker identity changed."));
    }
    Ok(())
}

fn marker_security_is_exact(path: &Path, sddl: &str) -> Result<bool> {
    if sddl == MACHINE_LOCK_FILE_SDDL {
        return staged_path_is_protected(path, false);
    }
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(EXIT_REJECTED, "Cannot inspect protected marker ACL."));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(EXIT_REJECTED, "Cannot encode protected marker ACL."));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok(value.eq_ignore_ascii_case("O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;AU)"))
}

fn file_identity_text(file: &File) -> Result<String> {
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock file identity is unavailable.",
        ));
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(format!("{}:{index}", information.dwVolumeSerialNumber))
}

struct ControllerChannel {
    handle: OwnedHandle,
    action: Action,
    silent: bool,
    lifecycle_parent: u32,
}

fn duplicate_delegated_process_handle(value: u64, expected_process: u32) -> Result<OwnedHandle> {
    let raw = value as usize as *mut c_void;
    let mut flags = 0;
    if value == 0
        || value >= usize::MAX.saturating_sub(15) as u64
        || unsafe { GetHandleInformation(raw, &mut flags) } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Worker delegated an invalid process handle.",
        ));
    }
    let mut duplicate = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
        || duplicate.is_null()
        || duplicate == INVALID_HANDLE_VALUE
    {
        return Err(fail(
            EXIT_REJECTED,
            "Worker process handle cannot be retained safely.",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    if unsafe { GetProcessId(process.as_raw_handle()) } != expected_process {
        return Err(fail(
            EXIT_REJECTED,
            "Worker delegated a handle for the wrong process.",
        ));
    }
    Ok(process)
}

impl ControllerChannel {
    fn create(action: Action, silent: bool, lifecycle_parent: u32) -> Result<Self> {
        let pid = std::process::id();
        let name = wide(OsStr::new(&format!(r"\\.\pipe\TalkingQuill.Setup.{pid}")));
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                256,
                256,
                30_000,
                ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot create the protected setup pipe.",
            ));
        }
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
            action,
            silent,
            lifecycle_parent,
        })
    }

    fn wait_relocated_status(&self, process: &OwnedHandle, image: &Path) -> Result<i32> {
        let operation = pipe_read::<13>(
            self.handle.as_raw_handle(),
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(30),
        )?;
        if operation == *b"TQ-ARM-DELETE" {
            let deletion = arm_mapped_image_deletion(image);
            pipe_write(
                self.handle.as_raw_handle(),
                &[u8::from(deletion.is_ok())],
                Some(process.as_raw_handle()),
                Instant::now() + Duration::from_secs(30),
            )?;
            deletion?;
        } else if operation != *b"TQ-KEEP-IMAGE" {
            return Err(fail(
                EXIT_REJECTED,
                "Relocated uninstall requested an invalid completion operation.",
            ));
        }
        Ok(i32::from_le_bytes(pipe_read::<4>(
            self.handle.as_raw_handle(),
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(700),
        )?))
    }

    fn authenticate(
        &self,
        shell_process: &OwnedHandle,
        image: &Path,
        before_accept: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<OwnedHandle> {
        let expected_worker = unsafe { GetProcessId(shell_process.as_raw_handle()) };
        let deadline = Instant::now() + Duration::from_secs(30);
        pipe_connect(
            self.handle.as_raw_handle(),
            shell_process.as_raw_handle(),
            deadline,
        )?;
        let delegated_value = u64::from_le_bytes(pipe_read::<8>(
            self.handle.as_raw_handle(),
            Some(shell_process.as_raw_handle()),
            deadline,
        )?);
        let worker_process = duplicate_delegated_process_handle(delegated_value, expected_worker)?;
        if unsafe { NtSuspendProcess(worker_process.as_raw_handle()) } < 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot suspend the setup worker for verification.",
            ));
        }
        let mut worker = 0;
        let verification = (|| {
            if unsafe { GetNamedPipeClientProcessId(self.handle.as_raw_handle(), &mut worker) } == 0
                || worker != expected_worker
                || file_hash(&process_image(worker)?)? != file_hash(image)?
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "The setup pipe client is not the elevated same-image worker.",
                ));
            }
            verify_peer_claims(std::process::id(), worker)
        })();
        if unsafe { NtResumeProcess(worker_process.as_raw_handle()) } < 0 {
            unsafe { TerminateProcess(worker_process.as_raw_handle(), EXIT_REJECTED as u32) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot resume the verified setup worker.",
            ));
        }
        let claims_binding = verification?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        let secret = ephemeral_secret()?;
        let public = secret.public_key().to_sec1_bytes();
        let mut hello = Vec::with_capacity(97);
        hello.extend_from_slice(&nonce);
        hello.extend_from_slice(&public);
        let monitor = Some(worker_process.as_raw_handle());
        pipe_write(self.handle.as_raw_handle(), &hello, monitor, deadline)?;
        let worker_public_bytes = pipe_read::<65>(self.handle.as_raw_handle(), monitor, deadline)?;
        let worker_public = PublicKey::from_sec1_bytes(&worker_public_bytes)
            .map_err(|_| fail(EXIT_REJECTED, "Worker P-256 key is invalid."))?;
        let shared = diffie_hellman(secret.to_nonzero_scalar(), worker_public.as_affine());
        let image_hash = peer_binding(&file_hash(image)?, &claims_binding);
        let expected = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            std::process::id(),
            worker,
            &image_hash,
            &public,
            &worker_public_bytes,
            b"worker",
            &[],
        );
        let worker_proof = pipe_read::<32>(self.handle.as_raw_handle(), monitor, deadline)?;
        if worker_proof != expected {
            return Err(fail(
                EXIT_REJECTED,
                "The elevated worker transcript proof is invalid.",
            ));
        }
        let mut request = Vec::with_capacity(6);
        request.extend_from_slice(&[
            match self.action {
                Action::Install => 1,
                Action::Repair => 2,
                Action::Uninstall => 3,
                #[cfg(feature = "stale-schema2-cleanup")]
                Action::CleanStaleSchema2 => 4,
                Action::Update => {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Update authority cannot come from a controller request.",
                    ));
                }
            },
            u8::from(self.silent),
        ]);
        request.extend_from_slice(&self.lifecycle_parent.to_le_bytes());
        pipe_write(
            self.handle.as_raw_handle(),
            b"TQ-SETUP-ACCEPTED",
            monitor,
            deadline,
        )?;
        pipe_write(self.handle.as_raw_handle(), &request, monitor, deadline)?;
        let controller_proof = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            std::process::id(),
            worker,
            &image_hash,
            &public,
            &worker_public_bytes,
            b"controller",
            &request,
        );
        pipe_write(
            self.handle.as_raw_handle(),
            &controller_proof,
            monitor,
            deadline,
        )?;
        if let Some(before_accept) = before_accept {
            if pipe_read::<22>(self.handle.as_raw_handle(), monitor, deadline)?
                != *b"TQ-UNINSTALL-JOURNALED"
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Elevated uninstall worker did not persist cleanup authority.",
                ));
            }
            before_accept()?;
            pipe_write(
                self.handle.as_raw_handle(),
                b"TQ-UNINSTALL-ARMED",
                monitor,
                deadline,
            )?;
        }
        let mut transcript = Sha256::new();
        transcript.update(b"TalkingQuill/setup-authenticated-transcript/v1");
        transcript.update(nonce);
        transcript.update(&public);
        transcript.update(worker_public_bytes);
        transcript.update(image_hash);
        transcript.update(&request);
        transcript.update(worker_proof);
        transcript.update(controller_proof);
        let transcript_hash: [u8; 32] = transcript.finalize().into();
        let receipt = AuthenticationReceiptInput {
            controller: std::process::id(),
            worker,
            package_sha256: &file_hash(image)?,
            peer_binding: &image_hash,
            transcript_sha256: &transcript_hash,
            nonce: &nonce,
            controller_public: &public,
            worker_public: &worker_public_bytes,
            request: &request,
            worker_proof: &worker_proof,
            controller_proof: &controller_proof,
        };
        let _ = publish_authentication_receipt(&receipt, worker_process.as_raw_handle());
        Ok(worker_process)
    }
}

struct AuthenticationReceiptInput<'a> {
    controller: u32,
    worker: u32,
    package_sha256: &'a [u8; 32],
    peer_binding: &'a [u8; 32],
    transcript_sha256: &'a [u8; 32],
    nonce: &'a [u8; 32],
    controller_public: &'a [u8],
    worker_public: &'a [u8; 65],
    request: &'a [u8],
    worker_proof: &'a [u8; 32],
    controller_proof: &'a [u8; 32],
}

fn publish_authentication_receipt(
    input: &AuthenticationReceiptInput<'_>,
    worker_process: std::os::windows::io::RawHandle,
) -> Result<()> {
    let AuthenticationReceiptInput {
        controller,
        worker,
        package_sha256,
        peer_binding,
        transcript_sha256,
        nonce,
        controller_public,
        worker_public,
        request,
        worker_proof,
        controller_proof,
    } = *input;
    let name = wide(OsStr::new(&format!(
        r"\\.\pipe\TalkingQuill.Setup.Receipt.{controller}"
    )));
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            2048,
            2048,
            1_000,
            ptr::null(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(EXIT_FAILURE, "Cannot create setup evidence pipe."));
    }
    let pipe = unsafe { OwnedHandle::from_raw_handle(raw) };
    let deadline = Instant::now() + Duration::from_secs(1);
    pipe_connect(pipe.as_raw_handle(), worker_process, deadline)?;
    let challenge = pipe_read::<32>(pipe.as_raw_handle(), Some(worker_process), deadline)?;
    let signing_key = evidence_signing_key()?;
    let evidence_public = signing_key.verifying_key().to_encoded_point(false);
    let mut signed = Vec::new();
    signed.extend_from_slice(b"TalkingQuill/setup-evidence-signature/v1");
    signed.extend_from_slice(&challenge);
    signed.extend_from_slice(&controller.to_le_bytes());
    signed.extend_from_slice(&worker.to_le_bytes());
    signed.extend_from_slice(package_sha256);
    signed.extend_from_slice(peer_binding);
    signed.extend_from_slice(transcript_sha256);
    signed.extend_from_slice(nonce);
    signed.extend_from_slice(controller_public);
    signed.extend_from_slice(worker_public);
    signed.extend_from_slice(request);
    signed.extend_from_slice(worker_proof);
    signed.extend_from_slice(controller_proof);
    let signature: Signature = signing_key.sign(&signed);
    let receipt = format!(
        "{{\"schemaVersion\":2,\"protocol\":\"P-256-ECDH/HMAC-SHA256-v1\",\"controllerPid\":{controller},\"workerPid\":{worker},\"packageSha256\":\"{}\",\"peerBinding\":\"{}\",\"transcriptSha256\":\"{}\",\"observerChallenge\":\"{}\",\"nonce\":\"{}\",\"controllerPublicKey\":\"{}\",\"workerPublicKey\":\"{}\",\"request\":\"{}\",\"workerProof\":\"{}\",\"controllerProof\":\"{}\",\"evidencePublicKey\":\"{}\",\"evidenceSignature\":\"{}\"}}",
        hex_hash(package_sha256),
        hex_hash(peer_binding),
        hex_hash(transcript_sha256),
        hex_hash(&challenge),
        hex_hash(nonce),
        hex_bytes(controller_public),
        hex_bytes(worker_public),
        hex_bytes(request),
        hex_hash(worker_proof),
        hex_hash(controller_proof),
        hex_bytes(evidence_public.as_bytes()),
        hex_bytes(&signature.to_bytes()),
    );
    pipe_write(
        pipe.as_raw_handle(),
        receipt.as_bytes(),
        Some(worker_process),
        deadline,
    )
}

struct WorkerChannel;
impl WorkerChannel {
    fn connect_and_authenticate(
        image: &Path,
        expected_server: Option<&Path>,
    ) -> Result<(Action, OwnedHandle, bool, u32)> {
        let server = parent_process_id()?;
        let name = wide(OsStr::new(&format!(
            r"\\.\pipe\TalkingQuill.Setup.{server}"
        )));
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot open the medium setup controller pipe.",
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let deadline = Instant::now() + Duration::from_secs(30);
        let server_process_raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_DUP_HANDLE | SYNCHRONIZE,
                0,
                server,
            )
        };
        if server_process_raw.is_null() {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot retain the setup controller process.",
            ));
        }
        let server_process = unsafe { OwnedHandle::from_raw_handle(server_process_raw) };
        let mut observed_server = 0;
        let server_image = process_image(server)?;
        if unsafe { GetNamedPipeServerProcessId(handle.as_raw_handle(), &mut observed_server) } == 0
            || observed_server != server
            || file_hash(&server_image)? != file_hash(image)?
            || expected_server.is_some_and(|expected| {
                !server_image
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            })
        {
            return Err(fail(
                EXIT_REJECTED,
                "The setup pipe server is not the same-image controller.",
            ));
        }
        let monitor = Some(server_process.as_raw_handle());
        let mut delegated = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentProcess(),
                server_process.as_raw_handle(),
                &mut delegated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
            || delegated.is_null()
        {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot delegate the worker lifecycle handle to its controller.",
            ));
        }
        pipe_write(
            handle.as_raw_handle(),
            &(delegated as usize as u64).to_le_bytes(),
            monitor,
            deadline,
        )?;
        let hello = pipe_read::<97>(handle.as_raw_handle(), monitor, deadline)?;
        let nonce: [u8; 32] = hello[..32].try_into().unwrap();
        let controller_public_bytes: [u8; 65] = hello[32..].try_into().unwrap();
        let controller_public = PublicKey::from_sec1_bytes(&controller_public_bytes)
            .map_err(|_| fail(EXIT_REJECTED, "Controller P-256 key is invalid."))?;
        let secret = ephemeral_secret()?;
        let worker_public = secret.public_key().to_sec1_bytes();
        let shared = diffie_hellman(secret.to_nonzero_scalar(), controller_public.as_affine());
        let image_hash = peer_binding(
            &file_hash(image)?,
            &verify_peer_claims(server, std::process::id())?,
        );
        pipe_write(handle.as_raw_handle(), &worker_public, monitor, deadline)?;
        let proof = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            server,
            std::process::id(),
            &image_hash,
            &controller_public_bytes,
            &worker_public,
            b"worker",
            &[],
        );
        pipe_write(handle.as_raw_handle(), &proof, monitor, deadline)?;
        if pipe_read::<17>(handle.as_raw_handle(), monitor, deadline)? != *b"TQ-SETUP-ACCEPTED" {
            return Err(fail(
                EXIT_REJECTED,
                "The medium setup controller rejected the worker.",
            ));
        }
        let request = pipe_read::<6>(handle.as_raw_handle(), monitor, deadline)?;
        let controller_proof = pipe_read::<32>(handle.as_raw_handle(), monitor, deadline)?;
        let expected = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            server,
            std::process::id(),
            &image_hash,
            &controller_public_bytes,
            &worker_public,
            b"controller",
            &request,
        );
        if controller_proof != expected {
            return Err(fail(
                EXIT_REJECTED,
                "The controller transcript proof is invalid.",
            ));
        }
        let silent = match request[1] {
            0 => false,
            1 => true,
            _ => {
                return Err(fail(
                    EXIT_REJECTED,
                    "The setup controller sent an invalid UI mode.",
                ));
            }
        };
        let lifecycle_parent = u32::from_le_bytes(request[2..6].try_into().unwrap());
        match request[0] {
            1 if lifecycle_parent == 0 => Ok((Action::Install, handle, silent, 0)),
            2 if lifecycle_parent == 0 => Ok((Action::Repair, handle, silent, 0)),
            3 => Ok((Action::Uninstall, handle, silent, lifecycle_parent)),
            #[cfg(feature = "stale-schema2-cleanup")]
            4 if lifecycle_parent != 0 => {
                Ok((Action::CleanStaleSchema2, handle, true, lifecycle_parent))
            }
            _ => Err(fail(
                EXIT_REJECTED,
                "The setup controller requested an invalid operation.",
            )),
        }
    }
}

fn peer_binding(image: &[u8; 32], claims: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(image);
    hash.update(claims);
    hash.finalize().into()
}

fn evidence_signing_key() -> Result<SigningKey> {
    for _ in 0..16 {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        if let Ok(key) = SigningKey::from_bytes((&bytes).into()) {
            return Ok(key);
        }
    }
    Err(fail(
        EXIT_FAILURE,
        "Cannot create setup evidence signing key.",
    ))
}

fn ephemeral_secret() -> Result<SecretKey> {
    for _ in 0..16 {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        if let Ok(secret) = SecretKey::from_slice(&bytes) {
            return Ok(secret);
        }
    }
    Err(fail(
        EXIT_FAILURE,
        "Cannot generate an ephemeral setup key.",
    ))
}

#[allow(clippy::too_many_arguments)]
fn authenticated_proof(
    shared: &[u8],
    nonce: &[u8; 32],
    controller: u32,
    worker: u32,
    image: &[u8; 32],
    controller_public: &[u8],
    worker_public: &[u8],
    role: &[u8],
    frame: &[u8],
) -> [u8; 32] {
    let mut mac =
        Hmac::<Sha256V10>::new_from_slice(shared).expect("P-256 secret is a valid HMAC key");
    mac.update(b"TalkingQuill/setup-pipe/p256/v3");
    mac.update(nonce);
    mac.update(&controller.to_le_bytes());
    mac.update(&worker.to_le_bytes());
    mac.update(image);
    mac.update(controller_public);
    mac.update(worker_public);
    mac.update(role);
    mac.update(frame);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
fn channel_proof(nonce: &[u8; 32], controller: u32, worker: u32, image: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/setup-pipe/v2");
    hash.update(nonce);
    hash.update(controller.to_le_bytes());
    hash.update(worker.to_le_bytes());
    hash.update(image);
    hash.finalize().into()
}

fn deadline_ms(deadline: Instant) -> Result<u32> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| fail(EXIT_REJECTED, "Setup pipe deadline expired."))?;
    Ok(u32::try_from(remaining.as_millis().max(1)).unwrap_or(u32::MAX - 1))
}

fn wait_overlapped(
    handle: std::os::windows::io::RawHandle,
    event: std::os::windows::io::RawHandle,
    monitor: Option<std::os::windows::io::RawHandle>,
    overlapped: &mut OVERLAPPED,
    deadline: Instant,
) -> Result<u32> {
    let handles = [event, monitor.unwrap_or(event)];
    let count = if monitor.is_some() { 2 } else { 1 };
    let result =
        unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, deadline_ms(deadline)?) };
    if result != WAIT_OBJECT_0 {
        unsafe { CancelIoEx(handle, overlapped) };
        return Err(fail(
            EXIT_REJECTED,
            if result == WAIT_OBJECT_0 + 1 {
                "Setup pipe peer exited during authentication."
            } else {
                "Setup pipe operation timed out."
            },
        ));
    }
    let mut transferred = 0;
    if unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 0) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Setup pipe overlapped operation failed.",
        ));
    }
    Ok(transferred)
}

fn new_overlapped() -> Result<(OwnedHandle, OVERLAPPED)> {
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot create setup pipe event."));
    }
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle();
    Ok((event, overlapped))
}

fn pipe_connect(
    handle: std::os::windows::io::RawHandle,
    monitor: std::os::windows::io::RawHandle,
    deadline: Instant,
) -> Result<()> {
    let (event, mut overlapped) = new_overlapped()?;
    if unsafe { ConnectNamedPipe(handle, &mut overlapped) } == 0 {
        match unsafe { GetLastError() } {
            ERROR_PIPE_CONNECTED => return Ok(()),
            ERROR_IO_PENDING => {
                wait_overlapped(
                    handle,
                    event.as_raw_handle(),
                    Some(monitor),
                    &mut overlapped,
                    deadline,
                )?;
            }
            _ => return Err(fail(EXIT_REJECTED, "The elevated worker did not connect.")),
        }
    }
    Ok(())
}

fn pipe_write(
    handle: std::os::windows::io::RawHandle,
    bytes: &[u8],
    monitor: Option<std::os::windows::io::RawHandle>,
    deadline: Instant,
) -> Result<()> {
    let (event, mut overlapped) = new_overlapped()?;
    let mut immediate = 0;
    let transferred = if unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut immediate,
            &mut overlapped,
        )
    } != 0
    {
        immediate
    } else if unsafe { GetLastError() } == ERROR_IO_PENDING {
        wait_overlapped(
            handle,
            event.as_raw_handle(),
            monitor,
            &mut overlapped,
            deadline,
        )?
    } else {
        return Err(fail(EXIT_REJECTED, "Setup pipe write failed."));
    };
    if transferred as usize != bytes.len() {
        return Err(fail(EXIT_REJECTED, "Setup pipe write was truncated."));
    }
    Ok(())
}

fn pipe_read<const N: usize>(
    handle: std::os::windows::io::RawHandle,
    monitor: Option<std::os::windows::io::RawHandle>,
    deadline: Instant,
) -> Result<[u8; N]> {
    let (event, mut overlapped) = new_overlapped()?;
    let mut bytes = [0_u8; N];
    let mut immediate = 0;
    let transferred = if unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr().cast(),
            N as u32,
            &mut immediate,
            &mut overlapped,
        )
    } != 0
    {
        immediate
    } else if unsafe { GetLastError() } == ERROR_IO_PENDING {
        wait_overlapped(
            handle,
            event.as_raw_handle(),
            monitor,
            &mut overlapped,
            deadline,
        )?
    } else {
        return Err(fail(EXIT_REJECTED, "Setup pipe read failed."));
    };
    if transferred as usize != N {
        return Err(fail(EXIT_REJECTED, "Setup pipe frame was truncated."));
    }
    Ok(bytes)
}

#[derive(PartialEq, Eq)]
struct PeerClaims {
    user_sid: Vec<u8>,
    authentication_id: (u32, i32),
    session_id: u32,
    integrity_rid: u32,
}

fn token_information(token: std::os::windows::io::RawHandle, class: i32) -> Result<Vec<usize>> {
    let mut length = 0;
    unsafe { GetTokenInformation(token, class, ptr::null_mut(), 0, &mut length) };
    if length == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot size setup peer token information.",
        ));
    }
    let mut bytes = vec![0_usize; (length as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, bytes.as_mut_ptr().cast(), length, &mut length) }
        == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read setup peer token information.",
        ));
    }
    Ok(bytes)
}

fn peer_claims(pid: u32) -> Result<PeerClaims> {
    let process_raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process_raw.is_null() {
        return Err(fail(EXIT_REJECTED, "Cannot inspect setup peer token."));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process_raw) };
    let mut token_raw = ptr::null_mut();
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token_raw) } == 0 {
        return Err(fail(EXIT_REJECTED, "Cannot open setup peer token."));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token_raw) };
    let user = token_information(token.as_raw_handle(), TokenUser)?;
    let user = unsafe { &*(user.as_ptr().cast::<TOKEN_USER>()) };
    let sid_length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if sid_length == 0 {
        return Err(fail(EXIT_REJECTED, "Setup peer user SID is invalid."));
    }
    let user_sid =
        unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), sid_length) }.to_vec();
    let statistics = token_information(token.as_raw_handle(), TokenStatistics)?;
    let statistics = unsafe { &*(statistics.as_ptr().cast::<TOKEN_STATISTICS>()) };
    let session = token_information(token.as_raw_handle(), TokenSessionId)?;
    if session.is_empty() {
        return Err(fail(EXIT_REJECTED, "Setup peer session is invalid."));
    }
    let session_id = u32::try_from(session[0] & u32::MAX as usize)
        .map_err(|_| fail(EXIT_REJECTED, "Setup peer session is invalid."))?;
    let integrity = token_information(token.as_raw_handle(), TokenIntegrityLevel)?;
    let integrity = unsafe { &*(integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()) };
    let count = unsafe { *GetSidSubAuthorityCount(integrity.Label.Sid) } as u32;
    if count == 0 {
        return Err(fail(EXIT_REJECTED, "Setup peer integrity SID is invalid."));
    }
    let integrity_rid = unsafe { *GetSidSubAuthority(integrity.Label.Sid, count - 1) };
    Ok(PeerClaims {
        user_sid,
        authentication_id: (
            statistics.AuthenticationId.LowPart,
            statistics.AuthenticationId.HighPart,
        ),
        session_id,
        integrity_rid,
    })
}

fn verify_peer_claims(controller: u32, worker: u32) -> Result<[u8; 32]> {
    let controller_claims = peer_claims(controller)?;
    let worker_claims = peer_claims(worker)?;
    if controller_claims.user_sid != worker_claims.user_sid
        || controller_claims.authentication_id != worker_claims.authentication_id
        || controller_claims.session_id != worker_claims.session_id
        || worker_claims.integrity_rid < controller_claims.integrity_rid
    {
        return Err(fail(
            EXIT_REJECTED,
            "Setup peers do not share the required user, logon, session, and integrity lineage.",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(&controller_claims.user_sid);
    hash.update(controller_claims.authentication_id.0.to_le_bytes());
    hash.update(controller_claims.authentication_id.1.to_le_bytes());
    hash.update(controller_claims.session_id.to_le_bytes());
    hash.update(controller_claims.integrity_rid.to_le_bytes());
    hash.update(worker_claims.integrity_rid.to_le_bytes());
    Ok(hash.finalize().into())
}

fn process_image(pid: u32) -> Result<PathBuf> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the setup peer process.",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read the setup peer image path.",
        ));
    }
    path.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_wide(&path)))
}
fn hash_reader(file: &mut File) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_failure)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}
fn file_hash(path: &Path) -> Result<[u8; 32]> {
    hash_reader(&mut File::open(path).map_err(io_failure)?)
}
fn retained_file_hash(path: &Path) -> Result<(File, [u8; 32])> {
    let mut file = File::from(open_plain_handle(path, false, false)?);
    let hash = hash_reader(&mut file)?;
    Ok((file, hash))
}

fn pending_uninstall_transaction(paths: &Paths) -> Result<bool> {
    if !path_present(&paths.transaction)? {
        return Ok(false);
    }
    assert_plain_file(&paths.transaction)?;
    let transaction: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
    if transaction.schema_version != TRANSACTION_SCHEMA {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction schema is invalid.",
        ));
    }
    Ok(transaction.action == "uninstall"
        && matches!(
            transaction.phase.as_str(),
            "uninstall-armed"
                | "uninstalling"
                | "uninstall-cleanup-owned"
                | "uninstall-quarantined"
                | "recovering-finish-uninstall"
                | "uninstall-cleanup-complete"
                | "uninstall-finalizer-publishing"
                | "uninstall-finalizer-published"
                | "uninstall-finalizer-deletion-owned"
                | "uninstall-terminal-committing"
                | "uninstall-app-path-retiring"
                | "uninstall-app-path-retired"
                | "uninstall-registration-retiring"
                | "uninstall-registration-retired"
        ))
}

fn arm_relocated_uninstall_controller(server: &OwnedHandle) -> Result<()> {
    pipe_write(
        server.as_raw_handle(),
        b"TQ-UNINSTALL-JOURNALED",
        None,
        Instant::now() + Duration::from_secs(30),
    )?;
    if pipe_read::<18>(
        server.as_raw_handle(),
        None,
        Instant::now() + Duration::from_secs(30),
    )? != *b"TQ-UNINSTALL-ARMED"
    {
        return Err(fail(
            EXIT_REJECTED,
            "Uninstall controller did not commit mapped-image deletion ownership.",
        ));
    }
    Ok(())
}

fn authorize_uninstall_controller(paths: &Paths, current: &Path) -> Result<()> {
    let controller = process_image(parent_process_id()?)?;
    let installed = paths.install.join("Uninstall Talking Quill.exe");
    let expected = if is_uninstall_finalizer(&controller)? {
        controller.clone()
    } else if path_present(&installed)? {
        installed
    } else if path_present(&paths.maintenance_uninstaller)? {
        paths.maintenance_uninstaller.clone()
    } else {
        assert_plain_file(&paths.transaction)?;
        let transaction: Transaction =
            serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
                .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
        if transaction.schema_version != TRANSACTION_SCHEMA
            || transaction.action != "uninstall"
            || !matches!(
                transaction.phase.as_str(),
                "uninstall-cleanup-owned"
                    | "uninstall-quarantined"
                    | "recovering-finish-uninstall"
                    | "uninstall-cleanup-complete"
                    | "uninstall-finalizer-publishing"
                    | "uninstall-finalizer-published"
                    | "uninstall-finalizer-deletion-owned"
                    | "uninstall-terminal-committing"
                    | "uninstall-app-path-retiring"
                    | "uninstall-app-path-retired"
                    | "uninstall-registration-retiring"
                    | "uninstall-registration-retired"
            )
        {
            return Err(fail(
                EXIT_REJECTED,
                "Uninstall cleanup lacks a protected durable authorization.",
            ));
        }
        current.to_owned()
    };
    assert_plain_file(&expected)?;
    if file_hash(&controller)? != file_hash(&expected)? {
        return Err(fail(
            EXIT_REJECTED,
            "Uninstall requires an authenticated exact copy of the installed controller image.",
        ));
    }
    Ok(())
}

fn validate_predecessor_arguments(package: &ParsedPackage, current: &Path) -> Result<()> {
    let predecessor = package
        .manifest
        .predecessor
        .as_ref()
        .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
    let expected = [
        ("/TQUPDATE=", hex_hash(&file_hash(current)?)),
        ("/TQGATEWAYHASH=", predecessor.gateway_sha256.clone()),
        ("/TQOWNERHASH=", predecessor.owner_sha256.clone()),
        ("/TQLAYOUT=", predecessor.release_build_digest.clone()),
    ];
    let arguments: Vec<String> = std::env::args_os()
        .skip(2)
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    if expected.iter().any(|(prefix, value)| {
        !arguments
            .iter()
            .any(|argument| argument == &format!("{prefix}{value}"))
    }) {
        return Err(fail(
            EXIT_REJECTED,
            "Authenticated predecessor arguments do not bind the exact package and installed identities.",
        ));
    }
    Ok(())
}

fn protected_file_handle_acl_is_exact(file: &File) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(EXIT_REJECTED, "Cannot inspect protected file ACL."));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(EXIT_REJECTED, "Cannot encode protected file ACL."));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    Ok([
        "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)",
        "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)",
    ]
    .iter()
    .any(|value| sddl.eq_ignore_ascii_case(value)))
}

fn staged_path_is_protected(path: &Path, directory: bool) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect staged helper protection.",
        ));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode staged helper protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    let expected = if directory {
        [
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)",
        ]
    } else {
        [
            "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)",
        ]
    };
    Ok(expected
        .iter()
        .any(|value| sddl.eq_ignore_ascii_case(value)))
}

fn medium_launcher_directory_is_protected(path: &Path) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect launcher staging protection.",
        ));
    }
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode launcher staging protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.eq_ignore_ascii_case("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)"))
}

fn authenticate_predecessor_helper(package: &ParsedPackage, paths: &Paths) -> Result<()> {
    if package.manifest.package_mode != "update" {
        return Err(fail(
            EXIT_REJECTED,
            "A medium setup controller is required.",
        ));
    }
    let previous = package
        .manifest
        .predecessor
        .as_ref()
        .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
    let parent = parent_process_id()?;
    let parent_image = process_image(parent)?;
    let (_parent_image_lock, parent_hash) = retained_file_hash(&parent_image)?;
    let installed_gateway = paths
        .install
        .join("resources/helper/talking-quill-helper.exe");
    let parent_is_installed = path_present(&installed_gateway)?
        && canonical(&parent_image)? == canonical(&installed_gateway)?;
    let parent_is_staged = parent_image
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("talking-quill-update-bootstrap.exe"))
        && parent_image
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| {
                canonical(root).ok().as_deref() == canonical(&paths.program_data).ok().as_deref()
            })
        && parent_image
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                let prefix = ".Talking Quill.update-bootstrap-";
                name.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            })
        && parent_image.parent().is_some_and(|parent| {
            staged_path_is_protected(parent, true).unwrap_or(false)
                && staged_path_is_protected(&parent_image, false).unwrap_or(false)
        });
    let installed_hash_matches = if parent_is_installed {
        parent_hash == file_hash(&installed_gateway)?
    } else {
        true
    };
    if (!parent_is_installed && !parent_is_staged)
        || !installed_hash_matches
        || hex_hash(&parent_hash) != previous.gateway_sha256
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update was not invoked by the exact authenticated predecessor helper.",
        ));
    }
    Ok(())
}

fn parent_process_id() -> Result<u32> {
    process_parent_id(std::process::id())
}

fn process_parent_id(process_id: u32) -> Result<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the update parent process.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while available {
        if entry.th32ProcessID == process_id {
            return Ok(entry.th32ParentProcessID);
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Err(fail(
        EXIT_REJECTED,
        "The update parent process is unavailable.",
    ))
}

fn hex_bytes(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_hash(value: &[u8; 32]) -> String {
    hex_bytes(value)
}

fn installed_matches_target(
    package: &ParsedPackage,
    paths: &Paths,
    candidate: &Path,
) -> Result<bool> {
    let installed_path = paths
        .install
        .join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&installed_path)?;
    let installed: serde_json::Value =
        serde_json::from_slice(&fs::read(installed_path).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
    let role = |name: &str| {
        installed
            .get("roles")
            .and_then(|value| value.as_array())
            .and_then(|roles| {
                roles
                    .iter()
                    .find(|role| role.get("role").and_then(|value| value.as_str()) == Some(name))
            })
            .and_then(|role| role.get("sha256"))
            .and_then(|value| value.as_str())
    };
    let installed_setup = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&installed_setup)?;
    let exact_setup = file_hash(candidate)? == file_hash(&installed_setup)?;
    // Fault-bearing packages are non-promotable process-test artifacts. Their crash
    // seam is package-bound (never command-line authority) and can only target the
    // exact installed release identity.
    let acceptance_fault = cfg!(feature = "acceptance-faults")
        && package.manifest.fault_phase.is_some()
        && package.manifest.package_mode == "repair";
    Ok((exact_setup || acceptance_fault)
        && installed.get("version").and_then(|value| value.as_str())
            == Some(package.manifest.version.as_str())
        && installed
            .get("architecture")
            .and_then(|value| value.as_str())
            == Some(package.manifest.architecture.as_str())
        && installed
            .get("releaseBuildDigest")
            .and_then(|value| value.as_str())
            == Some(package.manifest.target.release_build_digest.as_str())
        && role("gateway") == Some(package.manifest.target.gateway_sha256.as_str())
        && role("owner") == Some(package.manifest.target.owner_sha256.as_str())
        && role("recovery-launcher")
            == Some(package.manifest.target.recovery_launcher_sha256.as_str()))
}

fn authorize_package_mode(
    package: &ParsedPackage,
    paths: &Paths,
    candidate: &Path,
    predecessor_authorized: bool,
    requested: Action,
) -> Result<Action> {
    // The installed image is its own production repair authority. A renamed exact copy
    // may repair the same target without introducing a separately trusted repair binary.
    let installed_setup = paths.install.join("Uninstall Talking Quill.exe");
    if requested == Action::Repair
        && paths.install.exists()
        && installed_setup.exists()
        && file_hash(candidate)? == file_hash(&installed_setup)?
        && installed_matches_target(package, paths, candidate)?
    {
        return Ok(Action::Repair);
    }
    match package.manifest.package_mode.as_str() {
        "fresh" if !paths.install.exists() => Ok(Action::Install),
        "repair"
            if requested == Action::Repair
                && paths.install.exists()
                && installed_matches_target(package, paths, candidate)? =>
        {
            Ok(Action::Repair)
        }
        "update" if paths.install.exists() && predecessor_authorized => {
            let previous = package
                .manifest
                .predecessor
                .as_ref()
                .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
            let installed_path = paths
                .install
                .join("resources/keyboard-owner-release-v1.json");
            assert_plain_file(&installed_path)?;
            let installed: serde_json::Value =
                serde_json::from_slice(&fs::read(installed_path).map_err(io_failure)?)
                    .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
            let role = |name: &str| {
                installed
                    .get("roles")
                    .and_then(|value| value.as_array())
                    .and_then(|roles| {
                        roles.iter().find(|role| {
                            role.get("role").and_then(|value| value.as_str()) == Some(name)
                        })
                    })
                    .and_then(|role| role.get("sha256"))
                    .and_then(|value| value.as_str())
            };
            if installed.get("version").and_then(|value| value.as_str()) != Some(&previous.version)
                || installed
                    .get("architecture")
                    .and_then(|value| value.as_str())
                    != Some(&package.manifest.architecture)
                || installed
                    .get("releaseBuildDigest")
                    .and_then(|value| value.as_str())
                    != Some(&previous.release_build_digest)
                || role("gateway") != Some(&previous.gateway_sha256)
                || role("owner") != Some(&previous.owner_sha256)
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Update does not authorize the exact installed predecessor.",
                ));
            }
            Ok(Action::Update)
        }
        _ => Err(fail(
            EXIT_REJECTED,
            "Package mode does not match independently derived machine state.",
        )),
    }
}

fn is_uninstall_finalizer(path: &Path) -> Result<bool> {
    if !path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(UNINSTALL_FINALIZER_NAME))
    {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    if !parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.strip_prefix(UNINSTALL_FINALIZER_PREFIX)
                .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
        })
        || !medium_launcher_directory_is_protected(parent)?
    {
        return Ok(false);
    }
    let identity =
        owned_tree_identity(parent).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let marker = parent.join("finalizer-tree-identity-v1");
    Ok(
        marker_security_is_exact(&marker, MEDIUM_FINALIZER_FILE_SDDL)?
            && marker_security_is_exact(path, MEDIUM_FINALIZER_FILE_SDDL)?
            && fs::read_to_string(marker).is_ok_and(|value| value == identity),
    )
}

fn derive_action(current: &Path, paths: &Paths) -> Result<Action> {
    let maintenance = canonical(current)
        .ok()
        .zip(canonical(&paths.maintenance_uninstaller).ok())
        .is_some_and(|(current, maintenance)| current == maintenance);
    let finalizer = is_uninstall_finalizer(current)?;
    if maintenance
        || finalizer
        || current
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Uninstall Talking Quill.exe"))
    {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Invalid installed setup path."))?;
        if !maintenance && !finalizer && canonical(parent)? != canonical(&paths.install)? {
            return Err(fail(
                EXIT_REJECTED,
                "Uninstall image is outside the installed tree.",
            ));
        }
        return Ok(Action::Uninstall);
    }
    if pending_uninstall_transaction(paths)? {
        Ok(Action::Install)
    } else if paths.install.exists() {
        Ok(Action::Repair)
    } else {
        Ok(Action::Install)
    }
}

fn install(
    image: &mut File,
    package: &ParsedPackage,
    current: &Path,
    paths: &Paths,
    action: Action,
    system: &dyn NativeSystemAdapter,
) -> Result<()> {
    assert_plain_absent(&paths.staging)?;
    let had_predecessor = path_present(&paths.install)?;
    write_transaction(paths, "staging", action, had_predecessor)?;
    fs::create_dir(&paths.staging).map_err(io_failure)?;
    assert_plain_directory(&paths.staging)?;
    for entry in &package.manifest.files {
        let destination = paths.staging.join(entry.path.replace('/', "\\"));
        let parent = destination
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Package path has no parent."))?;
        create_plain_directories(&paths.staging, parent)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(io_failure)?;
        package::extract_file(image, package, entry, &mut output)
            .map_err(|error| fail(EXIT_REJECTED, format!("Package block failed: {error:?}")))?;
        output.sync_all().map_err(io_failure)?;
    }
    validate_staged_release_identity(package, &paths.staging)?;
    let uninstaller_path = paths.staging.join("Uninstall Talking Quill.exe");
    let mut uninstaller = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&uninstaller_path)
        .map_err(io_failure)?;
    let mut setup_image = File::open(current).map_err(io_failure)?;
    std::io::copy(&mut setup_image, &mut uninstaller).map_err(io_failure)?;
    uninstaller.sync_all().map_err(io_failure)?;
    drop(uninstaller);
    if file_hash(&uninstaller_path)? != file_hash(current)? {
        return Err(fail(
            EXIT_REJECTED,
            "Staged uninstaller verification failed.",
        ));
    }
    write_transaction(paths, "staged", action, had_predecessor)?;
    crash_at(package, "staged");
    write_transaction(paths, "prepared", action, had_predecessor)?;
    crash_at(package, "prepared");
    if had_predecessor {
        durable_rename(&paths.install, &paths.backup)?;
        write_transaction(paths, "predecessor-moved", action, had_predecessor)?;
        crash_at(package, "predecessorMoved");
    }
    write_transaction(paths, "publishing", action, had_predecessor)?;
    crash_at(package, "publishing");
    durable_rename(&paths.staging, &paths.install)?;
    write_transaction(paths, "published-before-persist", action, had_predecessor)?;
    crash_at(package, "publishedBeforePersist");
    write_transaction(paths, "published", action, had_predecessor)?;
    crash_at(package, "published");
    ensure_maintenance_uninstaller(paths)?;
    ensure_machine_relaunch_owner_installed(paths)?;
    system.register_version(paths, &package.manifest.version)?;
    write_transaction(paths, "registered", action, had_predecessor)?;
    crash_at(package, "registered");
    write_transaction(paths, "committed", action, had_predecessor)?;
    crash_at(package, "committed");
    write_transaction(paths, "legacy-retiring", action, had_predecessor)?;
    crash_at(package, "legacyRetiring");
    system.retire_legacy(paths)?;
    write_transaction(paths, "legacy-retired", action, had_predecessor)?;
    crash_at(package, "legacyRetired");
    remove_plain_tree(&paths.backup)?;
    remove_transaction(paths)?;
    Ok(())
}

fn validate_staged_release_identity(package: &ParsedPackage, staging: &Path) -> Result<()> {
    let path = staging.join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&path)?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).map_err(io_failure)?)
        .map_err(|_| fail(EXIT_REJECTED, "Staged release identity is invalid."))?;
    let role = |name: &str| {
        value
            .get("roles")
            .and_then(|roles| roles.as_array())
            .and_then(|roles| {
                roles
                    .iter()
                    .find(|role| role.get("role").and_then(|role| role.as_str()) == Some(name))
            })
            .and_then(|role| role.get("sha256"))
            .and_then(|hash| hash.as_str())
    };
    let roles_match = value
        .get("roles")
        .and_then(|roles| roles.as_array())
        .is_some_and(|roles| {
            let expected = [
                (
                    "gateway",
                    "resources/helper/talking-quill-helper.exe",
                    false,
                ),
                (
                    "owner",
                    "resources/helper/talking-quill-keyboard-owner.exe",
                    true,
                ),
                (
                    "recovery-launcher",
                    "resources/helper/talking-quill-update-recovery-launcher.exe",
                    false,
                ),
            ];
            roles.len() == expected.len()
                && roles.iter().zip(expected).all(|(role, expected)| {
                    role.get("role").and_then(|value| value.as_str()) == Some(expected.0)
                        && role.get("path").and_then(|value| value.as_str()) == Some(expected.1)
                        && role
                            .get("suppressionCapable")
                            .and_then(|value| value.as_bool())
                            == Some(expected.2)
                })
        });
    let predecessor_matches = match (&package.manifest.predecessor, value.get("predecessor")) {
        (None, Some(previous)) => previous.is_null(),
        (Some(expected), Some(previous)) => {
            previous.get("version").and_then(|item| item.as_str())
                == Some(expected.version.as_str())
                && previous
                    .get("releaseBuildDigest")
                    .and_then(|item| item.as_str())
                    == Some(expected.release_build_digest.as_str())
                && previous.get("gatewaySha256").and_then(|item| item.as_str())
                    == Some(expected.gateway_sha256.as_str())
                && previous.get("ownerSha256").and_then(|item| item.as_str())
                    == Some(expected.owner_sha256.as_str())
        }
        _ => false,
    };
    let acceptance_repair = cfg!(feature = "acceptance-faults")
        && package.manifest.fault_phase.is_some()
        && package.manifest.package_mode == "repair";
    if !roles_match
        || value.get("version").and_then(|item| item.as_str())
            != Some(package.manifest.version.as_str())
        || value.get("architecture").and_then(|item| item.as_str())
            != Some(package.manifest.architecture.as_str())
        || value.get("sourceCommit").and_then(|item| item.as_str())
            != Some(package.manifest.source_commit.as_str())
        || value.get("sourceTree").and_then(|item| item.as_str())
            != Some(package.manifest.source_tree.as_str())
        || (!acceptance_repair
            && value.get("packageMode").and_then(|item| item.as_str())
                != Some(package.manifest.package_mode.as_str()))
        || value
            .get("releaseBuildDigest")
            .and_then(|item| item.as_str())
            != Some(package.manifest.target.release_build_digest.as_str())
        || role("gateway") != Some(package.manifest.target.gateway_sha256.as_str())
        || role("owner") != Some(package.manifest.target.owner_sha256.as_str())
        || role("recovery-launcher")
            != Some(package.manifest.target.recovery_launcher_sha256.as_str())
        || (!acceptance_repair && !predecessor_matches)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Staged release identity does not match TQPKG2.",
        ));
    }
    for relative in [
        "resources/helper/talking-quill-helper.exe",
        "resources/helper/talking-quill-keyboard-owner.exe",
        "resources/helper/talking-quill-update-recovery-launcher.exe",
    ] {
        validate_staged_native_role(
            &staging.join(relative),
            &package.manifest.architecture,
            &package.manifest.source_commit,
            &package.manifest.source_tree,
        )?;
    }
    Ok(())
}

fn validate_staged_native_role(
    path: &Path,
    architecture: &str,
    source_commit: &str,
    source_tree: &str,
) -> Result<()> {
    let bytes = fs::read(path).map_err(io_failure)?;
    let pe = bytes
        .get(60..64)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .map(|value| value as usize);
    let machine = pe.and_then(|offset| {
        (bytes.get(offset..offset + 4) == Some(b"PE\0\0"))
            .then(|| bytes.get(offset + 4..offset + 6))
            .flatten()
            .and_then(|value| value.try_into().ok())
            .map(u16::from_le_bytes)
    });
    let expected_machine = match architecture {
        "x64" => 0x8664,
        "arm64" => 0xaa64,
        _ => 0,
    };
    let marker_is_exact = |name: &str, expected: &str| {
        let prefix = format!("{name}=");
        let matches: Vec<&[u8]> = bytes
            .windows(prefix.len())
            .enumerate()
            .filter(|(_, window)| *window == prefix.as_bytes())
            .filter_map(|(offset, _)| bytes.get(offset + prefix.len()..offset + prefix.len() + 40))
            .collect();
        matches.len() == 1 && matches[0] == expected.as_bytes()
    };
    if bytes.get(..2) != Some(b"MZ")
        || machine != Some(expected_machine)
        || !marker_is_exact("TALKING_QUILL_SOURCE_COMMIT", source_commit)
        || !marker_is_exact("TALKING_QUILL_SOURCE_TREE", source_tree)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Staged native role architecture or source identity is invalid.",
        ));
    }
    Ok(())
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn retire_legacy_authority(paths: &Paths) -> Result<()> {
    const LEGACY_SERVICE: &str = "TalkingQuillKeyboardAuthority";
    let manager_raw = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager_raw.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open Windows Service Control Manager.",
        ));
    }
    let manager = ServiceHandle(manager_raw);
    let service_name = wide(OsStr::new(LEGACY_SERVICE));
    let service_raw = unsafe {
        OpenServiceW(
            manager.0,
            service_name.as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_STOP | DELETE,
        )
    };
    if service_raw.is_null() && unsafe { GetLastError() } != 1060 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect the exact legacy service registration.",
        ));
    }
    if !service_raw.is_null() {
        let service = ServiceHandle(service_raw);
        let mut needed = 0;
        unsafe { QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect legacy service identity.",
            ));
        }
        let mut config = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
        if unsafe {
            QueryServiceConfigW(
                service.0,
                config.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(fail(EXIT_REJECTED, "Cannot read legacy service identity."));
        }
        let config = unsafe { &*(config.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()) };
        let binary = if config.lpBinaryPathName.is_null() {
            String::new()
        } else {
            unsafe { wide_ptr_string(config.lpBinaryPathName) }
        };
        let executable = service_executable(&binary)
            .ok_or_else(|| fail(EXIT_REJECTED, "Legacy service binary command is invalid."))?;
        if !owned_legacy_executable(&executable, paths)? {
            return Err(fail(
                EXIT_REJECTED,
                "Legacy service name belongs to an unexpected executable identity.",
            ));
        }
        let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(fail(EXIT_FAILURE, "Cannot query legacy service status."));
        }
        if status.dwCurrentState != SERVICE_STOPPED
            && unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0
            && unsafe { GetLastError() } != 1062
        {
            return Err(fail(EXIT_FAILURE, "Cannot stop the exact legacy service."));
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
                return Err(fail(EXIT_FAILURE, "Cannot poll legacy service stop."));
            }
            if status.dwCurrentState == SERVICE_STOPPED {
                break;
            }
            if Instant::now() >= deadline {
                return Err(fail(
                    EXIT_FAILURE,
                    "Legacy service did not stop before the retirement deadline.",
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if unsafe { DeleteService(service.0) } == 0 {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot delete the exact legacy service.",
            ));
        }
    }
    retire_legacy_task(paths)?;
    if paths.legacy_authority.exists() {
        assert_plain_absent(&paths.legacy_quarantine)?;
        durable_rename(&paths.legacy_authority, &paths.legacy_quarantine)?;
    }
    remove_plain_tree(&paths.legacy_quarantine)?;
    Ok(())
}

fn service_executable(command: &str) -> Option<PathBuf> {
    let value = command.trim();
    let executable = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"')?.0
    } else {
        value.split_whitespace().next()?
    };
    if executable.is_empty() {
        None
    } else {
        Some(PathBuf::from(executable))
    }
}

fn owned_legacy_executable(executable: &Path, paths: &Paths) -> Result<bool> {
    assert_plain_file(executable)?;
    canonical_path_is_within(executable, &paths.legacy_authority)
}

fn retire_legacy_task(paths: &Paths) -> Result<()> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let result = (|| {
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
                .map_err(|_| fail(EXIT_FAILURE, "Cannot create Task Scheduler service."))?;
        unsafe {
            service.Connect(
                &VARIANT::default(),
                &VARIANT::default(),
                &VARIANT::default(),
                &VARIANT::default(),
            )
        }
        .map_err(|_| fail(EXIT_FAILURE, "Cannot connect to Task Scheduler."))?;
        let root = unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|_| fail(EXIT_FAILURE, "Cannot open the Task Scheduler root."))?;
        let name = BSTR::from("TalkingQuillKeyboardAuthority");
        match unsafe { root.GetTask(&name) } {
            Ok(task) => {
                let actions = unsafe { task.Definition() }
                    .and_then(|definition| unsafe { definition.Actions() })
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot inspect legacy task actions."))?;
                let mut count = 0;
                unsafe { actions.Count(&mut count) }
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot count legacy task actions."))?;
                if count != 1 {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Legacy task must have exactly one owned action.",
                    ));
                }
                let action = unsafe { actions.get_Item(1) }
                    .and_then(|action| action.cast::<IExecAction>())
                    .map_err(|_| {
                        fail(
                            EXIT_REJECTED,
                            "Legacy task action is not an executable action.",
                        )
                    })?;
                let mut executable = BSTR::new();
                unsafe { action.Path(&mut executable) }
                    .map_err(|_| fail(EXIT_REJECTED, "Cannot read legacy task executable."))?;
                let executable = PathBuf::from(executable.to_string());
                if !owned_legacy_executable(&executable, paths)? {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Legacy task name belongs to an unexpected executable identity.",
                    ));
                }
                let _ = unsafe { task.Stop(0) };
                unsafe { root.DeleteTask(&name, 0) }.map_err(|_| {
                    fail(
                        EXIT_FAILURE,
                        "Cannot delete the exact legacy scheduled task.",
                    )
                })?;
            }
            Err(error) if error.code().0 == -2147024894 => {}
            Err(_) => return Err(fail(EXIT_FAILURE, "Cannot inspect legacy scheduled task.")),
        }
        if paths.legacy_task_file.exists() {
            return Err(fail(
                EXIT_REJECTED,
                "Task Scheduler retained the legacy task file.",
            ));
        }
        Ok(())
    })();
    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn wide_ptr_string(pointer: *const u16) -> String {
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
}

#[cfg(feature = "acceptance-faults")]
fn crash_at(package: &ParsedPackage, phase: &str) {
    if package.manifest.fault_phase.as_deref() == Some(phase) {
        std::process::exit(197);
    }
}

#[cfg(not(feature = "acceptance-faults"))]
fn crash_at(package: &ParsedPackage, _phase: &str) {
    debug_assert!(package.manifest.fault_phase.is_none());
}

fn uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    defer_mapped_controller_cleanup: bool,
    machine_lock: &mut Option<MachineLock>,
) -> Result<()> {
    write_transaction(paths, "uninstalling", Action::Uninstall, true)?;
    // Keep both authenticated recovery entry points durable until a relocated cleanup worker
    // has committed all machine cleanup and reported success to its supervising worker.
    system.register_installed(paths)?;
    write_transaction(paths, "uninstall-cleanup-owned", Action::Uninstall, true)?;
    ensure_uninstall_finalizer_registered(&std::env::current_exe().map_err(io_failure)?, paths)?;
    system.retire_legacy(paths)?;
    system.clear_update_recovery(paths)?;
    if defer_mapped_controller_cleanup && path_present(&paths.install)? {
        remove_plain_tree(&paths.backup)?;
        durable_rename(&paths.install, &paths.backup)?;
        write_transaction(paths, "uninstall-quarantined", Action::Uninstall, true)?;
        remove_plain_tree(&paths.staging)?;
        // Hand the exclusive machine lock to the authenticated child. A zero exit is its durable
        // completion report. Reacquire before clearing terminal recovery records.
        drop(machine_lock.take());
        launch_same_token_uninstall_cleanup(&std::env::current_exe().map_err(io_failure)?)?;
        *machine_lock = Some(MachineLock::acquire(
            paths,
            120_000,
            installed_recovery_policy_epoch(paths)?,
        )?);
        if !path_present(&paths.transaction)? && !path_present(&paths.install)? {
            return Ok(());
        }
        require_uninstall_cleanup_complete(paths)?;
        return Ok(());
    }
    finish_uninstall_machine_cleanup(paths, system)
}

fn finish_uninstall_machine_cleanup(paths: &Paths, system: &dyn NativeSystemAdapter) -> Result<()> {
    write_transaction(
        paths,
        "recovering-finish-uninstall",
        Action::Uninstall,
        true,
    )?;
    system.retire_legacy(paths)?;
    system.clear_update_recovery(paths)?;
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    write_transaction(paths, "uninstall-cleanup-complete", Action::Uninstall, true)
}

fn require_uninstall_cleanup_complete(paths: &Paths) -> Result<()> {
    assert_plain_file(&paths.transaction)?;
    let value: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer cleanup journal is invalid."))?;
    if value.schema_version != TRANSACTION_SCHEMA
        || value.action != "uninstall"
        || value.phase != "uninstall-cleanup-complete"
    {
        return Err(fail(
            EXIT_REJECTED,
            "Relocated uninstall cleanup did not commit completion.",
        ));
    }
    Ok(())
}

fn retire_and_remove_machine_lock(
    paths: &Paths,
    machine_lock: &mut Option<MachineLock>,
) -> Result<Option<LegacyMutexPair>> {
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    Ok(legacy)
}

fn complete_terminal_uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
    machine_lock: &mut Option<MachineLock>,
) -> Result<()> {
    require_uninstall_cleanup_complete(paths)?;
    let generation = if let Some(record) = read_terminal_uninstall_record(paths)? {
        if record.phase != "cleanup-complete" {
            resume_terminal_service(paths, &record)?;
        }
        record.generation
    } else {
        finalize_uninstall(paths, system, current)?
    };
    // The service cannot acquire the lifecycle lock until its creator releases it.
    drop(machine_lock.take());
    wait_for_terminal_service_retirement(paths, &generation)
}

fn finalize_uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
) -> Result<String> {
    require_uninstall_cleanup_complete(paths)?;
    // Publish and start the protected SCM owner before retiring any callable registration.
    register_uninstall_executable(&paths.maintenance_uninstaller)?;
    let terminal_generation = publish_terminal_uninstall_record(paths, current)?;
    let _ = system;
    Ok(terminal_generation)
}

fn retire_terminal_machine_state(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
    generation: &str,
) -> Result<()> {
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    if record.phase == "armed" {
        if path_present(&paths.transaction)? {
            recover_with_adapter(paths, system)?;
            require_uninstall_cleanup_complete(paths)?;
        }
        write_transaction(
            paths,
            "uninstall-app-path-retiring",
            Action::Uninstall,
            true,
        )?;
        system.unregister_app_path()?;
        write_transaction(paths, "uninstall-app-path-retired", Action::Uninstall, true)?;
        register_uninstall_executable(&paths.maintenance_uninstaller)?;
        let _ = current;
        // Keep the journal and registered maintenance command as recovery authority while the
        // service performs filesystem cleanup.
        write_transaction(paths, "uninstall-cleanup-complete", Action::Uninstall, true)?;
        write_terminal_uninstall_phase(paths, generation, "machine-retired")?;
    }
    let retired = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if retired.generation != generation || retired.phase != "machine-retired" {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall machine retirement is invalid.",
        ));
    }
    // The maintenance registration remains callable until it observes stopped-success and
    // retires the service and image.
    register_uninstall_executable(&paths.maintenance_uninstaller)
}

#[derive(Debug, PartialEq, Eq)]
enum RecoveryPlan {
    RestorePredecessor,
    DiscardStaging,
    RemoveFreshCandidate,
    FinishCommit,
    FinishUninstall,
}

fn recovery_plan(
    value: &Transaction,
    backup_exists: bool,
    install_exists: bool,
) -> Result<RecoveryPlan> {
    if !matches!(
        value.action.as_str(),
        "install" | "update" | "repair" | "uninstall"
    ) || (value.action == "update" && !value.had_predecessor)
        || (value.action == "repair" && !value.had_predecessor)
        || (value.action == "uninstall"
            && !matches!(
                value.phase.as_str(),
                "uninstall-armed"
                    | "uninstalling"
                    | "uninstall-cleanup-owned"
                    | "uninstall-quarantined"
                    | "recovering-finish-uninstall"
                    | "uninstall-cleanup-complete"
                    | "uninstall-finalizer-publishing"
                    | "uninstall-finalizer-published"
                    | "uninstall-finalizer-deletion-owned"
                    | "uninstall-terminal-committing"
                    | "uninstall-app-path-retiring"
                    | "uninstall-app-path-retired"
                    | "uninstall-registration-retiring"
                    | "uninstall-registration-retired"
            ))
    {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction action is invalid.",
        ));
    }
    match value.phase.as_str() {
        "staging" | "staged" | "prepared" | "publishing"
            if !backup_exists && install_exists == value.had_predecessor =>
        {
            Ok(RecoveryPlan::DiscardStaging)
        }
        "prepared" | "publishing" | "published-before-persist"
            if !value.had_predecessor && !backup_exists && install_exists =>
        {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "staging"
        | "staged"
        | "prepared"
        | "predecessor-moved"
        | "publishing"
        | "published-before-persist"
        | "published"
        | "registered"
            if value.had_predecessor && backup_exists =>
        {
            Ok(RecoveryPlan::RestorePredecessor)
        }
        "published" | "registered"
            if !value.had_predecessor && !backup_exists && install_exists =>
        {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "committed" | "legacy-retiring" | "legacy-retired" if install_exists => {
            Ok(RecoveryPlan::FinishCommit)
        }
        "recovering-restore-predecessor"
            if value.had_predecessor && (backup_exists || install_exists) =>
        {
            Ok(RecoveryPlan::RestorePredecessor)
        }
        "recovering-discard-staging"
            if !backup_exists && install_exists == value.had_predecessor =>
        {
            Ok(RecoveryPlan::DiscardStaging)
        }
        "recovering-remove-fresh" if !value.had_predecessor && !backup_exists => {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "recovering-finish-commit" if install_exists => Ok(RecoveryPlan::FinishCommit),
        "uninstall-armed"
        | "uninstalling"
        | "uninstall-cleanup-owned"
        | "uninstall-quarantined"
        | "recovering-finish-uninstall"
        | "uninstall-cleanup-complete"
        | "uninstall-finalizer-publishing"
        | "uninstall-finalizer-published"
        | "uninstall-finalizer-deletion-owned"
        | "uninstall-terminal-committing"
        | "uninstall-app-path-retiring"
        | "uninstall-app-path-retired"
        | "uninstall-registration-retiring"
        | "uninstall-registration-retired" => Ok(RecoveryPlan::FinishUninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction topology is invalid.",
        )),
    }
}

fn recover_with_adapter(paths: &Paths, system: &dyn NativeSystemAdapter) -> Result<()> {
    if !path_present(&paths.transaction)? {
        if path_present(&paths.backup)? && !path_present(&paths.install)? {
            durable_rename(&paths.backup, &paths.install)?;
        }
        remove_plain_tree(&paths.staging)?;
        return Ok(());
    }
    assert_plain_file(&paths.transaction)?;
    let value: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
    if value.schema_version != TRANSACTION_SCHEMA {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction schema is invalid.",
        ));
    }
    let plan = recovery_plan(
        &value,
        path_present(&paths.backup)?,
        path_present(&paths.install)?,
    )?;
    let finishing_uninstall = plan == RecoveryPlan::FinishUninstall;
    if finishing_uninstall && value.phase == "uninstall-cleanup-complete" {
        return Ok(());
    }
    let progress_phase = match plan {
        RecoveryPlan::RestorePredecessor => "recovering-restore-predecessor",
        RecoveryPlan::DiscardStaging => "recovering-discard-staging",
        RecoveryPlan::RemoveFreshCandidate => "recovering-remove-fresh",
        RecoveryPlan::FinishCommit => "recovering-finish-commit",
        RecoveryPlan::FinishUninstall => "recovering-finish-uninstall",
    };
    if value.phase != progress_phase {
        write_transaction(
            paths,
            progress_phase,
            transaction_action(&value)?,
            value.had_predecessor,
        )?;
    }
    match plan {
        RecoveryPlan::RestorePredecessor => {
            remove_plain_tree(&paths.staging)?;
            if path_present(&paths.backup)? {
                remove_plain_tree(&paths.install)?;
                durable_rename(&paths.backup, &paths.install)?;
            }
            if !path_present(&paths.install)? {
                return Err(fail(EXIT_REJECTED, "Recovered predecessor is missing."));
            }
            system.register_installed(paths)?;
        }
        RecoveryPlan::DiscardStaging => remove_plain_tree(&paths.staging)?,
        RecoveryPlan::RemoveFreshCandidate => {
            system.unregister_app_path()?;
            system.unregister_uninstall()?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.staging)?;
            remove_maintenance_uninstaller(paths)?;
            remove_update_recovery_launcher_residue(paths)?;
            remove_transaction(paths)?;
            clear_machine_relaunch_owner(paths)?;
            return Ok(());
        }
        RecoveryPlan::FinishCommit => {
            if value.action == "repair" && path_present(&paths.backup)? {
                restore_repair_controller(paths)?;
            }
            system.register_installed(paths)?;
            system.retire_legacy(paths)?;
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.staging)?;
        }
        RecoveryPlan::FinishUninstall => finish_uninstall_machine_cleanup(paths, system)?,
    }
    if finishing_uninstall {
        Ok(())
    } else {
        remove_transaction(paths)
    }
}

#[cfg(test)]
struct InjectedNativeSystem;

#[cfg(test)]
impl NativeSystemAdapter for InjectedNativeSystem {
    fn register_version(&self, _paths: &Paths, _version: &str) -> Result<()> {
        Ok(())
    }
    fn register_installed(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn unregister_app_path(&self) -> Result<()> {
        Ok(())
    }
    fn unregister_uninstall(&self) -> Result<()> {
        Ok(())
    }
    fn retire_legacy(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn clear_update_recovery(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
fn recover_with_system(paths: &Paths, update_system_state: bool) -> Result<()> {
    if update_system_state {
        recover_with_adapter(paths, &WindowsNativeSystem)
    } else {
        recover_with_adapter(paths, &InjectedNativeSystem)
    }
}

fn maintenance_generation_from_name(name: &str) -> Option<&str> {
    name.strip_prefix("Talking Quill Maintenance-")
        .and_then(|value| value.strip_suffix(".exe"))
        .filter(|generation| validate_machine_lock_suffix(generation).is_ok())
}

fn registered_maintenance_generation(program_files: &Path) -> Result<Option<String>> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Ok(None);
    }
    let quiet = read_registry_value(key, "QuietUninstallString", 2048)?;
    unsafe { RegCloseKey(key) };
    let Some(command) = quiet else {
        return Ok(None);
    };
    let Some(path) = command
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix("\" /S"))
        .map(PathBuf::from)
    else {
        return Err(fail(
            EXIT_REJECTED,
            "Registered maintenance command is invalid.",
        ));
    };
    if path.parent() != Some(program_files) {
        return Err(fail(
            EXIT_REJECTED,
            "Registered maintenance path is invalid.",
        ));
    }
    Ok(path
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(maintenance_generation_from_name)
        .map(str::to_owned))
}

fn current_install_generation(
    program_files: &Path,
    program_data: &Path,
    generation_record: &Path,
) -> Result<String> {
    let current = std::env::current_exe().map_err(io_failure)?;
    if let Some(generation) = current
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(maintenance_generation_from_name)
    {
        return Ok(generation.to_owned());
    }
    let current_text = current.to_string_lossy();
    let recovery_context = current.starts_with(program_files)
        || current.starts_with(program_data)
        || current_text.contains(".TalkingQuill-uninstall-");
    if recovery_context && let Some(generation) = registered_maintenance_generation(program_files)?
    {
        return Ok(generation);
    }
    if path_present(generation_record)? {
        assert_plain_file(generation_record)?;
        let generation = fs::read_to_string(generation_record).map_err(io_failure)?;
        validate_machine_lock_suffix(&generation)?;
        return Ok(generation);
    }
    let generation = random_machine_lock_suffix()?;
    // Only the elevated worker publishes this protected generation. If it is interrupted, every
    // recovery process reuses the durable record rather than trusting caller-controlled state.
    if token_is_elevated()? {
        let mut record = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(generation_record)
            .map_err(io_failure)?;
        record
            .write_all(generation.as_bytes())
            .map_err(io_failure)?;
        record.sync_all().map_err(io_failure)?;
        drop(record);
        flush_setup_directory(program_files)?;
    }
    Ok(generation)
}

fn paths() -> Result<Paths> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = known_folder(&FOLDERID_System)?;
    let profile = known_folder(&FOLDERID_RoamingAppData)?.join("Talking Quill");
    let maintenance_generation_record =
        program_files.join(".Talking Quill.maintenance-generation-v1");
    let maintenance_generation = current_install_generation(
        &program_files,
        &program_data,
        &maintenance_generation_record,
    )?;
    let maintenance_uninstaller = program_files.join(format!(
        "Talking Quill Maintenance-{maintenance_generation}.exe"
    ));
    let recovery_launcher = program_data.join(format!(
        "Talking Quill Update Recovery/talking-quill-update-recovery-launcher-{maintenance_generation}.exe"
    ));
    Ok(Paths {
        install: program_files.join("Talking Quill"),
        staging: program_files.join(".Talking Quill.native-staging"),
        backup: program_files.join(".Talking Quill.native-backup"),
        transaction: program_files.join(".Talking Quill.native-transaction-v2.json"),
        maintenance_generation_record,
        maintenance_uninstaller,
        recovery_launcher,
        profile,
        legacy_authority: program_data.join("Talking Quill/KeyboardAuthority"),
        legacy_quarantine: program_data
            .join("Talking Quill/.KeyboardAuthority.retirement-quarantine"),
        legacy_task_file: system.join("Tasks/TalkingQuillKeyboardAuthority"),
        program_data,
    })
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill";
const APP_PATH_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe";

fn restore_repair_controller(paths: &Paths) -> Result<()> {
    let source = paths.backup.join("Uninstall Talking Quill.exe");
    let target = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    assert_plain_file(&target)?;
    let temporary = paths.install.join(".repair-controller-recovery.tmp");
    if path_present(&temporary)? {
        assert_plain_file(&temporary)?;
        if file_hash(&temporary)? == file_hash(&source)? {
            return durable_replace(&temporary, &target);
        }
        // The fixed plain file is installer-owned inside the protected target tree;
        // a mismatched value is an interrupted copy and is safe to recreate.
        fs::remove_file(&temporary).map_err(io_failure)?;
    }
    let mut input = File::open(source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    durable_replace(&temporary, &target)
}

fn remove_maintenance_temporary_files(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let owned = name
            .to_str()
            .and_then(|value| value.strip_prefix("Talking Quill Maintenance."))
            .and_then(|value| value.strip_suffix(".tmp"))
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if owned {
            assert_plain_file(&entry.path())?;
            fs::remove_file(entry.path()).map_err(io_failure)?;
        }
    }
    Ok(())
}

fn remove_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    if path_present(&paths.maintenance_uninstaller)? {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        fs::remove_file(&paths.maintenance_uninstaller).map_err(io_failure)?;
    }
    Ok(())
}

fn ensure_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    let source = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    let replacing = path_present(&paths.maintenance_uninstaller)?;
    if replacing {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        if file_hash(&source)? == file_hash(&paths.maintenance_uninstaller)? {
            return Ok(());
        }
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = paths
        .maintenance_uninstaller
        .with_extension(format!("{suffix}.tmp"));
    let mut input = File::open(&source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    if file_hash(&source)? != file_hash(&temporary)? {
        return Err(fail(
            EXIT_REJECTED,
            "Maintenance uninstaller verification failed.",
        ));
    }
    if replacing {
        durable_replace(&temporary, &paths.maintenance_uninstaller)
    } else {
        durable_rename(&temporary, &paths.maintenance_uninstaller)
    }
}

fn ensure_uninstall_finalizer_registered(current: &Path, paths: &Paths) -> Result<PathBuf> {
    if let Some(existing) = registered_uninstall_executable()?
        && existing
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(UNINSTALL_FINALIZER_NAME))
        && path_present(&existing)?
        && is_uninstall_finalizer(&existing)?
        && marker_security_is_exact(&existing, MEDIUM_FINALIZER_FILE_SDDL)?
        && file_hash(&existing)? == file_hash(current)?
    {
        return Ok(existing);
    }
    write_transaction(
        paths,
        "uninstall-finalizer-publishing",
        Action::Uninstall,
        true,
    )?;
    let token = new_machine_lock_suffix()?;
    let suffix = new_machine_lock_suffix()?;
    let pending = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PENDING_PREFIX}{token}"));
    let published = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PREFIX}{suffix}"));
    create_directory_with_security(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    apply_lock_dacl(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    let identity =
        owned_tree_identity(&pending).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    create_or_verify_finalizer_marker(&pending.join("finalizer-tree-identity-v1"), &identity)?;
    let executable = pending.join(UNINSTALL_FINALIZER_NAME);
    let mut source = File::open(current).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&executable)
        .map_err(io_failure)?;
    apply_lock_dacl(&executable, MEDIUM_FINALIZER_FILE_SDDL)?;
    std::io::copy(&mut source, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    if file_hash(current)? != file_hash(&executable)? {
        return Err(fail(EXIT_REJECTED, "Uninstall finalizer copy changed."));
    }
    flush_setup_directory(&pending)?;
    if unsafe {
        MoveFileExW(
            wide(pending.as_os_str()).as_ptr(),
            wide(published.as_os_str()).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot publish the uninstall finalizer.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    let executable = published.join(UNINSTALL_FINALIZER_NAME);
    register_uninstall_executable(&executable)?;
    write_transaction(
        paths,
        "uninstall-finalizer-published",
        Action::Uninstall,
        true,
    )?;
    Ok(executable)
}

fn create_or_verify_finalizer_marker(path: &Path, identity: &str) -> Result<()> {
    create_atomic_marker(path, identity, MEDIUM_FINALIZER_FILE_SDDL)
}

fn register_uninstall_executable(executable: &Path) -> Result<()> {
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot open uninstall recovery registration.",
        ));
    }
    let values = [
        ("UninstallString", format!("\"{}\"", executable.display())),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", executable.display()),
        ),
    ];
    for (name, value) in values {
        let bytes = wide(OsStr::new(&value));
        if unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        } != 0
        {
            unsafe { RegCloseKey(key) };
            return Err(fail(EXIT_FAILURE, "Cannot register uninstall recovery."));
        }
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(EXIT_FAILURE, "Cannot flush uninstall recovery."))
    }
}

fn registered_uninstall_executable() -> Result<Option<PathBuf>> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read uninstall recovery registration.",
        ));
    }
    let value = read_registry_value(key, "UninstallString", 4096)?;
    unsafe { RegCloseKey(key) };
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    let path = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'));
    Ok(path.map(PathBuf::from))
}

fn read_registry_value(key: *mut c_void, name: &str, maximum: u32) -> Result<Option<String>> {
    let name = wide(OsStr::new(name));
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        return Ok(None);
    }
    if first != 0 || kind != REG_SZ || bytes < 2 || bytes > maximum || !bytes.is_multiple_of(2) {
        return Err(fail(EXIT_REJECTED, "Registry string is invalid."));
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot read registry string."));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    String::from_utf16(&value)
        .map(Some)
        .map_err(|_| fail(EXIT_REJECTED, "Registry string is invalid."))
}

fn reclaim_stale_maintenance_uninstallers(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let path = entry.path();
        if path == paths.maintenance_uninstaller {
            continue;
        }
        let stale = entry
            .file_name()
            .to_str()
            .and_then(maintenance_generation_from_name)
            .is_some();
        if stale {
            assert_plain_file(&path)?;
            fs::remove_file(path).map_err(io_failure)?;
        }
    }
    flush_setup_directory(parent)
}

fn register_installed_uninstall(paths: &Paths) -> Result<()> {
    ensure_maintenance_uninstaller(paths)?;
    let manifest = paths
        .install
        .join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&manifest)?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest).map_err(io_failure)?)
        .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
    let version = value
        .get("version")
        .and_then(|item| item.as_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Installed release version is invalid."))?;
    register_uninstall(paths, version)?;
    register_app_path(paths)?;
    reclaim_stale_maintenance_uninstallers(paths)
}

fn register_uninstall(paths: &Paths, version: &str) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(UNINSTALL_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native uninstall registration.",
        ));
    }
    let values = [
        ("DisplayName", "Talking Quill".to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "Talking Quill contributors".to_string()),
        ("InstallLocation", paths.install.display().to_string()),
        (
            "UninstallString",
            format!("\"{}\"", paths.maintenance_uninstaller.display()),
        ),
        (
            "QuietUninstallString",
            format!("\"{}\" /S", paths.maintenance_uninstaller.display()),
        ),
    ];
    let result = values.iter().try_for_each(|(name, value)| {
        let bytes = wide(OsStr::new(value));
        let status = unsafe {
            RegSetValueExW(
                key,
                wide(OsStr::new(name)).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr().cast(),
                (bytes.len() * 2) as u32,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(fail(
                EXIT_FAILURE,
                "Cannot write the native uninstall registration.",
            ))
        }
    });
    let flushed = result.is_ok() && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        result
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot durably write the native uninstall registration.",
        ))
    }
}

fn register_app_path(paths: &Paths) -> Result<()> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(APP_PATH_KEY)).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create the native application registration.",
        ));
    }
    let executable = wide(paths.install.join("Talking Quill.exe").as_os_str());
    let directory = wide(paths.install.as_os_str());
    let first = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("")).as_ptr(),
            0,
            REG_SZ,
            executable.as_ptr().cast(),
            (executable.len() * 2) as u32,
        )
    };
    let second = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Path")).as_ptr(),
            0,
            REG_SZ,
            directory.as_ptr().cast(),
            (directory.len() * 2) as u32,
        )
    };
    let flushed = first == 0 && second == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot write the native application registration.",
        ))
    }
}

fn unregister_app_path() -> Result<()> {
    delete_registry_tree_durable(
        APP_PATH_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths",
        "native application registration",
    )
}

fn clear_update_recovery(paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const PREFIX: &str = "Talking Quill Update Recovery ";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 0 {
        let mut index = 0;
        loop {
            let mut name = [0_u16; 512];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if status == 259 {
                break;
            }
            if status != 0 {
                unsafe { RegCloseKey(key) };
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot enumerate update recovery values.",
                ));
            }
            let value = String::from_utf16_lossy(&name[..length as usize]);
            let owned = value.strip_prefix(PREFIX).is_some_and(|generation| {
                generation.len() == 32
                    && generation
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
            if owned {
                if unsafe { RegDeleteValueW(key, name.as_ptr()) } != 0 {
                    unsafe { RegCloseKey(key) };
                    return Err(fail(EXIT_FAILURE, "Cannot remove update recovery value."));
                }
            } else {
                index += 1;
            }
        }
        if unsafe { RegFlushKey(key) } != 0 {
            unsafe { RegCloseKey(key) };
            return Err(fail(EXIT_FAILURE, "Cannot flush update recovery cleanup."));
        }
        unsafe { RegCloseKey(key) };
    } else if opened != 2 {
        return Err(fail(EXIT_FAILURE, "Cannot open update recovery values."));
    }
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
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
        let launcher_pending = name
            .strip_prefix(".Talking Quill.update-launcher-pending-")
            .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        if pending || published || launcher_pending {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
            if !metadata.is_dir()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || !(if launcher_pending {
                    medium_launcher_directory_is_protected(&path)?
                } else {
                    staged_path_is_protected(&path, true)?
                })
            {
                continue;
            }
            let identity = owned_tree_identity(&path)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
            let recorded = if launcher_pending {
                fs::read_to_string(path.join("launcher-tree-identity-v1"))
            } else {
                fs::read_to_string(path.join("cleanup-tree-identity-v1"))
            };
            if pending
                || launcher_pending
                || (published && recorded.is_ok_and(|value| value == identity))
            {
                remove_owned_tree(&path, &identity)
                    .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
            }
        }
    }
    // The protected launcher is a stable machine component. Per-user relaunch
    // ownership may live in a different HKCU hive under over-the-shoulder UAC,
    // so update cleanup must not infer that no relaunch owner exists.
    Ok(())
}

fn terminal_uninstall_root(paths: &Paths) -> PathBuf {
    paths.program_data.join("Talking Quill Update Recovery")
}

fn terminal_uninstall_record_path(paths: &Paths) -> PathBuf {
    terminal_uninstall_root(paths).join(TERMINAL_UNINSTALL_RECORD_NAME)
}

fn write_terminal_uninstall_record(paths: &Paths, record: &TerminalUninstallRecord) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    let record_path = terminal_uninstall_record_path(paths);
    let temporary = root.join(format!(
        ".terminal-uninstall.tmp-{}",
        random_machine_lock_suffix()?
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(io_failure)?;
    apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    let mut published = record.clone();
    published.record_file_identity = file_identity_text(&file)?;
    let bytes = serde_json::to_vec(&published)
        .map_err(|_| fail(EXIT_FAILURE, "Cannot encode terminal uninstall recovery."))?;
    file.write_all(&bytes).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    if identity != published.record_file_identity {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal record identity changed before publication.",
        ));
    }
    durable_replace(&temporary, &record_path)?;
    flush_setup_directory(&root)?;
    let reopened = File::open(&record_path).map_err(io_failure)?;
    let reopened_identity = file_identity_text(&reopened)?;
    drop(reopened);
    if reopened_identity != published.record_file_identity
        || !marker_security_is_exact(&record_path, MEDIUM_LAUNCHER_FILE_SDDL)?
        || fs::read(&record_path).map_err(io_failure)? != bytes
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal record publication is invalid.",
        ));
    }
    Ok(())
}

fn random_machine_lock_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_terminal_uninstall_record(paths: &Paths) -> Result<Option<TerminalUninstallRecord>> {
    let path = terminal_uninstall_record_path(paths);
    if !path_present(&path)? {
        return Ok(None);
    }
    assert_plain_file(&path)?;
    let bytes = fs::read(&path).map_err(io_failure)?;
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err(fail(EXIT_REJECTED, "Terminal uninstall record is invalid."));
    }
    let record: TerminalUninstallRecord = serde_json::from_slice(&bytes)
        .map_err(|_| fail(EXIT_REJECTED, "Terminal uninstall record is invalid."))?;
    let file = File::open(&path).map_err(io_failure)?;
    let record_identity = file_identity_text(&file)?;
    drop(file);
    let (uninstall_command, quiet_uninstall_command) = terminal_uninstall_commands(paths);
    if record.schema_version != 3
        || validate_machine_lock_suffix(&record.generation).is_err()
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
        || record.maintenance_sha256.len() != 64
        || !record
            .maintenance_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
        || record.uninstall_command != uninstall_command
        || record.quiet_uninstall_command != quiet_uninstall_command
        || record.service_name != terminal_service_name(&record.generation)?
        || record.service_image
            != terminal_service_image(paths, &record.generation)?.to_string_lossy()
        || record.service_sha256.len() != 64
        || !record
            .service_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
        || record.service_file_identity.is_empty()
        || record.record_file_identity != record_identity
    {
        return Err(fail(EXIT_REJECTED, "Terminal uninstall record is invalid."));
    }
    Ok(Some(record))
}

fn ensure_machine_relaunch_owner_installed(paths: &Paths) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    if !path_present(&root)? {
        create_directory_with_security(&root, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
        apply_lock_dacl(&root, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    }
    if !medium_launcher_directory_is_protected(&root)? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine recovery launcher is not protected.",
        ));
    }
    let source = paths
        .install
        .join("resources/helper/talking-quill-update-recovery-launcher.exe");
    assert_plain_file(&source)?;
    let target = paths.recovery_launcher.clone();
    let temporary = root.join(format!(".launcher.tmp-{}", random_machine_lock_suffix()?));
    fs::copy(&source, &temporary).map_err(io_failure)?;
    apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    let copied = File::open(&temporary).map_err(io_failure)?;
    copied.sync_all().map_err(io_failure)?;
    drop(copied);
    durable_replace(&temporary, &target)?;
    let identity =
        owned_tree_identity(&root).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let marker = root.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
    } else {
        create_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL)?;
    }
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create machine recovery owner."));
    }
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        target.display()
    );
    let value = wide(OsStr::new(&command));
    let status = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Talking Quill Update Relaunch")).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(EXIT_FAILURE, "Cannot flush machine recovery owner."));
    }
    flush_setup_directory(&root)
}

fn require_machine_relaunch_owner(paths: &Paths) -> Result<()> {
    let root = terminal_uninstall_root(paths);
    let launcher = paths.recovery_launcher.clone();
    if !medium_launcher_directory_is_protected(&root)? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine recovery launcher is not protected.",
        ));
    }
    assert_plain_file(&launcher)?;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_REJECTED, "Machine recovery owner is missing."));
    }
    let expected = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    let actual = read_registry_value(key, "Talking Quill Update Relaunch", 1024)?;
    unsafe { RegCloseKey(key) };
    if actual.as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(fail(EXIT_REJECTED, "Machine recovery owner is invalid."))
    }
}

fn terminal_service_image(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_SERVICE_IMAGE_PREFIX}{generation}.exe")))
}

fn terminal_service_command(paths: &Paths, generation: &str) -> Result<String> {
    Ok(format!(
        "\"{}\" /TQ-TERMINAL-SERVICE={generation}",
        terminal_service_image(paths, generation)?.display()
    ))
}

fn publish_terminal_service_image(
    paths: &Paths,
    source: &Path,
    generation: &str,
) -> Result<(String, String)> {
    assert_plain_file(source)?;
    let pending = paths.program_data.join(format!(
        "{TERMINAL_SERVICE_PENDING_PREFIX}{}.exe",
        random_machine_lock_suffix()?
    ));
    let published = terminal_service_image(paths, generation)?;
    assert_plain_absent(&pending)?;
    assert_plain_absent(&published)?;
    fs::copy(source, &pending).map_err(io_failure)?;
    apply_lock_dacl(&pending, TERMINAL_SERVICE_FILE_SDDL)?;
    let file = File::open(&pending).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    let file_identity = file_identity_text(&file)?;
    drop(file);
    let sha256 = hex_hash(&file_hash(&pending)?);
    if sha256 != hex_hash(&file_hash(source)?) {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service image changed during publication.",
        ));
    }
    durable_rename(&pending, &published)?;
    flush_setup_directory(&paths.program_data)?;
    assert_plain_file(&published)?;
    let published_file = File::open(&published).map_err(io_failure)?;
    let published_identity = file_identity_text(&published_file)?;
    drop(published_file);
    if published_identity != file_identity
        || !marker_security_is_exact(&published, TERMINAL_SERVICE_FILE_SDDL)?
        || file_hash(&published)? != file_hash(source)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service publication is invalid.",
        ));
    }
    Ok((sha256, file_identity))
}

fn terminal_service_dacl_is_exact(service: SC_HANDLE) -> Result<bool> {
    let mut needed = 0;
    unsafe {
        QueryServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 || needed > 64 * 1024 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect terminal service ACL."));
    }
    let mut actual = vec![0_u8; needed as usize];
    if unsafe {
        QueryServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION,
            actual.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(fail(EXIT_REJECTED, "Cannot read terminal service ACL."));
    }
    let expected_wide = wide(OsStr::new(TERMINAL_SERVICE_SDDL));
    let mut expected = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            expected_wide.as_ptr(),
            SDDL_REVISION_1,
            &mut expected,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create terminal service ACL."));
    }
    let convert = |descriptor: *mut c_void| -> Result<String> {
        let mut text = ptr::null_mut();
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot normalize terminal service ACL.",
            ));
        }
        let value = unsafe { wide_ptr_string(text) };
        unsafe { LocalFree(text.cast()) };
        Ok(value)
    };
    let expected_text = convert(expected)?;
    unsafe { LocalFree(expected) };
    Ok(convert(actual.as_mut_ptr().cast())? == expected_text)
}

fn apply_terminal_service_dacl(service: SC_HANDLE) -> Result<()> {
    let sddl = wide(OsStr::new(TERMINAL_SERVICE_SDDL));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot create terminal service ACL."));
    }
    let applied = unsafe {
        SetServiceObjectSecurity(
            service,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    } != 0;
    unsafe { LocalFree(descriptor) };
    if applied && terminal_service_dacl_is_exact(service)? {
        Ok(())
    } else {
        Err(fail(EXIT_FAILURE, "Cannot protect terminal service."))
    }
}

fn configure_terminal_service_restarts(service: SC_HANDLE) -> Result<()> {
    let mut actions = [SC_ACTION {
        Type: SC_ACTION_RESTART,
        Delay: 60_000,
    }; 3];
    let failure_actions = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: ptr::null_mut(),
        lpCommand: ptr::null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            (&failure_actions as *const SERVICE_FAILURE_ACTIONSW).cast(),
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot configure terminal service recovery.",
        ));
    }
    let non_crash = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&non_crash as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        )
    } == 0
    {
        Err(fail(
            EXIT_FAILURE,
            "Cannot enable terminal service failure recovery.",
        ))
    } else {
        Ok(())
    }
}

fn install_terminal_service(
    paths: &Paths,
    record: &TerminalUninstallRecord,
    start: bool,
) -> Result<()> {
    let manager = unsafe {
        OpenSCManagerW(
            ptr::null(),
            ptr::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open the service manager."));
    }
    let command = terminal_service_command(paths, &record.generation)?;
    let mut service = unsafe {
        CreateServiceW(
            manager,
            wide(OsStr::new(&record.service_name)).as_ptr(),
            wide(OsStr::new(&record.service_name)).as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            wide(OsStr::new(&command)).as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    };
    if service.is_null() && unsafe { GetLastError() } == 1073 {
        service = unsafe {
            OpenServiceW(
                manager,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_ALL_ACCESS,
            )
        };
    }
    if service.is_null() {
        unsafe { CloseServiceHandle(manager) };
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create or open terminal cleanup service.",
        ));
    }
    let result = apply_terminal_service_dacl(service)
        .and_then(|()| configure_terminal_service_restarts(service))
        .and_then(|()| {
            if start
                && unsafe { StartServiceW(service, 0, ptr::null()) } == 0
                && unsafe { GetLastError() } != 1056
            {
                Err(fail(EXIT_FAILURE, "Cannot start terminal cleanup service."))
            } else {
                Ok(())
            }
        });
    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
    }
    result
}

fn resume_terminal_service(paths: &Paths, record: &TerminalUninstallRecord) -> Result<()> {
    let image = PathBuf::from(&record.service_image);
    validate_terminal_service_image(record, &image)?;
    install_terminal_service(paths, record, true)?;
    verify_terminal_service_registration(paths, record)
}

fn terminal_uninstall_commands(paths: &Paths) -> (String, String) {
    let executable = format!("\"{}\"", paths.maintenance_uninstaller.display());
    (executable.clone(), format!("{executable} /S"))
}

fn reclaim_unpublished_terminal_service_images(paths: &Paths) -> Result<()> {
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let suffix = name
            .strip_prefix(TERMINAL_SERVICE_IMAGE_PREFIX)
            .or_else(|| name.strip_prefix(TERMINAL_SERVICE_PENDING_PREFIX));
        let Some(suffix) = suffix else {
            continue;
        };
        let generation = suffix.strip_suffix(".exe").unwrap_or("");
        if validate_machine_lock_suffix(generation).is_err() {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal service namespace is invalid.",
            ));
        }
        let path = entry.path();
        assert_plain_file(&path)?;
        if name.starts_with(TERMINAL_SERVICE_IMAGE_PREFIX)
            && terminal_service_exists(&terminal_service_name(generation)?)?
        {
            self_retire_absent_terminal_service(paths, generation, &path)?;
        }
        if !marker_security_is_exact(&path, TERMINAL_SERVICE_FILE_SDDL)? {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal service residue is unprotected.",
            ));
        }
        fs::remove_file(path).map_err(io_failure)?;
    }
    flush_setup_directory(&paths.program_data)
}

fn publish_terminal_uninstall_record(paths: &Paths, current: &Path) -> Result<String> {
    require_machine_relaunch_owner(paths)?;
    reclaim_unpublished_terminal_service_images(paths)?;
    let generation = random_machine_lock_suffix()?;
    let (service_sha256, service_file_identity) =
        publish_terminal_service_image(paths, current, &generation)?;
    let (uninstall_command, quiet_uninstall_command) = terminal_uninstall_commands(paths);
    let record = TerminalUninstallRecord {
        schema_version: 3,
        generation: generation.clone(),
        phase: "armed".into(),
        maintenance_sha256: hex_hash(&file_hash(&paths.maintenance_uninstaller)?),
        uninstall_command,
        quiet_uninstall_command,
        service_name: terminal_service_name(&generation)?,
        service_image: terminal_service_image(paths, &generation)?
            .to_string_lossy()
            .into_owned(),
        service_sha256,
        service_file_identity,
        record_file_identity: String::new(),
    };
    terminal_maintenance_crash_at("pre-CreateService");
    install_terminal_service(paths, &record, false)?;
    terminal_maintenance_crash_at("post-service-pre-record");
    write_terminal_uninstall_record(paths, &record)?;
    terminal_maintenance_crash_at("post-record-pre-start");
    let published = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    install_terminal_service(paths, &published, true)?;
    Ok(generation)
}

fn write_terminal_uninstall_phase(paths: &Paths, generation: &str, phase: &str) -> Result<()> {
    let mut record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation
        || !matches!(
            phase,
            "armed"
                | "machine-retired"
                | "cleanup-complete"
                | "final-launcher-owned"
                | "maintenance-deletion-owned"
                | "maintenance-deleted"
                | "uninstall-unregistered"
                | "journal-removed"
        )
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    record.phase = phase.into();
    write_terminal_uninstall_record(paths, &record)
}

#[derive(Debug, PartialEq, Eq)]
enum TerminalUninstallRecoveryStep {
    RetireMachine,
    FinishCleanup,
    CleanupComplete,
}

fn terminal_uninstall_recovery_step(
    phase: &str,
    journal_present: bool,
) -> Result<TerminalUninstallRecoveryStep> {
    match (phase, journal_present) {
        ("armed", true) => Ok(TerminalUninstallRecoveryStep::RetireMachine),
        ("machine-retired", true) => Ok(TerminalUninstallRecoveryStep::FinishCleanup),
        (
            "cleanup-complete"
            | "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "maintenance-deleted"
            | "uninstall-unregistered"
            | "journal-removed",
            _,
        ) => Ok(TerminalUninstallRecoveryStep::CleanupComplete),
        _ => Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall recovery phase is invalid.",
        )),
    }
}

fn validate_terminal_service_image(record: &TerminalUninstallRecord, current: &Path) -> Result<()> {
    let file = File::open(current).map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    if canonical(current)? != canonical(Path::new(&record.service_image))?
        || record.service_name != terminal_service_name(&record.generation)?
        || hex_hash(&file_hash(current)?) != record.service_sha256
        || identity != record.service_file_identity
        || !marker_security_is_exact(current, TERMINAL_SERVICE_FILE_SDDL)?
    {
        return Err(fail(EXIT_REJECTED, "Terminal service image is invalid."));
    }
    Ok(())
}

fn terminal_service_exists(name: &str) -> Result<bool> {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal service manager.",
        ));
    }
    let service = unsafe {
        OpenServiceW(
            manager,
            wide(OsStr::new(name)).as_ptr(),
            SERVICE_QUERY_STATUS,
        )
    };
    let error = if service.is_null() {
        unsafe { GetLastError() }
    } else {
        0
    };
    if !service.is_null() {
        unsafe { CloseServiceHandle(service) };
    }
    unsafe { CloseServiceHandle(manager) };
    if error == 0 {
        Ok(true)
    } else if error == 1060 {
        Ok(false)
    } else {
        Err(fail(EXIT_FAILURE, "Cannot inspect terminal service."))
    }
}

fn terminal_service_handle(
    record: &TerminalUninstallRecord,
    access: u32,
) -> Result<(ServiceHandle, ServiceHandle)> {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot open terminal service manager."));
    }
    let manager = ServiceHandle(manager);
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide(OsStr::new(&record.service_name)).as_ptr(),
            access,
        )
    };
    if service.is_null() {
        return Err(fail(EXIT_REJECTED, "Terminal cleanup service is missing."));
    }
    Ok((manager, ServiceHandle(service)))
}

fn terminal_service_restarts_are_exact(service: SC_HANDLE) -> Result<bool> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if needed == 0 || needed > 64 * 1024 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect terminal service recovery.",
        ));
    }
    let mut storage = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service recovery.",
        ));
    }
    let actions = unsafe { &*storage.as_ptr().cast::<SERVICE_FAILURE_ACTIONSW>() };
    if actions.dwResetPeriod != 86_400 || actions.cActions != 3 || actions.lpsaActions.is_null() {
        return Ok(false);
    }
    let actions = unsafe { std::slice::from_raw_parts(actions.lpsaActions, 3) };
    if !actions
        .iter()
        .all(|action| action.Type == SC_ACTION_RESTART && action.Delay == 60_000)
    {
        return Ok(false);
    }
    let mut flag: SERVICE_FAILURE_ACTIONS_FLAG = unsafe { mem::zeroed() };
    let mut flag_needed = 0;
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&mut flag as *mut SERVICE_FAILURE_ACTIONS_FLAG).cast(),
            mem::size_of::<SERVICE_FAILURE_ACTIONS_FLAG>() as u32,
            &mut flag_needed,
        )
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service failure flag.",
        ));
    }
    Ok(flag.fFailureActionsOnNonCrashFailures != 0)
}

fn verify_terminal_service_registration_mode(
    paths: &Paths,
    record: &TerminalUninstallRecord,
    require_hardening: bool,
) -> Result<()> {
    let (_manager, service) =
        terminal_service_handle(record, SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS)?;
    let mut needed = 0;
    unsafe { QueryServiceConfigW(service.0, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect terminal service identity.",
        ));
    }
    let mut storage = vec![0_usize; (needed as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe { QueryServiceConfigW(service.0, storage.as_mut_ptr().cast(), needed, &mut needed) }
        == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read terminal service identity.",
        ));
    }
    let config = unsafe { &*storage.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
    let binary = unsafe { wide_ptr_string(config.lpBinaryPathName) };
    let account = if config.lpServiceStartName.is_null() {
        String::new()
    } else {
        unsafe { wide_ptr_string(config.lpServiceStartName) }
    };
    if config.dwServiceType != SERVICE_WIN32_OWN_PROCESS
        || config.dwStartType != SERVICE_AUTO_START
        || config.dwErrorControl != SERVICE_ERROR_NORMAL
        || !account.eq_ignore_ascii_case("LocalSystem")
        || binary != terminal_service_command(paths, &record.generation)?
        || (require_hardening && !terminal_service_dacl_is_exact(service.0)?)
        || (require_hardening && !terminal_service_restarts_are_exact(service.0)?)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service registration is invalid.",
        ));
    }
    Ok(())
}

fn verify_terminal_service_registration(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<()> {
    verify_terminal_service_registration_mode(paths, record, true)
}

fn registry_key_absent(path: &str) -> Result<bool> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(path)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 0 {
        unsafe { RegCloseKey(key) };
        Ok(false)
    } else if status == 2 {
        Ok(true)
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot inspect terminal cleanup registry.",
        ))
    }
}

fn self_retire_absent_terminal_service(
    paths: &Paths,
    generation: &str,
    current: &Path,
) -> Result<()> {
    let expected = terminal_service_image(paths, generation)?;
    if canonical(current)? != canonical(&expected)?
        || !marker_security_is_exact(current, TERMINAL_SERVICE_FILE_SDDL)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Recordless terminal service image is invalid.",
        ));
    }
    let file = File::open(current).map_err(io_failure)?;
    let record = TerminalUninstallRecord {
        schema_version: 3,
        generation: generation.to_owned(),
        phase: "armed".into(),
        maintenance_sha256: String::new(),
        uninstall_command: String::new(),
        quiet_uninstall_command: String::new(),
        service_name: terminal_service_name(generation)?,
        service_image: expected.to_string_lossy().into_owned(),
        service_sha256: hex_hash(&file_hash(current)?),
        service_file_identity: file_identity_text(&file)?,
        record_file_identity: String::new(),
    };
    drop(file);
    verify_terminal_service_registration_mode(paths, &record, false)?;
    let (_manager, service) = terminal_service_handle(&record, DELETE | SERVICE_QUERY_CONFIG)?;
    if unsafe { DeleteService(service.0) } == 0 && unsafe { GetLastError() } != 1072 {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot retire recordless terminal service.",
        ));
    }
    Ok(())
}

fn finish_terminal_service_cleanup(paths: &Paths) -> Result<()> {
    require_uninstall_cleanup_complete(paths)?;
    clear_update_recovery(paths)?;
    clear_legacy_profile_relaunch_owners(paths)?;
    remove_plain_tree(&terminal_uninstall_root(paths).join("Relaunch Records"))?;
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    remove_uninstall_finalizer_residue(paths)?;
    flush_setup_directory(&paths.program_data)
}

fn service_cleanup_topology_is_complete(paths: &Paths, generation: &str) -> Result<bool> {
    require_uninstall_cleanup_complete(paths)?;
    require_machine_relaunch_owner(paths)?;
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation || record.phase != "cleanup-complete" {
        return Ok(false);
    }
    for path in [&paths.install, &paths.staging, &paths.backup] {
        if path_present(path)? {
            return Ok(false);
        }
    }
    if !registry_key_absent(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe",
    )? || registry_key_absent(UNINSTALL_KEY)?
        || registry_key_absent(MACHINE_LOCK_REGISTRY_KEY)?
    {
        return Ok(false);
    }
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let name = entry.map_err(io_failure)?.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(UNINSTALL_FINALIZER_PREFIX)
            || name.starts_with(UNINSTALL_FINALIZER_PENDING_PREFIX)
            || name.starts_with(MACHINE_LOCK_PENDING_PREFIX)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn run_terminal_cleanup_service(generation: &str) -> Result<()> {
    let paths = paths()?;
    let current = std::env::current_exe().map_err(io_failure)?;
    let Some(record) = read_terminal_uninstall_record(&paths)? else {
        return self_retire_absent_terminal_service(&paths, generation, &current);
    };
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    validate_terminal_service_image(&record, &current)?;
    verify_terminal_service_registration(&paths, &record)?;
    terminal_service_fail_once("failure-action-restart")?;
    let step = terminal_uninstall_recovery_step(&record.phase, path_present(&paths.transaction)?)?;
    if step == TerminalUninstallRecoveryStep::CleanupComplete {
        if !service_cleanup_topology_is_complete(&paths, generation)? {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal cleanup topology is incomplete.",
            ));
        }
        return Ok(());
    }
    let mut machine_lock = Some(MachineLock::acquire(
        &paths,
        120_000,
        installed_recovery_policy_epoch(&paths)?,
    )?);
    if step == TerminalUninstallRecoveryStep::RetireMachine {
        retire_terminal_machine_state(&paths, &WindowsNativeSystem, &current, generation)?;
    }
    let retired = read_terminal_uninstall_record(&paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if terminal_uninstall_recovery_step(&retired.phase, path_present(&paths.transaction)?)?
        != TerminalUninstallRecoveryStep::FinishCleanup
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall machine state is not retired.",
        ));
    }
    finish_terminal_service_cleanup(&paths)?;
    drop(machine_lock.take());
    write_terminal_uninstall_phase(&paths, generation, "cleanup-complete")?;
    if !service_cleanup_topology_is_complete(&paths, generation)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal cleanup topology is incomplete.",
        ));
    }
    Ok(())
}

fn remove_retired_terminal_service_image(
    paths: &Paths,
    record: &TerminalUninstallRecord,
) -> Result<bool> {
    let image = PathBuf::from(&record.service_image);
    if !path_present(&image)? {
        return Ok(true);
    }
    validate_terminal_service_image(record, &image)?;
    if terminal_force_pending_delete() {
        schedule_terminal_service_deletion(&image)?;
        return Ok(false);
    }
    match fs::remove_file(&image) {
        Ok(()) => {
            flush_setup_directory(&paths.program_data)?;
            Ok(true)
        }
        Err(_) => {
            // Registration is already absent. A reboot may now own deletion of only this inert,
            // authenticated image; maintenance and the journal remain callable until it is gone.
            schedule_terminal_service_deletion(&image)?;
            Ok(false)
        }
    }
}

fn wait_for_terminal_service_retirement(paths: &Paths, generation: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(240);
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    loop {
        let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
        if manager.is_null() {
            return Err(fail(EXIT_FAILURE, "Cannot poll terminal cleanup service."));
        }
        let service = unsafe {
            OpenServiceW(
                manager,
                wide(OsStr::new(&record.service_name)).as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG | DELETE,
            )
        };
        if service.is_null() {
            let error = unsafe { GetLastError() };
            unsafe { CloseServiceHandle(manager) };
            if error == 1072 {
                if Instant::now() >= deadline {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Terminal service deletion did not commit.",
                    ));
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            if error != 1060 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot inspect terminal cleanup service.",
                ));
            }
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if !complete {
                return Err(fail(
                    EXIT_REJECTED,
                    "Terminal service disappeared before cleanup completed.",
                ));
            }
            terminal_maintenance_crash_at("post-delete-pre-image-removal");
            if !remove_retired_terminal_service_image(paths, &record)? {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal service image deletion requires reboot.",
                ));
            }
            return finish_terminal_uninstall(paths);
        }
        let service = ServiceHandle(service);
        let manager = ServiceHandle(manager);
        let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
        if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
            return Err(fail(EXIT_FAILURE, "Cannot query terminal cleanup service."));
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            let complete = read_terminal_uninstall_record(paths)?.is_some_and(|value| {
                value.generation == generation
                    && matches!(
                        value.phase.as_str(),
                        "cleanup-complete"
                            | "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "maintenance-deleted"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
            });
            if status.dwWin32ExitCode == 0 && complete {
                verify_terminal_service_registration(paths, &record)?;
                terminal_maintenance_crash_at("service-stopped-pre-DeleteService");
                if unsafe { DeleteService(service.0) } == 0 && unsafe { GetLastError() } != 1072 {
                    return Err(fail(
                        EXIT_FAILURE,
                        "Cannot delete terminal cleanup service.",
                    ));
                }
                drop(service);
                drop(manager);
                continue;
            }
            if status.dwWin32ExitCode == ERROR_SERVICE_SPECIFIC_ERROR {
                // Failure actions are configured for non-crash failures. Poll through the bounded
                // SCM restart interval without converting the test into a manual restart.
            } else if status.dwWin32ExitCode != 0 {
                return Err(fail(
                    EXIT_FAILURE,
                    "Terminal cleanup service stopped unexpectedly.",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(fail(
                EXIT_FAILURE,
                "Terminal cleanup service did not complete.",
            ));
        }
        drop(service);
        drop(manager);
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn enumerate_registry_subkeys(key: *mut c_void) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut index = 0;
    loop {
        let mut name = [0_u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == 259 {
            break;
        }
        if status != 0 {
            return Err(fail(EXIT_FAILURE, "Cannot enumerate user registry hives."));
        }
        values.push(String::from_utf16_lossy(&name[..length as usize]));
        index += 1;
    }
    Ok(values)
}

fn clear_legacy_relaunch_values_in_hive(hive: *mut c_void, paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const PREFIX: &str = "Talking Quill Update Relaunch ";
    let mut run = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut run,
        )
    };
    if opened == 2 {
        return Ok(());
    }
    if opened != 0 {
        return Ok(());
    }
    let launcher = paths.recovery_launcher.clone();
    let mut index = 0;
    loop {
        let mut name = [0_u16; 512];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                run,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == 259 {
            break;
        }
        if status != 0 {
            break;
        }
        let value_name = String::from_utf16_lossy(&name[..length as usize]);
        let Some(generation) = value_name.strip_prefix(PREFIX) else {
            index += 1;
            continue;
        };
        if validate_machine_lock_suffix(generation).is_err() {
            index += 1;
            continue;
        }
        let expected = format!(
            "\"{}\" --windows-update-relaunch-v1={generation}",
            launcher.display()
        );
        if read_registry_value(run, &value_name, 2048)
            .ok()
            .flatten()
            .as_deref()
            != Some(expected.as_str())
        {
            index += 1;
            continue;
        }
        if unsafe { RegDeleteValueW(run, wide(OsStr::new(&value_name)).as_ptr()) } != 0 {
            index += 1;
        }
    }
    let _ = unsafe { RegFlushKey(run) };
    unsafe { RegCloseKey(run) };
    Ok(())
}

fn read_profile_image_path(key: *mut c_void) -> Result<Option<PathBuf>> {
    let name = wide(OsStr::new("ProfileImagePath"));
    let mut kind = 0;
    let mut bytes = 0;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 || !matches!(kind, REG_SZ | REG_EXPAND_SZ) || bytes > 32768 {
        return Err(fail(EXIT_REJECTED, "Profile path is invalid."));
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    if unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot read profile path."));
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    let text =
        String::from_utf16(&value).map_err(|_| fail(EXIT_REJECTED, "Profile path is invalid."))?;
    let source = wide(OsStr::new(&text));
    let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), ptr::null_mut(), 0) };
    if needed == 0 || needed > 32768 {
        return Err(fail(EXIT_REJECTED, "Profile path expansion failed."));
    }
    let mut expanded = vec![0_u16; needed as usize];
    if unsafe { ExpandEnvironmentStringsW(source.as_ptr(), expanded.as_mut_ptr(), needed) }
        != needed
    {
        return Err(fail(EXIT_REJECTED, "Profile path expansion failed."));
    }
    if expanded.last() == Some(&0) {
        expanded.pop();
    }
    let expanded = String::from_utf16(&expanded)
        .map_err(|_| fail(EXIT_REJECTED, "Profile path is invalid."))?;
    let path = PathBuf::from(&expanded);
    if expanded.contains('%')
        || !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(fail(EXIT_REJECTED, "Profile path is not canonical."));
    }
    Ok(Some(path))
}

fn remove_legacy_profile_relaunch_records(root: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let generation = entry.file_name().to_string_lossy().into_owned();
        if validate_machine_lock_suffix(&generation).is_err() {
            continue;
        }
        let directory = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&directory) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        let record_path = directory.join("relaunch-record-v1.json");
        let Ok(bytes) = fs::read(&record_path) else {
            continue;
        };
        if bytes.is_empty() || bytes.len() > 64 * 1024 {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let exact = matches!(
            value
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64),
            Some(1 | 2)
        ) && value.get("generation").and_then(serde_json::Value::as_str)
            == Some(generation.as_str())
            && value
                .get("request")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|request| request.starts_with("--windows-update-bootstrap-v2="));
        if exact {
            let _ = remove_plain_tree(&directory);
        }
    }
    let _ = fs::remove_dir(root);
    Ok(())
}

fn clear_legacy_profile_relaunch_owners(paths: &Paths) -> Result<()> {
    let loaded = enumerate_registry_subkeys(HKEY_USERS).unwrap_or_default();
    for sid in loaded
        .iter()
        .filter(|sid| sid.starts_with("S-1-5-") && !sid.ends_with("_Classes"))
    {
        let mut hive = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                wide(OsStr::new(sid)).as_ptr(),
                0,
                KEY_READ | KEY_WRITE,
                &mut hive,
            )
        } != 0
        {
            continue;
        }
        let _ = clear_legacy_relaunch_values_in_hive(hive, paths);
        unsafe { RegCloseKey(hive) };
    }
    const PROFILE_LIST: &str = r"Software\Microsoft\Windows NT\CurrentVersion\ProfileList";
    let mut profiles = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(PROFILE_LIST)).as_ptr(),
            0,
            KEY_READ,
            &mut profiles,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot open the profile inventory."));
    }
    for sid in enumerate_registry_subkeys(profiles).unwrap_or_default() {
        if !sid.starts_with("S-1-5-") {
            continue;
        }
        let mut profile = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                profiles,
                wide(OsStr::new(&sid)).as_ptr(),
                0,
                KEY_READ,
                &mut profile,
            )
        } != 0
        {
            continue;
        }
        let profile_path = read_profile_image_path(profile).ok().flatten();
        unsafe { RegCloseKey(profile) };
        let Some(profile_path) = profile_path else {
            continue;
        };
        let legacy_records =
            profile_path.join("AppData/Local/Talking Quill/Windows Update Recovery");
        remove_legacy_profile_relaunch_records(&legacy_records)?;
        if loaded.iter().any(|value| value == &sid) {
            continue;
        }
        let ntuser = profile_path.join("NTUSER.DAT");
        if fs::symlink_metadata(&ntuser).is_err() {
            continue;
        }
        let mut offline = ptr::null_mut();
        if unsafe {
            RegLoadAppKeyW(
                wide(ntuser.as_os_str()).as_ptr(),
                &mut offline,
                KEY_READ | KEY_WRITE,
                0,
                0,
            )
        } != 0
        {
            continue;
        }
        let _ = clear_legacy_relaunch_values_in_hive(offline, paths);
        let _ = unsafe { RegFlushKey(offline) };
        unsafe { RegCloseKey(offline) };
    }
    unsafe { RegCloseKey(profiles) };
    Ok(())
}

fn terminal_recovery_tombstone(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_RECOVERY_TOMBSTONE_PREFIX}{generation}")))
}

fn terminal_final_launcher(paths: &Paths, generation: &str) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_FINAL_LAUNCHER_PREFIX}{generation}.exe")))
}

fn publish_terminal_final_launcher(paths: &Paths, generation: &str) -> Result<PathBuf> {
    let source = paths.recovery_launcher.clone();
    let target = terminal_final_launcher(paths, generation)?;
    if !path_present(&target)? {
        let temporary = paths.program_data.join(format!(
            ".Talking Quill.terminal-relaunch-pending-{}.exe",
            random_machine_lock_suffix()?
        ));
        fs::copy(&source, &temporary).map_err(io_failure)?;
        apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
        File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(io_failure)?;
        durable_replace(&temporary, &target)?;
    }
    if file_hash(&source)? != file_hash(&target)?
        || !marker_security_is_exact(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(fail(EXIT_REJECTED, "Terminal final launcher is invalid."));
    }
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
    }
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        target.display()
    );
    let value = wide(OsStr::new(&command));
    let status = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Talking Quill Update Relaunch")).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot publish terminal final launcher.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    Ok(target)
}

fn pending_deletion_is_owned(path: &Path) -> Result<bool> {
    let expected = canonical(path)?;
    let key = open_session_manager(KEY_READ)?;
    let pairs = read_pending_rename_pairs(key)?;
    unsafe { RegCloseKey(key) };
    Ok(pairs.iter().any(|(source, destination)| {
        destination.is_empty() && normalized_pending_source(source) == expected
    }))
}

fn remove_terminal_recovery_tombstone(_paths: &Paths, tombstone: &Path) -> Result<()> {
    if !path_present(tombstone)? {
        return Ok(());
    }
    if !medium_launcher_directory_is_protected(tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is unprotected.",
        ));
    }
    let marker = tombstone.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        let identity = owned_tree_identity(tombstone)
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
        let mut tree = Vec::new();
        collect_finalizer_deletion_paths(tombstone, &mut tree)?;
        let record = tombstone.join(TERMINAL_UNINSTALL_RECORD_NAME);
        let record_present = path_present(&record)?;
        if !record_present {
            let entries = fs::read_dir(tombstone)
                .map_err(io_failure)?
                .map(|entry| entry.map(|value| value.file_name()))
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(io_failure)?;
            if entries != [OsString::from("launcher-tree-identity-v1")] {
                return Err(fail(
                    EXIT_REJECTED,
                    "Terminal marker-only tombstone inventory is invalid.",
                ));
            }
        }
        for target in tree {
            if target == tombstone || target == marker || target == record {
                continue;
            }
            let metadata = fs::symlink_metadata(&target).map_err(io_failure)?;
            if metadata.is_dir() {
                fs::remove_dir(&target).map_err(io_failure)?;
            } else {
                fs::remove_file(&target).map_err(io_failure)?;
            }
        }
        flush_setup_directory(tombstone)?;
        terminal_maintenance_crash_at("post-tombstone-content-removal");
        if record_present {
            fs::remove_file(&record).map_err(io_failure)?;
            flush_setup_directory(tombstone)?;
            terminal_maintenance_crash_at("post-tombstone-record-removal");
        }
        fs::remove_file(&marker).map_err(io_failure)?;
        flush_setup_directory(tombstone)?;
        terminal_maintenance_crash_at("post-tombstone-marker-removal");
    } else if fs::read_dir(tombstone)
        .map_err(io_failure)?
        .next()
        .transpose()
        .map_err(io_failure)?
        .is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is invalid.",
        ));
    }
    flush_setup_directory(tombstone)?;
    terminal_maintenance_crash_at("post-tombstone-removal");
    Ok(())
}

fn recover_terminal_recovery_tombstones(paths: &Paths) -> Result<()> {
    let mut tombstones = Vec::new();
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(generation) = name.strip_prefix(TERMINAL_RECOVERY_TOMBSTONE_PREFIX) else {
            continue;
        };
        validate_machine_lock_suffix(generation)?;
        let tombstone = entry.path();
        let marker = tombstone.join("launcher-tree-identity-v1");
        if path_present(&marker)? {
            let record_path = tombstone.join(TERMINAL_UNINSTALL_RECORD_NAME);
            if path_present(&record_path)? {
                let bytes = fs::read(&record_path).map_err(io_failure)?;
                let record: TerminalUninstallRecord = serde_json::from_slice(&bytes)
                    .map_err(|_| fail(EXIT_REJECTED, "Terminal tombstone record is invalid."))?;
                if record.generation != generation
                    || !matches!(
                        record.phase.as_str(),
                        "final-launcher-owned"
                            | "maintenance-deletion-owned"
                            | "uninstall-unregistered"
                            | "journal-removed"
                    )
                {
                    return Err(fail(EXIT_REJECTED, "Terminal tombstone record is invalid."));
                }
            }
        }
        remove_terminal_recovery_tombstone(paths, &tombstone)?;
        tombstones.push(tombstone);
    }
    if !tombstones.is_empty() || !path_present(&terminal_uninstall_root(paths))? {
        for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
            let entry = entry.map_err(io_failure)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = name
                .strip_prefix(TERMINAL_FINAL_LAUNCHER_PREFIX)
                .and_then(|value| value.strip_suffix(".exe"))
            else {
                continue;
            };
            validate_machine_lock_suffix(suffix)?;
            let path = entry.path();
            if !marker_security_is_exact(&path, MEDIUM_LAUNCHER_FILE_SDDL)? {
                return Err(fail(EXIT_REJECTED, "Terminal final launcher is invalid."));
            }
            if !pending_deletion_is_owned(&path)? {
                schedule_terminal_service_deletion(&path)?;
            }
        }
        for tombstone in &tombstones {
            if !pending_deletion_is_owned(tombstone)? {
                schedule_empty_terminal_tombstone_deletion(tombstone)?;
            }
        }
        if !tombstones.is_empty() {
            clear_machine_relaunch_owner(paths)?;
        }
        flush_setup_directory(&paths.program_data)?;
    }
    Ok(())
}

fn finish_terminal_uninstall(paths: &Paths) -> Result<()> {
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if !matches!(
        record.phase.as_str(),
        "cleanup-complete"
            | "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "maintenance-deleted"
            | "uninstall-unregistered"
            | "journal-removed"
    ) {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall cleanup is not complete.",
        ));
    }
    if path_present(&paths.transaction)? {
        require_uninstall_cleanup_complete(paths)?;
    }
    if !registry_key_absent(&format!(
        r"SYSTEM\CurrentControlSet\Services\{}",
        record.service_name
    ))? || path_present(Path::new(&record.service_image))?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service retirement is incomplete.",
        ));
    }
    let mut machine_lock = Some(MachineLock::acquire(
        paths,
        120_000,
        installed_recovery_policy_epoch(paths)?,
    )?);
    clear_update_recovery(paths)?;
    clear_legacy_profile_relaunch_owners(paths)?;
    remove_uninstall_finalizer_residue(paths)?;
    let root = terminal_uninstall_root(paths);
    remove_plain_tree(&root.join("Relaunch Records"))?;
    require_machine_relaunch_owner(paths)?;
    if record.phase == "cleanup-complete" {
        publish_terminal_final_launcher(paths, &record.generation)?;
        write_terminal_uninstall_phase(paths, &record.generation, "final-launcher-owned")?;
        terminal_maintenance_crash_at("post-final-launcher-ownership");
    }
    let published_phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if published_phase == "final-launcher-owned" {
        terminal_maintenance_crash_at("pre-maintenance-deletion-ownership");
        if hex_hash(&file_hash(&paths.maintenance_uninstaller)?) != record.maintenance_sha256 {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance image is invalid.",
            ));
        }
        schedule_terminal_service_deletion(&paths.maintenance_uninstaller)?;
        write_terminal_uninstall_phase(paths, &record.generation, "maintenance-deletion-owned")?;
        terminal_maintenance_crash_at("post-maintenance-deletion-ownership");
    }
    let mut phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if phase == "maintenance-deletion-owned" {
        if !path_present(&paths.maintenance_uninstaller)?
            || !pending_deletion_is_owned(&paths.maintenance_uninstaller)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance deletion ownership is invalid.",
            ));
        }
        publish_terminal_final_launcher(paths, &record.generation)?;
        unregister_uninstall()?;
        write_terminal_uninstall_phase(paths, &record.generation, "uninstall-unregistered")?;
        terminal_maintenance_crash_at("post-uninstall-unregister");
        phase = "uninstall-unregistered".into();
    }
    if phase == "uninstall-unregistered" {
        remove_transaction(paths)?;
        write_terminal_uninstall_phase(paths, &record.generation, "journal-removed")?;
        terminal_maintenance_crash_at("post-journal-removal");
        phase = "journal-removed".into();
    }
    if phase != "journal-removed" {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal maintenance phase is invalid.",
        ));
    }
    let identity =
        owned_tree_identity(&root).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&root)?
        || !fs::read_to_string(root.join("launcher-tree-identity-v1"))
            .is_ok_and(|value| value == identity)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery root identity is invalid.",
        ));
    }
    let tombstone = terminal_recovery_tombstone(paths, &record.generation)?;
    if path_present(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone already exists.",
        ));
    }
    fs::rename(&root, &tombstone).map_err(io_failure)?;
    flush_setup_directory(&paths.program_data)?;
    terminal_maintenance_crash_at("post-root-tombstone-rename");
    remove_terminal_recovery_tombstone(paths, &tombstone)?;
    if path_present(&paths.maintenance_uninstaller)? {
        arm_mapped_image_deletion(&paths.maintenance_uninstaller)?;
    }
    terminal_maintenance_crash_at("post-maintenance-posix-delete");
    let final_launcher = terminal_final_launcher(paths, &record.generation)?;
    if !pending_deletion_is_owned(&final_launcher)? {
        schedule_terminal_service_deletion(&final_launcher)?;
    }
    if !pending_deletion_is_owned(&tombstone)? {
        schedule_empty_terminal_tombstone_deletion(&tombstone)?;
    }
    if !pending_deletion_is_owned(&final_launcher)? || !pending_deletion_is_owned(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal final deletion ownership is invalid.",
        ));
    }
    terminal_maintenance_crash_at("post-final-deletion-ownership");
    terminal_maintenance_crash_at("post-final-launcher-posix-delete");
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    drop(legacy);
    terminal_maintenance_crash_at("pre-machine-relaunch-owner-clear");
    clear_machine_relaunch_owner(paths)?;
    terminal_maintenance_crash_at("post-machine-relaunch-owner-clear");
    if path_present(&final_launcher)? {
        arm_mapped_image_deletion(&final_launcher)?;
    }
    if path_present(&tombstone)? {
        fs::remove_dir(&tombstone).map_err(io_failure)?;
        flush_setup_directory(&paths.program_data)?;
    }
    terminal_maintenance_crash_at("post-owner-clear-posix-cleanup");
    Ok(())
}

fn clear_machine_relaunch_owner(paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Talking Quill Update Relaunch";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(());
    }
    if opened != 0 {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
    }
    let launcher = paths.recovery_launcher.clone();
    let expected = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    let actual = read_registry_value(key, VALUE, 1024)?;
    let final_owner = actual.as_deref().is_some_and(|value| {
        let prefix = format!(
            "\"{}\\{TERMINAL_FINAL_LAUNCHER_PREFIX}",
            paths.program_data.display()
        );
        let suffix = ".exe\" --windows-update-relaunch-owner-v1";
        value.starts_with(&prefix)
            && value.ends_with(suffix)
            && value[prefix.len()..value.len() - suffix.len()].len() == 32
            && value[prefix.len()..value.len() - suffix.len()]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    });
    if actual
        .as_deref()
        .is_some_and(|value| value != expected && !final_owner)
    {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_REJECTED, "Machine relaunch owner was replaced."));
    }
    if actual.is_some() && unsafe { RegDeleteValueW(key, wide(OsStr::new(VALUE)).as_ptr()) } != 0 {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_FAILURE, "Cannot retire machine relaunch owner."));
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot flush machine relaunch retirement.",
        ))
    }
}

fn remove_update_recovery_launcher_residue(paths: &Paths) -> Result<()> {
    let launcher = paths.program_data.join("Talking Quill Update Recovery");
    if !path_present(&launcher)? {
        return Ok(());
    }
    let identity =
        owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&launcher)?
        || !fs::read_to_string(launcher.join("launcher-tree-identity-v1"))
            .is_ok_and(|value| value == identity)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update recovery launcher identity is invalid.",
        ));
    }
    let mut tree = Vec::new();
    collect_finalizer_deletion_paths(&launcher, &mut tree)?;
    for target in tree {
        if target == launcher {
            continue;
        }
        let metadata = fs::symlink_metadata(&target).map_err(io_failure)?;
        if metadata.is_dir() {
            fs::remove_dir(&target).map_err(io_failure)?;
        } else {
            fs::remove_file(&target).map_err(io_failure)?;
        }
    }
    // Keep the exact protected directory as a non-squattable namespace until HKLM Run is
    // flushed. Its identity was retained above and no executable remains in it.
    if owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
        != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update launcher directory was replaced.",
        ));
    }
    Ok(())
}

fn relocated_uninstall_matches_maintenance(target: &Path, paths: &Paths) -> Result<bool> {
    let canonical_target = std::fs::canonicalize(target).map_err(io_failure)?;
    let canonical_temp = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = canonical_target
        .file_name()
        .and_then(|value| value.to_str());
    Ok(canonical_target.parent() == Some(canonical_temp.as_path())
        && name.is_some_and(|value| {
            value.starts_with(".TalkingQuill-uninstall-")
                && value.ends_with(".exe")
                && value.len() == ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        })
        && path_present(&paths.maintenance_uninstaller)?
        && file_hash(target)? == file_hash(&paths.maintenance_uninstaller)?)
}

fn collect_finalizer_deletion_paths(path: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(io_failure)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Finalizer tree contains a reparse point.",
            ));
        }
        if metadata.is_dir() {
            collect_finalizer_deletion_paths(&child, output)?;
        } else if metadata.is_file() {
            output.push(child);
        } else {
            return Err(fail(EXIT_REJECTED, "Finalizer tree entry type is invalid."));
        }
    }
    output.push(path.to_path_buf());
    Ok(())
}

fn decode_pending_rename_pairs(data: &[u16]) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    let mut cursor = 0;
    let mut terminated = false;
    while cursor < data.len() {
        let source_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(fail(EXIT_REJECTED, "Pending deletion data is truncated."));
        }
        if cursor == source_start {
            let required_terminators = if pairs.is_empty() { 2 } else { 1 };
            if data.len() - cursor < required_terminators
                || data[cursor..].iter().any(|value| *value != 0)
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Pending deletion terminator is invalid.",
                ));
            }
            terminated = true;
            break;
        }
        let source = String::from_utf16(&data[source_start..cursor])
            .map_err(|_| fail(EXIT_REJECTED, "Pending deletion source is invalid."))?;
        cursor += 1;
        let destination_start = cursor;
        while cursor < data.len() && data[cursor] != 0 {
            cursor += 1;
        }
        if cursor == data.len() {
            return Err(fail(EXIT_REJECTED, "Pending deletion pair is truncated."));
        }
        let destination = String::from_utf16(&data[destination_start..cursor])
            .map_err(|_| fail(EXIT_REJECTED, "Pending deletion destination is invalid."))?;
        cursor += 1;
        pairs.push((source, destination));
    }
    if !terminated {
        return Err(fail(
            EXIT_REJECTED,
            "Pending deletion data lacks its final terminator.",
        ));
    }
    Ok(pairs)
}

fn read_pending_rename_pairs(key: HKEY) -> Result<Vec<(String, String)>> {
    const VALUE: &str = "PendingFileRenameOperations";
    let mut bytes = 0_u32;
    let mut value_type = 0_u32;
    let queried = unsafe {
        RegQueryValueExW(
            key,
            wide(OsStr::new(VALUE)).as_ptr(),
            ptr::null_mut(),
            &mut value_type,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if queried == 2 {
        return Ok(Vec::new());
    }
    if queried != 0
        || value_type != REG_MULTI_SZ
        || bytes == 0
        || bytes > 1024 * 1024
        || !bytes.is_multiple_of(2)
    {
        return Err(fail(EXIT_FAILURE, "Pending deletion ownership is invalid."));
    }
    let capacity = bytes;
    let mut data = vec![0_u16; bytes as usize / 2];
    let mut actual = bytes;
    let mut actual_type = 0_u32;
    if unsafe {
        RegQueryValueExW(
            key,
            wide(OsStr::new(VALUE)).as_ptr(),
            ptr::null_mut(),
            &mut actual_type,
            data.as_mut_ptr().cast(),
            &mut actual,
        )
    } != 0
        || actual_type != REG_MULTI_SZ
        || actual > capacity
        || !actual.is_multiple_of(2)
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot read pending deletion ownership.",
        ));
    }
    data.truncate(actual as usize / 2);
    decode_pending_rename_pairs(&data)
}

fn open_session_manager(access: u32) -> Result<HKEY> {
    const SESSION_MANAGER: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(SESSION_MANAGER)).as_ptr(),
            0,
            access,
            &mut key,
        )
    } != 0
    {
        Err(fail(EXIT_FAILURE, "Cannot open pending deletion state."))
    } else {
        Ok(key)
    }
}

fn normalized_pending_source(value: &str) -> String {
    let value = value.replace('/', "\\");
    if let Some(unc) = value.strip_prefix(r"\??\UNC\") {
        format!(r"\\{}", unc.to_ascii_lowercase())
    } else {
        value
            .strip_prefix(r"\??\")
            .or_else(|| value.strip_prefix(r"\\?\"))
            .unwrap_or(&value)
            .to_ascii_lowercase()
    }
}

fn schedule_terminal_service_deletion(path: &Path) -> Result<()> {
    assert_plain_file(path)?;
    let expected_path = canonical(path)?;
    let before_key = open_session_manager(KEY_READ)?;
    let before = read_pending_rename_pairs(before_key)?;
    unsafe { RegCloseKey(before_key) };
    if unsafe {
        MoveFileExW(
            wide(path.as_os_str()).as_ptr(),
            ptr::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Windows could not take terminal service deletion ownership.",
        ));
    }
    let key = open_session_manager(KEY_READ | KEY_WRITE)?;
    let after = read_pending_rename_pairs(key)?;
    let valid = after.len() == before.len() + 1
        && after.get(..before.len()) == Some(before.as_slice())
        && after.last().is_some_and(|(source, destination)| {
            destination.is_empty() && normalized_pending_source(source) == expected_path
        });
    let flushed = valid && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Terminal service deletion ownership is invalid.",
        ));
    }
    Ok(())
}

fn schedule_empty_terminal_tombstone_deletion(path: &Path) -> Result<()> {
    if !medium_launcher_directory_is_protected(path)?
        || fs::read_dir(path)
            .map_err(io_failure)?
            .next()
            .transpose()
            .map_err(io_failure)?
            .is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone is invalid.",
        ));
    }
    let expected = canonical(path)?;
    let before_key = open_session_manager(KEY_READ)?;
    let before = read_pending_rename_pairs(before_key)?;
    unsafe { RegCloseKey(before_key) };
    if unsafe {
        MoveFileExW(
            wide(path.as_os_str()).as_ptr(),
            ptr::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Windows could not own tombstone deletion.",
        ));
    }
    let key = open_session_manager(KEY_READ | KEY_WRITE)?;
    let after = read_pending_rename_pairs(key)?;
    let valid = after.len() == before.len() + 1
        && after.get(..before.len()) == Some(before.as_slice())
        && after.last().is_some_and(|(source, destination)| {
            destination.is_empty() && normalized_pending_source(source) == expected
        });
    let flushed = valid && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Tombstone deletion ownership is invalid.",
        ))
    }
}

fn remove_uninstall_finalizer_residue(paths: &Paths) -> Result<()> {
    for entry in fs::read_dir(&paths.program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let pending_suffix = name.strip_prefix(UNINSTALL_FINALIZER_PENDING_PREFIX);
        let published_suffix = name.strip_prefix(UNINSTALL_FINALIZER_PREFIX);
        if pending_suffix.is_none() && published_suffix.is_none() {
            continue;
        }
        let pending =
            pending_suffix.is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        let published =
            published_suffix.is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok());
        if !pending && !published {
            return Err(fail(EXIT_REJECTED, "Finalizer namespace entry is invalid."));
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !medium_launcher_directory_is_protected(&path)?
        {
            continue;
        }
        let identity =
            owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let marker = path.join("finalizer-tree-identity-v1");
        if pending || fs::read_to_string(marker).is_ok_and(|value| value == identity) {
            let executable = path.join(UNINSTALL_FINALIZER_NAME);
            if path_present(&executable)? {
                arm_mapped_image_deletion(&executable)?;
            }
            remove_owned_tree(&path, &identity)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        }
    }
    Ok(())
}

const SYNTHETIC_SCHEMA2_GENERATION: &str = "78bd88811b14faf1e11ba59620088aa0";
const SYNTHETIC_SCHEMA2_PENDING: &str = ".relaunch-pending-f3d466fe9027728be142ba84d6258074";
const SYNTHETIC_SCHEMA2_SHA256: &str =
    "abb2d6183c58b6ec52e28f6befbe43d949d2eeaf1998122921118272da8f3bad";
const SYNTHETIC_SCHEMA2_BYTES: &[u8] = br#"{"schemaVersion":2,"generation":"78bd88811b14faf1e11ba59620088aa0","userSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","logonSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","request":"--windows-update-bootstrap-v2=dGVzdA==","nonce":"11111111111111111111111111111111","sourceVersion":"0.0.69","targetVersion":"0.0.70","phase":"armed","completedVersion":null,"predecessor":{"version":"0.0.69","platform":"win32","architecture":"x64","sourceCommit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceTree":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","releaseBuildDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","roles":[]}}"#;

fn registry_key_present(root: HKEY, path: &str) -> Result<bool> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 0 {
        unsafe { RegCloseKey(key) };
        Ok(true)
    } else if status == 2 {
        Ok(false)
    } else {
        Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale coordination registry state.",
        ))
    }
}

fn directory_names(path: &Path) -> Result<Vec<String>> {
    let mut names = fs::read_dir(path)
        .map_err(io_failure)?
        .map(|entry| entry.map(|value| value.file_name().to_string_lossy().into_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(io_failure)?;
    names.sort_unstable();
    Ok(names)
}

fn no_talking_quill_process_except_authenticated_pair(authenticated_parent: bool) -> Result<bool> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale coordination processes.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let parent = authenticated_parent.then(parent_process_id).transpose()?;
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot enumerate stale coordination processes.",
        ));
    }
    loop {
        let executable_length = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let snapshot_name =
            String::from_utf16_lossy(&entry.szExeFile[..executable_length]).to_ascii_lowercase();
        let candidate = snapshot_name.starts_with("talking quill")
            || snapshot_name.starts_with("talking-quill");
        if candidate
            && entry.th32ProcessID != std::process::id()
            && Some(entry.th32ProcessID) != parent
        {
            let image = process_image(entry.th32ProcessID)?;
            let name = image
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if name == snapshot_name {
                return Ok(false);
            }
            return Err(fail(EXIT_REJECTED, "A candidate process identity changed."));
        }
        if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            if unsafe { GetLastError() } != 18 {
                return Err(fail(
                    EXIT_REJECTED,
                    "Process inventory changed during inspection.",
                ));
            }
            break;
        }
    }
    Ok(true)
}

fn hive_has_owned_run_value(hive: HKEY) -> Result<bool> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(false);
    }
    if status != 0 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect a Run owner hive."));
    }
    let result = (|| {
        let mut index = 0;
        loop {
            let mut name = [0_u16; 512];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if status == 259 {
                return Ok(false);
            }
            if status != 0 {
                return Err(fail(EXIT_REJECTED, "Cannot enumerate Run owners."));
            }
            if String::from_utf16_lossy(&name[..length as usize])
                .to_ascii_lowercase()
                .starts_with("talking quill")
            {
                return Ok(true);
            }
            index += 1;
        }
    })();
    unsafe { RegCloseKey(key) };
    result
}

fn no_owned_run_values() -> Result<bool> {
    if hive_has_owned_run_value(HKEY_LOCAL_MACHINE)? {
        return Ok(false);
    }
    let loaded = enumerate_registry_subkeys(HKEY_USERS)?;
    for hive_name in &loaded {
        if !hive_name.starts_with("S-1-5-") || hive_name.ends_with("_Classes") {
            continue;
        }
        let mut hive = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_USERS,
                wide(OsStr::new(hive_name)).as_ptr(),
                0,
                KEY_READ,
                &mut hive,
            )
        } != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A loaded user hive changed during inspection.",
            ));
        }
        let owned = hive_has_owned_run_value(hive);
        unsafe { RegCloseKey(hive) };
        if owned? {
            return Ok(false);
        }
    }
    const PROFILE_LIST: &str = r"Software\Microsoft\Windows NT\CurrentVersion\ProfileList";
    let mut profiles = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(PROFILE_LIST)).as_ptr(),
            0,
            KEY_READ,
            &mut profiles,
        )
    } != 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect offline profile Run owners.",
        ));
    }
    let profile_sids = match enumerate_registry_subkeys(profiles) {
        Ok(value) => value,
        Err(error) => {
            unsafe { RegCloseKey(profiles) };
            return Err(error);
        }
    };
    for sid in profile_sids {
        if !sid.starts_with("S-1-5-") || loaded.iter().any(|value| value == &sid) {
            continue;
        }
        let mut profile = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                profiles,
                wide(OsStr::new(&sid)).as_ptr(),
                0,
                KEY_READ,
                &mut profile,
            )
        } != 0
        {
            unsafe { RegCloseKey(profiles) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect an offline profile path.",
            ));
        }
        let profile_path = read_profile_image_path(profile);
        unsafe { RegCloseKey(profile) };
        let profile_path = match profile_path {
            Ok(value) => value,
            Err(error) => {
                unsafe { RegCloseKey(profiles) };
                return Err(error);
            }
        };
        let Some(ntuser) = profile_path.map(|path| path.join("NTUSER.DAT")) else {
            continue;
        };
        if !path_present(&ntuser)? {
            continue;
        }
        let mut offline = ptr::null_mut();
        if unsafe {
            RegLoadAppKeyW(
                wide(ntuser.as_os_str()).as_ptr(),
                &mut offline,
                KEY_READ,
                0,
                0,
            )
        } != 0
        {
            unsafe { RegCloseKey(profiles) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot inspect an offline profile Run owner.",
            ));
        }
        let owned = hive_has_owned_run_value(offline);
        unsafe { RegCloseKey(offline) };
        if owned? {
            unsafe { RegCloseKey(profiles) };
            return Ok(false);
        }
    }
    unsafe { RegCloseKey(profiles) };
    Ok(true)
}

fn registry_value_names(root: HKEY, path: &str) -> Result<Option<Vec<String>>> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect cleanup registry values.",
        ));
    }
    let result = (|| {
        let mut names = Vec::new();
        let mut index = 0;
        loop {
            let mut name = [0_u16; 256];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if status == 259 {
                break;
            }
            if status != 0 {
                return Err(fail(
                    EXIT_REJECTED,
                    "Cannot enumerate cleanup registry values.",
                ));
            }
            names.push(String::from_utf16_lossy(&name[..length as usize]));
            index += 1;
        }
        names.sort_unstable();
        Ok(names)
    })();
    unsafe { RegCloseKey(key) };
    result.map(Some)
}

fn registry_subkeys(root: HKEY, path: &str) -> Result<Option<Vec<String>>> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale registry inventory.",
        ));
    }
    let values = enumerate_registry_subkeys(key);
    unsafe { RegCloseKey(key) };
    let mut values = values?;
    values.sort_unstable();
    Ok(Some(values))
}

fn no_owned_task_files(system: &Path) -> Result<bool> {
    let tasks = system.join("Tasks");
    Ok(!directory_names(&tasks)?
        .iter()
        .any(|name| name.to_ascii_lowercase().starts_with("talkingquill")))
}

fn no_owned_service_keys() -> Result<bool> {
    Ok(
        !registry_subkeys(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services")?
            .unwrap_or_default()
            .iter()
            .any(|name| name.to_ascii_lowercase().starts_with("talkingquill")),
    )
}

const STALE_REGISTRY_HARDENED_SDDL: &str = "O:BAG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)";

fn protect_stale_registry_key(key: HKEY) -> Result<()> {
    let sddl = wide(OsStr::new(STALE_REGISTRY_HARDENED_SDDL));
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot create stale registry cleanup ACL.",
        ));
    }
    let status = unsafe {
        RegSetKeySecurity(
            key,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor) };
    if status == 0 && unsafe { RegFlushKey(key) } == 0 {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot protect stale registry cleanup state.",
        ))
    }
}

fn registry_acl_is_exact(key: HKEY) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetSecurityInfo(
            key,
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect machine lock registry ACL.",
        ));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode machine lock registry ACL.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
        .to_ascii_uppercase();
    unsafe { LocalFree(text.cast()) };
    Ok(value == "O:S-1-5-21-1333774511-1103852894-3119617217-1001G:S-1-5-21-1333774511-1103852894-3119617217-513D:AI(A;CIID;KR;;;BU)(A;CIID;KA;;;BA)(A;CIID;KA;;;SY)(A;ID;KA;;;S-1-5-21-1333774511-1103852894-3119617217-1001)(A;CIIOID;KA;;;CO)(A;CIID;KR;;;AC)(A;CIID;KR;;;S-1-15-3-1024-1065365936-1281604716-3511738428-1654721687-432734479-3232135806-4053264122-3456934681)".to_ascii_uppercase()
        || value == STALE_REGISTRY_HARDENED_SDDL.to_ascii_uppercase())
}

fn exact_machine_lock_publication() -> Result<Option<String>> {
    let mut key = ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(MACHINE_LOCK_REGISTRY_KEY)).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if status == 2 {
        return Ok(None);
    }
    if status != 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect machine lock publication.",
        ));
    }
    let publication = read_machine_lock_registry_string(key)?;
    if !registry_acl_is_exact(key)? {
        unsafe { RegCloseKey(key) };
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock registry ACL is not the recognized fixture ACL.",
        ));
    }
    let mut index = 0;
    let mut values = Vec::new();
    loop {
        let mut name = [0_u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if status == 259 {
            break;
        }
        if status != 0 {
            unsafe { RegCloseKey(key) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot enumerate machine lock publication.",
            ));
        }
        values.push(String::from_utf16_lossy(&name[..length as usize]));
        index += 1;
    }
    let mut child = [0_u16; 2];
    let mut child_len = 1;
    let child_status = unsafe {
        RegEnumKeyExW(
            key,
            0,
            child.as_mut_ptr(),
            &mut child_len,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    unsafe { RegCloseKey(key) };
    if values != [MACHINE_LOCK_REGISTRY_VALUE] || child_status != 259 {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock registry inventory is not exact.",
        ));
    }
    Ok(publication)
}

struct RetainedStaleObject {
    file: File,
    identity: String,
    path: PathBuf,
    directory: bool,
}

impl RetainedStaleObject {
    fn open(path: &Path, directory: bool) -> Result<Self> {
        Self::open_with_share(path, directory, 0)
    }

    fn open_lifecycle(path: &Path) -> Result<Self> {
        // Delete sharing permits POSIX unlink while a duplicate retains the same verified file
        // object. Read and write sharing remain denied, so an active lifecycle owner conflicts.
        Self::open_with_share(path, false, FILE_SHARE_DELETE)
    }

    fn open_with_share(path: &Path, directory: bool, share: u32) -> Result<Self> {
        let raw = unsafe {
            CreateFileW(
                wide(path.as_os_str()).as_ptr(),
                FILE_GENERIC_READ | DELETE,
                share,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_REJECTED,
                "A stale fixture object is active or unavailable.",
            ));
        }
        let file = unsafe { File::from_raw_handle(raw) };
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity is invalid.",
            ));
        }
        let identity = file_identity_text(&file)?;
        Ok(Self {
            file,
            identity,
            path: path.to_owned(),
            directory,
        })
    }

    fn verify(&self) -> Result<()> {
        if file_identity_text(&self.file)? != self.identity {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity changed.",
            ));
        }
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(self.file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != self.directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture type changed.",
            ));
        }
        Ok(())
    }

    fn names(&self) -> Result<Vec<String>> {
        if !self.directory {
            return Err(fail(
                EXIT_REJECTED,
                "A retained file has no child inventory.",
            ));
        }
        let mut names = retained_directory_names(self.file.as_raw_handle())
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        names.sort_unstable();
        Ok(names)
    }

    fn read_all(&mut self) -> Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).map_err(io_failure)?;
        Ok(bytes)
    }

    fn rename(&mut self, destination: &Path) -> Result<()> {
        rename_handle(self.file.as_raw_handle(), destination)?;
        self.path = destination.to_owned();
        Ok(())
    }

    fn mark_posix_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn mark_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn finish_deleted(self) -> Result<()> {
        let path = self.path.clone();
        drop(self);
        if path_present(&path)? {
            return Err(fail(
                EXIT_FAILURE,
                "Windows retained a handle-deleted stale object.",
            ));
        }
        Ok(())
    }

    fn delete(self) -> Result<()> {
        self.mark_deleted()?;
        self.finish_deleted()
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
const STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES: &[&str] = &[
    "request.exact-argv",
    "token.identity",
    "self.path",
    "self.open",
    "self.acl",
    "self.identity",
    "self.sha256",
    "package.tqpkg2",
    "package.source-binding",
    "audit.environment",
    "audit.path",
    "audit.acl",
    "audit.open",
    "audit.initialize",
    "audit.event",
    "mutex.availability",
    "paths.known-folders",
    "lifecycle-lock.availability",
    "registry.inventory",
    "active-state.inventory",
    "fixture.identity",
    "fixture.sha256",
    "image.stability",
    "diagnostic.complete",
    "cleanup.rejected.before-audit",
    "cleanup.rejected.after-audit",
];

#[cfg(feature = "stale-schema2-cleanup")]
struct StaleSchema2Diagnostic {
    file: File,
    parent: PathBuf,
    identity: String,
    operation: String,
    sequence: u32,
    chain: [u8; 32],
}

#[cfg(feature = "stale-schema2-cleanup")]
impl StaleSchema2Diagnostic {
    fn open() -> Result<Self> {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| {
                fail(
                    EXIT_REJECTED,
                    "TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH is required.",
                )
            })?;
        if !path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnostic path must be absolute.",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        let mut file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected diagnostic file.",
                )
            })?;
        if !protected_file_handle_acl_is_exact(&file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnostic is not administrator protected.",
            ));
        }
        let identity = file_identity_text(&file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut prior = Vec::new();
        file.read_to_end(&mut prior).map_err(io_failure)?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Diagnostic operation randomness is unavailable.",
            )
        })?;
        let operation = hex_hash(&nonce);
        let mut initial = Sha256::new();
        initial.update(b"TalkingQuill/stale-schema2-diagnostic-chain/v1\0");
        initial.update(identity.as_bytes());
        initial.update(&prior);
        initial.update(operation.as_bytes());
        Ok(Self {
            file,
            parent,
            identity,
            operation,
            sequence: 0,
            chain: initial.finalize().into(),
        })
    }

    fn record(
        &mut self,
        stage_code: &str,
        outcome: &str,
        evidence: serde_json::Value,
    ) -> Result<()> {
        if !STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.contains(&stage_code)
            || !matches!(outcome, "passed" | "rejected")
        {
            return Err(fail(EXIT_REJECTED, "Diagnostic stage code is invalid."));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic sequence is exhausted."))?;
        let previous = hex_hash(&self.chain);
        let unsigned = serde_json::json!({
            "schemaVersion": 1,
            "operation": "stale-schema2-diagnostic",
            "operationId": self.operation,
            "sequence": self.sequence,
            "diagnosticIdentity": self.identity,
            "stageCode": stage_code,
            "outcome": outcome,
            "evidence": evidence,
            "previousSha256": previous,
        });
        let bytes = serde_json::to_vec(&unsigned)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic event serialization failed."))?;
        let mut event_hash = Sha256::new();
        event_hash.update(self.chain);
        event_hash.update(&bytes);
        self.chain = event_hash.finalize().into();
        let mut event = unsigned;
        event
            .as_object_mut()
            .expect("diagnostic event is an object")
            .insert(
                "eventSha256".to_owned(),
                serde_json::Value::String(hex_hash(&self.chain)),
            );
        let mut line = serde_json::to_vec(&event)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic event serialization failed."))?;
        line.push(b'\n');
        self.file.write_all(&line).map_err(io_failure)?;
        self.file.sync_all().map_err(io_failure)?;
        flush_setup_directory(&self.parent)
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
fn diagnostic_stage<T, F>(
    diagnostic: &mut StaleSchema2Diagnostic,
    stage_code: &str,
    operation: F,
) -> Result<T>
where
    F: FnOnce() -> Result<(T, serde_json::Value)>,
{
    match operation() {
        Ok((value, evidence)) => {
            diagnostic.record(stage_code, "passed", evidence)?;
            Ok(value)
        }
        Err(error) => {
            let evidence = serde_json::json!({ "error": error.message });
            diagnostic.record(stage_code, "rejected", evidence)?;
            Err(fail(EXIT_REJECTED, error.message))
        }
    }
}

struct StaleCleanupAudit {
    file: File,
    parent: PathBuf,
    identity: String,
    operation: String,
    chain: [u8; 32],
}

impl StaleCleanupAudit {
    fn open() -> Result<Self> {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| fail(EXIT_REJECTED, "TQ_STALE_SCHEMA2_AUDIT_PATH is required."))?;
        if !path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale cleanup audit path must be absolute.",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Audit path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        let file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected cleanup audit file.",
                )
            })?;
        if !protected_file_handle_acl_is_exact(&file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Cleanup audit is not administrator protected.",
            ));
        }
        Self::from_retained(file, parent)
    }

    fn from_retained(mut file: File, parent: PathBuf) -> Result<Self> {
        let identity = file_identity_text(&file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut prior = Vec::new();
        file.read_to_end(&mut prior).map_err(io_failure)?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|_| fail(EXIT_FAILURE, "Audit operation randomness is unavailable."))?;
        let operation: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut initial = Sha256::new();
        initial.update(b"TalkingQuill/stale-schema2-audit-chain/v1\0");
        initial.update(identity.as_bytes());
        initial.update(&prior);
        initial.update(operation.as_bytes());
        Ok(Self {
            file,
            parent,
            identity,
            operation,
            chain: initial.finalize().into(),
        })
    }

    fn record(&mut self, stage: &str, binding: &str, proof: &str) -> Result<()> {
        let previous = hex_hash(&self.chain);
        let mut event = Sha256::new();
        event.update(self.chain);
        event.update(stage.as_bytes());
        event.update(binding.as_bytes());
        event.update(proof.as_bytes());
        self.chain = event.finalize().into();
        let line = format!(
            "{{\"schemaVersion\":1,\"operation\":\"stale-schema2-cleanup\",\"operationId\":\"{}\",\"auditIdentity\":\"{}\",\"stage\":\"{stage}\",\"bindingSha256\":\"{binding}\",\"proofSha256\":\"{proof}\",\"previousSha256\":\"{previous}\",\"eventSha256\":\"{}\"}}\n",
            self.operation,
            self.identity,
            hex_hash(&self.chain),
        );
        self.file.write_all(line.as_bytes()).map_err(io_failure)?;
        self.file.sync_all().map_err(io_failure)?;
        flush_setup_directory(&self.parent)
    }
}

fn retained_binding(objects: &[&RetainedStaleObject], suffix: &str) -> String {
    let mut entries = objects
        .iter()
        .map(|object| format!("{}:{}", object.path.display(), object.identity))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/stale-schema2-cleanup-binding/v1\0");
    hash.update(suffix.as_bytes());
    for entry in entries {
        hash.update(entry.as_bytes());
        hash.update([0]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    hex_hash(&digest)
}

fn active_state_proof(
    program_files: &Path,
    program_data: &Path,
    system: &Path,
    allow_coordination: bool,
    authenticated_parent: bool,
) -> Result<String> {
    let fixed_paths = [
        program_files.join("Talking Quill"),
        program_files.join(".Talking Quill.native-staging"),
        program_files.join(".Talking Quill.native-backup"),
        program_files.join(".Talking Quill.native-transaction-v2.json"),
        program_files.join(".Talking Quill.maintenance-generation-v1"),
        program_data.join("Talking Quill/KeyboardAuthority"),
        program_data.join("Talking Quill/.KeyboardAuthority.retirement-quarantine"),
        system.join("Tasks/TalkingQuillKeyboardAuthority"),
    ];
    let uninstall_present = registry_key_present(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?;
    let app_path_present = registry_key_present(HKEY_LOCAL_MACHINE, APP_PATH_KEY)?;
    let run_absent = no_owned_run_values()?;
    let processes_absent =
        no_talking_quill_process_except_authenticated_pair(authenticated_parent)?;
    let services_absent = no_owned_service_keys()?;
    let tasks_absent = no_owned_task_files(system)?;
    let legacy_service_present = terminal_service_exists("TalkingQuillKeyboardAuthority")?;
    if fixed_paths
        .iter()
        .any(|path| path_present(path).unwrap_or(true))
        || uninstall_present
        || app_path_present
        || !run_absent
        || !processes_absent
        || !services_absent
        || !tasks_absent
        || legacy_service_present
    {
        return Err(fail(
            EXIT_REJECTED,
            "Active or unknown Talking Quill state blocks stale cleanup.",
        ));
    }
    let mut evidence = fixed_paths
        .iter()
        .map(|path| format!("absent:{}", path.display()))
        .collect::<Vec<_>>();
    evidence.extend([
        format!("uninstall-present:{uninstall_present}"),
        format!("app-path-present:{app_path_present}"),
        format!("run-absent:{run_absent}"),
        format!("processes-absent:{processes_absent}"),
        format!("services-absent:{services_absent}"),
        format!("tasks-absent:{tasks_absent}"),
        format!("legacy-service-present:{legacy_service_present}"),
    ]);
    let mut program_files_inventory = Vec::new();
    for entry in fs::read_dir(program_files).map_err(io_failure)? {
        let name = entry
            .map_err(io_failure)?
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase();
        program_files_inventory.push(name.clone());
        if name.starts_with("talking quill maintenance-")
            || name.starts_with(".talking quill.native-transaction-v2.tmp-")
        {
            return Err(fail(
                EXIT_REJECTED,
                "Program Files recovery residue blocks stale cleanup.",
            ));
        }
    }
    program_files_inventory.sort_unstable();
    evidence.extend(
        program_files_inventory
            .into_iter()
            .map(|name| format!("pf:{name}")),
    );
    let mut program_data_inventory = Vec::new();
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let name = entry
            .map_err(io_failure)?
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase();
        program_data_inventory.push(name.clone());
        let coordination = name == "talking quill update recovery"
            || name.starts_with(".talking quill.machine-lock-")
            || name.starts_with(".talking quill.machine-lifecycle-retained-");
        let other = name.starts_with(".talking quill.update-")
            || name.starts_with(".talking quill.uninstall-finalizer-")
            || name.starts_with(".talking quill.terminal-")
            || name.starts_with("talking quill terminal");
        if other || (coordination && !allow_coordination) {
            return Err(fail(
                EXIT_REJECTED,
                "ProgramData recovery residue blocks stale cleanup.",
            ));
        }
    }
    program_data_inventory.sort_unstable();
    evidence.extend(
        program_data_inventory
            .into_iter()
            .map(|name| format!("pd:{name}")),
    );
    let talking_quill_registry =
        registry_key_present(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
    if !allow_coordination && talking_quill_registry {
        return Err(fail(
            EXIT_REJECTED,
            "Talking Quill registry residue remains.",
        ));
    }
    evidence.push(format!(
        "talking-quill-registry-present:{talking_quill_registry}"
    ));
    evidence.push(format!("allow-coordination:{allow_coordination}"));
    evidence.push(format!("authenticated-parent:{authenticated_parent}"));
    evidence.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/stale-schema2-machine-proof/v2\0");
    for entry in evidence {
        hash.update(entry.as_bytes());
        hash.update([0]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    Ok(hex_hash(&digest))
}

fn exact_cleanup_registry(suffix: &str) -> Result<()> {
    if exact_machine_lock_publication()?.as_deref() != Some(suffix)
        || registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?
            != Some(vec!["RecoveryStateLockV1".to_owned()])
        || registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cleanup registry inventory is not exact.",
        ));
    }
    Ok(())
}

fn complete_stale_cleanup_zero_state(
    audit: &mut StaleCleanupAudit,
    binding: &str,
    program_files: &Path,
    program_data: &Path,
    system: &Path,
    authenticated_parent: bool,
) -> Result<()> {
    let zero = active_state_proof(
        program_files,
        program_data,
        system,
        false,
        authenticated_parent,
    )?;
    audit.record("completed", binding, &zero)
}

#[cfg(feature = "stale-schema2-cleanup")]
fn direct_cleanup_source_identity() -> Result<(&'static str, &'static str)> {
    let commit = option_env!("TALKING_QUILL_RELEASE_COMMIT").ok_or_else(|| {
        fail(
            EXIT_REJECTED,
            "Direct cleanup build source commit is unavailable.",
        )
    })?;
    let tree = option_env!("TALKING_QUILL_RELEASE_TREE").ok_or_else(|| {
        fail(
            EXIT_REJECTED,
            "Direct cleanup build source tree is unavailable.",
        )
    })?;
    let exact_git_identity = |value: &str| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if !exact_git_identity(commit) || !exact_git_identity(tree) {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup build source identity is invalid.",
        ));
    }
    Ok((commit, tree))
}

#[cfg(feature = "stale-schema2-cleanup")]
fn open_authenticated_direct_cleanup_image() -> Result<(File, [u8; 32])> {
    let current = std::env::current_exe().map_err(io_failure)?;
    let kernel_image = process_image(std::process::id())?;
    if canonical(&current)? != canonical(&kernel_image)? {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup process image does not match its self path.",
        ));
    }
    let mut image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&current)
        .map_err(|_| fail(EXIT_REJECTED, "Direct cleanup image cannot be retained."))?;
    let parent = current
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Direct cleanup image has no parent."))?;
    if !staged_path_is_protected(parent, true)? || !protected_file_handle_acl_is_exact(&image)? {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup image is not administrator protected.",
        ));
    }
    let identity = file_identity_text(&image)?;
    let length = image.metadata().map_err(io_failure)?.len();
    let digest = hash_reader(&mut image)?;
    image.seek(SeekFrom::Start(0)).map_err(io_failure)?;
    let package = package::parse(&mut image, length).map_err(|error| {
        fail(
            EXIT_REJECTED,
            format!("Direct cleanup TQPKG2 validation failed: {error:?}"),
        )
    })?;
    let (source_commit, source_tree) = direct_cleanup_source_identity()?;
    let expected_architecture = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unsupported"
    };
    if package.manifest.source_commit != source_commit
        || package.manifest.source_tree != source_tree
        || package.manifest.architecture != expected_architecture
        || package.manifest.package_mode != "stale-schema2-cleanup"
        || package.manifest.predecessor.is_some()
        || package.manifest.fault_phase.is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup image does not match its compiled source identity.",
        ));
    }
    let path_image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&kernel_image)
        .map_err(|_| fail(EXIT_REJECTED, "Direct cleanup process image changed."))?;
    if file_identity_text(&path_image)? != identity {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup process image identity changed.",
        ));
    }
    Ok((image, digest))
}

#[cfg(feature = "stale-schema2-cleanup")]
fn run_direct_stale_schema2_diagnostic(arguments: &[OsString]) -> Result<i32> {
    let mut diagnostic = StaleSchema2Diagnostic::open()?;
    run_direct_stale_schema2_diagnostic_inner(&mut diagnostic, arguments)
}

#[cfg(feature = "stale-schema2-cleanup")]
fn run_direct_stale_schema2_diagnostic_inner(
    diagnostic: &mut StaleSchema2Diagnostic,
    arguments: &[OsString],
) -> Result<i32> {
    diagnostic_stage(diagnostic, "request.exact-argv", || {
        let argv_utf16 = arguments
            .iter()
            .map(|argument| argument.encode_wide().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        Ok(((), serde_json::json!({ "argvUtf16": argv_utf16 })))
    })?;
    diagnostic_stage(diagnostic, "token.identity", || {
        let elevated = token_is_elevated()?;
        let claims = peer_claims(std::process::id())?;
        if !direct_cleanup_token_is_authorized(elevated, claims.integrity_rid) {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnosis requires a high elevated token.",
            ));
        }
        Ok((
            (),
            serde_json::json!({
                "elevated": elevated,
                "integrityRid": claims.integrity_rid,
                "processId": std::process::id(),
            }),
        ))
    })?;
    let (current, kernel_image) = diagnostic_stage(diagnostic, "self.path", || {
        let current = std::env::current_exe().map_err(io_failure)?;
        let kernel_image = process_image(std::process::id())?;
        if canonical(&current)? != canonical(&kernel_image)? {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic process image does not match its self path.",
            ));
        }
        let evidence = serde_json::json!({
            "current": current.to_string_lossy(),
            "kernel": kernel_image.to_string_lossy(),
        });
        Ok(((current, kernel_image), evidence))
    })?;
    let mut retained_image = diagnostic_stage(diagnostic, "self.open", || {
        let image = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&current)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic image cannot be retained."))?;
        Ok((image, serde_json::json!({ "retained": true })))
    })?;
    diagnostic_stage(diagnostic, "self.acl", || {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic image has no parent."))?;
        if !staged_path_is_protected(parent, true)?
            || !protected_file_handle_acl_is_exact(&retained_image)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image is not administrator protected.",
            ));
        }
        Ok(((), serde_json::json!({ "protected": true })))
    })?;
    let identity = diagnostic_stage(diagnostic, "self.identity", || {
        let identity = file_identity_text(&retained_image)?;
        let path_image = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&kernel_image)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic process image changed."))?;
        if file_identity_text(&path_image)? != identity {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic process image identity changed.",
            ));
        }
        Ok((
            identity.clone(),
            serde_json::json!({ "fileIdentity": identity }),
        ))
    })?;
    let expected_hash = diagnostic_stage(diagnostic, "self.sha256", || {
        let digest = hash_reader(&mut retained_image)?;
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        Ok((digest, serde_json::json!({ "sha256": hex_hash(&digest) })))
    })?;
    let package = diagnostic_stage(diagnostic, "package.tqpkg2", || {
        let length = retained_image.metadata().map_err(io_failure)?.len();
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        let package = package::parse(&mut retained_image, length).map_err(|error| {
            fail(
                EXIT_REJECTED,
                format!("Diagnostic TQPKG2 validation failed: {error:?}"),
            )
        })?;
        Ok((
            package,
            serde_json::json!({ "parser": "rust", "format": "TQPKG2", "schemaVersion": 2 }),
        ))
    })?;
    diagnostic_stage(diagnostic, "package.source-binding", || {
        let (source_commit, source_tree) = direct_cleanup_source_identity()?;
        let expected_architecture = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "unsupported"
        };
        if package.manifest.source_commit != source_commit
            || package.manifest.source_tree != source_tree
            || package.manifest.architecture != expected_architecture
            || package.manifest.package_mode != "stale-schema2-cleanup"
            || package.manifest.predecessor.is_some()
            || package.manifest.fault_phase.is_some()
        {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image does not match its compiled source identity.",
            ));
        }
        Ok((
            (),
            serde_json::json!({
                "architecture": package.manifest.architecture,
                "packageMode": package.manifest.package_mode,
                "sourceCommit": package.manifest.source_commit,
                "sourceTree": package.manifest.source_tree,
            }),
        ))
    })?;
    let audit_path = diagnostic_stage(diagnostic, "audit.environment", || {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| fail(EXIT_REJECTED, "TQ_STALE_SCHEMA2_AUDIT_PATH is required."))?;
        Ok((
            path.clone(),
            serde_json::json!({ "path": path.to_string_lossy() }),
        ))
    })?;
    let audit_parent = diagnostic_stage(diagnostic, "audit.path", || {
        if !audit_path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale cleanup audit path must be absolute.",
            ));
        }
        let parent = audit_path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Audit path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        Ok((parent, serde_json::json!({ "absolute": true })))
    })?;
    let audit_file = diagnostic_stage(diagnostic, "audit.open", || {
        let file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&audit_path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected cleanup audit file.",
                )
            })?;
        Ok((file, serde_json::json!({ "opened": true })))
    })?;
    let audit_file = diagnostic_stage(diagnostic, "audit.acl", || {
        if !protected_file_handle_acl_is_exact(&audit_file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Cleanup audit is not administrator protected.",
            ));
        }
        Ok((audit_file, serde_json::json!({ "protected": true })))
    })?;
    let mut audit = diagnostic_stage(diagnostic, "audit.initialize", || {
        let audit = StaleCleanupAudit::from_retained(audit_file, audit_parent)?;
        let evidence = serde_json::json!({ "auditIdentity": audit.identity });
        Ok((audit, evidence))
    })?;
    diagnostic_stage(diagnostic, "audit.event", || {
        let empty = retained_binding(&[], "diagnostic-start");
        audit.record("diagnostic-start", &empty, &empty)?;
        Ok(((), serde_json::json!({ "stage": "diagnostic-start" })))
    })?;
    let legacy = diagnostic_stage(diagnostic, "mutex.availability", || {
        let mutex = LegacyMutexPair::acquire()?;
        Ok((mutex, serde_json::json!({ "available": true })))
    })?;
    let (program_files, program_data, system) =
        diagnostic_stage(diagnostic, "paths.known-folders", || {
            let program_files = known_folder(&FOLDERID_ProgramFiles)?;
            let program_data = known_folder(&FOLDERID_ProgramData)?;
            let system = known_folder(&FOLDERID_System)?;
            Ok((
                (program_files, program_data, system),
                serde_json::json!({ "resolved": true }),
            ))
        })?;
    let suffix = diagnostic_stage(diagnostic, "registry.inventory", || {
        let suffix = exact_machine_lock_publication()?;
        let subkeys = registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
        let values = registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
        if let Some(suffix) = suffix.as_deref() {
            validate_machine_lock_suffix(suffix)?;
            exact_cleanup_registry(suffix)?;
        }
        let evidence = serde_json::json!({
            "machineLockSuffix": suffix,
            "subkeys": subkeys,
            "values": values,
        });
        Ok((suffix, evidence))
    })?;
    let active = diagnostic_stage(diagnostic, "active-state.inventory", || {
        let proof = active_state_proof(&program_files, &program_data, &system, true, false)?;
        Ok((proof.clone(), serde_json::json!({ "proofSha256": proof })))
    })?;
    let Some(suffix) = suffix else {
        let binding = retained_binding(&[], "no-machine-lock-publication");
        diagnostic_stage(diagnostic, "image.stability", || {
            retained_image
                .seek(SeekFrom::Start(0))
                .map_err(io_failure)?;
            if hash_reader(&mut retained_image)? != expected_hash {
                return Err(fail(
                    EXIT_REJECTED,
                    "Diagnostic image changed during inspection.",
                ));
            }
            Ok(((), serde_json::json!({ "fileIdentity": identity })))
        })?;
        diagnostic_stage(diagnostic, "audit.event", || {
            audit.record("diagnostic-complete", &binding, &active)?;
            Ok(((), serde_json::json!({ "stage": "diagnostic-complete" })))
        })?;
        diagnostic_stage(diagnostic, "diagnostic.complete", || {
            Ok((
                (),
                serde_json::json!({ "bindingSha256": binding, "state": "absent" }),
            ))
        })?;
        drop(legacy);
        return Ok(0);
    };
    let lock_directory = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    let lifecycle = diagnostic_stage(diagnostic, "lifecycle-lock.availability", || {
        let object =
            RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?;
        let evidence = serde_json::json!({ "fileIdentity": object.identity });
        Ok((object, evidence))
    })?;
    let (
        lock_root,
        lock_tree_identity,
        lock_file_identity,
        pending,
        generation_guard,
        relaunch_guard,
        recovery_guard,
        bytes,
    ) = diagnostic_stage(diagnostic, "fixture.identity", || {
        let lock_root = RetainedStaleObject::open(&lock_directory, true)?;
        let mut lock_tree_identity =
            RetainedStaleObject::open(&lock_directory.join("lock-tree-identity-v1"), false)?;
        let mut lock_file_identity = RetainedStaleObject::open(
            &lock_directory.join("recovery-state-v1.identity-v1"),
            false,
        )?;
        let recovery = program_data.join("Talking Quill Update Recovery");
        let relaunch = recovery.join("Relaunch Records");
        let generation = relaunch.join(format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}"));
        let mut pending =
            RetainedStaleObject::open(&generation.join(SYNTHETIC_SCHEMA2_PENDING), false)?;
        let generation_guard = RetainedStaleObject::open(&generation, true)?;
        let relaunch_guard = RetainedStaleObject::open(&relaunch, true)?;
        let recovery_guard = RetainedStaleObject::open(&recovery, true)?;
        for object in [
            &lifecycle,
            &lock_root,
            &lock_tree_identity,
            &lock_file_identity,
            &pending,
            &generation_guard,
            &relaunch_guard,
            &recovery_guard,
        ] {
            object.verify()?;
        }
        let bytes = pending.read_all()?;
        if lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
            || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
            || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
            || relaunch_guard.names()?
                != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
            || recovery_guard.names()? != ["Relaunch Records"]
            || lock_root.names()?
                != [
                    "lock-tree-identity-v1",
                    "recovery-state-v1.identity-v1",
                    "recovery-state-v1.lock",
                ]
        {
            return Err(fail(
                EXIT_REJECTED,
                "Retained stale fixture identity is not exact.",
            ));
        }
        let evidence = serde_json::json!({
            "lifecycle": lifecycle.identity,
            "lockRoot": lock_root.identity,
            "pending": pending.identity,
        });
        Ok((
            (
                lock_root,
                lock_tree_identity,
                lock_file_identity,
                pending,
                generation_guard,
                relaunch_guard,
                recovery_guard,
                bytes,
            ),
            evidence,
        ))
    })?;
    diagnostic_stage(diagnostic, "fixture.sha256", || {
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes != SYNTHETIC_SCHEMA2_BYTES || hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256 {
            return Err(fail(
                EXIT_REJECTED,
                "Retained stale fixture hash is not exact.",
            ));
        }
        Ok((
            (),
            serde_json::json!({ "sha256": hex_hash(&digest), "size": bytes.len() }),
        ))
    })?;
    let objects = [
        &lifecycle,
        &lock_root,
        &lock_tree_identity,
        &lock_file_identity,
        &pending,
        &generation_guard,
        &relaunch_guard,
        &recovery_guard,
    ];
    let binding = retained_binding(&objects, &suffix);
    diagnostic_stage(diagnostic, "image.stability", || {
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        if hash_reader(&mut retained_image)? != expected_hash {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image changed during inspection.",
            ));
        }
        Ok(((), serde_json::json!({ "fileIdentity": identity })))
    })?;
    diagnostic_stage(diagnostic, "audit.event", || {
        audit.record("diagnostic-complete", &binding, &active)?;
        Ok(((), serde_json::json!({ "stage": "diagnostic-complete" })))
    })?;
    diagnostic_stage(diagnostic, "diagnostic.complete", || {
        Ok((
            (),
            serde_json::json!({ "bindingSha256": binding, "state": "exact-schema2-fixture" }),
        ))
    })?;
    drop(legacy);
    Ok(0)
}

#[cfg(feature = "stale-schema2-cleanup")]
fn force_stale_cleanup_rejection(stage: &str) -> Result<()> {
    if std::env::var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE").as_deref() == Ok(stage) {
        Err(fail(
            EXIT_REJECTED,
            format!("Forced stale schema-2 cleanup rejection at {stage}."),
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
fn record_post_audit_cleanup_rejection(
    diagnostic: &mut StaleSchema2Diagnostic,
    audit: &mut StaleCleanupAudit,
    error: SetupError,
) -> Result<()> {
    let stage = "cleanup.rejected.after-audit";
    let evidence = serde_json::json!({ "error": error.message, "exitCode": EXIT_REJECTED });
    let diagnostic_result = diagnostic.record(stage, "rejected", evidence);
    let empty = retained_binding(&[], stage);
    let audit_result = audit.record(stage, &empty, &empty);
    if let Err(audit_error) = audit_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection audit failed: {}",
                error.message, audit_error.message
            ),
        ));
    }
    if let Err(diagnostic_error) = diagnostic_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection diagnostic failed: {}",
                error.message, diagnostic_error.message
            ),
        ));
    }
    Err(fail(EXIT_REJECTED, error.message))
}

#[cfg(feature = "stale-schema2-cleanup")]
fn run_direct_elevated_stale_schema2_cleanup() -> Result<()> {
    let mut diagnostic = StaleSchema2Diagnostic::open()?;
    let token = token_is_elevated().and_then(|elevated| {
        peer_claims(std::process::id()).map(|claims| (elevated, claims.integrity_rid))
    });
    let (elevated, integrity_rid) = match token {
        Ok(value) if direct_cleanup_token_is_authorized(value.0, value.1) => {
            diagnostic.record(
                "token.identity",
                "passed",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1 }),
            )?;
            value
        }
        Ok(value) => {
            let error = "Direct stale cleanup requires a high elevated token.";
            diagnostic.record(
                "token.identity",
                "rejected",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1, "error": error }),
            )?;
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error }),
            )?;
            return Err(fail(EXIT_REJECTED, error));
        }
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    debug_assert!(direct_cleanup_token_is_authorized(elevated, integrity_rid));
    let image = open_authenticated_direct_cleanup_image();
    let (mut retained_image, expected_hash) = match image {
        Ok(value) => value,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    // Retain a protected append handle so a later cleanup failure can be recorded durably.
    let audit = StaleCleanupAudit::open();
    let mut audit = match audit {
        Ok(audit) => audit,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    let operation = (|| -> Result<()> {
        reclaim_exact_schema2_orphan_with_audit(true, false, &mut audit)?;
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        if hash_reader(&mut retained_image)? != expected_hash {
            return Err(fail(
                EXIT_REJECTED,
                "Direct cleanup image changed during the operation.",
            ));
        }
        Ok(())
    })();
    if let Err(error) = operation {
        return record_post_audit_cleanup_rejection(&mut diagnostic, &mut audit, error);
    }
    Ok(())
}

fn reclaim_exact_schema2_orphan_v2(
    developer_command: bool,
    authenticated_parent: bool,
    audit: &mut StaleCleanupAudit,
) -> Result<()> {
    if !token_is_elevated()? {
        return Err(fail(EXIT_REJECTED, "Stale cleanup requires elevation."));
    }
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = known_folder(&FOLDERID_System)?;
    let legacy = LegacyMutexPair::acquire()?;
    if !developer_command && std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH").is_none() {
        return Err(fail(
            EXIT_REJECTED,
            "Production orphan reclaim requires an administrator audit path.",
        ));
    }
    let Some(suffix) = exact_machine_lock_publication()? else {
        let binding = retained_binding(&[], "no-machine-lock-publication");
        complete_stale_cleanup_zero_state(
            audit,
            &binding,
            &program_files,
            &program_data,
            &system,
            authenticated_parent,
        )?;
        drop(legacy);
        return Ok(());
    };
    validate_machine_lock_suffix(&suffix)?;
    let lock_directory = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    let lifecycle =
        RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?;
    exact_cleanup_registry(&suffix)?;

    // The lifecycle file is retained before any process, role, service, task, Run, registration,
    // journal, terminal, Program Files, or ProgramData activity inspection.
    if !staged_path_is_protected(&lock_directory, true)?
        || !staged_path_is_protected(&lock_directory.join("recovery-state-v1.lock"), false)?
        || !staged_path_is_protected(&lock_directory.join("lock-tree-identity-v1"), false)?
        || !staged_path_is_protected(&lock_directory.join("recovery-state-v1.identity-v1"), false)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lifecycle ACL inventory is not exact.",
        ));
    }
    let lock_root = RetainedStaleObject::open(&lock_directory, true)?;
    let mut lock_tree_identity =
        RetainedStaleObject::open(&lock_directory.join("lock-tree-identity-v1"), false)?;
    let mut lock_file_identity =
        RetainedStaleObject::open(&lock_directory.join("recovery-state-v1.identity-v1"), false)?;
    let recovery = program_data.join("Talking Quill Update Recovery");
    let relaunch = recovery.join("Relaunch Records");
    let generation = relaunch.join(format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}"));
    let mut pending =
        RetainedStaleObject::open(&generation.join(SYNTHETIC_SCHEMA2_PENDING), false)?;
    let admission = active_state_proof(
        &program_files,
        &program_data,
        &system,
        true,
        authenticated_parent,
    )?;
    // The pending file and lifecycle lock are exclusive before this first mutation. Harden each
    // parent from the inside out, then retain it without sharing. Exact handle inventories below
    // reject anything that appeared before hardening.
    apply_lock_dacl(&generation, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let generation_guard = RetainedStaleObject::open(&generation, true)?;
    apply_lock_dacl(&relaunch, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let relaunch_guard = RetainedStaleObject::open(&relaunch, true)?;
    apply_lock_dacl(&recovery, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let recovery_guard = RetainedStaleObject::open(&recovery, true)?;
    let bytes = pending.read_all()?;
    let lock_root_marker = String::from_utf8(lock_tree_identity.read_all()?)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lock root marker is not UTF-8."))?;
    let lock_file_marker = String::from_utf8(lock_file_identity.read_all()?)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lifecycle marker is not UTF-8."))?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes != SYNTHETIC_SCHEMA2_BYTES
        || lock_root_marker != lock_root.identity
        || lock_file_marker != lifecycle.identity
        || hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256
        || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
        || relaunch_guard.names()? != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
        || recovery_guard.names()? != ["Relaunch Records"]
        || lock_root.names()?
            != [
                "lock-tree-identity-v1",
                "recovery-state-v1.identity-v1",
                "recovery-state-v1.lock",
            ]
    {
        return Err(fail(
            EXIT_REJECTED,
            "Retained stale fixture inventory is not exact.",
        ));
    }
    for object in [
        &lifecycle,
        &lock_root,
        &lock_tree_identity,
        &lock_file_identity,
        &pending,
        &generation_guard,
        &relaunch_guard,
        &recovery_guard,
    ] {
        object.verify()?;
    }
    let objects = [
        &lifecycle,
        &lock_root,
        &lock_tree_identity,
        &lock_file_identity,
        &pending,
        &generation_guard,
        &relaunch_guard,
        &recovery_guard,
    ];
    let binding = retained_binding(&objects, &suffix);
    audit.record("inspected", &binding, &admission)?;
    #[cfg(feature = "stale-schema2-cleanup")]
    force_stale_cleanup_rejection("post-inspected")?;
    std::thread::sleep(Duration::from_millis(750));
    for object in objects {
        object.verify()?;
    }
    if pending.read_all()? != SYNTHETIC_SCHEMA2_BYTES
        || lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
        || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
        || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
        || relaunch_guard.names()? != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
        || recovery_guard.names()? != ["Relaunch Records"]
        || lock_root.names()?
            != [
                "lock-tree-identity-v1",
                "recovery-state-v1.identity-v1",
                "recovery-state-v1.lock",
            ]
    {
        return Err(fail(
            EXIT_REJECTED,
            "Retained fixture changed during the stability wait.",
        ));
    }
    let second = active_state_proof(
        &program_files,
        &program_data,
        &system,
        true,
        authenticated_parent,
    )?;
    exact_cleanup_registry(&suffix)?;
    audit.record("commit-intent", &binding, &second)?;
    #[cfg(feature = "stale-schema2-cleanup")]
    force_stale_cleanup_rejection("post-commit-intent")?;

    // Repeat registry identity after the second active proof and before the first mutation.
    if exact_machine_lock_publication()?.as_deref() != Some(&suffix) {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lifecycle publication changed before mutation.",
        ));
    }
    for registry_path in [r"Software\Talking Quill", MACHINE_LOCK_REGISTRY_KEY] {
        let mut key = ptr::null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                wide(OsStr::new(registry_path)).as_ptr(),
                0,
                KEY_READ | KEY_WRITE | WRITE_DAC,
                &mut key,
            )
        } != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "Stale registry key changed before mutation.",
            ));
        }
        let result = protect_stale_registry_key(key);
        unsafe { RegCloseKey(key) };
        result?;
    }

    pending.delete()?;
    generation_guard.delete()?;
    relaunch_guard.delete()?;
    recovery_guard.delete()?;
    lock_tree_identity.delete()?;
    lock_file_identity.delete()?;
    // Move the retained lifecycle file out of its tree, mark it delete-pending, and keep that
    // same identity handle through registry-last deletion. This lets the now-empty lock tree be
    // handle-deleted without releasing lifecycle authority.
    let mut lifecycle = lifecycle;
    let lifecycle_identity = lifecycle.identity.clone();
    let retained_lifecycle_path = program_data.join(format!(
        ".Talking Quill.machine-lifecycle-retained-{suffix}"
    ));
    lifecycle.rename(&retained_lifecycle_path)?;
    lifecycle.mark_posix_deleted()?;
    lock_root.delete()?;
    flush_setup_directory(&program_data)?;
    exact_cleanup_registry(&suffix)?;
    delete_registry_tree_durable(
        MACHINE_LOCK_REGISTRY_KEY,
        r"Software\Talking Quill",
        "stale machine lifecycle publication",
    )?;
    if registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
        || registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
    {
        return Err(fail(
            EXIT_REJECTED,
            "Registry parent gained unknown content.",
        ));
    }
    delete_registry_tree_durable(
        r"Software\Talking Quill",
        r"Software",
        "empty Talking Quill registry parent",
    )?;
    if file_identity_text(&lifecycle.file)? != lifecycle_identity {
        return Err(fail(
            EXIT_REJECTED,
            "Retained lifecycle authority changed during registry deletion.",
        ));
    }
    lifecycle.finish_deleted()?;
    flush_setup_directory(&program_data)?;
    complete_stale_cleanup_zero_state(
        audit,
        &binding,
        &program_files,
        &program_data,
        &system,
        authenticated_parent,
    )?;
    drop(legacy);
    Ok(())
}

fn reclaim_exact_schema2_orphan_with_audit(
    developer_command: bool,
    authenticated_parent: bool,
    audit: &mut StaleCleanupAudit,
) -> Result<()> {
    reclaim_exact_schema2_orphan_v2(developer_command, authenticated_parent, audit)
}

fn reclaim_exact_schema2_orphan(developer_command: bool, authenticated_parent: bool) -> Result<()> {
    let mut audit = StaleCleanupAudit::open()?;
    reclaim_exact_schema2_orphan_with_audit(developer_command, authenticated_parent, &mut audit)
}

fn remove_machine_lock_residue(paths: &Paths, suffix: &str) -> Result<()> {
    validate_machine_lock_suffix(suffix)?;
    let path = paths
        .program_data
        .join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path_present(&path)? {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity =
        owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    remove_owned_tree(&path, &identity).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    flush_setup_directory(&paths.program_data)
}

fn unregister_uninstall() -> Result<()> {
    delete_registry_tree_durable(
        UNINSTALL_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "native uninstall registration",
    )
}

fn delete_registry_tree_durable(path: &str, parent: &str, label: &str) -> Result<()> {
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(OsStr::new(path)).as_ptr()) };
    if status != 0 && status != 2 {
        return Err(fail(EXIT_FAILURE, format!("Cannot remove the {label}.")));
    }
    let mut deleted = ptr::null_mut();
    let observed = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(path)).as_ptr(),
            0,
            KEY_READ,
            &mut deleted,
        )
    };
    if observed == 0 {
        unsafe { RegCloseKey(deleted) };
        return Err(fail(EXIT_FAILURE, format!("Windows retained the {label}.")));
    }
    if observed != 2 {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot verify removal of the {label}."),
        ));
    }
    let mut parent_key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(parent)).as_ptr(),
            0,
            KEY_READ,
            &mut parent_key,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot open the {label} parent."),
        ));
    }
    let flushed = unsafe { RegFlushKey(parent_key) } == 0;
    unsafe { RegCloseKey(parent_key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            format!("Cannot flush removal of the {label}."),
        ))
    }
}

fn transaction_action(value: &Transaction) -> Result<Action> {
    match value.action.as_str() {
        "install" => Ok(Action::Install),
        "update" => Ok(Action::Update),
        "repair" => Ok(Action::Repair),
        "uninstall" => Ok(Action::Uninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction action is invalid.",
        )),
    }
}

fn write_transaction(
    paths: &Paths,
    phase: &str,
    action: Action,
    had_predecessor: bool,
) -> Result<()> {
    let temporary = paths
        .transaction
        .with_extension(format!("tmp-{}", std::process::id()));
    let action = match action {
        Action::Install => "install",
        Action::Update => "update",
        Action::Repair => "repair",
        Action::Uninstall => "uninstall",
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => {
            return Err(fail(EXIT_REJECTED, "Cleanup cannot create a transaction."));
        }
    };
    let bytes = serde_json::to_vec(&Transaction {
        schema_version: TRANSACTION_SCHEMA,
        phase: phase.into(),
        action: action.into(),
        had_predecessor,
    })
    .map_err(|_| fail(EXIT_FAILURE, "Cannot encode installer transaction."))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    output
        .write_all(&bytes)
        .and_then(|_| output.sync_all())
        .map_err(io_failure)?;
    durable_replace(&temporary, &paths.transaction)
}

fn cleanup_transaction_residue(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(pid) = name.strip_prefix(".Talking Quill.native-transaction-v2.tmp-") else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let file = open_plain_handle(&entry.path(), false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
}

fn remove_transaction(paths: &Paths) -> Result<()> {
    if paths.transaction.exists() {
        let file = open_plain_handle(&paths.transaction, false, true)?;
        delete_retained(&file)?;
    }
    if paths.maintenance_generation_record.exists() {
        let file = open_plain_handle(&paths.maintenance_generation_record, false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
}

fn create_plain_directories(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| fail(EXIT_REJECTED, "Package escaped staging."))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if !current.exists() {
            fs::create_dir(&current).map_err(io_failure)?;
        }
        assert_plain_directory(&current)?;
    }
    Ok(())
}

fn open_plain_handle(path: &Path, directory: bool, delete: bool) -> Result<OwnedHandle> {
    let access = FILE_GENERIC_READ | if delete { DELETE } else { 0 };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
        || tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || directory != (tag.FileAttributes & 0x10 != 0)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Installer object identity is not a plain expected file type.",
        ));
    }
    Ok(handle)
}

fn delete_retained(handle: &OwnedHandle) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn path_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_failure(error)),
    }
}

fn remove_plain_tree(path: &Path) -> Result<()> {
    if !path_present(path)? {
        return Ok(());
    }
    let identity =
        owned_tree_identity(path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    remove_owned_tree(path, &identity).map_err(|error| fail(EXIT_REJECTED, error.to_string()))
}

fn assert_plain_absent(path: &Path) -> Result<()> {
    if path_present(path)? {
        Err(fail(EXIT_REJECTED, "Installer staging already exists."))
    } else {
        Ok(())
    }
}
fn assert_plain_directory(path: &Path) -> Result<()> {
    open_plain_handle(path, true, false).map(|_| ())
}
fn assert_plain_file(path: &Path) -> Result<()> {
    open_plain_handle(path, false, false).map(|_| ())
}
fn durable_rename(from: &Path, to: &Path) -> Result<()> {
    move_file(from, to, MOVEFILE_WRITE_THROUGH)
}
fn durable_replace(from: &Path, to: &Path) -> Result<()> {
    move_file(from, to, MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING)
}
fn move_file(from: &Path, to: &Path, flags: u32) -> Result<()> {
    if unsafe {
        MoveFileExW(
            wide(from.as_os_str()).as_ptr(),
            wide(to.as_os_str()).as_ptr(),
            flags,
        )
    } == 0
    {
        Err(io_failure(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

fn canonical(path: &Path) -> Result<String> {
    Ok(fs::canonicalize(path)
        .map_err(io_failure)?
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase())
}

fn canonical_path_is_within(path: &Path, root: &Path) -> Result<bool> {
    let path = PathBuf::from(canonical(path)?);
    let root = PathBuf::from(canonical(root)?);
    Ok(path.starts_with(root))
}
fn io_failure(error: std::io::Error) -> SetupError {
    fail(EXIT_FAILURE, error.to_string())
}

fn known_folder(identifier: *const windows_sys::core::GUID) -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(identifier, 0, ptr::null_mut(), &mut raw) } != 0
        || raw.is_null()
    {
        return Err(fail(EXIT_FAILURE, "Windows known-folder lookup failed."));
    }
    let length = unsafe { (0..).position(|index| *raw.add(index) == 0).unwrap_or(0) };
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(value))
}

fn token_is_elevated() -> Result<bool> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect the setup token."));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(fail(EXIT_REJECTED, "Cannot read setup elevation."));
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn message_box(text: &str, flags: u32) -> i32 {
    let caption = wide(OsStr::new("Talking Quill setup"));
    let text = wide(OsStr::new(text));
    unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), caption.as_ptr(), flags) }
}
fn report(message: &str) {
    message_box(message, 0x10);
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    static CHANNEL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn atomic_marker_rename_reopens_the_same_identity_on_windows() {
        let root =
            std::env::temp_dir().join(format!("tq-marker-publication-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let marker = root.join("identity-v1");
        create_atomic_marker(&marker, "expected", MACHINE_LOCK_FILE_SDDL).unwrap();
        verify_atomic_marker(&marker, "expected", MACHINE_LOCK_FILE_SDDL, None).unwrap();
        assert_eq!(fs::read_to_string(&marker).unwrap(), "expected");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    fn transaction(phase: &str, action: &str, had_predecessor: bool) -> Transaction {
        Transaction {
            schema_version: TRANSACTION_SCHEMA,
            phase: phase.into(),
            action: action.into(),
            had_predecessor,
        }
    }

    #[test]
    fn every_durable_phase_has_a_bounded_recovery_direction() {
        for phase in ["staging", "staged", "prepared"] {
            assert_eq!(
                recovery_plan(&transaction(phase, "install", false), false, false).unwrap(),
                RecoveryPlan::DiscardStaging,
                "{phase}"
            );
            assert_eq!(
                recovery_plan(&transaction(phase, "repair", true), false, true).unwrap(),
                RecoveryPlan::DiscardStaging,
                "{phase}"
            );
        }
        for phase in [
            "predecessor-moved",
            "publishing",
            "published-before-persist",
            "published",
            "registered",
        ] {
            for action in ["install", "update", "repair"] {
                assert_eq!(
                    recovery_plan(
                        &transaction(phase, action, true),
                        true,
                        phase != "predecessor-moved"
                    )
                    .unwrap(),
                    RecoveryPlan::RestorePredecessor,
                    "{action}:{phase}"
                );
            }
        }
        for phase in [
            "prepared",
            "publishing",
            "published-before-persist",
            "published",
            "registered",
        ] {
            assert_eq!(
                recovery_plan(&transaction(phase, "install", false), false, true).unwrap(),
                RecoveryPlan::RemoveFreshCandidate,
                "{phase}"
            );
        }
        for phase in ["committed", "legacy-retiring", "legacy-retired"] {
            assert_eq!(
                recovery_plan(&transaction(phase, "repair", true), true, true).unwrap(),
                RecoveryPlan::FinishCommit,
                "{phase}"
            );
        }
        for phase in ["uninstall-armed", "uninstalling", "uninstall-quarantined"] {
            assert_eq!(
                recovery_plan(&transaction(phase, "uninstall", true), true, true).unwrap(),
                RecoveryPlan::FinishUninstall,
                "{phase}"
            );
        }
        assert!(recovery_plan(&transaction("prepared", "repair", true), false, false).is_err());
        assert!(recovery_plan(&transaction("unknown", "repair", true), true, true).is_err());
    }

    #[test]
    fn delayed_deletion_pairs_preserve_empty_destinations_and_exact_order() {
        let mut data = Vec::new();
        for value in [r"\??\C:\recovery\child.exe", "", r"\??\C:\recovery", ""] {
            data.extend(value.encode_utf16());
            data.push(0);
        }
        data.push(0);
        assert_eq!(
            decode_pending_rename_pairs(&data).unwrap(),
            vec![
                (r"\??\C:\recovery\child.exe".into(), String::new()),
                (r"\??\C:\recovery".into(), String::new()),
            ]
        );
        let malformed: Vec<u16> = "source-without-terminator".encode_utf16().collect();
        assert!(decode_pending_rename_pairs(&malformed).is_err());
        let mut missing_final = Vec::new();
        for value in [r"\??\C:\final.exe", ""] {
            missing_final.extend(value.encode_utf16());
            missing_final.push(0);
        }
        assert!(decode_pending_rename_pairs(&missing_final).is_err());
        assert!(decode_pending_rename_pairs(&[0]).is_err());
        assert_eq!(decode_pending_rename_pairs(&[0, 0]).unwrap(), Vec::new());
        let mut rename = Vec::new();
        for value in [r"\??\C:\source.exe", r"\??\C:\destination.exe"] {
            rename.extend(value.encode_utf16());
            rename.push(0);
        }
        rename.push(0);
        assert_eq!(
            decode_pending_rename_pairs(&rename).unwrap(),
            vec![(
                r"\??\C:\source.exe".into(),
                r"\??\C:\destination.exe".into()
            )]
        );
    }

    #[test]
    fn canonical_prefix_checks_are_component_aware() {
        let root = std::env::temp_dir().join(format!("tq-prefix-test-{}", std::process::id()));
        let child = root.join("child");
        let sibling = root.with_file_name(format!(
            "{}-sibling",
            root.file_name().unwrap().to_string_lossy()
        ));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&sibling);
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        assert!(canonical_path_is_within(&child, &root).unwrap());
        assert!(!canonical_path_is_within(&sibling, &root).unwrap());
        assert_eq!(
            normalized_pending_source(r"\??\UNC\server\share\cleanup.exe"),
            r"\\server\share\cleanup.exe"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(sibling).unwrap();
    }

    #[test]
    fn finalizer_cleanup_is_child_before_parent() {
        let root = std::env::temp_dir().join(format!(
            "tq-terminal-delete-plan-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        let child = nested.join("child");
        fs::write(&child, b"child").unwrap();
        let launcher = root.join("launcher.exe");
        fs::write(&launcher, b"launcher").unwrap();
        let mut plan = Vec::new();
        collect_finalizer_deletion_paths(&root, &mut plan).unwrap();
        let child_index = plan.iter().position(|path| path == &child).unwrap();
        let nested_index = plan.iter().position(|path| path == &nested).unwrap();
        let root_index = plan.iter().position(|path| path == &root).unwrap();
        assert!(child_index < nested_index && nested_index < root_index);
        assert_eq!(plan.last(), Some(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn orphaned_machine_lock_directory_is_reclaimable_after_publication_loss() {
        let root = std::env::temp_dir().join(format!(
            "tq-machine-lock-orphan-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let suffix = "44".repeat(16);
        let directory = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
        create_restricted_lock_directory(&directory).unwrap();
        apply_lock_dacl(&directory, MACHINE_LOCK_DIRECTORY_SDDL).unwrap();
        let identity = owned_tree_identity(&directory).unwrap();
        create_or_verify_lock_marker(
            &directory.join("publication-pending-v1"),
            &format!("{suffix}:{identity}"),
        )
        .unwrap();
        initialize_machine_lock_tree(&directory, &identity).unwrap();
        reclaim_unpublished_machine_lock_directories(&root).unwrap();
        assert!(!directory.exists());
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn terminal_uninstall_crash_transitions_recover_only_in_order() {
        assert_eq!(
            terminal_uninstall_recovery_step("armed", true).unwrap(),
            TerminalUninstallRecoveryStep::RetireMachine
        );
        assert!(terminal_uninstall_recovery_step("armed", false).is_err());
        assert_eq!(
            terminal_uninstall_recovery_step("machine-retired", true).unwrap(),
            TerminalUninstallRecoveryStep::FinishCleanup
        );
        for phase in [
            "cleanup-complete",
            "final-launcher-owned",
            "maintenance-deletion-owned",
            "maintenance-deleted",
            "uninstall-unregistered",
            "journal-removed",
        ] {
            for journal_present in [true, false] {
                assert_eq!(
                    terminal_uninstall_recovery_step(phase, journal_present).unwrap(),
                    TerminalUninstallRecoveryStep::CleanupComplete,
                    "{phase}:{journal_present}"
                );
            }
        }
    }

    #[test]
    fn terminal_uninstall_record_schema_is_strict() {
        let valid = br#"{"schemaVersion":3,"generation":"11111111111111111111111111111111","phase":"armed","maintenanceSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","uninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\"","quietUninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\" /S","serviceName":"TalkingQuillTerminalCleanup-11111111111111111111111111111111","serviceImage":"C:\\ProgramData\\.Talking Quill Terminal Cleanup-11111111111111111111111111111111.exe","serviceSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","serviceFileIdentity":"1:2","recordFileIdentity":"3:4"}"#;
        let record: TerminalUninstallRecord = serde_json::from_slice(valid).unwrap();
        assert_eq!(record.phase, "armed");
        let unknown = br#"{"schemaVersion":3,"generation":"11111111111111111111111111111111","phase":"armed","maintenanceSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","uninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\"","quietUninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\" /S","serviceName":"TalkingQuillTerminalCleanup-11111111111111111111111111111111","serviceImage":"C:\\ProgramData\\cleanup.exe","serviceSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","serviceFileIdentity":"1:2","recordFileIdentity":"3:4","path":"C:\\untrusted.exe"}"#;
        assert!(serde_json::from_slice::<TerminalUninstallRecord>(unknown).is_err());
    }

    #[test]
    fn legacy_profile_records_remove_only_exact_generations() {
        let root =
            std::env::temp_dir().join(format!("tq-profile-relaunch-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let generation = "22".repeat(16);
        let directory = root.join(&generation);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("relaunch-record-v1.json"),
            format!(
                "{{\"schemaVersion\":1,\"generation\":\"{generation}\",\"request\":\"--windows-update-bootstrap-v2=dGVzdA==\"}}"
            ),
        )
        .unwrap();
        remove_legacy_profile_relaunch_records(&root).unwrap();
        assert!(!root.exists());

        fs::create_dir_all(root.join("not-owned")).unwrap();
        fs::create_dir_all(root.join("33".repeat(16))).unwrap();
        fs::write(
            root.join("33".repeat(16)).join("relaunch-record-v1.json"),
            b"malformed user data",
        )
        .unwrap();
        remove_legacy_profile_relaunch_records(&root).unwrap();
        assert!(root.join("not-owned").exists());
        assert!(root.join("33".repeat(16)).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normal_relocated_uninstall_binds_original_and_maintenance_identity() {
        let suffix = "11".repeat(16);
        let root =
            std::env::temp_dir().join(format!("tq-relocated-source-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let maintenance = root.join(format!("Talking Quill Maintenance-{suffix}.exe"));
        let installed = root.join("Uninstall Talking Quill.exe");
        let relocated = std::env::temp_dir().join(format!(".TalkingQuill-uninstall-{suffix}.exe"));
        let _ = fs::remove_file(&relocated);
        fs::copy(std::env::current_exe().unwrap(), &maintenance).unwrap();
        fs::copy(&maintenance, &installed).unwrap();
        fs::copy(&maintenance, &relocated).unwrap();
        validate_relocated_uninstall_image(&relocated, &installed, &installed, &maintenance)
            .unwrap();
        fs::write(&installed, b"replaced").unwrap();
        assert!(
            validate_relocated_uninstall_image(&relocated, &installed, &installed, &maintenance)
                .is_err()
        );
        fs::remove_file(relocated).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mapped_uninstall_image_is_kernel_owned_before_machine_cleanup() {
        if let Some(marker) = std::env::var_os("TQ_SETUP_MAPPED_DELETE_MARKER") {
            let current = std::env::current_exe().unwrap();
            arm_mapped_image_deletion(&current).unwrap();
            fs::write(marker, b"armed").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let root =
            std::env::temp_dir().join(format!("tq-mapped-uninstall-delete-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let image = root.join("Uninstall Talking Quill.exe");
        fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
        let marker = root.join("armed");
        let mut child = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::mapped_uninstall_image_is_kernel_owned_before_machine_cleanup",
                "--nocapture",
            ])
            .env("TQ_SETUP_MAPPED_DELETE_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists());
        assert!(
            !image.exists(),
            "mapped image must be unlinked before success"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn controller_worker_cross_process_authenticates() {
        if std::env::var_os("TQ_SETUP_CHANNEL_CHILD").is_some() {
            let image = std::env::current_exe().unwrap();
            let (action, _, silent, lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&image, None).unwrap();
            assert_eq!(lifecycle_parent, 0);
            assert!(action == Action::Repair && silent);
            return;
        }
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        let channel = ControllerChannel::create(Action::Repair, true, 0).unwrap();
        let image = std::env::current_exe().unwrap();
        let mut child = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::controller_worker_cross_process_authenticates",
                "--nocapture",
            ])
            .env("TQ_SETUP_CHANNEL_CHILD", "1")
            .spawn()
            .unwrap();
        let mut duplicate = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let child_handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        let retained = channel.authenticate(&child_handle, &image, None).unwrap();
        assert_eq!(
            unsafe { GetProcessId(retained.as_raw_handle()) },
            child.id()
        );
        assert!(child.wait().unwrap().success());
    }

    fn test_paths(root: &Path) -> Paths {
        Paths {
            install: root.join("Talking Quill"),
            staging: root.join("staging"),
            backup: root.join("backup"),
            transaction: root.join("transaction.json"),
            maintenance_generation_record: root.join("maintenance-generation-v1"),
            maintenance_uninstaller: root
                .join(format!("Talking Quill Maintenance-{}.exe", "11".repeat(16))),
            recovery_launcher: root.join(format!(
                "Talking Quill Update Recovery/talking-quill-update-recovery-launcher-{}.exe",
                "11".repeat(16)
            )),
            profile: root.join("profile"),
            legacy_authority: root.join("legacy"),
            legacy_quarantine: root.join("quarantine"),
            legacy_task_file: root.join("task"),
            program_data: root.join("program-data"),
        }
    }

    #[test]
    fn pending_uninstall_reselection_is_process_stable_without_a_lifecycle_parent() {
        if let Some(root) = std::env::var_os("TQ_PENDING_UNINSTALL_RESELECT_CHILD") {
            let paths = test_paths(Path::new(&root));
            let lifecycle_parent = std::env::var("TQ_PENDING_UNINSTALL_LIFECYCLE_PARENT")
                .unwrap()
                .parse::<u32>()
                .unwrap();
            assert!(matches!(lifecycle_parent, 0 | 42));
            assert!(
                derive_action(&std::env::current_exe().unwrap(), &paths).unwrap()
                    == Action::Install
            );
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "tq-pending-uninstall-reselect-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let paths = test_paths(&root);
        fs::write(
            &paths.transaction,
            br#"{"schemaVersion":2,"phase":"uninstall-cleanup-complete","action":"uninstall","hadPredecessor":true}"#,
        )
        .unwrap();
        let image = std::env::current_exe().unwrap();
        for lifecycle_parent in [0, 42] {
            let status = Command::new(&image)
                .args([
                    "--exact",
                    "windows::tests::pending_uninstall_reselection_is_process_stable_without_a_lifecycle_parent",
                    "--nocapture",
                ])
                .env("TQ_PENDING_UNINSTALL_RESELECT_CHILD", &root)
                .env(
                    "TQ_PENDING_UNINSTALL_LIFECYCLE_PARENT",
                    lifecycle_parent.to_string(),
                )
                .status()
                .unwrap();
            assert!(status.success());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn controller_worker_kill_recovers_every_durable_phase() {
        if std::env::var_os("TQ_SETUP_SECOND_CLEANUP_CHILD").is_some() {
            let image = std::env::current_exe().unwrap();
            let (action, _, silent, lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&image, None).unwrap();
            assert!(action == Action::Uninstall && silent && lifecycle_parent == 0);
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        if let (Some(root), Some(phase)) = (
            std::env::var_os("TQ_SETUP_FAULT_ROOT"),
            std::env::var_os("TQ_SETUP_FAULT_PHASE"),
        ) {
            let image = std::env::current_exe().unwrap();
            let (_action, _, _, _) = WorkerChannel::connect_and_authenticate(&image, None).unwrap();
            let paths = test_paths(Path::new(&root));
            let phase = phase.to_string_lossy();
            let uninstalling = matches!(
                phase.as_ref(),
                "uninstall-armed"
                    | "uninstalling"
                    | "uninstall-cleanup-owned"
                    | "uninstall-quarantined"
                    | "recovering-finish-uninstall"
                    | "uninstall-cleanup-complete"
                    | "uninstall-finalizer-publishing"
                    | "uninstall-finalizer-published"
                    | "uninstall-finalizer-deletion-owned"
                    | "uninstall-terminal-committing"
                    | "uninstall-app-path-retiring"
                    | "uninstall-app-path-retired"
                    | "uninstall-registration-retiring"
                    | "uninstall-registration-retired"
                    | "uninstall-cleanup-elevation"
            );
            let had_predecessor = !uninstalling;
            let journal_phase = if phase == "uninstall-cleanup-elevation" {
                "uninstall-quarantined"
            } else {
                phase.as_ref()
            };
            match phase.as_ref() {
                "staging" | "staged" | "prepared" => {
                    fs::create_dir(&paths.install).unwrap();
                    fs::write(paths.install.join("identity"), b"predecessor").unwrap();
                    fs::create_dir(&paths.staging).unwrap();
                    fs::write(paths.staging.join("identity"), b"candidate").unwrap();
                }
                "predecessor-moved" | "publishing" => {
                    fs::create_dir(&paths.backup).unwrap();
                    fs::write(paths.backup.join("identity"), b"predecessor").unwrap();
                    fs::create_dir(&paths.staging).unwrap();
                    fs::write(paths.staging.join("identity"), b"candidate").unwrap();
                }
                "published-before-persist"
                | "published"
                | "registered"
                | "committed"
                | "legacy-retiring"
                | "legacy-retired" => {
                    fs::create_dir(&paths.backup).unwrap();
                    fs::write(paths.backup.join("identity"), b"predecessor").unwrap();
                    fs::create_dir(&paths.install).unwrap();
                    fs::write(paths.install.join("identity"), b"candidate").unwrap();
                }
                "uninstall-armed" | "uninstalling" | "uninstall-cleanup-owned" => {
                    fs::create_dir(&paths.install).unwrap();
                    fs::write(paths.install.join("identity"), b"candidate").unwrap();
                }
                "uninstall-quarantined"
                | "recovering-finish-uninstall"
                | "uninstall-cleanup-elevation" => {
                    fs::create_dir(&paths.backup).unwrap();
                    fs::write(paths.backup.join("identity"), b"candidate").unwrap();
                }
                "uninstall-cleanup-complete"
                | "uninstall-finalizer-publishing"
                | "uninstall-finalizer-published"
                | "uninstall-finalizer-deletion-owned"
                | "uninstall-terminal-committing"
                | "uninstall-app-path-retiring"
                | "uninstall-app-path-retired"
                | "uninstall-registration-retiring"
                | "uninstall-registration-retired" => {}
                _ => unreachable!(),
            }
            write_transaction(
                &paths,
                journal_phase,
                if uninstalling {
                    Action::Uninstall
                } else {
                    Action::Update
                },
                had_predecessor,
            )
            .unwrap();
            if phase == "uninstall-cleanup-elevation" {
                let channel = ControllerChannel::create(Action::Uninstall, true, 0).unwrap();
                let mut child = Command::new(&image)
                    .args([
                        "--exact",
                        "windows::tests::controller_worker_kill_recovers_every_durable_phase",
                        "--nocapture",
                    ])
                    .env("TQ_SETUP_SECOND_CLEANUP_CHILD", "1")
                    .spawn()
                    .unwrap();
                let mut duplicate = ptr::null_mut();
                assert_ne!(
                    unsafe {
                        DuplicateHandle(
                            GetCurrentProcess(),
                            child.as_raw_handle(),
                            GetCurrentProcess(),
                            &mut duplicate,
                            0,
                            0,
                            DUPLICATE_SAME_ACCESS,
                        )
                    },
                    0
                );
                let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
                let cleanup = channel.authenticate(&shell, &image, None).unwrap();
                fs::create_dir(&paths.program_data).unwrap();
                fs::write(
                    paths.program_data.join("cleanup-pid"),
                    child.id().to_string(),
                )
                .unwrap();
                drop(cleanup);
                let _ = child.wait();
            } else {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        let image = std::env::current_exe().unwrap();
        for phase in [
            "staging",
            "staged",
            "prepared",
            "predecessor-moved",
            "publishing",
            "published-before-persist",
            "published",
            "registered",
            "committed",
            "legacy-retiring",
            "legacy-retired",
            "uninstall-armed",
            "uninstalling",
            "uninstall-cleanup-owned",
            "uninstall-quarantined",
            "recovering-finish-uninstall",
            "uninstall-cleanup-complete",
            "uninstall-finalizer-publishing",
            "uninstall-finalizer-published",
            "uninstall-finalizer-deletion-owned",
            "uninstall-terminal-committing",
            "uninstall-app-path-retiring",
            "uninstall-app-path-retired",
            "uninstall-registration-retiring",
            "uninstall-registration-retired",
            "uninstall-cleanup-elevation",
        ] {
            let root =
                std::env::temp_dir().join(format!("tq-setup-fault-{}-{phase}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).unwrap();
            let uninstalling = matches!(
                phase,
                "uninstall-armed"
                    | "uninstalling"
                    | "uninstall-cleanup-owned"
                    | "uninstall-quarantined"
                    | "recovering-finish-uninstall"
                    | "uninstall-cleanup-complete"
                    | "uninstall-finalizer-publishing"
                    | "uninstall-finalizer-published"
                    | "uninstall-finalizer-deletion-owned"
                    | "uninstall-terminal-committing"
                    | "uninstall-app-path-retiring"
                    | "uninstall-app-path-retired"
                    | "uninstall-registration-retiring"
                    | "uninstall-registration-retired"
                    | "uninstall-cleanup-elevation"
            );
            let channel = ControllerChannel::create(
                if uninstalling {
                    Action::Uninstall
                } else {
                    Action::Repair
                },
                true,
                if uninstalling { std::process::id() } else { 0 },
            )
            .unwrap();
            let mut child = Command::new(&image)
                .args([
                    "--exact",
                    "windows::tests::controller_worker_kill_recovers_every_durable_phase",
                    "--nocapture",
                ])
                .env("TQ_SETUP_FAULT_ROOT", &root)
                .env("TQ_SETUP_FAULT_PHASE", phase)
                .spawn()
                .unwrap();
            let mut duplicate = ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        child.as_raw_handle(),
                        GetCurrentProcess(),
                        &mut duplicate,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
            let worker = channel.authenticate(&shell, &image, None).unwrap();
            let transaction = root.join("transaction.json");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !transaction.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(transaction.exists(), "{phase}");
            if phase == "uninstall-cleanup-elevation" {
                let marker = root.join("program-data/cleanup-pid");
                while !marker.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let pid: u32 = fs::read_to_string(marker).unwrap().parse().unwrap();
                let cleanup = unsafe {
                    OwnedHandle::from_raw_handle(OpenProcess(
                        windows_sys::Win32::System::Threading::PROCESS_TERMINATE | SYNCHRONIZE,
                        0,
                        pid,
                    ))
                };
                assert_ne!(unsafe { TerminateProcess(cleanup.as_raw_handle(), 197) }, 0);
                assert_eq!(
                    unsafe { WaitForSingleObject(cleanup.as_raw_handle(), 30_000) },
                    WAIT_OBJECT_0
                );
            }
            assert_ne!(unsafe { TerminateProcess(worker.as_raw_handle(), 197) }, 0);
            assert_eq!(
                unsafe { WaitForSingleObject(worker.as_raw_handle(), 30_000) },
                WAIT_OBJECT_0
            );
            let _ = child.wait();
            let paths = test_paths(&root);
            recover_with_system(&paths, false).unwrap();
            if uninstalling {
                require_uninstall_cleanup_complete(&paths).unwrap();
                remove_transaction(&paths).unwrap();
                assert!(!paths.install.exists(), "{phase}");
            } else {
                let expected =
                    if matches!(phase, "committed" | "legacy-retiring" | "legacy-retired") {
                        b"candidate".as_slice()
                    } else {
                        b"predecessor".as_slice()
                    };
                assert_eq!(
                    fs::read(paths.install.join("identity")).unwrap(),
                    expected,
                    "{phase}"
                );
            }
            assert!(
                !paths.backup.exists() && !paths.staging.exists() && !paths.transaction.exists(),
                "{phase}"
            );
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn recovery_progress_states_accept_every_terminal_topology() {
        let root =
            std::env::temp_dir().join(format!("tq-recovery-progress-{}", std::process::id()));
        for (phase, action, had_predecessor, installed) in [
            ("recovering-restore-predecessor", Action::Update, true, true),
            ("recovering-discard-staging", Action::Update, true, true),
            ("recovering-remove-fresh", Action::Install, false, false),
            ("recovering-finish-commit", Action::Update, true, true),
            (
                "recovering-finish-uninstall",
                Action::Uninstall,
                true,
                false,
            ),
        ] {
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).unwrap();
            let paths = test_paths(&root);
            if installed {
                fs::create_dir(&paths.install).unwrap();
                fs::write(paths.install.join("identity"), b"terminal").unwrap();
            }
            write_transaction(&paths, phase, action, had_predecessor).unwrap();
            recover_with_system(&paths, false).unwrap();
            if action == Action::Uninstall {
                require_uninstall_cleanup_complete(&paths).unwrap();
                remove_transaction(&paths).unwrap();
            }
            assert!(!paths.transaction.exists(), "{phase}");
            assert_eq!(paths.install.exists(), installed, "{phase}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delegated_handle_validation_never_owns_or_closes_the_callers_handle() {
        assert!(duplicate_delegated_process_handle(0, std::process::id()).is_err());
        assert!(duplicate_delegated_process_handle(u64::MAX, std::process::id()).is_err());
        let event =
            unsafe { OwnedHandle::from_raw_handle(CreateEventW(ptr::null(), 1, 0, ptr::null())) };
        let value = event.as_raw_handle() as usize as u64;
        assert!(duplicate_delegated_process_handle(value, std::process::id()).is_err());
        let mut flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(event.as_raw_handle(), &mut flags) },
            0
        );
    }

    #[test]
    fn pipe_proof_binds_nonce_both_peers_and_image() {
        let nonce = [1_u8; 32];
        let image = [2_u8; 32];
        let expected = channel_proof(&nonce, 10, 11, &image);
        assert_ne!(expected, channel_proof(&nonce, 11, 10, &image));
        assert_ne!(expected, channel_proof(&[3_u8; 32], 10, 11, &image));
        assert_ne!(expected, channel_proof(&nonce, 10, 11, &[4_u8; 32]));
    }

    #[test]
    fn p256_transcript_authenticates_roles_frames_and_both_ephemeral_keys() {
        let controller = ephemeral_secret().unwrap();
        let worker = ephemeral_secret().unwrap();
        let controller_public = controller.public_key().to_sec1_bytes();
        let worker_public = worker.public_key().to_sec1_bytes();
        let left = diffie_hellman(
            controller.to_nonzero_scalar(),
            worker.public_key().as_affine(),
        );
        let right = diffie_hellman(
            worker.to_nonzero_scalar(),
            controller.public_key().as_affine(),
        );
        assert_eq!(left.raw_secret_bytes(), right.raw_secret_bytes());
        let nonce = [7_u8; 32];
        let image = [9_u8; 32];
        let frame = [2_u8, 1];
        let proof = authenticated_proof(
            left.raw_secret_bytes(),
            &nonce,
            10,
            11,
            &image,
            &controller_public,
            &worker_public,
            b"controller",
            &frame,
        );
        assert_ne!(
            proof,
            authenticated_proof(
                right.raw_secret_bytes(),
                &nonce,
                10,
                11,
                &image,
                &controller_public,
                &worker_public,
                b"worker",
                &frame
            )
        );
        assert_ne!(
            proof,
            authenticated_proof(
                right.raw_secret_bytes(),
                &nonce,
                10,
                11,
                &image,
                &controller_public,
                &worker_public,
                b"controller",
                &[1, 1]
            )
        );
        assert!(deadline_ms(Instant::now() - Duration::from_millis(1)).is_err());
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn direct_stale_cleanup_rejects_wrong_arguments_and_token_modes() {
        let command = OsString::from("/TQ-CLEAN-STALE-SCHEMA2");
        assert!(direct_cleanup_arguments(
            std::slice::from_ref(&command),
            true
        ));
        assert!(!direct_cleanup_arguments(
            std::slice::from_ref(&command),
            false
        ));
        assert!(!direct_cleanup_arguments(&[], true));
        assert!(!direct_cleanup_arguments(
            &[command.clone(), OsString::from("/S")],
            true,
        ));
        assert!(!direct_cleanup_arguments(&[OsString::from("/S")], true));
        assert!(direct_cleanup_token_is_authorized(true, 0x3000));
        assert!(direct_cleanup_token_is_authorized(true, 0x4000));
        assert!(!direct_cleanup_token_is_authorized(false, 0x3000));
        assert!(!direct_cleanup_token_is_authorized(true, 0x2fff));
        let diagnostic = OsString::from("/TQ-DIAGNOSE-STALE-SCHEMA2");
        assert!(direct_diagnostic_arguments(std::slice::from_ref(
            &diagnostic
        )));
        assert!(!direct_diagnostic_arguments(&[]));
        assert!(!direct_diagnostic_arguments(&[
            diagnostic,
            OsString::from("/S")
        ]));
        assert_eq!(STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.len(), 26);
        let unique = STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.len());
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn direct_stale_cleanup_rejects_missing_relative_and_unprotected_audits() {
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
        assert!(StaleCleanupAudit::open().is_err());
        unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", "audit.jsonl") };
        assert!(StaleCleanupAudit::open().is_err());

        let root =
            std::env::temp_dir().join(format!("tq-stale-unprotected-audit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("audit.jsonl");
        fs::write(&path, b"").unwrap();
        unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path) };
        assert!(StaleCleanupAudit::open().is_err());
        unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_retained_handles_block_mutation_and_prove_zero() {
        let root = std::env::temp_dir().join(format!("tq-stale-retained-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let child = root.join("fixture");
        fs::write(&child, b"exact").unwrap();
        let child_guard = RetainedStaleObject::open(&child, false).unwrap();
        apply_lock_dacl(&root, MACHINE_LOCK_DIRECTORY_SDDL).unwrap();
        let root_guard = RetainedStaleObject::open(&root, true).unwrap();
        assert!(OpenOptions::new().write(true).open(&child).is_err());
        let mutation = root.join("mutation");
        let mutation_attempt = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&mutation);
        child_guard.verify().unwrap();
        root_guard.verify().unwrap();
        child_guard.delete().unwrap();
        if mutation_attempt.is_ok() {
            drop(mutation_attempt);
            assert!(root_guard.delete().is_err());
            assert!(
                mutation.exists(),
                "an unretained mutation must never be deleted"
            );
            fs::remove_file(mutation).unwrap();
            fs::remove_dir(root).unwrap();
        } else {
            root_guard.delete().unwrap();
            assert!(!root.exists() && !child.exists());
        }
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_concurrent_installer_lifecycle_lock_is_rejected() {
        if let (Some(lock), Some(ready)) = (
            std::env::var_os("TQ_STALE_LOCK_CHILD"),
            std::env::var_os("TQ_STALE_LOCK_READY"),
        ) {
            let _guard = RetainedStaleObject::open(Path::new(&lock), false).unwrap();
            fs::write(ready, b"ready").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let root = std::env::temp_dir().join(format!("tq-stale-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let lock = root.join("recovery-state-v1.lock");
        let ready = root.join("ready");
        fs::write(&lock, b"").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::tests::stale_concurrent_installer_lifecycle_lock_is_rejected",
                "--nocapture",
            ])
            .env("TQ_STALE_LOCK_CHILD", &lock)
            .env("TQ_STALE_LOCK_READY", &ready)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists());
        assert!(RetainedStaleObject::open(&lock, false).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_active_process_state_is_rejected() {
        if std::env::var_os("TQ_STALE_ACTIVE_CHILD").is_some() {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        let root = std::env::temp_dir().join(format!("tq-stale-active-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let image = root.join("Talking Quill Active Test.exe");
        fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
        let mut child = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::stale_active_process_state_is_rejected",
                "--nocapture",
            ])
            .env("TQ_STALE_ACTIVE_CHILD", "1")
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!no_talking_quill_process_except_authenticated_pair(true).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_cleanup_authorization_is_authenticated_cross_process() {
        if std::env::var_os("TQ_STALE_AUTH_CHILD").is_some() {
            let image = std::env::current_exe().unwrap();
            let (action, _, silent, parent) =
                WorkerChannel::connect_and_authenticate(&image, None).unwrap();
            assert!(action == Action::CleanStaleSchema2 && silent && parent != 0);
            return;
        }
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        let channel =
            ControllerChannel::create(Action::CleanStaleSchema2, true, std::process::id()).unwrap();
        let image = std::env::current_exe().unwrap();
        let mut child = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::stale_cleanup_authorization_is_authenticated_cross_process",
                "--nocapture",
            ])
            .env("TQ_STALE_AUTH_CHILD", "1")
            .spawn()
            .unwrap();
        let mut duplicate = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    child.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        let worker = channel.authenticate(&shell, &image, None).unwrap();
        assert_eq!(unsafe { GetProcessId(worker.as_raw_handle()) }, child.id());
        assert!(child.wait().unwrap().success());
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn forced_post_inspected_and_post_commit_rejections_keep_one_audit_chain() {
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        for forced_stage in ["post-inspected", "post-commit-intent"] {
            let root = std::env::temp_dir().join(format!(
                "tq-stale-forced-rejection-{}-{forced_stage}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).unwrap();
            let path = root.join("audit.jsonl");
            let diagnostic_path = root.join("diagnostic.jsonl");
            fs::write(&path, b"").unwrap();
            fs::write(&diagnostic_path, b"").unwrap();
            apply_lock_dacl(&path, MACHINE_LOCK_FILE_SDDL).unwrap();
            apply_lock_dacl(&diagnostic_path, MACHINE_LOCK_FILE_SDDL).unwrap();
            unsafe {
                std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path);
                std::env::set_var("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH", &diagnostic_path);
                std::env::set_var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE", forced_stage);
            }
            let mut audit = StaleCleanupAudit::open().unwrap();
            let mut diagnostic = StaleSchema2Diagnostic::open().unwrap();
            let binding = "ab".repeat(32);
            let proof = "cd".repeat(32);
            audit.record("inspected", &binding, &proof).unwrap();
            if forced_stage == "post-commit-intent" {
                audit.record("commit-intent", &binding, &proof).unwrap();
            }
            let forced = force_stale_cleanup_rejection(forced_stage).unwrap_err();
            assert!(
                record_post_audit_cleanup_rejection(&mut diagnostic, &mut audit, forced,).is_err()
            );
            unsafe {
                std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH");
                std::env::remove_var("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH");
                std::env::remove_var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE");
            }
            drop(diagnostic);
            drop(audit);
            let events = fs::read_to_string(&path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(events.first().unwrap()["stage"], "inspected");
            assert_eq!(
                events.last().unwrap()["stage"],
                "cleanup.rejected.after-audit"
            );
            let operation = events.first().unwrap()["operationId"].as_str().unwrap();
            assert!(
                events
                    .iter()
                    .all(|event| event["operationId"].as_str() == Some(operation))
            );
            for pair in events.windows(2) {
                assert_eq!(pair[1]["previousSha256"], pair[0]["eventSha256"]);
            }
            let diagnostic_events = fs::read_to_string(&diagnostic_path).unwrap();
            assert!(diagnostic_events.contains("cleanup.rejected.after-audit"));
            assert!(diagnostic_events.contains("\"outcome\":\"rejected\""));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_audit_is_flushed_and_identity_bound() {
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("tq-stale-audit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("audit.jsonl");
        fs::write(&path, b"").unwrap();
        apply_lock_dacl(&path, MACHINE_LOCK_FILE_SDDL).unwrap();
        unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path) };
        let mut audit = StaleCleanupAudit::open().unwrap();
        unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
        audit
            .record("commit-intent", &"ab".repeat(32), &"cd".repeat(32))
            .unwrap();
        audit
            .record("completed", &"ab".repeat(32), &"ef".repeat(32))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("commit-intent") && content.contains("completed"));
        assert!(content.contains(&"ab".repeat(32)) && content.contains(&"ef".repeat(32)));
        drop(audit);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_no_publication_active_residue_cannot_complete_audit() {
        let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
        let root =
            std::env::temp_dir().join(format!("tq-stale-no-publication-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let program_files = root.join("Program Files");
        let program_data = root.join("ProgramData");
        let system = root.join("System32");
        fs::create_dir_all(program_files.join("Talking Quill")).unwrap();
        fs::create_dir(&program_data).unwrap();
        fs::create_dir_all(system.join("Tasks")).unwrap();
        let audit_path = root.join("audit.jsonl");
        fs::write(&audit_path, b"").unwrap();
        apply_lock_dacl(&audit_path, MACHINE_LOCK_FILE_SDDL).unwrap();
        unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &audit_path) };
        let mut audit = StaleCleanupAudit::open().unwrap();
        unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
        let binding = retained_binding(&[], "no-machine-lock-publication");
        assert!(
            complete_stale_cleanup_zero_state(
                &mut audit,
                &binding,
                &program_files,
                &program_data,
                &system,
                true,
            )
            .is_err()
        );
        drop(audit);
        assert!(fs::read(&audit_path).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(feature = "stale-schema2-cleanup")]
    #[test]
    fn stale_interrupted_registry_last_never_claims_zero() {
        let root =
            std::env::temp_dir().join(format!("tq-stale-registry-last-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let registry_publication = root.join("RecoveryStateLockV1");
        fs::write(&registry_publication, b"suffix").unwrap();
        let fixture = root.join("fixture");
        fs::create_dir(&fixture).unwrap();
        let lifecycle_path = fixture.join("recovery-state-v1.lock");
        fs::write(&lifecycle_path, b"").unwrap();
        let lifecycle = RetainedStaleObject::open_lifecycle(&lifecycle_path).unwrap();
        let fixture_guard = RetainedStaleObject::open(&fixture, true).unwrap();
        let mut lifecycle = lifecycle;
        let lifecycle_identity = lifecycle.identity.clone();
        let retained = root.join("retained-lifecycle.lock");
        lifecycle.rename(&retained).unwrap();
        lifecycle.mark_posix_deleted().unwrap();
        fixture_guard.delete().unwrap();
        assert!(
            registry_publication.exists(),
            "registry-last interruption must remain observable while lifecycle authority remains"
        );
        fs::remove_file(&registry_publication).unwrap();
        assert_eq!(
            file_identity_text(&lifecycle.file).unwrap(),
            lifecycle_identity
        );
        lifecycle.finish_deleted().unwrap();
        assert!(!fixture.exists() && !registry_publication.exists() && !retained.exists());
        fs::remove_dir(root).unwrap();
    }
}
