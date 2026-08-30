#![cfg(target_os = "macos")]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;

pub struct UnregisteredLoginItem(());
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the authenticated outer application owner-removal bridge failed")]
pub struct LoginItemError;

pub struct RemovalServiceBridge {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    key: Bytes32,
    sequence: u64,
    audit_session: u32,
}
impl RemovalServiceBridge {
    pub fn start(expected: &super::RolePolicy) -> Result<Self, LoginItemError> {
        let mut retained = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&expected.canonical_executable_path)
            .map_err(|_| LoginItemError)?;
        let metadata = retained.metadata().map_err(|_| LoginItemError)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.nlink() != 1 {
            return Err(LoginItemError);
        }
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let read = retained.read(&mut buffer).map_err(|_| LoginItemError)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
        if digest.finalize().as_slice() != expected.executable_sha256.as_bytes() {
            return Err(LoginItemError);
        }
        let before = retained.metadata().map_err(|_| LoginItemError)?;
        let mut command = Command::new(&expected.canonical_executable_path);
        command
            .arg("serve-authenticated")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| LoginItemError)?;
        let after_file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&expected.canonical_executable_path)
        {
            Ok(file) => file,
            Err(_) => return terminate_start(&mut child),
        };
        let after = match after_file.metadata() {
            Ok(metadata) => metadata,
            Err(_) => return terminate_start(&mut child),
        };
        if before.dev() != after.dev() || before.ino() != after.ino() || before.len() != after.len()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LoginItemError);
        }
        let audit_session = match super::current_audit_token() {
            Ok(token) => token.audit_session_id(),
            Err(_) => return terminate_start(&mut child),
        };
        if super::native_identity::validate_spawned_process_against(
            child.id(),
            audit_session,
            expected,
        )
        .is_err()
        {
            return terminate_start(&mut child);
        }
        let Some(mut input) = child.stdin.take() else {
            return terminate_start(&mut child);
        };
        let Some(output) = child.stdout.take() else {
            return terminate_start(&mut child);
        };
        let flags = unsafe { libc::fcntl(output.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(output.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                != 0
        {
            return terminate_start(&mut child);
        }
        let key = match Bytes32::random() {
            Ok(key) => key,
            Err(_) => return terminate_start(&mut child),
        };
        if writeln!(input, "{}", hex(key.as_bytes()))
            .and_then(|_| input.flush())
            .is_err()
        {
            return terminate_start(&mut child);
        }
        Ok(Self {
            child,
            input,
            output,
            key,
            sequence: 0,
            audit_session,
        })
    }
    pub fn unregister(&mut self) -> Result<UnregisteredLoginItem, LoginItemError> {
        self.sequence = self.sequence.checked_add(1).ok_or(LoginItemError)?;
        writeln!(self.input, "unregister")
            .and_then(|_| self.input.flush())
            .map_err(|_| LoginItemError)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut response = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            match self.output.read(&mut byte) {
                Ok(0) => return Err(LoginItemError),
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) if response.len() < 511 => response.push(byte[0]),
                Ok(_) => return Err(LoginItemError),
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return Err(LoginItemError),
            }
        }
        let response = std::str::from_utf8(&response).map_err(|_| LoginItemError)?;
        let status = verify_response(
            response,
            self.key,
            self.child.id(),
            self.audit_session,
            self.sequence,
        )?;
        matches!(status, 0 | 3)
            .then_some(UnregisteredLoginItem(()))
            .ok_or(LoginItemError)
    }
}
fn terminate_start(
    child: &mut std::process::Child,
) -> Result<RemovalServiceBridge, LoginItemError> {
    let _ = child.kill();
    let _ = child.wait();
    Err(LoginItemError)
}

impl Drop for RemovalServiceBridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn verify_response(
    response: &str,
    key: Bytes32,
    pid: u32,
    audit_session: u32,
    sequence: u64,
) -> Result<usize, LoginItemError> {
    let fields: Vec<_> = response.split(' ').collect();
    if fields.len() != 6
        || fields[0].parse::<u32>().ok() != Some(pid)
        || fields[1].parse::<u32>().ok() != Some(audit_session)
        || fields[2].parse::<u64>().ok() != Some(sequence)
        || fields[3] != "unregister"
    {
        return Err(LoginItemError);
    }
    let status = fields[4].parse::<usize>().map_err(|_| LoginItemError)?;
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()).map_err(|_| LoginItemError)?;
    mac.update(b"talking-quill/macos-service-bridge-response/v1\0");
    mac.update(&pid.to_be_bytes());
    mac.update(&audit_session.to_be_bytes());
    mac.update(&sequence.to_be_bytes());
    mac.update(b"unregister");
    mac.update(&(status as u64).to_be_bytes());
    (fields[5] == hex(&mac.finalize().into_bytes()))
        .then_some(status)
        .ok_or(LoginItemError)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
