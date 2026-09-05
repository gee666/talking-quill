mod probe;
#[cfg(test)]
mod tests;
use probe::*;
mod control;
use control::*;
mod startup;
use startup::*;
mod startup_io;
use startup_io::*;
mod process;
use process::*;
mod pipe_io;
use pipe_io::*;
mod identity;
use identity::*;
mod sign;
use sign::*;
mod image;
use image::*;
mod protocol;
use protocol::*;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, Write};
use std::mem::{size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::Duration;
use talking_quill_acceptance_signer::windows_key_security::open_validated_private_key;
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetFileInformationByHandle,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
    QueryFullProcessImageNameW, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

#[used]
static SOURCE_COMMIT_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_COMMIT=",
    env!("TALKING_QUILL_SOURCE_COMMIT")
);
#[used]
static SOURCE_TREE_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_TREE=",
    env!("TALKING_QUILL_SOURCE_TREE")
);

const MAX_REQUEST: u64 = 160 * 1024;
const MAX_PAYLOAD: usize = 64 * 1024;
const MAX_IMAGE: u64 = 256 * 1024 * 1024;
const TIMEOUT_MS: u32 = 10_000;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Sign {
        version: u8,
        correlation: String,
        #[serde(rename = "brokerSha256")]
        broker_sha256: String,
        #[serde(rename = "brokerBytes")]
        broker_bytes: u64,
        #[serde(rename = "signerPath")]
        signer_path: String,
        #[serde(rename = "signerSha256")]
        signer_sha256: String,
        #[serde(rename = "signerBytes")]
        signer_bytes: u64,
        #[serde(rename = "privateKeyPath")]
        private_key_path: String,
        #[serde(rename = "payloadHex")]
        payload_hex: String,
        #[serde(rename = "sourceCommit")]
        source_commit: Option<String>,
        #[serde(rename = "sourceTree")]
        source_tree: Option<String>,
    },
    Probe {
        version: u8,
        correlation: String,
        #[serde(rename = "brokerSha256")]
        broker_sha256: String,
        #[serde(rename = "brokerBytes")]
        broker_bytes: u64,
        #[serde(rename = "executablePath")]
        executable_path: String,
        #[serde(rename = "executableSha256")]
        executable_sha256: String,
        #[serde(rename = "executableBytes")]
        executable_bytes: u64,
        #[serde(rename = "sourceCommit")]
        source_commit: String,
        #[serde(rename = "sourceTree")]
        source_tree: String,
        #[serde(rename = "startupFrameHex")]
        startup_frame_hex: String,
        #[serde(rename = "readinessPipe")]
        readiness_pipe: String,
        #[serde(rename = "armedPipe")]
        armed_pipe: Option<String>,
        #[serde(rename = "armedExpectedPhase")]
        armed_expected_phase: Option<String>,
        #[serde(rename = "launchCorrelation")]
        launch_correlation: String,
        #[serde(rename = "absoluteDeadlineMs")]
        absolute_deadline_ms: u64,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    version: u8,
    correlation: String,
    result: &'static str,
    signer_sha256: String,
    signer_bytes: u64,
    retained_identity_matches: bool,
    process_identity_matches: bool,
    process_hash_matches: bool,
    parent_identity_matches: bool,
    creation_identity_matches: bool,
    signature_hex: String,
    public_key_sec1_hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    volume: u32,
    index: u64,
    bytes: u64,
    sha256: [u8; 32],
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _storage: Vec<usize>,
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if !self.list.is_null() {
            unsafe { DeleteProcThreadAttributeList(self.list) };
        }
    }
}

struct SnapshotDir(PathBuf);
impl Drop for SnapshotDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run() -> Result<(), &'static str> {
    std::hint::black_box(SOURCE_COMMIT_MARKER);
    std::hint::black_box(SOURCE_TREE_MARKER);
    let mut frame = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_REQUEST + 1)
        .read_until(b'\n', &mut frame)
        .map_err(|_| "request read")?;
    if frame.is_empty()
        || frame.len() as u64 > MAX_REQUEST
        || frame.last() != Some(&b'\n')
        || frame.get(frame.len().saturating_sub(2)) == Some(&b'\r')
    {
        return Err("request frame");
    }
    frame.pop();
    let request: Request = serde_json::from_slice(&frame).map_err(|_| "request schema")?;
    match request {
        Request::Sign {
            version,
            correlation,
            broker_sha256,
            broker_bytes,
            signer_path,
            signer_sha256,
            signer_bytes,
            private_key_path,
            payload_hex,
            source_commit,
            source_tree,
        } => {
            validate_header(version, &correlation)?;
            let mut broker = retain_self(decode_hash(&broker_sha256)?, broker_bytes)?;
            if !source_matches(
                &mut broker,
                source_commit.as_deref(),
                source_tree.as_deref(),
            )? {
                return Err("broker provenance");
            }
            let response = sign(SignInput {
                correlation,
                signer_path: PathBuf::from(signer_path),
                signer_sha256: decode_hash(&signer_sha256)?,
                signer_bytes,
                private_key_path: PathBuf::from(private_key_path),
                payload: decode_hex(&payload_hex, MAX_PAYLOAD)?,
                source_commit,
                source_tree,
            })?;
            emit_response(&response)
        }
        Request::Probe {
            version,
            correlation,
            broker_sha256,
            broker_bytes,
            executable_path,
            executable_sha256,
            executable_bytes,
            source_commit,
            source_tree,
            startup_frame_hex,
            readiness_pipe,
            armed_pipe,
            armed_expected_phase,
            launch_correlation,
            absolute_deadline_ms,
        } => {
            validate_header(version, &correlation)?;
            let mut broker = retain_self(decode_hash(&broker_sha256)?, broker_bytes)?;
            if !source_matches(&mut broker, Some(&source_commit), Some(&source_tree))? {
                return Err("broker provenance");
            }
            run_probe(ProbeInput {
                correlation,
                executable_path: PathBuf::from(executable_path),
                executable_sha256: decode_hash(&executable_sha256)?,
                executable_bytes,
                source_commit,
                source_tree,
                startup_frame: decode_hex(&startup_frame_hex, 20 * 1024)?,
                readiness_pipe,
                armed_pipe,
                armed_expected_phase,
                launch_correlation,
                absolute_deadline_ms,
            })
        }
    }
}

fn emit_response<T: Serialize>(response: &T) -> Result<(), &'static str> {
    let encoded = serde_json::to_vec(response).map_err(|_| "response encode")?;
    if encoded.len() > 64 * 1024 {
        return Err("response bound");
    }
    std::io::stdout()
        .write_all(&encoded)
        .map_err(|_| "response write")?;
    std::io::stdout()
        .write_all(b"\n")
        .map_err(|_| "response write")?;
    std::io::stdout().flush().map_err(|_| "response flush")
}
