#![cfg(windows)]

use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, ReleaseMutex,
    WaitForSingleObject,
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(120);
const OWNER_RELEASE_TIMEOUT: Duration = Duration::from_secs(8);

/// Serializes the production Windows owner namespace used by the native harness.
/// The kernel releases an abandoned mutex when a prior harness process dies.
pub fn run_serialized() -> Result<(), String> {
    let integrity = current_integrity_rid()?;
    if integrity != 0x2000 {
        return Err(format!(
            "Windows helper harness requires medium integrity 0x2000; observed 0x{integrity:04x}"
        ));
    }
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err("unable to identify the Windows harness session".into());
    }
    let name: Vec<u16> = format!("Local\\TalkingQuill.HelperHarness.Personal.V1.{session}")
        .encode_utf16()
        .chain([0])
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("unable to create the Windows harness lock".into());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), LOCK_TIMEOUT.as_millis() as u32) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => {}
        _ => return Err("timed out waiting for the Windows harness lock".into()),
    }
    let result = crate::run().map_err(|error| error.to_string());
    let owner_release =
        wait_for_owner_singleton_release(session).and_then(|()| audit_process_images());
    if unsafe { ReleaseMutex(handle.as_raw_handle()) } == 0 {
        return Err("unable to release the Windows harness lock".into());
    }
    result.and(owner_release)
}

fn current_integrity_rid() -> Result<u32, String> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("unable to inspect the Windows harness token".into());
    }
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut bytes,
        )
    };
    let mut buffer = vec![0_u8; bytes as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    };
    unsafe { CloseHandle(token) };
    if ok == 0 || buffer.len() < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() {
        return Err("unable to read the Windows harness integrity label".into());
    }
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
    let count = unsafe { *GetSidSubAuthorityCount(sid) };
    if count == 0 {
        return Err("Windows harness integrity SID is malformed".into());
    }
    Ok(unsafe { *GetSidSubAuthority(sid, u32::from(count - 1)) })
}

fn audit_process_images() -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|_| "unable to resolve the staged harness image".to_owned())?;
    let helper_directory = executable
        .parent()
        .ok_or_else(|| "staged harness helper directory is missing".to_owned())?;
    let package_root = helper_directory
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| "staged harness package root is missing".to_owned())?;
    let repository_root = package_root
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| "native harness repository root is missing".to_owned())?;
    let source_directory = repository_root.join("app").join("native");
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err("unable to snapshot Windows processes for harness audit".into());
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        if entry.th32ProcessID != unsafe { GetCurrentProcessId() } {
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
            if !process.is_null() {
                let process = unsafe { OwnedHandle::from_raw_handle(process) };
                let mut path = vec![0_u16; 32_768];
                let mut length = path.len() as u32;
                if unsafe {
                    QueryFullProcessImageNameW(
                        process.as_raw_handle(),
                        0,
                        path.as_mut_ptr(),
                        &mut length,
                    )
                } != 0
                {
                    let image = std::path::PathBuf::from(std::ffi::OsString::from_wide(
                        &path[..length as usize],
                    ));
                    if path_is_within(&image, package_root)
                        || path_is_within(&image, &source_directory)
                    {
                        return Err(format!(
                            "Windows harness process-image audit found live PID {} at {}",
                            entry.th32ProcessID,
                            image.display()
                        ));
                    }
                }
            }
        }
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(())
}

fn path_is_within(path: &std::path::Path, directory: &std::path::Path) -> bool {
    let path = path.as_os_str().to_string_lossy().to_ascii_lowercase();
    let mut directory = directory
        .as_os_str()
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    directory.push('\\');
    path.starts_with(&directory)
}

fn wait_for_owner_singleton_release(session: u32) -> Result<(), String> {
    let name: Vec<u16> = format!("Local\\TalkingQuill.KeyboardOwner.Personal.V1.{session}")
        .encode_utf16()
        .chain([0])
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("unable to open the Windows owner singleton for final audit".into());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    match unsafe {
        WaitForSingleObject(
            handle.as_raw_handle(),
            OWNER_RELEASE_TIMEOUT.as_millis() as u32,
        )
    } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => {
            if unsafe { ReleaseMutex(handle.as_raw_handle()) } == 0 {
                Err("Windows owner singleton audit could not release its probe".into())
            } else {
                Ok(())
            }
        }
        _ => Err("Windows owner did not release its production singleton".into()),
    }
}
