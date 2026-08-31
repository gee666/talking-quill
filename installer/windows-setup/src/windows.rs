use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    ERROR_PIPE_CONNECTED, GetHandleInformation, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount,
    GetTokenInformation, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER, TokenElevation,
    TokenIntegrityLevel, TokenSessionId, TokenStatistics, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_RENAME_INFO,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo,
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, GetFileInformationByHandleEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, SYNCHRONIZE, SetFileInformationByHandle, WriteFile,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegFlushKey, RegSetValueExW,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, DeleteService, OpenSCManagerW, OpenServiceW,
    QUERY_SERVICE_CONFIGW, QueryServiceConfigW, QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT,
    SERVICE_CONTROL_STOP, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOP,
    SERVICE_STOPPED,
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

use crate::owned_tree::{owned_tree_identity, remove_owned_tree};
use crate::package::{self, ParsedPackage};

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

pub fn run() -> i32 {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Internal relocated and elevated roles never own UI. Authentication still
    // determines operation authority inside run_inner.
    let silent = arguments
        .iter()
        .any(|value| value == "/S" || value == "/TQ-RELOCATED")
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Install,
    Update,
    Repair,
    Uninstall,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Transaction {
    schema_version: u8,
    phase: String,
    action: String,
    had_predecessor: bool,
}

struct Paths {
    install: PathBuf,
    staging: PathBuf,
    backup: PathBuf,
    transaction: PathBuf,
    maintenance_uninstaller: PathBuf,
    profile: PathBuf,
    legacy_authority: PathBuf,
    legacy_quarantine: PathBuf,
    legacy_task_file: PathBuf,
    program_data: PathBuf,
}

fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let elevated = token_is_elevated()?;
    let relocated = !elevated
        && arguments.iter().any(|value| value == "/TQ-RELOCATED")
        && arguments
            .iter()
            .all(|value| value == "/TQ-RELOCATED" || value == "/S");
    let legacy_predecessor = elevated && legacy_predecessor_arguments(&arguments);
    if !((arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "/S"))
        || legacy_predecessor
        || relocated)
    {
        return Err(fail(EXIT_USAGE, "The native setup accepts only /S."));
    }
    let mut silent = arguments.first().is_some_and(|value| value == "/S");
    if !elevated {
        let current =
            std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        let controller_paths = paths()?;
        let retained = retain_controller_image(&current, relocated)?;
        let mut lifecycle_parent = 0;
        let mut relocation_server = None;
        let action = if relocated {
            let installed = controller_paths.install.join("Uninstall Talking Quill.exe");
            let original = process_image(parent_process_id()?)?;
            let expected = if canonical(&original).ok()
                == canonical(&controller_paths.maintenance_uninstaller).ok()
            {
                &controller_paths.maintenance_uninstaller
            } else {
                &installed
            };
            assert_plain_file(expected)?;
            let (action, server, requested_silent, requested_lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&current, Some(expected))?;
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
        let before_accept = (relocated && lifecycle_parent != 0)
            .then_some(&arm_deletion as &dyn Fn() -> Result<()>);
        let channel = ControllerChannel::create(action, silent, lifecycle_parent)?;
        let result = elevate(&current, silent, &channel, before_accept);
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

fn rename_retained(handle: &OwnedHandle, destination: &Path) -> Result<()> {
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
            handle.as_raw_handle(),
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
    let paths = paths()?;
    let _machine_lock = MachineLock::acquire()?;
    let state_root = paths
        .transaction
        .parent()
        .ok_or_else(|| fail(EXIT_FAILURE, "Installer state root is invalid."))?;
    assert_plain_directory(state_root)?;
    cleanup_transaction_residue(state_root)?;
    // The protected staged predecessor authenticates the persisted signed snapshot and exact
    // setup package before native journal recovery touches a moved tree.
    let predecessor_authorized = if legacy_predecessor {
        authenticate_predecessor_helper(&package, &paths)?;
        validate_predecessor_arguments(&package, &current)?;
        true
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
    // Authentication and package validation happen before recovery. Once the machine lock is held,
    // every installed entry point must finish durable recovery before arming or deriving a new action.
    recover_with_adapter(&paths, &system)?;
    if let Some((Action::Uninstall, server, _, lifecycle_parent)) =
        authenticated_controller.as_ref()
        && *lifecycle_parent != 0
    {
        write_transaction(&paths, "uninstall-armed", Action::Uninstall, true)?;
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
            uninstall(&paths, &system, original_controller != 0)
        }
    }?;
    Ok(0)
}

trait NativeSystemAdapter {
    fn register_version(&self, paths: &Paths, version: &str) -> Result<()>;
    fn register_installed(&self, paths: &Paths) -> Result<()>;
    fn unregister(&self) -> Result<()>;
    fn retire_legacy(&self, paths: &Paths) -> Result<()>;
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
    fn unregister(&self) -> Result<()> {
        unregister_uninstall()?;
        unregister_app_path()
    }
    fn retire_legacy(&self, paths: &Paths) -> Result<()> {
        retire_legacy_authority(paths)
    }
}

struct MachineLock(OwnedHandle);
impl MachineLock {
    fn acquire() -> Result<Self> {
        let sddl = wide(OsStr::new("O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"));
        let mut descriptor: *mut c_void = ptr::null_mut();
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
                "Cannot create the machine setup lock ACL.",
            ));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateMutexW(
                &attributes,
                0,
                wide(OsStr::new("Global\\TalkingQuill.NativeSetup.V2")).as_ptr(),
            )
        };
        unsafe { LocalFree(descriptor) };
        if handle.is_null() {
            return Err(fail(EXIT_FAILURE, "Cannot create the machine setup lock."));
        }
        let lock = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let wait = unsafe { WaitForSingleObject(lock.0.as_raw_handle(), 120_000) };
        if wait != 0 && wait != 0x80 {
            return Err(fail(EXIT_FAILURE, "Machine setup lock timed out."));
        }
        Ok(lock)
    }
}
impl Drop for MachineLock {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.as_raw_handle()) };
    }
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
        if pipe_read::<13>(
            self.handle.as_raw_handle(),
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(30),
        )? != *b"TQ-ARM-DELETE"
        {
            return Err(fail(
                EXIT_REJECTED,
                "Relocated uninstall requested an invalid completion operation.",
            ));
        }
        let deletion = arm_mapped_image_deletion(image);
        pipe_write(
            self.handle.as_raw_handle(),
            &[u8::from(deletion.is_ok())],
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(30),
        )?;
        deletion?;
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

fn authorize_uninstall_controller(paths: &Paths, current: &Path) -> Result<()> {
    let controller = process_image(parent_process_id()?)?;
    let installed = paths.install.join("Uninstall Talking Quill.exe");
    let expected = if path_present(&installed)? {
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
                "uninstall-quarantined" | "recovering-finish-uninstall"
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
        && role("owner") == Some(package.manifest.target.owner_sha256.as_str()))
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

fn derive_action(current: &Path, paths: &Paths) -> Result<Action> {
    let maintenance = canonical(current)
        .ok()
        .zip(canonical(&paths.maintenance_uninstaller).ok())
        .is_some_and(|(current, maintenance)| current == maintenance);
    if maintenance
        || current
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Uninstall Talking Quill.exe"))
    {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Invalid installed setup path."))?;
        if !maintenance && canonical(parent)? != canonical(&paths.install)? {
            return Err(fail(
                EXIT_REJECTED,
                "Uninstall image is outside the installed tree.",
            ));
        }
        return Ok(Action::Uninstall);
    }
    if paths.install.exists() {
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
    if value.get("version").and_then(|item| item.as_str())
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
        || (!acceptance_repair && !predecessor_matches)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Staged release identity does not match TQPKG2.",
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
    let executable = canonical(executable)?;
    let authority = format!("{}\\", canonical(&paths.legacy_authority)?);
    Ok(executable.starts_with(&authority))
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
) -> Result<()> {
    write_transaction(paths, "uninstalling", Action::Uninstall, true)?;
    system.unregister()?;
    system.retire_legacy(paths)?;
    if defer_mapped_controller_cleanup && path_present(&paths.install)? {
        remove_plain_tree(&paths.backup)?;
        durable_rename(&paths.install, &paths.backup)?;
        write_transaction(paths, "uninstall-quarantined", Action::Uninstall, true)?;
        remove_plain_tree(&paths.staging)?;
        // The first elevated worker launches an authenticated same-token cleanup worker.
        // That worker recovers this exact journal and must finish before success propagates.
        return launch_same_token_uninstall_cleanup(&std::env::current_exe().map_err(io_failure)?);
    }
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    remove_transaction(paths)?;
    remove_maintenance_uninstaller(paths)?;
    Ok(())
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
                    | "uninstall-quarantined"
                    | "recovering-finish-uninstall"
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
        | "uninstall-quarantined"
        | "recovering-finish-uninstall" => Ok(RecoveryPlan::FinishUninstall),
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
            system.unregister()?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.staging)?;
            remove_maintenance_uninstaller(paths)?;
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
        RecoveryPlan::FinishUninstall => {
            system.unregister()?;
            system.retire_legacy(paths)?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.staging)?;
        }
    }
    remove_transaction(paths)?;
    if finishing_uninstall {
        remove_maintenance_uninstaller(paths)?;
    }
    Ok(())
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
    fn unregister(&self) -> Result<()> {
        Ok(())
    }
    fn retire_legacy(&self, _paths: &Paths) -> Result<()> {
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

fn paths() -> Result<Paths> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = known_folder(&FOLDERID_System)?;
    let profile = known_folder(&FOLDERID_RoamingAppData)?.join("Talking Quill");
    Ok(Paths {
        install: program_files.join("Talking Quill"),
        staging: program_files.join(".Talking Quill.native-staging"),
        backup: program_files.join(".Talking Quill.native-backup"),
        transaction: program_files.join(".Talking Quill.native-transaction-v2.json"),
        maintenance_uninstaller: program_files.join("Talking Quill Maintenance.exe"),
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

fn remove_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    if path_present(&paths.maintenance_uninstaller)? {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        fs::remove_file(&paths.maintenance_uninstaller).map_err(io_failure)?;
    }
    Ok(())
}

fn ensure_maintenance_uninstaller(paths: &Paths) -> Result<()> {
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
    register_app_path(paths)
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
    let status =
        unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(OsStr::new(APP_PATH_KEY)).as_ptr()) };
    if status == 0 || status == 2 {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot remove the native application registration.",
        ))
    }
}

fn unregister_uninstall() -> Result<()> {
    let status =
        unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, wide(OsStr::new(UNINSTALL_KEY)).as_ptr()) };
    if status == 0 || status == 2 {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot remove the native uninstall registration.",
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
            maintenance_uninstaller: root.join("Talking Quill Maintenance.exe"),
            profile: root.join("profile"),
            legacy_authority: root.join("legacy"),
            legacy_quarantine: root.join("quarantine"),
            legacy_task_file: root.join("task"),
            program_data: root.join("program-data"),
        }
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
                    | "uninstall-quarantined"
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
                "uninstall-armed" | "uninstalling" => {
                    fs::create_dir(&paths.install).unwrap();
                    fs::write(paths.install.join("identity"), b"candidate").unwrap();
                }
                "uninstall-quarantined" | "uninstall-cleanup-elevation" => {
                    fs::create_dir(&paths.backup).unwrap();
                    fs::write(paths.backup.join("identity"), b"candidate").unwrap();
                }
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
            "uninstall-quarantined",
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
                    | "uninstall-quarantined"
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
}
