use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::schema::ProtocolHeader;

use super::{ConfigError, Role};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct WireRole {
    canonical_executable_path: PathBuf,
    executable_sha256: String,
    capture_authorized: bool,
    signing: WireSigning,
}

impl WireRole {
    pub(super) fn into_role(
        self,
        identifier: &str,
        capture_authorized: bool,
    ) -> Result<Role, ConfigError> {
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

pub(super) fn file_stat_size(fd: libc::c_int) -> Result<i64, ConfigError> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &raw mut stat) } != 0 {
        Err(ConfigError)
    } else {
        Ok(stat.st_size)
    }
}

pub(super) fn stable_hash(path: &Path) -> Result<Bytes32, ConfigError> {
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

pub(super) fn requirement_digest(value: &str) -> Bytes32 {
    Bytes32::new(Sha256::digest(value.as_bytes()).into())
}

pub(super) fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

pub(super) fn bytes32(value: &str) -> Result<Bytes32, ConfigError> {
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
