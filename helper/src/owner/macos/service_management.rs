#![cfg(target_os = "macos")]

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;

use super::config::InstalledConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginItemStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unknown,
}
#[derive(Debug, thiserror::Error)]
pub enum ServiceManagementError {
    #[error("outer application ServiceManagement bridge is unavailable")]
    Unavailable,
    #[error("the authenticated keyboard-owner LoginItem operation failed")]
    Operation,
}
#[derive(Debug, Default)]
pub struct MacosLoginItemService;
impl MacosLoginItemService {
    pub fn status(
        &self,
        config: &InstalledConfig,
    ) -> Result<LoginItemStatus, ServiceManagementError> {
        decode_status(invoke(config, "status")?)
    }
    pub fn register(&self, config: &InstalledConfig) -> Result<(), ServiceManagementError> {
        matches!(
            decode_status(invoke(config, "register")?)?,
            LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval
        )
        .then_some(())
        .ok_or(ServiceManagementError::Operation)
    }
    pub fn unregister(
        &self,
        config: &InstalledConfig,
    ) -> Result<UnregisteredLoginItem, ServiceManagementError> {
        matches!(
            decode_status(invoke(config, "unregister")?)?,
            LoginItemStatus::NotRegistered | LoginItemStatus::NotFound
        )
        .then_some(UnregisteredLoginItem { _private: () })
        .ok_or(ServiceManagementError::Operation)
    }
    pub fn ensure_registered(
        &self,
        config: &InstalledConfig,
    ) -> Result<(), ServiceManagementError> {
        match self.status(config)? {
            LoginItemStatus::Enabled => Ok(()),
            LoginItemStatus::NotRegistered | LoginItemStatus::NotFound => self.register(config),
            LoginItemStatus::RequiresApproval => Err(ServiceManagementError::Operation),
            LoginItemStatus::Unknown => Err(ServiceManagementError::Unavailable),
        }
    }
}
pub struct UnregisteredLoginItem {
    _private: (),
}

fn invoke(config: &InstalledConfig, operation: &str) -> Result<usize, ServiceManagementError> {
    let bridge = &config.bridge.canonical_executable_path;
    let mut retained = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(bridge)
        .map_err(|_| ServiceManagementError::Unavailable)?;
    let metadata = retained
        .metadata()
        .map_err(|_| ServiceManagementError::Unavailable)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.nlink() != 1 {
        return Err(ServiceManagementError::Unavailable);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = retained
            .read(&mut buffer)
            .map_err(|_| ServiceManagementError::Unavailable)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if digest.finalize().as_slice() != config.bridge.executable_sha256.as_bytes() {
        return Err(ServiceManagementError::Unavailable);
    }
    let key = Bytes32::random().map_err(|_| ServiceManagementError::Unavailable)?;
    let before = retained
        .metadata()
        .map_err(|_| ServiceManagementError::Unavailable)?;
    let mut command = Command::new(bridge);
    command
        .arg("serve-authenticated")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| ServiceManagementError::Unavailable)?;
    let after_file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(bridge)
    {
        Ok(file) => file,
        Err(_) => return terminate_child(&mut child, ServiceManagementError::Unavailable),
    };
    let after = match after_file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return terminate_child(&mut child, ServiceManagementError::Unavailable),
    };
    if before.dev() != after.dev() || before.ino() != after.ino() || before.len() != after.len() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ServiceManagementError::Unavailable);
    }
    let pid = child.id();
    if super::identity::validate_spawned_sec_code_identity(
        pid,
        bridge,
        config.bridge.executable_sha256,
        config.bridge.code_directory_hash,
        &config.bridge.designated_requirement,
    )
    .is_err()
    {
        return terminate_child(&mut child, ServiceManagementError::Unavailable);
    }
    let Some(mut input) = child.stdin.take() else {
        return terminate_child(&mut child, ServiceManagementError::Unavailable);
    };
    if writeln!(input, "{}", hex(key.as_bytes()))
        .and_then(|_| writeln!(input, "{operation}"))
        .is_err()
    {
        return terminate_child(&mut child, ServiceManagementError::Operation);
    }
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => return terminate_child(&mut child, ServiceManagementError::Operation),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ServiceManagementError::Operation);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(ServiceManagementError::Operation);
    }
    let mut response = String::new();
    child
        .stdout
        .take()
        .ok_or(ServiceManagementError::Operation)?
        .take(512)
        .read_to_string(&mut response)
        .map_err(|_| ServiceManagementError::Operation)?;
    let audit_session =
        super::config::current_audit_token().map_err(|_| ServiceManagementError::Operation)?[6];
    verify_response(&response, key, pid, audit_session, operation)
}

fn terminate_child(
    child: &mut std::process::Child,
    error: ServiceManagementError,
) -> Result<usize, ServiceManagementError> {
    let _ = child.kill();
    let _ = child.wait();
    Err(error)
}

fn verify_response(
    response: &str,
    key: Bytes32,
    pid: u32,
    audit_session: u32,
    operation: &str,
) -> Result<usize, ServiceManagementError> {
    let fields: Vec<_> = response.trim_end().split(' ').collect();
    if fields.len() != 6
        || fields[0].parse::<u32>().ok() != Some(pid)
        || fields[1].parse::<u32>().ok() != Some(audit_session)
        || fields[2] != "1"
        || fields[3] != operation
    {
        return Err(ServiceManagementError::Operation);
    }
    let status = fields[4]
        .parse::<usize>()
        .map_err(|_| ServiceManagementError::Operation)?;
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes())
        .map_err(|_| ServiceManagementError::Operation)?;
    mac.update(b"talking-quill/macos-service-bridge-response/v1\0");
    mac.update(&pid.to_be_bytes());
    mac.update(&audit_session.to_be_bytes());
    mac.update(&1_u64.to_be_bytes());
    mac.update(operation.as_bytes());
    mac.update(&(status as u64).to_be_bytes());
    let expected = hex(&mac.finalize().into_bytes());
    (fields[5] == expected)
        .then_some(status)
        .ok_or(ServiceManagementError::Operation)
}
fn decode_status(raw: usize) -> Result<LoginItemStatus, ServiceManagementError> {
    Ok(match raw {
        0 => LoginItemStatus::NotRegistered,
        1 => LoginItemStatus::Enabled,
        2 => LoginItemStatus::RequiresApproval,
        3 => LoginItemStatus::NotFound,
        _ => LoginItemStatus::Unknown,
    })
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
