//! Release manifest schema and canonical layout hashing.

use super::InstalledReleaseError;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ReleaseManifest {
    pub(super) schema_version: u16,
    pub(super) kind: String,
    pub(super) version: String,
    pub(super) platform: String,
    pub(super) architecture: String,
    pub(super) owner_mode: String,
    pub(super) package_mode: String,
    pub(super) source_commit: String,
    pub(super) source_tree: String,
    pub(super) roles: Vec<ManifestRole>,
    pub(super) predecessor: Option<ManifestPredecessor>,
    pub(super) fresh_install: Option<bool>,
    pub(super) release_build_digest: String,
    pub(super) package_layout_digest: String,
    pub(super) update: ManifestUpdate,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ManifestPredecessor {
    pub(super) platform: String,
    pub(super) architecture: String,
    pub(super) version: String,
    pub(super) release_build_digest: String,
    pub(super) gateway_sha256: String,
    pub(super) owner_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ManifestRole {
    pub(super) role: String,
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) suppression_capable: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ManifestUpdate {
    pub(super) channel: String,
    pub(super) payload: String,
    pub(super) companion: Option<serde_json::Value>,
    pub(super) transaction_binding: String,
    pub(super) maintenance_installer: String,
}

pub(super) fn canonical_package_layout(
    manifest: &ReleaseManifest,
) -> Result<[u8; 32], InstalledReleaseError> {
    let mut hash = Sha256::new();
    hash.update(b"talking-quill/package-layout/v1\0");
    for (name, value) in [
        ("version", manifest.version.as_str()),
        ("platform", manifest.platform.as_str()),
        ("architecture", manifest.architecture.as_str()),
        ("ownerMode", manifest.owner_mode.as_str()),
        ("packageMode", manifest.package_mode.as_str()),
        ("sourceCommit", manifest.source_commit.as_str()),
        ("sourceTree", manifest.source_tree.as_str()),
    ] {
        hash_layout_field(&mut hash, name, value)?;
    }
    for role in &manifest.roles {
        hash_layout_field(
            &mut hash,
            "role",
            &format!(
                "{}\0{}\0{}\0{}",
                role.role, role.path, role.sha256, role.suppression_capable
            ),
        )?;
    }
    hash_layout_field(
        &mut hash,
        "predecessorPresent",
        if manifest.predecessor.is_some() {
            "true"
        } else {
            "false"
        },
    )?;
    if let Some(predecessor) = &manifest.predecessor {
        for (name, value) in [
            ("predecessorPlatform", predecessor.platform.as_str()),
            ("predecessorArchitecture", predecessor.architecture.as_str()),
            ("predecessorVersion", predecessor.version.as_str()),
            (
                "predecessorReleaseBuildDigest",
                predecessor.release_build_digest.as_str(),
            ),
            (
                "predecessorGatewaySha256",
                predecessor.gateway_sha256.as_str(),
            ),
            ("predecessorOwnerSha256", predecessor.owner_sha256.as_str()),
        ] {
            hash_layout_field(&mut hash, name, value)?;
        }
    }
    if manifest.fresh_install == Some(true) {
        hash_layout_field(&mut hash, "freshInstall", "true")?;
    }
    Ok(hash.finalize().into())
}

pub(super) fn hash_layout_field(
    hash: &mut Sha256,
    name: &str,
    value: &str,
) -> Result<(), InstalledReleaseError> {
    let name_length = u16::try_from(name.len()).map_err(|_| InstalledReleaseError::Manifest)?;
    let value_length = u32::try_from(value.len()).map_err(|_| InstalledReleaseError::Manifest)?;
    hash.update(name_length.to_be_bytes());
    hash.update(value_length.to_be_bytes());
    hash.update(name.as_bytes());
    hash.update(value.as_bytes());
    Ok(())
}

pub(super) fn valid_source_identity(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn parse_hex32(value: &str) -> Result<[u8; 32], InstalledReleaseError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(InstalledReleaseError::Manifest);
    }
    let mut result = [0_u8; 32];
    for (index, output) in result.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| InstalledReleaseError::Manifest)?;
    }
    Ok(result)
}
