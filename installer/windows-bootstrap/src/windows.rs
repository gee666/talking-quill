use std::ffi::{OsStr, OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::{mem, ptr};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ, GetFileAttributesW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Environment::SetEnvironmentVariableW;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    PROCESS_INFORMATION, ReleaseMutex, STARTUPINFOW, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHGetKnownFolderPath,
    ShellExecuteExW,
};

const ELEVATED_MARKER: &str = "/TQBOOTSTRAP-ELEVATED-V1";
const PROTECTED_MARKER: &str = "/TQPROTECTEDTEMP=";
const FOOTER_MAGIC: &[u8; 8] = b"TQNSIS01";
const FOOTER_SIZE: usize = 64;
const EXIT_USAGE: i32 = 64;
const EXIT_FAILURE: i32 = 70;
const EXIT_REJECTED: i32 = 78;
const EXIT_ELEVATION: i32 = 79;

pub fn run() -> i32 {
    match run_inner() {
        Ok(code) => code,
        Err(error) => {
            report(&error.message);
            error.code
        }
    }
}

struct BootstrapError {
    code: i32,
    message: String,
}

type Result<T> = std::result::Result<T, BootstrapError>;

fn fail(code: i32, message: impl Into<String>) -> BootstrapError {
    BootstrapError {
        code,
        message: message.into(),
    }
}

fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let elevated_count = arguments
        .iter()
        .filter(|value| value == &&OsString::from(ELEVATED_MARKER))
        .count();
    let reserved = arguments
        .iter()
        .filter(|value| {
            let text = value.to_string_lossy();
            text.eq_ignore_ascii_case(ELEVATED_MARKER)
                || text.to_ascii_uppercase().starts_with(PROTECTED_MARKER)
        })
        .count();
    if elevated_count == 0 {
        if reserved != 0 {
            return Err(fail(EXIT_USAGE, "reserved installer bootstrap argument"));
        }
        return elevate(&arguments);
    }
    if elevated_count != 1 || reserved != 1 || !token_is_elevated()? {
        return Err(fail(EXIT_REJECTED, "invalid elevated bootstrap invocation"));
    }
    let public: Vec<OsString> = arguments
        .into_iter()
        .filter(|value| value != ELEVATED_MARKER)
        .collect();
    run_elevated(&public)
}

fn elevate(arguments: &[OsString]) -> Result<i32> {
    let executable =
        std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
    // Downloads and user TEMP are writable by the caller. Keep the exact outer
    // image locked against writes, deletion, and replacement through the full
    // elevated child lifetime. The child then hashes payload bytes from that
    // same path while this retained handle prevents a post-consent swap.
    let retained_image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&executable)
        .map_err(|error| {
            fail(
                EXIT_FAILURE,
                format!("cannot retain installer image: {error}"),
            )
        })?;
    let mut elevated = vec![OsString::from(ELEVATED_MARKER)];
    elevated.extend(arguments.iter().cloned());
    let parameters = join_arguments(&elevated);
    let file = wide(executable.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let parameters = wide(&parameters);
    let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 1;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = unsafe { GetLastError() };
        return Err(fail(
            EXIT_ELEVATION,
            if error == ERROR_CANCELLED {
                "installer elevation was cancelled"
            } else {
                "installer elevation failed"
            },
        ));
    }
    if info.hProcess.is_null() {
        return Err(fail(
            EXIT_ELEVATION,
            "elevated installer process handle is missing",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let result = wait_exit(process.as_raw_handle())
        .map_err(|_| fail(EXIT_ELEVATION, "elevated installer wait failed"));
    drop(retained_image);
    result
}

fn run_elevated(arguments: &[OsString]) -> Result<i32> {
    let _serialization = BootstrapMutex::acquire()?;
    cleanup_abandoned_leaves()?;
    let current = std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
    let payload = read_payload(&current)?;
    let leaf = ProtectedLeaf::create()?;
    let inner = leaf.path.join("Talking-Quill-inner-installer.exe");
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&inner)
            .map_err(|error| {
                fail(
                    EXIT_REJECTED,
                    format!("cannot create protected inner installer: {error}"),
                )
            })?;
        output
            .write_all(&payload.bytes)
            .and_then(|()| output.sync_all())
            .map_err(|error| {
                fail(
                    EXIT_FAILURE,
                    format!("cannot write inner installer: {error}"),
                )
            })?;
        let actual: [u8; 32] = Sha256::digest(&payload.bytes).into();
        if actual != payload.sha256 {
            return Err(fail(EXIT_REJECTED, "embedded installer digest mismatch"));
        }
        set_environment("TEMP", leaf.path.as_os_str())?;
        set_environment("TMP", leaf.path.as_os_str())?;
        let mut child_arguments = arguments.to_vec();
        child_arguments.push(OsString::from(format!(
            "{PROTECTED_MARKER}{}",
            leaf.path.display()
        )));
        launch_inner(&inner, &child_arguments)
    })();
    let cleanup = leaf.remove_file_and_directory(&inner);
    match (result, cleanup) {
        (Ok(code), Ok(())) => Ok(code),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

struct Payload {
    bytes: Vec<u8>,
    sha256: [u8; 32],
}

fn read_payload(path: &Path) -> Result<Payload> {
    let mut file = File::open(path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let length = file
        .metadata()
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
        .len();
    if length < FOOTER_SIZE as u64 {
        return Err(fail(
            EXIT_REJECTED,
            "installer bootstrap payload footer is missing",
        ));
    }
    let footer_position = payload_footer_position(&mut file, length)?;
    file.seek(SeekFrom::Start(footer_position))
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let mut footer = [0_u8; FOOTER_SIZE];
    file.read_exact(&mut footer)
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if &footer[..8] != FOOTER_MAGIC || u32::from_le_bytes(footer[8..12].try_into().unwrap()) != 1 {
        return Err(fail(
            EXIT_REJECTED,
            "installer bootstrap payload footer is invalid",
        ));
    }
    let offset = u64::from_le_bytes(footer[16..24].try_into().unwrap());
    let payload_length = u64::from_le_bytes(footer[24..32].try_into().unwrap());
    if offset.checked_add(payload_length) != Some(footer_position)
        || payload_length == 0
        || payload_length > usize::MAX as u64
    {
        return Err(fail(EXIT_REJECTED, "embedded installer range is invalid"));
    }
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(&footer[32..64]);
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let mut bytes = vec![0; payload_length as usize];
    file.read_exact(&mut bytes)
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    Ok(Payload { bytes, sha256 })
}

fn payload_footer_position(file: &mut File, length: u64) -> Result<u64> {
    let mut dos = [0_u8; 64];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut dos))
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if &dos[..2] != b"MZ" {
        return Err(fail(
            EXIT_REJECTED,
            "native bootstrap DOS header is invalid",
        ));
    }
    let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    let mut optional = [0_u8; 160];
    file.seek(SeekFrom::Start(pe + 24))
        .and_then(|_| file.read_exact(&mut optional))
        .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let directory = match u16::from_le_bytes(optional[..2].try_into().unwrap()) {
        0x20b => 112,
        0x10b => 96,
        _ => {
            return Err(fail(
                EXIT_REJECTED,
                "native bootstrap optional header is invalid",
            ));
        }
    };
    let certificate_offset =
        u32::from_le_bytes(optional[directory + 32..directory + 36].try_into().unwrap()) as u64;
    let certificate_size =
        u32::from_le_bytes(optional[directory + 36..directory + 40].try_into().unwrap()) as u64;
    let payload_end = if certificate_offset == 0 && certificate_size == 0 {
        length
    } else if certificate_offset >= FOOTER_SIZE as u64
        && certificate_size > 0
        && certificate_offset.checked_add(certificate_size) == Some(length)
    {
        certificate_offset
    } else {
        return Err(fail(
            EXIT_REJECTED,
            "native bootstrap certificate range is invalid",
        ));
    };
    payload_end
        .checked_sub(FOOTER_SIZE as u64)
        .ok_or_else(|| fail(EXIT_REJECTED, "native bootstrap footer range is invalid"))
}

struct BootstrapMutex(OwnedHandle);

impl BootstrapMutex {
    fn acquire() -> Result<Self> {
        let name = wide(OsStr::new("Global\\TalkingQuill.InstallerBootstrap.V1"));
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
            return Err(fail(EXIT_FAILURE, "cannot create installer mutex ACL"));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = unsafe { CreateMutexW(&attributes, 0, name.as_ptr()) };
        unsafe { LocalFree(descriptor) };
        if handle.is_null() {
            return Err(fail(
                EXIT_FAILURE,
                "cannot create installer bootstrap mutex",
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let wait = unsafe { WaitForSingleObject(handle.as_raw_handle(), 120_000) };
        if wait != 0 && wait != 0x80 {
            return Err(fail(EXIT_FAILURE, "installer bootstrap mutex wait failed"));
        }
        Ok(Self(handle))
    }
}

impl Drop for BootstrapMutex {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.as_raw_handle()) };
    }
}

fn cleanup_abandoned_leaves() -> Result<()> {
    let root = known_program_data()?;
    let entries = std::fs::read_dir(&root).map_err(|error| {
        fail(
            EXIT_FAILURE,
            format!("cannot audit installer residue: {error}"),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        let text = entry.file_name().to_string_lossy().into_owned();
        let Some(suffix) = text.strip_prefix(".Talking Quill.Installer-") else {
            continue;
        };
        if suffix.len() != 32
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            continue;
        }
        let path = entry.path();
        if !is_plain_directory(&path) {
            continue;
        }
        let children = std::fs::read_dir(&path)
            .map_err(|error| {
                fail(
                    EXIT_REJECTED,
                    format!("cannot audit installer residue leaf: {error}"),
                )
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        if children.len() > 1
            || children.iter().any(|child| {
                child.file_name() != "Talking-Quill-inner-installer.exe"
                    || child.file_type().is_err()
                    || child
                        .file_type()
                        .is_ok_and(|kind| !kind.is_file() || kind.is_symlink())
            })
        {
            continue;
        }
        if let Some(child) = children.first() {
            std::fs::remove_file(child.path()).map_err(|error| {
                fail(
                    EXIT_FAILURE,
                    format!("cannot retire stale inner installer: {error}"),
                )
            })?;
        }
        std::fs::remove_dir(&path).map_err(|error| {
            fail(
                EXIT_FAILURE,
                format!("cannot retire stale installer leaf: {error}"),
            )
        })?;
    }
    Ok(())
}

struct ProtectedLeaf {
    path: PathBuf,
}

impl ProtectedLeaf {
    fn create() -> Result<Self> {
        let root = known_program_data()?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness unavailable"))?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = root.join(format!(".Talking Quill.Installer-{suffix}"));
        let sddl = wide(OsStr::new("O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"));
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
            return Err(fail(EXIT_FAILURE, "cannot create protected installer ACL"));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let created = unsafe {
            windows_sys::Win32::Storage::FileSystem::CreateDirectoryW(
                wide(path.as_os_str()).as_ptr(),
                &attributes,
            )
        };
        unsafe { LocalFree(descriptor) };
        if created == 0 || !is_plain_directory(&path) {
            return Err(fail(
                EXIT_REJECTED,
                "cannot create protected installer directory",
            ));
        }
        Ok(Self { path })
    }

    fn remove_file_and_directory(self, inner: &Path) -> Result<()> {
        if !is_plain_directory(&self.path) {
            return Err(fail(
                EXIT_REJECTED,
                "protected installer directory identity changed",
            ));
        }
        if let Err(error) = std::fs::remove_file(inner)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(fail(
                EXIT_FAILURE,
                format!("cannot remove inner installer: {error}"),
            ));
        }
        std::fs::remove_dir(&self.path).map_err(|error| {
            fail(
                EXIT_FAILURE,
                format!("cannot remove protected installer directory: {error}"),
            )
        })
    }
}

fn known_program_data() -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, ptr::null_mut(), &mut raw) } != 0
        || raw.is_null()
    {
        return Err(fail(EXIT_FAILURE, "native ProgramData lookup failed"));
    }
    let length = unsafe { (0..).position(|index| *raw.add(index) == 0).unwrap_or(0) };
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(value))
}

fn is_plain_directory(path: &Path) -> bool {
    let attributes = unsafe { GetFileAttributesW(wide(path.as_os_str()).as_ptr()) };
    attributes != u32::MAX
        && attributes & FILE_ATTRIBUTE_DIRECTORY != 0
        && attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

fn launch_inner(path: &Path, arguments: &[OsString]) -> Result<i32> {
    let command = join_arguments(
        &std::iter::once(path.as_os_str().to_os_string())
            .chain(arguments.iter().cloned())
            .collect::<Vec<_>>(),
    );
    let mut command = wide(&command);
    let mut startup: STARTUPINFOW = unsafe { mem::zeroed() };
    startup.cb = mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    if unsafe {
        CreateProcessW(
            ptr::null(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            0,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "cannot launch protected inner installer",
        ));
    }
    unsafe { CloseHandle(process.hThread) };
    let handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
    wait_exit(handle.as_raw_handle()).map_err(|_| fail(EXIT_FAILURE, "inner installer wait failed"))
}

fn wait_exit(handle: HANDLE) -> std::result::Result<i32, ()> {
    if unsafe { WaitForSingleObject(handle, u32::MAX) } != 0 {
        return Err(());
    }
    let mut code = 0;
    if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
        return Err(());
    }
    Ok(code as i32)
}

fn token_is_elevated() -> Result<bool> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(fail(EXIT_REJECTED, "cannot inspect bootstrap token"));
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
        return Err(fail(EXIT_REJECTED, "cannot read bootstrap elevation"));
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn set_environment(name: &str, value: &OsStr) -> Result<()> {
    if unsafe { SetEnvironmentVariableW(wide(OsStr::new(name)).as_ptr(), wide(value).as_ptr()) }
        == 0
    {
        return Err(fail(EXIT_FAILURE, format!("cannot set {name}")));
    }
    Ok(())
}

fn join_arguments(arguments: &[OsString]) -> OsString {
    OsString::from(
        arguments
            .iter()
            .map(|value| quote_argument(value))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn quote_argument(argument: &OsStr) -> String {
    let value = argument.to_string_lossy();
    if !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return value.into_owned();
    }
    let mut output = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
        } else if character == '"' {
            output.push_str(&"\\".repeat(slashes * 2 + 1));
            output.push('"');
            slashes = 0;
        } else {
            output.push_str(&"\\".repeat(slashes));
            slashes = 0;
            output.push(character);
        }
    }
    output.push_str(&"\\".repeat(slashes * 2));
    output.push('"');
    output
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

fn report(message: &str) {
    let caption = wide(OsStr::new("Talking Quill installer"));
    let message = wide(OsStr::new(message));
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            ptr::null_mut(),
            message.as_ptr(),
            caption.as_ptr(),
            0x10,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_windows_arguments() {
        assert_eq!(quote_argument(OsStr::new("plain")), "plain");
        assert_eq!(quote_argument(OsStr::new("a b")), "\"a b\"");
        assert_eq!(quote_argument(OsStr::new("a\\\"b")), "\"a\\\\\\\"b\"");
        assert_eq!(quote_argument(OsStr::new("a b\\")), "\"a b\\\\\"");
    }
}
