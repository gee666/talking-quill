#![cfg(target_os = "macos")]

use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};
use talking_quill_owner_protocol::schema::Architecture;

mod validation;

use validation::{
    WireRole, bytes32, file_stat_size, protocol_header, requirement_digest, stable_hash,
};

const MAX_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct InstalledConfig {
    pub release_build_digest: Bytes32,
    pub installation_identity_digest: Bytes32,
    pub gateway: Role,
    pub owner: Role,
    pub bridge: Role,
    pub gateway_release_policy: PolicyBlob,
    pub gateway_release_policy_signature: PolicySignature,
    pub owner_release_policy: PolicyBlob,
    pub owner_release_policy_signature: PolicySignature,
    pub bridge_release_policy: PolicyBlob,
    pub bridge_release_policy_signature: PolicySignature,
    pub architecture: Architecture,
    pub socket_path: PathBuf,
}

impl std::fmt::Debug for InstalledConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstalledConfig(<redacted>)")
    }
}

#[derive(Clone)]
pub struct Role {
    pub canonical_executable_path: PathBuf,
    pub executable_sha256: Bytes32,
    pub code_directory_hash: [u8; 20],
    pub designated_requirement: String,
}

impl InstalledConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_for(false)
    }

    pub(crate) fn load_for_bridge() -> Result<Self, ConfigError> {
        Self::load_for(true)
    }

    fn load_for(bridge_process: bool) -> Result<Self, ConfigError> {
        let executable = std::env::current_exe().map_err(|_| ConfigError)?;
        let resources = installed_resources(&executable, bridge_process).ok_or(ConfigError)?;
        let sidecar = resources.join("keyboard-owner-r5m.json");
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(sidecar)
            .map_err(|_| ConfigError)?;
        let before = file.metadata().map_err(|_| ConfigError)?;
        if !before.is_file()
            || before.nlink() != 1
            || before.len() == 0
            || before.len() > MAX_CONFIG_BYTES as u64
        {
            return Err(ConfigError);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ConfigError)?;
        let after = file.metadata().map_err(|_| ConfigError)?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || file_stat_size(file.as_raw_fd())? != bytes.len() as i64
        {
            return Err(ConfigError);
        }
        let wire: WireConfig = serde_json::from_slice(&bytes).map_err(|_| ConfigError)?;
        let architecture = match std::env::consts::ARCH {
            "x86_64" => Architecture::X64,
            "aarch64" => Architecture::Arm64,
            _ => return Err(ConfigError),
        };
        let gateway = wire
            .gateway
            .into_role("com.talkingquill.app.helper", true)?;
        let owner = wire
            .owner
            .into_role("com.talkingquill.app.keyboard-owner", true)?;
        let bridge = wire
            .bridge
            .into_role("com.talkingquill.app.service-management", false)?;
        super::cms::verify(
            &wire.gateway_release_policy,
            &wire.gateway_release_policy_signature,
        )
        .map_err(|_| ConfigError)?;
        super::cms::verify(
            &wire.owner_release_policy,
            &wire.owner_release_policy_signature,
        )
        .map_err(|_| ConfigError)?;
        super::cms::verify(
            &wire.bridge_release_policy,
            &wire.bridge_release_policy_signature,
        )
        .map_err(|_| ConfigError)?;
        if wire.gateway_release_policy != wire.owner_release_policy {
            return Err(ConfigError);
        }
        let gateway_policy = wire
            .gateway_release_policy
            .decode()
            .map_err(|_| ConfigError)?;
        let owner_policy = wire
            .owner_release_policy
            .decode()
            .map_err(|_| ConfigError)?;
        if gateway_policy != owner_policy {
            return Err(ConfigError);
        }
        let bridge_policy = wire
            .bridge_release_policy
            .decode()
            .map_err(|_| ConfigError)?;
        let release_build_digest = bytes32(&wire.release_build_digest)?;
        if gateway_policy.architecture != architecture
            || owner_policy.architecture != architecture
            || gateway_policy.release_build_digest != release_build_digest
            || owner_policy.release_build_digest != release_build_digest
            || gateway_policy.gateway_sha256 != gateway.executable_sha256
            || gateway_policy.owner_sha256 != owner.executable_sha256
            || owner_policy.gateway_sha256 != gateway.executable_sha256
            || owner_policy.owner_sha256 != owner.executable_sha256
            || gateway_policy.gateway_signer_policy_digest
                != requirement_digest(&gateway.designated_requirement)
            || gateway_policy.owner_signer_policy_digest
                != requirement_digest(&owner.designated_requirement)
            || owner_policy.gateway_signer_policy_digest
                != requirement_digest(&gateway.designated_requirement)
            || owner_policy.owner_signer_policy_digest
                != requirement_digest(&owner.designated_requirement)
            || gateway_policy.gateway_protocol != protocol_header()
            || gateway_policy.owner_protocol != protocol_header()
            || bridge_policy.architecture != architecture
            || bridge_policy.release_build_digest != release_build_digest
            || bridge_policy.gateway_sha256 != bridge.executable_sha256
            || bridge_policy.owner_sha256 != bridge.executable_sha256
            || bridge_policy.gateway_signer_policy_digest
                != requirement_digest(&bridge.designated_requirement)
            || bridge_policy.owner_signer_policy_digest
                != requirement_digest(&bridge.designated_requirement)
            || bridge_policy.predecessor.is_some()
        {
            return Err(ConfigError);
        }
        let outer = resources
            .parent()
            .and_then(Path::parent)
            .ok_or(ConfigError)?;
        let expected_gateway = outer.join("Contents/Resources/helper/talking-quill-helper");
        let current_executable = executable.canonicalize().map_err(|_| ConfigError)?;
        if (!bridge_process && current_executable != expected_gateway)
            || (bridge_process && current_executable != bridge.canonical_executable_path)
        {
            return Err(ConfigError);
        }
        let expected_owner = outer.join(
            "Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner",
        );
        let expected_bridge = outer.join("Contents/MacOS/talking-quill-macos-service-bridge");
        if expected_owner != owner.canonical_executable_path
            || expected_bridge != bridge.canonical_executable_path
            || expected_owner.canonicalize().map_err(|_| ConfigError)? != expected_owner
            || stable_hash(&expected_gateway)? != gateway.executable_sha256
            || stable_hash(&expected_owner)? != owner.executable_sha256
            || stable_hash(&expected_bridge)? != bridge.executable_sha256
        {
            return Err(ConfigError);
        }
        super::identity::validate_native_sec_code_identity(
            &expected_gateway,
            &gateway.designated_requirement,
        )
        .map_err(|_| ConfigError)?;
        super::identity::validate_native_sec_code_identity(
            &expected_owner,
            &owner.designated_requirement,
        )
        .map_err(|_| ConfigError)?;
        super::identity::validate_native_sec_code_identity(
            &expected_bridge,
            &bridge.designated_requirement,
        )
        .map_err(|_| ConfigError)?;
        let socket_path = runtime_socket()?;
        Ok(Self {
            release_build_digest,
            installation_identity_digest: bytes32(&wire.installation_identity_digest)?,
            gateway,
            owner,
            bridge,
            gateway_release_policy: wire.gateway_release_policy,
            gateway_release_policy_signature: wire.gateway_release_policy_signature,
            owner_release_policy: wire.owner_release_policy,
            owner_release_policy_signature: wire.owner_release_policy_signature,
            bridge_release_policy: wire.bridge_release_policy,
            bridge_release_policy_signature: wire.bridge_release_policy_signature,
            architecture,
            socket_path,
        })
    }
}

fn installed_resources(executable: &Path, bridge_process: bool) -> Option<PathBuf> {
    if bridge_process {
        let macos = executable.parent()?;
        let contents = macos.parent()?;
        return (executable.file_name()?.to_str()? == "talking-quill-macos-service-bridge"
            && macos.file_name()?.to_str()? == "MacOS"
            && contents.file_name()?.to_str()? == "Contents")
            .then(|| contents.join("Resources"));
    }
    let helper = executable.parent()?;
    let resources = helper.parent()?;
    let contents = resources.parent()?;
    (helper.file_name()?.to_str()? == "helper"
        && resources.file_name()?.to_str()? == "Resources"
        && contents.file_name()?.to_str()? == "Contents")
        .then(|| resources.to_path_buf())
}

fn runtime_socket() -> Result<PathBuf, ConfigError> {
    let home = std::env::var_os("HOME").ok_or(ConfigError)?;
    let audit = current_audit_token()?;
    if audit[6] == 0 || audit[1] != unsafe { libc::geteuid() } {
        return Err(ConfigError);
    }
    Ok(PathBuf::from(home)
        .join("Library/Application Support/Talking Quill/KeyboardOwner/run-v1")
        .join(audit[6].to_string())
        .join("owner-v1.sock"))
}

pub fn current_audit_token() -> Result<[u32; 8], ConfigError> {
    let mut descriptors = [0; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM,
            0,
            descriptors.as_mut_ptr(),
        )
    } != 0
    {
        return Err(ConfigError);
    }
    let mut words = [0_u32; 8];
    let mut length = std::mem::size_of::<[u32; 8]>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            descriptors[0],
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            words.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    unsafe {
        libc::close(descriptors[0]);
        libc::close(descriptors[1]);
    }
    if status != 0 || length as usize != std::mem::size_of::<[u32; 8]>() {
        Err(ConfigError)
    } else {
        Ok(words)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WireConfig {
    release_build_digest: String,
    installation_identity_digest: String,
    gateway: WireRole,
    owner: WireRole,
    bridge: WireRole,
    gateway_release_policy: PolicyBlob,
    gateway_release_policy_signature: PolicySignature,
    owner_release_policy: PolicyBlob,
    owner_release_policy_signature: PolicySignature,
    bridge_release_policy: PolicyBlob,
    bridge_release_policy_signature: PolicySignature,
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("installed macOS owner policy is unavailable or invalid")]
pub struct ConfigError;
