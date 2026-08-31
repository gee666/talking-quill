use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::{mem, ptr};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    MoveFileExW, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SYNCHRONIZE, WriteFile,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcess,
    OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex,
    WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramFiles, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SHGetKnownFolderPath, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDOK, IDYES, MB_ICONQUESTION, MB_OKCANCEL, MB_YESNO, MessageBoxW,
};

use crate::package::{self, ParsedPackage};

const EXIT_USAGE: i32 = 64;
const EXIT_FAILURE: i32 = 70;
const EXIT_REJECTED: i32 = 78;
const EXIT_ELEVATION: i32 = 79;
const TRANSACTION_SCHEMA: u8 = 2;

pub fn run() -> i32 {
    match run_inner() {
        Ok(code) => code,
        Err(error) => {
            report(&error.message);
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
    profile: PathBuf,
}

fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let elevated = token_is_elevated()?;
    let relocated = arguments.len() == 1 && arguments[0] == "/TQ-RELOCATED";
    let legacy_predecessor = elevated && legacy_predecessor_arguments(&arguments);
    if !((arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "/S"))
        || legacy_predecessor
        || (!elevated && relocated))
    {
        return Err(fail(EXIT_USAGE, "The native setup accepts only /S."));
    }
    let mut silent = arguments.first().is_some_and(|value| value == "/S");
    if !elevated {
        let current =
            std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        let retained = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&current)
            .map_err(io_failure)?;
        let controller_paths = paths()?;
        let action = if relocated {
            let installed = controller_paths.install.join("Uninstall Talking Quill.exe");
            let (action, parent, requested_silent) =
                WorkerChannel::connect_and_authenticate(&current, Some(&installed))?;
            silent = requested_silent;
            if action != Action::Uninstall {
                return Err(fail(
                    EXIT_REJECTED,
                    "Relocation requested an invalid operation.",
                ));
            }
            wait_for_process_exit(parent)?;
            Action::Uninstall
        } else {
            derive_action(&current, &controller_paths)?
        };
        if action == Action::Uninstall && !relocated {
            let (path, lock) = create_relocated_image(&current)?;
            let channel = ControllerChannel::create(Action::Uninstall, silent)?;
            launch_relocated(&path, &channel)?;
            drop(lock);
            return Ok(0);
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
        let channel = ControllerChannel::create(action, silent)?;
        let result = elevate(&current, silent, &channel);
        drop(retained);
        if relocated {
            let _ = fs::remove_file(&current);
        }
        if result.as_ref().is_ok_and(|code| *code == 0) && delete_profile {
            remove_plain_tree(&controller_paths.profile)?;
        }
        return result;
    }
    run_worker(silent, legacy_predecessor)
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
        Action::Install => "Install or update Talking Quill for all users?",
        Action::Repair => "Repair Talking Quill for all users?",
        Action::Uninstall => {
            "Uninstall Talking Quill?\n\nYour profile is preserved unless you delete it in the application first."
        }
    };
    let response = message_box(text, MB_OKCANCEL | MB_ICONQUESTION);
    Ok(response == IDOK)
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
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(io_failure)?;
    let mut source = File::open(source).map_err(io_failure)?;
    std::io::copy(&mut source, &mut target).map_err(io_failure)?;
    target.sync_all().map_err(io_failure)?;
    Ok((path, target))
}

fn launch_relocated(executable: &Path, channel: &ControllerChannel) -> Result<()> {
    let file = wide(executable.as_os_str());
    let parameters = wide(OsStr::new("/TQ-RELOCATED"));
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
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    channel.authenticate(unsafe { GetProcessId(process.as_raw_handle()) }, executable)
}

fn wait_for_process_exit(pid: u32) -> Result<()> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return Ok(());
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 30_000) } != 0 {
        return Err(fail(
            EXIT_FAILURE,
            "Installed uninstall controller did not exit.",
        ));
    }
    Ok(())
}

fn elevate(executable: &Path, silent: bool, channel: &ControllerChannel) -> Result<i32> {
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
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    channel.authenticate(unsafe { GetProcessId(process.as_raw_handle()) }, executable)?;
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 120_000) } != 0 {
        return Err(fail(EXIT_ELEVATION, "The elevated worker timed out."));
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
    let predecessor_authorized = authenticate_predecessor_helper(&package, &paths).is_ok();
    let requested_action = if predecessor_authorized {
        None
    } else {
        Some(WorkerChannel::connect_and_authenticate(&current, None)?.0)
    };
    if legacy_predecessor && !predecessor_authorized {
        return Err(fail(
            EXIT_REJECTED,
            "Legacy update arguments require the exact authenticated predecessor helper.",
        ));
    }
    let state_root = paths
        .transaction
        .parent()
        .ok_or_else(|| fail(EXIT_FAILURE, "Installer state root is invalid."))?;
    assert_plain_directory(state_root)?;
    cleanup_transaction_residue(state_root)?;
    recover(&paths)?;
    let mut action = if requested_action == Some(Action::Uninstall) {
        authorize_uninstall_controller(&paths)?;
        Action::Uninstall
    } else {
        derive_action(&current, &paths)?
    };
    if action != Action::Uninstall {
        action = authorize_package_mode(&package, &paths, predecessor_authorized)?;
    }
    match action {
        Action::Install | Action::Repair => install(&mut image, &package, &current, &paths, action),
        Action::Uninstall => uninstall(&paths),
    }?;
    Ok(0)
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
}

impl ControllerChannel {
    fn create(action: Action, silent: bool) -> Result<Self> {
        let pid = std::process::id();
        let name = wide(OsStr::new(&format!(r"\\.\pipe\TalkingQuill.Setup.{pid}")));
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64,
                64,
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
        })
    }

    fn authenticate(&self, expected_worker: u32, image: &Path) -> Result<()> {
        let connected = unsafe { ConnectNamedPipe(self.handle.as_raw_handle(), ptr::null_mut()) };
        if connected == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            return Err(fail(EXIT_REJECTED, "The elevated worker did not connect."));
        }
        let mut worker = 0;
        if unsafe { GetNamedPipeClientProcessId(self.handle.as_raw_handle(), &mut worker) } == 0
            || worker != expected_worker
            || file_hash(&process_image(worker)?)? != file_hash(image)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "The setup pipe client is not the elevated same-image worker.",
            ));
        }
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        pipe_write(self.handle.as_raw_handle(), &nonce)?;
        let proof = pipe_read::<32>(self.handle.as_raw_handle())?;
        let expected = channel_proof(&nonce, std::process::id(), worker, &file_hash(image)?);
        if proof != expected {
            return Err(fail(
                EXIT_REJECTED,
                "The elevated worker pipe proof is invalid.",
            ));
        }
        pipe_write(self.handle.as_raw_handle(), b"TQ-SETUP-ACCEPTED")?;
        pipe_write(
            self.handle.as_raw_handle(),
            &[
                match self.action {
                    Action::Install => 1,
                    Action::Repair => 2,
                    Action::Uninstall => 3,
                },
                u8::from(self.silent),
            ],
        )
    }
}

struct WorkerChannel;
impl WorkerChannel {
    fn connect_and_authenticate(
        image: &Path,
        expected_server: Option<&Path>,
    ) -> Result<(Action, u32, bool)> {
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
                FILE_ATTRIBUTE_NORMAL,
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
        let nonce = pipe_read::<32>(handle.as_raw_handle())?;
        let proof = channel_proof(&nonce, server, std::process::id(), &file_hash(image)?);
        pipe_write(handle.as_raw_handle(), &proof)?;
        if pipe_read::<17>(handle.as_raw_handle())? != *b"TQ-SETUP-ACCEPTED" {
            return Err(fail(
                EXIT_REJECTED,
                "The medium setup controller rejected the worker.",
            ));
        }
        let request = pipe_read::<2>(handle.as_raw_handle())?;
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
        match request[0] {
            1 => Ok((Action::Install, server, silent)),
            2 => Ok((Action::Repair, server, silent)),
            3 => Ok((Action::Uninstall, server, silent)),
            _ => Err(fail(
                EXIT_REJECTED,
                "The setup controller requested an invalid operation.",
            )),
        }
    }
}

fn channel_proof(nonce: &[u8; 32], controller: u32, worker: u32, image: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/setup-pipe/v2");
    hash.update(nonce);
    hash.update(controller.to_le_bytes());
    hash.update(worker.to_le_bytes());
    hash.update(image);
    hash.finalize().into()
}

fn pipe_write(handle: std::os::windows::io::RawHandle, bytes: &[u8]) -> Result<()> {
    let mut written = 0;
    if unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    } == 0
        || written as usize != bytes.len()
    {
        return Err(fail(EXIT_REJECTED, "Setup pipe write failed."));
    }
    Ok(())
}
fn pipe_read<const N: usize>(handle: std::os::windows::io::RawHandle) -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    let mut read = 0;
    if unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr().cast(),
            N as u32,
            &mut read,
            ptr::null_mut(),
        )
    } == 0
        || read as usize != N
    {
        return Err(fail(EXIT_REJECTED, "Setup pipe read failed."));
    }
    Ok(bytes)
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
fn file_hash(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).map_err(io_failure)?;
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

fn authorize_uninstall_controller(paths: &Paths) -> Result<()> {
    let controller = process_image(parent_process_id()?)?;
    let installed = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&installed)?;
    if file_hash(&controller)? != file_hash(&installed)? {
        return Err(fail(
            EXIT_REJECTED,
            "Uninstall requires an authenticated exact copy of the installed controller image.",
        ));
    }
    Ok(())
}

fn authenticate_predecessor_helper(package: &ParsedPackage, paths: &Paths) -> Result<()> {
    if !matches!(package.manifest.package_mode.as_str(), "update" | "release") {
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
    let parent_hash = file_hash(&process_image(parent)?)?;
    let installed_gateway = paths
        .install
        .join("resources/helper/talking-quill-helper.exe");
    assert_plain_file(&installed_gateway)?;
    if parent_hash != file_hash(&installed_gateway)?
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
        if entry.th32ProcessID == std::process::id() {
            return Ok(entry.th32ParentProcessID);
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Err(fail(
        EXIT_REJECTED,
        "The update parent process is unavailable.",
    ))
}

fn hex_hash(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn authorize_package_mode(
    package: &ParsedPackage,
    paths: &Paths,
    predecessor_authorized: bool,
) -> Result<Action> {
    match package.manifest.package_mode.as_str() {
        "fresh" | "release" if !paths.install.exists() => Ok(Action::Install),
        "repair" if paths.install.exists() => Ok(Action::Repair),
        "update" | "release" if paths.install.exists() && predecessor_authorized => {
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
            Ok(Action::Install)
        }
        _ => Err(fail(
            EXIT_REJECTED,
            "Package mode does not match independently derived machine state.",
        )),
    }
}

fn derive_action(current: &Path, paths: &Paths) -> Result<Action> {
    if current
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("Uninstall Talking Quill.exe"))
    {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Invalid installed setup path."))?;
        if canonical(parent)? != canonical(&paths.install)? {
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
) -> Result<()> {
    assert_plain_absent(&paths.staging)?;
    write_transaction(paths, "staging", action, paths.install.exists())?;
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
    fs::copy(current, paths.staging.join("Uninstall Talking Quill.exe")).map_err(io_failure)?;
    write_transaction(paths, "prepared", action, paths.install.exists())?;
    if paths.install.exists() {
        durable_rename(&paths.install, &paths.backup)?;
    }
    durable_rename(&paths.staging, &paths.install)?;
    register_uninstall(paths, &package.manifest.version)?;
    register_app_path(paths)?;
    write_transaction(paths, "committed", action, paths.backup.exists())?;
    remove_plain_tree(&paths.backup)?;
    remove_transaction(paths)?;
    Ok(())
}

fn uninstall(paths: &Paths) -> Result<()> {
    write_transaction(paths, "uninstalling", Action::Uninstall, true)?;
    unregister_uninstall()?;
    unregister_app_path()?;
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    remove_transaction(paths)?;
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

fn recovery_plan(phase: &str, backup_exists: bool, install_exists: bool) -> Result<RecoveryPlan> {
    match phase {
        "staging" | "prepared" if backup_exists => Ok(RecoveryPlan::RestorePredecessor),
        "prepared" if install_exists => Ok(RecoveryPlan::RemoveFreshCandidate),
        "staging" | "prepared" => Ok(RecoveryPlan::DiscardStaging),
        "committed" => Ok(RecoveryPlan::FinishCommit),
        "uninstalling" => Ok(RecoveryPlan::FinishUninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction phase is invalid.",
        )),
    }
}

fn recover(paths: &Paths) -> Result<()> {
    if !paths.transaction.exists() {
        if paths.backup.exists() && !paths.install.exists() {
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
    match recovery_plan(&value.phase, paths.backup.exists(), paths.install.exists())? {
        RecoveryPlan::RestorePredecessor => {
            remove_plain_tree(&paths.staging)?;
            remove_plain_tree(&paths.install)?;
            durable_rename(&paths.backup, &paths.install)?;
            register_installed_uninstall(paths)?;
        }
        RecoveryPlan::DiscardStaging => remove_plain_tree(&paths.staging)?,
        RecoveryPlan::RemoveFreshCandidate => {
            unregister_uninstall()?;
            unregister_app_path()?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.staging)?;
        }
        RecoveryPlan::FinishCommit => {
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.staging)?;
        }
        RecoveryPlan::FinishUninstall => {
            unregister_uninstall()?;
            unregister_app_path()?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.staging)?;
        }
    }
    remove_transaction(paths)
}

fn paths() -> Result<Paths> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let profile = known_folder(&FOLDERID_LocalAppData)?.join("Talking Quill");
    Ok(Paths {
        install: program_files.join("Talking Quill"),
        staging: program_files.join(".Talking Quill.native-staging"),
        backup: program_files.join(".Talking Quill.native-backup"),
        transaction: program_files.join(".Talking Quill.native-transaction-v2.json"),
        profile,
    })
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill";
const APP_PATH_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe";

fn register_installed_uninstall(paths: &Paths) -> Result<()> {
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
            format!(
                "\"{}\"",
                paths.install.join("Uninstall Talking Quill.exe").display()
            ),
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
    unsafe { RegCloseKey(key) };
    result
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
    unsafe { RegCloseKey(key) };
    if first == 0 && second == 0 {
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
        assert_plain_file(&entry.path())?;
        fs::remove_file(entry.path()).map_err(io_failure)?;
    }
    Ok(())
}

fn remove_transaction(paths: &Paths) -> Result<()> {
    if paths.transaction.exists() {
        fs::remove_file(&paths.transaction).map_err(io_failure)?;
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

fn remove_plain_tree(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    assert_plain_directory(path)?;
    for entry in fs::read_dir(path).map_err(io_failure)? {
        let child = entry.map_err(io_failure)?.path();
        let metadata = fs::symlink_metadata(&child).map_err(io_failure)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(fail(EXIT_REJECTED, "Installer refuses a reparse point."));
        }
        if metadata.is_dir() {
            remove_plain_tree(&child)?;
        } else if metadata.is_file() {
            fs::remove_file(&child).map_err(io_failure)?;
        } else {
            return Err(fail(EXIT_REJECTED, "Installer refuses a special file."));
        }
    }
    fs::remove_dir(path).map_err(io_failure)
}

fn assert_plain_absent(path: &Path) -> Result<()> {
    if path.exists() {
        Err(fail(EXIT_REJECTED, "Installer staging already exists."))
    } else {
        Ok(())
    }
}
fn assert_plain_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_failure)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(fail(
            EXIT_REJECTED,
            "Installer path is not a plain directory.",
        ))
    } else {
        Ok(())
    }
}
fn assert_plain_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_failure)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(fail(EXIT_REJECTED, "Installer path is not a plain file."))
    } else {
        Ok(())
    }
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

    #[test]
    fn every_durable_phase_has_a_bounded_recovery_direction() {
        assert_eq!(
            recovery_plan("staging", false, false).unwrap(),
            RecoveryPlan::DiscardStaging
        );
        assert_eq!(
            recovery_plan("prepared", false, false).unwrap(),
            RecoveryPlan::DiscardStaging
        );
        assert_eq!(
            recovery_plan("prepared", false, true).unwrap(),
            RecoveryPlan::RemoveFreshCandidate
        );
        assert_eq!(
            recovery_plan("staging", true, false).unwrap(),
            RecoveryPlan::RestorePredecessor
        );
        assert_eq!(
            recovery_plan("prepared", true, true).unwrap(),
            RecoveryPlan::RestorePredecessor
        );
        assert_eq!(
            recovery_plan("committed", true, true).unwrap(),
            RecoveryPlan::FinishCommit
        );
        assert_eq!(
            recovery_plan("uninstalling", true, true).unwrap(),
            RecoveryPlan::FinishUninstall
        );
        assert!(recovery_plan("unknown", true, true).is_err());
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
}
