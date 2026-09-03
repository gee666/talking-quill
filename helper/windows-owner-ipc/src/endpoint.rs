#![cfg(windows)]

use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use thiserror::Error;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY, TokenLogonSid,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
};

pub const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
pub const PIPE_OPEN_MODE: u32 = windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX
    | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
pub const PIPE_OUTBOUND_OPEN_MODE: u32 =
    windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_OUTBOUND
        | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
pub const PIPE_MODE: u32 = windows_sys::Win32::System::Pipes::PIPE_TYPE_BYTE
    | windows_sys::Win32::System::Pipes::PIPE_READMODE_BYTE
    | windows_sys::Win32::System::Pipes::PIPE_WAIT
    | windows_sys::Win32::System::Pipes::PIPE_REJECT_REMOTE_CLIENTS;

#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("Windows session endpoint identity is unavailable")]
    Unavailable,
    #[error("Windows session endpoint security descriptor is invalid")]
    Security,
}

pub fn current_wts_session_id() -> Result<u32, EndpointError> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(EndpointError::Unavailable);
    }
    Ok(session)
}

pub fn pipe_name(session: u32) -> String {
    format!(r"\\.\pipe\TalkingQuill.KeyboardOwner.Personal.V2.{session}")
}

pub fn current_pipe_name() -> Result<String, EndpointError> {
    current_wts_session_id().map(pipe_name)
}

/// Owns the allocated security descriptor used by CreateNamedPipeW.
pub struct EndpointSecurity {
    descriptor: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

impl std::fmt::Debug for EndpointSecurity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EndpointSecurity(<redacted>)")
    }
}

impl EndpointSecurity {
    pub fn for_current_logon() -> Result<Self, EndpointError> {
        let logon_sid = current_logon_sid_string()?;
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{logon_sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(EndpointError::Security);
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Ok(Self {
            descriptor,
            attributes,
        })
    }

    pub const fn attributes(&self) -> &SECURITY_ATTRIBUTES {
        &self.attributes
    }
}

impl Drop for EndpointSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe { LocalFree(self.descriptor) };
        }
    }
}

pub fn create_server_instance(
    name: &str,
    security: &EndpointSecurity,
    first: bool,
) -> Result<OwnedHandle, EndpointError> {
    create_server_instance_with_mode(name, security, first, PIPE_OPEN_MODE)
}

/// Creates a server-to-client startup channel. The handle is non-inheritable and
/// the first-instance flag prevents an attacker from pre-creating the endpoint.
pub fn create_outbound_server_instance(
    name: &str,
    security: &EndpointSecurity,
    first: bool,
) -> Result<OwnedHandle, EndpointError> {
    create_server_instance_with_mode(name, security, first, PIPE_OUTBOUND_OPEN_MODE)
}

fn create_server_instance_with_mode(
    name: &str,
    security: &EndpointSecurity,
    first: bool,
    open_mode: u32,
) -> Result<OwnedHandle, EndpointError> {
    let name: Vec<u16> = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain([0])
        .collect();
    let first_flag = if first {
        windows_sys::Win32::Storage::FileSystem::FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        0
    };
    let handle = unsafe {
        windows_sys::Win32::System::Pipes::CreateNamedPipeW(
            name.as_ptr(),
            open_mode | first_flag,
            PIPE_MODE,
            4,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            security.attributes(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(EndpointError::Unavailable);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn current_logon_sid_string() -> Result<String, EndpointError> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(EndpointError::Unavailable);
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let token = Token(token);
    let mut length = 0;
    unsafe { GetTokenInformation(token.0, TokenLogonSid, std::ptr::null_mut(), 0, &mut length) };
    if length < std::mem::size_of::<TOKEN_GROUPS>() as u32 {
        return Err(EndpointError::Unavailable);
    }
    let mut buffer = vec![0_u8; length as usize];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenLogonSid,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(EndpointError::Unavailable);
    }
    let groups = unsafe { &*buffer.as_ptr().cast::<TOKEN_GROUPS>() };
    if groups.GroupCount != 1 {
        return Err(EndpointError::Unavailable);
    }
    let sid = groups.Groups[0].Sid;
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(EndpointError::Unavailable);
    }
    struct LocalString(*mut u16);
    impl Drop for LocalString {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0.cast()) };
        }
    }
    let text = LocalString(text);
    let length = (0..)
        .take_while(|&i| unsafe { *text.0.add(i) } != 0)
        .count();
    String::from_utf16(unsafe { std::slice::from_raw_parts(text.0, length) })
        .map_err(|_| EndpointError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_name_is_stable_and_session_scoped() {
        assert_eq!(
            pipe_name(42),
            r"\\.\pipe\TalkingQuill.KeyboardOwner.Personal.V2.42"
        );
    }

    #[test]
    fn pipe_policy_is_byte_mode_local_only_and_overlapped() {
        assert_ne!(
            PIPE_MODE & windows_sys::Win32::System::Pipes::PIPE_REJECT_REMOTE_CLIENTS,
            0
        );
        assert_ne!(
            PIPE_OPEN_MODE & windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED,
            0
        );
    }

    #[test]
    fn first_instance_prevents_endpoint_squatting() {
        let name = format!(
            r"\\.\pipe\TalkingQuill.KeyboardOwner.Test.{}.{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("pipe")
        );
        let security = EndpointSecurity::for_current_logon().unwrap();
        let first = create_server_instance(&name, &security, true).unwrap();
        assert!(create_server_instance(&name, &security, true).is_err());
        drop(first);
        assert!(create_server_instance(&name, &security, true).is_ok());
    }

    #[test]
    fn outbound_startup_pipe_is_first_instance_and_non_inheritable() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
        let name = format!(
            r"\\.\pipe\TalkingQuill.AcceptanceStartup.Test.{}",
            std::process::id()
        );
        let security = EndpointSecurity::for_current_logon().unwrap();
        let first = create_outbound_server_instance(&name, &security, true).unwrap();
        let mut flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(first.as_raw_handle(), &mut flags) },
            0
        );
        assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
        assert!(create_outbound_server_instance(&name, &security, true).is_err());
    }

    #[test]
    fn sequential_clients_observe_the_same_owner_process_id() {
        use std::os::windows::io::AsHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, OPEN_EXISTING,
        };
        let name = format!(
            r"\\.\pipe\TalkingQuill.KeyboardOwner.ReconnectTest.{}",
            std::process::id()
        );
        let security = EndpointSecurity::for_current_logon().unwrap();
        let first = create_server_instance(&name, &security, true).unwrap();
        let second = create_server_instance(&name, &security, false).unwrap();
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let open = || {
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            assert!(!handle.is_null() && handle as isize != -1);
            unsafe { OwnedHandle::from_raw_handle(handle) }
        };
        let first_client = open();
        let first_pid = crate::peer::named_pipe_server_pid(first_client.as_handle()).unwrap();
        drop(first_client);
        drop(first);
        let second_client = open();
        let second_pid = crate::peer::named_pipe_server_pid(second_client.as_handle()).unwrap();
        assert_eq!(first_pid, std::process::id());
        assert_eq!(second_pid, first_pid);
        drop(second_client);
        drop(second);
    }
}
