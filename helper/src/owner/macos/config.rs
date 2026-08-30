#![cfg(target_os = "macos")]

use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};
use talking_quill_owner_protocol::schema::{Architecture, ProtocolHeader};

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WireRole {
    canonical_executable_path: PathBuf,
    executable_sha256: String,
    capture_authorized: bool,
    signing: WireSigning,
}

impl WireRole {
    fn into_role(self, identifier: &str, capture_authorized: bool) -> Result<Role, ConfigError> {
        if self.capture_authorized != capture_authorized
            || !normal_absolute(&self.canonical_executable_path)
        {
            return Err(ConfigError);
        }
        let (designated_requirement, cdhash) = match self.signing {
            WireSigning::AdHoc { cdhash } => {
                require_hex(&cdhash, 20)?;
                (
                    format!(
                        "identifier \"{identifier}\" and cdhash H\"{cdhash}\" and not anchor apple"
                    ),
                    cdhash,
                )
            }
            WireSigning::LocallyTrustedSelfSigned {
                certificate_sha256,
                certificate_requirement_hash,
                cdhash,
            } => {
                require_hex(&certificate_sha256, 32)?;
                require_hex(&certificate_requirement_hash, 20)?;
                require_hex(&cdhash, 20)?;
                (
                    format!(
                        "identifier \"{identifier}\" and anchor trusted and certificate leaf = H\"{certificate_requirement_hash}\" and certificate root = H\"{certificate_requirement_hash}\" and not anchor apple"
                    ),
                    cdhash,
                )
            }
        };
        Ok(Role {
            canonical_executable_path: self.canonical_executable_path,
            executable_sha256: bytes32(&self.executable_sha256)?,
            code_directory_hash: bytes20(&cdhash)?,
            designated_requirement,
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum WireSigning {
    AdHoc {
        cdhash: String,
    },
    LocallyTrustedSelfSigned {
        #[serde(rename = "certificateSha256")]
        certificate_sha256: String,
        #[serde(rename = "certificateRequirementHash")]
        certificate_requirement_hash: String,
        cdhash: String,
    },
}

fn bytes20(value: &str) -> Result<[u8; 20], ConfigError> {
    require_hex(value, 20)?;
    let mut bytes = [0_u8; 20];
    for (target, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *target = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| ConfigError)?, 16)
            .map_err(|_| ConfigError)?;
    }
    Ok(bytes)
}

fn normal_absolute(path: &Path) -> bool {
    use std::path::Component;
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn file_stat_size(fd: libc::c_int) -> Result<i64, ConfigError> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        Err(ConfigError)
    } else {
        Ok(stat.st_size)
    }
}

fn stable_hash(path: &Path) -> Result<Bytes32, ConfigError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ConfigError)?;
    let before = file.metadata().map_err(|_| ConfigError)?;
    if !before.is_file() || before.nlink() != 1 || before.len() == 0 {
        return Err(ConfigError);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| ConfigError)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let after = file.metadata().map_err(|_| ConfigError)?;
    if before.dev() != after.dev() || before.ino() != after.ino() || before.len() != after.len() {
        return Err(ConfigError);
    }
    Ok(Bytes32::new(digest.finalize().into()))
}

fn requirement_digest(value: &str) -> Bytes32 {
    Bytes32::new(Sha256::digest(value.as_bytes()).into())
}

fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

fn bytes32(value: &str) -> Result<Bytes32, ConfigError> {
    let bytes = decode_hex(value, 32)?;
    Ok(Bytes32::new(bytes.try_into().map_err(|_| ConfigError)?))
}

fn require_hex(value: &str, bytes: usize) -> Result<(), ConfigError> {
    decode_hex(value, bytes).map(|_| ())
}

fn decode_hex(value: &str, bytes: usize) -> Result<Vec<u8>, ConfigError> {
    if value.len() != bytes * 2
        || !value
            .bytes()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
    {
        return Err(ConfigError);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| ConfigError)?, 16)
                .map_err(|_| ConfigError)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("installed macOS owner policy is unavailable or invalid")]
pub struct ConfigError;
