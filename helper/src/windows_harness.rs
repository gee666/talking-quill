#![cfg(windows)]

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, ReleaseMutex, WaitForSingleObject,
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// Serializes the production Windows owner namespace used by the native harness.
/// The kernel releases an abandoned mutex when a prior harness process dies.
pub fn run_serialized() -> Result<(), String> {
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
    unsafe { ReleaseMutex(handle.as_raw_handle()) };
    result
}
