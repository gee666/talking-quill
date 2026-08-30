#![cfg(windows)]

use std::io::{Read, Seek};
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use thiserror::Error;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::Security::{
    GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_GROUPS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_TYPE, TOKEN_USER, TokenIntegrityLevel, TokenLogonSid,
    TokenPrimary, TokenSessionId, TokenType, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, IsWow64Process2, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

use crate::image_policy::WindowsArchitecture;

const SYNCHRONIZE: u32 = 0x0010_0000;
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const SOURCE_MARKER_PREFIX: &[u8] = b"TALKING_QUILL_SOURCE_";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    pub volume_serial: u32,
    pub file_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    pub commit: String,
    pub tree: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerFacts {
    pub process_id: u32,
    pub creation_marker: u64,
    pub wts_session_id: u32,
    pub user_sid: Vec<u8>,
    pub logon_sid: Vec<u8>,
    pub integrity_rid: u32,
    pub architecture: WindowsArchitecture,
    pub canonical_image: PathBuf,
    pub file_identity: FileIdentity,
    pub image_sha256: [u8; 32],
    pub source_identity: Option<SourceIdentity>,
}

pub struct VerifiedPeer {
    pub facts: PeerFacts,
    process: OwnedHandle,
    image: std::fs::File,
}

impl std::fmt::Debug for VerifiedPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedPeer")
            .field("facts", &self.facts)
            .finish_non_exhaustive()
    }
}

impl VerifiedPeer {
    pub fn from_process_id(process_id: u32) -> Result<Self, PeerError> {
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
                0,
                process_id,
            )
        };
        if process.is_null() {
            return Err(PeerError::Process);
        }
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        let creation_marker = process_creation_marker(process.as_raw_handle())?;
        let canonical_image = process_image_path(process.as_raw_handle())?
            .canonicalize()
            .map_err(|_| PeerError::Image)?;
        let mut image = open_image(&canonical_image)?;
        let file_identity = file_identity(&image)?;
        let (image_sha256, source_identity) = inspect_image(&mut image)?;
        let (user_sid, logon_sid, token_session_id, integrity_rid) =
            token_facts(process.as_raw_handle())?;
        let mut wts_session_id = 0;
        if unsafe {
            windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                process_id,
                &mut wts_session_id,
            )
        } == 0
            || wts_session_id != token_session_id
        {
            return Err(PeerError::Token);
        }
        let architecture = process_architecture(process.as_raw_handle())?;
        Ok(Self {
            facts: PeerFacts {
                process_id,
                creation_marker,
                wts_session_id,
                user_sid,
                logon_sid,
                integrity_rid,
                architecture,
                canonical_image,
                file_identity,
                image_sha256,
                source_identity,
            },
            process,
            image,
        })
    }

    pub fn still_running(&self) -> bool {
        (unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                self.process.as_raw_handle(),
                0,
            )
        }) == windows_sys::Win32::Foundation::WAIT_TIMEOUT
    }

    pub fn retained_image(&self) -> &std::fs::File {
        &self.image
    }
}

#[derive(Debug, Error)]
pub enum PeerError {
    #[error("pipe peer process is unavailable")]
    Process,
    #[error("pipe peer token identity is invalid")]
    Token,
    #[error("pipe peer executable identity is invalid")]
    Image,
    #[error("pipe peer architecture is unsupported")]
    Architecture,
}

pub fn named_pipe_client_pid(pipe: BorrowedHandle<'_>) -> Result<u32, PeerError> {
    let mut pid = 0;
    if unsafe {
        windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(
            pipe.as_raw_handle(),
            &mut pid,
        )
    } == 0
        || pid == 0
    {
        return Err(PeerError::Process);
    }
    Ok(pid)
}

pub fn named_pipe_server_pid(pipe: BorrowedHandle<'_>) -> Result<u32, PeerError> {
    let mut pid = 0;
    if unsafe {
        windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
            pipe.as_raw_handle(),
            &mut pid,
        )
    } == 0
        || pid == 0
    {
        return Err(PeerError::Process);
    }
    Ok(pid)
}

fn process_creation_marker(process: HANDLE) -> Result<u64, PeerError> {
    let mut create = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(PeerError::Process);
    }
    Ok((u64::from(create.dwHighDateTime) << 32) | u64::from(create.dwLowDateTime))
}

fn process_image_path(process: HANDLE) -> Result<PathBuf, PeerError> {
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) } == 0
        || length == 0
    {
        return Err(PeerError::Image);
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(
        String::from_utf16(&buffer).map_err(|_| PeerError::Image)?,
    ))
}

fn open_image(path: &std::path::Path) -> Result<std::fs::File, PeerError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(PeerError::Image);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle).into() })
}

fn file_identity(file: &std::fs::File) -> Result<FileIdentity, PeerError> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(PeerError::Image);
    }
    Ok(FileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

fn inspect_image(
    file: &mut std::fs::File,
) -> Result<([u8; 32], Option<SourceIdentity>), PeerError> {
    let length = file.metadata().map_err(|_| PeerError::Image)?.len();
    if !(1..=MAX_IMAGE_BYTES).contains(&length) {
        return Err(PeerError::Image);
    }
    file.rewind().map_err(|_| PeerError::Image)?;
    let mut hash = Sha256::new();
    let mut commit = MarkerScan::new(b"COMMIT=");
    let mut tree = MarkerScan::new(b"TREE=");
    let mut tail = Vec::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| PeerError::Image)?;
        if read == 0 {
            break;
        }
        let before = copied;
        copied += read as u64;
        if copied > MAX_IMAGE_BYTES {
            return Err(PeerError::Image);
        }
        hash.update(&buffer[..read]);
        let origin = before.saturating_sub(tail.len() as u64);
        tail.extend_from_slice(&buffer[..read]);
        commit.scan(&tail, origin)?;
        tree.scan(&tail, origin)?;
        const RETAINED_MARKER_BYTES: usize = 96;
        if tail.len() > RETAINED_MARKER_BYTES {
            tail.drain(..tail.len() - RETAINED_MARKER_BYTES);
        }
    }
    if copied != length {
        return Err(PeerError::Image);
    }
    file.rewind().map_err(|_| PeerError::Image)?;
    let source_identity = match (commit.finish(), tree.finish()) {
        (Ok(commit), Ok(tree)) => Some(SourceIdentity { commit, tree }),
        (Err(_), Err(_)) => None,
        _ => return Err(PeerError::Image),
    };
    Ok((hash.finalize().into(), source_identity))
}

struct MarkerScan {
    marker: Vec<u8>,
    offset: Option<u64>,
    value: Option<String>,
}

impl MarkerScan {
    fn new(suffix: &[u8]) -> Self {
        let mut marker = Vec::with_capacity(SOURCE_MARKER_PREFIX.len() + suffix.len());
        marker.extend_from_slice(SOURCE_MARKER_PREFIX);
        marker.extend_from_slice(suffix);
        Self {
            marker,
            offset: None,
            value: None,
        }
    }

    fn scan(&mut self, bytes: &[u8], origin: u64) -> Result<(), PeerError> {
        for (local, window) in bytes.windows(self.marker.len()).enumerate() {
            if window != self.marker.as_slice() {
                continue;
            }
            let offset = origin
                .checked_add(u64::try_from(local).map_err(|_| PeerError::Image)?)
                .ok_or(PeerError::Image)?;
            if self.offset == Some(offset) {
                continue;
            }
            if self.offset.is_some() {
                return Err(PeerError::Image);
            }
            let start = local
                .checked_add(self.marker.len())
                .ok_or(PeerError::Image)?;
            let Some(value) = bytes.get(start..start + 40) else {
                continue;
            };
            if !value
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            {
                return Err(PeerError::Image);
            }
            self.offset = Some(offset);
            self.value = Some(String::from_utf8(value.to_vec()).map_err(|_| PeerError::Image)?);
        }
        Ok(())
    }

    fn finish(self) -> Result<String, PeerError> {
        self.value.ok_or(PeerError::Image)
    }
}

fn query_token(token: HANDLE, class: i32) -> Result<Vec<u8>, PeerError> {
    let mut length = 0;
    unsafe { GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut length) };
    if length == 0 || length > 64 * 1024 {
        return Err(PeerError::Token);
    }
    let mut buffer = vec![0_u8; length as usize];
    if unsafe {
        GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(PeerError::Token);
    }
    Ok(buffer)
}

fn sid_bytes(sid: windows_sys::Win32::Security::PSID) -> Result<Vec<u8>, PeerError> {
    let length = unsafe { GetLengthSid(sid) };
    if !(8..=68).contains(&length) {
        return Err(PeerError::Token);
    }
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length as usize) }.to_vec())
}

fn token_facts(process: HANDLE) -> Result<(Vec<u8>, Vec<u8>, u32, u32), PeerError> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(PeerError::Token);
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let token = Token(token);
    let token_type = query_token(token.0, TokenType)?;
    if token_type.len() < std::mem::size_of::<TOKEN_TYPE>()
        || unsafe { *token_type.as_ptr().cast::<TOKEN_TYPE>() } != TokenPrimary
    {
        return Err(PeerError::Token);
    }
    let user = query_token(token.0, TokenUser)?;
    let user_sid = sid_bytes(unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid })?;
    let logon = query_token(token.0, TokenLogonSid)?;
    let groups = unsafe { &*logon.as_ptr().cast::<TOKEN_GROUPS>() };
    if groups.GroupCount != 1 {
        return Err(PeerError::Token);
    }
    let logon_sid = sid_bytes(groups.Groups[0].Sid)?;
    let session = query_token(token.0, TokenSessionId)?;
    if session.len() < 4 {
        return Err(PeerError::Token);
    }
    let wts_session_id = unsafe { *session.as_ptr().cast::<u32>() };
    let integrity = query_token(token.0, TokenIntegrityLevel)?;
    let sid = unsafe {
        (*integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>())
            .Label
            .Sid
    };
    let count = unsafe { *GetSidSubAuthorityCount(sid) };
    if count == 0 {
        return Err(PeerError::Token);
    }
    let integrity_rid = unsafe { *GetSidSubAuthority(sid, u32::from(count - 1)) };
    Ok((user_sid, logon_sid, wts_session_id, integrity_rid))
}

fn process_architecture(process: HANDLE) -> Result<WindowsArchitecture, PeerError> {
    use windows_sys::Win32::System::SystemInformation::{
        IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_UNKNOWN,
    };
    let mut process_machine = 0;
    let mut native_machine = 0;
    if unsafe { IsWow64Process2(process, &mut process_machine, &mut native_machine) } == 0 {
        return Err(PeerError::Architecture);
    }
    let effective = if process_machine == IMAGE_FILE_MACHINE_UNKNOWN {
        native_machine
    } else {
        process_machine
    };
    match effective {
        IMAGE_FILE_MACHINE_AMD64 => Ok(WindowsArchitecture::X64),
        IMAGE_FILE_MACHINE_ARM64 => Ok(WindowsArchitecture::Arm64),
        _ => Err(PeerError::Architecture),
    }
}

pub fn peer_facts_match_session(left: &PeerFacts, right: &PeerFacts) -> bool {
    left.user_sid == right.user_sid
        && left.logon_sid == right.logon_sid
        && left.wts_session_id == right.wts_session_id
        && left.integrity_rid == right.integrity_rid
        && left.architecture == right.architecture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_facts_come_from_kernel_token_and_image_handles() {
        let peer = VerifiedPeer::from_process_id(std::process::id()).unwrap();
        assert_eq!(peer.facts.process_id, std::process::id());
        assert_ne!(peer.facts.creation_marker, 0);
        assert!(!peer.facts.user_sid.is_empty());
        assert!(!peer.facts.logon_sid.is_empty());
        assert_ne!(peer.facts.integrity_rid, 0);
        assert!(peer.facts.canonical_image.is_absolute());
        assert_ne!(peer.facts.file_identity.file_index, 0);
        assert_ne!(peer.facts.image_sha256, [0; 32]);
        assert!(peer.still_running());
    }

    #[test]
    fn logon_session_match_rejects_each_security_boundary() {
        let peer = VerifiedPeer::from_process_id(std::process::id()).unwrap();
        let base = peer.facts;
        let mut changed = base.clone();
        changed.logon_sid.push(1);
        assert!(!peer_facts_match_session(&base, &changed));
        changed = base.clone();
        changed.wts_session_id ^= 1;
        assert!(!peer_facts_match_session(&base, &changed));
        changed = base.clone();
        changed.integrity_rid ^= 0x1000;
        assert!(!peer_facts_match_session(&base, &changed));
    }
}
