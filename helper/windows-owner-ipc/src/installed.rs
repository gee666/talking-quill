#![cfg(windows)]

use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::channel::{ChannelPurpose, SessionBinding, StablePipeBinding};
use crate::image_policy::{Sha256Digest, WindowsArchitecture};
use crate::peer::{PeerFacts, peer_facts_match_session};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MANIFEST_NAME: &str = "keyboard-owner-release-v1.json";

#[derive(Clone, Debug)]
pub struct InstalledRelease {
    pub binding: StablePipeBinding,
    pub policy: [u8; 328],
}

#[derive(Debug, Error)]
pub enum InstalledReleaseError {
    #[error("Windows owner peers are not in the same interactive logon session")]
    Session,
    #[error("Windows owner peer image location or role is invalid")]
    Image,
    #[error("Windows owner peer integrity or architecture is invalid")]
    Policy,
    #[error("installed Windows owner release metadata is unavailable or invalid")]
    Manifest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReleaseManifest {
    schema_version: u16,
    kind: String,
    version: String,
    platform: String,
    architecture: String,
    owner_mode: String,
    package_mode: String,
    source_commit: String,
    source_tree: String,
    roles: Vec<ManifestRole>,
    predecessor: Option<ManifestPredecessor>,
    fresh_install: Option<bool>,
    release_build_digest: String,
    package_layout_digest: String,
    update: ManifestUpdate,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ManifestPredecessor {
    platform: String,
    architecture: String,
    version: String,
    release_build_digest: String,
    gateway_sha256: String,
    owner_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ManifestRole {
    role: String,
    path: String,
    sha256: String,
    suppression_capable: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ManifestUpdate {
    channel: String,
    payload: String,
    companion: Option<serde_json::Value>,
    transaction_binding: String,
    maintenance_installer: String,
}

impl InstalledRelease {
    pub fn from_peer_facts(
        gateway: &PeerFacts,
        owner: &PeerFacts,
        purpose: ChannelPurpose,
    ) -> Result<Self, InstalledReleaseError> {
        if !peer_facts_match_session(gateway, owner) {
            return Err(InstalledReleaseError::Session);
        }
        if gateway.integrity_rid != 0x2000 || owner.integrity_rid != 0x2000 {
            return Err(InstalledReleaseError::Policy);
        }
        if gateway.architecture != owner.architecture {
            return Err(InstalledReleaseError::Policy);
        }
        let gateway_parent = gateway
            .canonical_image
            .parent()
            .ok_or(InstalledReleaseError::Image)?;
        let owner_parent = owner
            .canonical_image
            .parent()
            .ok_or(InstalledReleaseError::Image)?;
        if !path_eq(gateway_parent, owner_parent)
            || !file_name_eq(&gateway.canonical_image, "talking-quill-helper.exe")
            || !file_name_eq(&owner.canonical_image, "talking-quill-keyboard-owner.exe")
        {
            return Err(InstalledReleaseError::Image);
        }
        let resources = gateway_parent
            .parent()
            .ok_or(InstalledReleaseError::Manifest)?;
        let manifest_path = resources.join(MANIFEST_NAME);
        let bytes = read_locked_manifest(&manifest_path)?;
        let recovery_launcher = read_locked_file(
            &resources.join("helper/talking-quill-update-recovery-launcher.exe"),
            64 * 1024 * 1024,
        )?;
        Self::from_manifest_bytes(
            gateway,
            owner,
            purpose,
            resources,
            &bytes,
            &recovery_launcher,
        )
    }

    fn from_manifest_bytes(
        gateway: &PeerFacts,
        owner: &PeerFacts,
        purpose: ChannelPurpose,
        resources: &Path,
        bytes: &[u8],
        recovery_launcher: &[u8],
    ) -> Result<Self, InstalledReleaseError> {
        if !peer_facts_match_session(gateway, owner)
            || gateway.architecture != owner.architecture
            || gateway.integrity_rid != 0x2000
            || owner.integrity_rid != 0x2000
        {
            return Err(InstalledReleaseError::Policy);
        }
        let manifest: ReleaseManifest =
            serde_json::from_slice(bytes).map_err(|_| InstalledReleaseError::Manifest)?;
        let architecture = match gateway.architecture {
            WindowsArchitecture::X64 => "x64",
            WindowsArchitecture::Arm64 => "arm64",
        };
        if manifest.schema_version != 1
            || manifest.kind != "talking-quill-local-owner-release"
            || manifest.platform != "win"
            || manifest.architecture != architecture
            || manifest.owner_mode != "local-unsigned-enabled"
            || !matches!(manifest.package_mode.as_str(), "fresh" | "update")
            || (manifest.package_mode == "fresh"
                && (manifest.fresh_install != Some(true) || manifest.predecessor.is_some()))
            || (manifest.package_mode == "update"
                && (manifest.fresh_install.is_some() || manifest.predecessor.is_none()))
            || manifest.version.is_empty()
            || manifest.version.len() > 64
            || !valid_source_identity(&manifest.source_commit)
            || !valid_source_identity(&manifest.source_tree)
            || gateway
                .source_identity
                .as_ref()
                .map(|value| value.commit.as_str())
                != Some(manifest.source_commit.as_str())
            || owner
                .source_identity
                .as_ref()
                .map(|value| value.commit.as_str())
                != Some(manifest.source_commit.as_str())
            || gateway
                .source_identity
                .as_ref()
                .map(|value| value.tree.as_str())
                != Some(manifest.source_tree.as_str())
            || owner
                .source_identity
                .as_ref()
                .map(|value| value.tree.as_str())
                != Some(manifest.source_tree.as_str())
            || manifest.roles.len() != 3
            || manifest.update.channel != format!("latest-{architecture}")
            || manifest.update.payload != "tqpkg2"
            || manifest.update.companion.is_some()
            || manifest.update.transaction_binding != "source-target-package-sha256-v1"
            || manifest.update.maintenance_installer != "native-setup"
        {
            return Err(InstalledReleaseError::Manifest);
        }
        let release_digest = parse_hex32(&manifest.release_build_digest)?;
        let layout_digest = parse_hex32(&manifest.package_layout_digest)?;
        let canonical_layout = canonical_package_layout(&manifest)?;
        if release_digest != layout_digest || layout_digest != canonical_layout {
            return Err(InstalledReleaseError::Manifest);
        }
        let gateway_relative = "resources/helper/talking-quill-helper.exe";
        let owner_relative = "resources/helper/talking-quill-keyboard-owner.exe";
        let recovery_launcher_relative =
            "resources/helper/talking-quill-update-recovery-launcher.exe";
        if pe_architecture(recovery_launcher) != Some(architecture)
            || !has_source_identity(
                recovery_launcher,
                &manifest.source_commit,
                &manifest.source_tree,
            )
        {
            return Err(InstalledReleaseError::Manifest);
        }
        let recovery_launcher_sha256: [u8; 32] = Sha256::digest(recovery_launcher).into();
        let mut gateway_seen = false;
        let mut owner_seen = false;
        let mut recovery_launcher_seen = false;
        for (index, role) in manifest.roles.iter().enumerate() {
            match (index, role.role.as_str()) {
                (0, "gateway")
                    if !gateway_seen
                        && !role.suppression_capable
                        && role.path == gateway_relative
                        && parse_hex32(&role.sha256)? == gateway.image_sha256 =>
                {
                    gateway_seen = true;
                }
                (1, "owner")
                    if !owner_seen
                        && role.suppression_capable
                        && role.path == owner_relative
                        && parse_hex32(&role.sha256)? == owner.image_sha256 =>
                {
                    owner_seen = true;
                }
                (2, "recovery-launcher")
                    if !recovery_launcher_seen
                        && !role.suppression_capable
                        && role.path == recovery_launcher_relative
                        && parse_hex32(&role.sha256)? == recovery_launcher_sha256 =>
                {
                    recovery_launcher_seen = true;
                }
                _ => return Err(InstalledReleaseError::Manifest),
            }
        }
        let helper_directory = resources.join("helper");
        if !gateway_seen
            || !owner_seen
            || !recovery_launcher_seen
            || !path_eq(
                &helper_directory.join("talking-quill-helper.exe"),
                &gateway.canonical_image,
            )
            || !path_eq(
                &helper_directory.join("talking-quill-keyboard-owner.exe"),
                &owner.canonical_image,
            )
        {
            return Err(InstalledReleaseError::Manifest);
        }

        let gateway_role = role_digest(1, gateway.image_sha256);
        let owner_role = role_digest(2, owner.image_sha256);
        let manifest_sha256: [u8; 32] = Sha256::digest(bytes).into();
        let predecessor = manifest
            .predecessor
            .as_ref()
            .map(|value| {
                if value.platform != "win"
                    || value.architecture != architecture
                    || value.version.is_empty()
                    || value.version.len() > 64
                {
                    return Err(InstalledReleaseError::Manifest);
                }
                Ok((
                    parse_hex32(&value.release_build_digest)?,
                    parse_hex32(&value.gateway_sha256)?,
                    parse_hex32(&value.owner_sha256)?,
                ))
            })
            .transpose()?;
        let policy = encode_policy(
            gateway.architecture,
            release_digest,
            gateway.image_sha256,
            owner.image_sha256,
            gateway_role,
            owner_role,
            predecessor,
        );
        let policy_digest: [u8; 32] = Sha256::digest(policy).into();
        let user_sid_digest: [u8; 32] = Sha256::digest(&gateway.user_sid).into();
        let logon_sid_digest: [u8; 32] = Sha256::digest(&gateway.logon_sid).into();

        let mut endpoint = Sha256::new();
        endpoint.update(b"TQKO-WINDOWS-STABLE-PIPE-BINDING-V2\0");
        endpoint.update(gateway.process_id.to_be_bytes());
        endpoint.update(gateway.creation_marker.to_be_bytes());
        endpoint.update(owner.process_id.to_be_bytes());
        endpoint.update(owner.creation_marker.to_be_bytes());
        endpoint.update(user_sid_digest);
        endpoint.update(logon_sid_digest);
        endpoint.update(gateway.wts_session_id.to_be_bytes());
        endpoint.update(gateway.integrity_rid.to_be_bytes());
        endpoint.update(gateway.file_identity.volume_serial.to_be_bytes());
        endpoint.update(gateway.file_identity.file_index.to_be_bytes());
        endpoint.update(gateway.image_sha256);
        endpoint.update(owner.file_identity.volume_serial.to_be_bytes());
        endpoint.update(owner.file_identity.file_index.to_be_bytes());
        endpoint.update(owner.image_sha256);
        endpoint.update(manifest_sha256);
        endpoint.update(release_digest);
        endpoint.update(layout_digest);
        endpoint.update(policy_digest);
        let credential_binding_digest: [u8; 32] = endpoint.finalize().into();

        let peer_binding_id: [u8; 32] = Sha256::digest(
            [
                b"TQKO-WINDOWS-PEER-BINDING-V2\0".as_slice(),
                &credential_binding_digest,
            ]
            .concat(),
        )
        .into();
        let endpoint_binding_id: [u8; 32] = Sha256::digest(
            [
                b"TQKO-WINDOWS-ENDPOINT-BINDING-V2\0".as_slice(),
                &credential_binding_digest,
            ]
            .concat(),
        )
        .into();
        Ok(Self {
            binding: StablePipeBinding {
                peer_binding_id,
                endpoint_binding_id,
                purpose,
                architecture: gateway.architecture,
                gateway_sha256: Sha256Digest::new(gateway.image_sha256),
                owner_sha256: Sha256Digest::new(owner.image_sha256),
                gateway_role_digest: Sha256Digest::new(gateway_role),
                owner_role_digest: Sha256Digest::new(owner_role),
                release_policy_digest: Sha256Digest::new(policy_digest),
                release_build_digest: Sha256Digest::new(release_digest),
                manifest_sha256: Sha256Digest::new(manifest_sha256),
                installation_id: manifest.package_layout_digest,
                build_id: manifest.version,
                gateway_process_id: gateway.process_id,
                owner_process_id: owner.process_id,
                gateway_creation_marker: gateway.creation_marker,
                owner_creation_marker: owner.creation_marker,
                owner_integrity_rid: owner.integrity_rid,
                session: SessionBinding {
                    user_sid_digest,
                    logon_sid_digest,
                    wts_session_id: gateway.wts_session_id,
                    integrity_rid: gateway.integrity_rid,
                },
                credential_binding_digest,
            },
            policy,
        })
    }
}

fn read_locked_manifest(path: &Path) -> Result<Vec<u8>, InstalledReleaseError> {
    read_locked_file(path, MAX_MANIFEST_BYTES)
}

fn read_locked_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, InstalledReleaseError> {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(InstalledReleaseError::Manifest);
    }
    let mut file: std::fs::File = unsafe { OwnedHandle::from_raw_handle(handle) }.into();
    let length = file
        .metadata()
        .map_err(|_| InstalledReleaseError::Manifest)?
        .len();
    if !(1..=max_bytes).contains(&length) {
        return Err(InstalledReleaseError::Manifest);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.read_to_end(&mut bytes)
        .map_err(|_| InstalledReleaseError::Manifest)?;
    if bytes.len() as u64 != length {
        return Err(InstalledReleaseError::Manifest);
    }
    Ok(bytes)
}

fn pe_architecture(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 64 || bytes.get(..2) != Some(b"MZ") {
        return None;
    }
    let pe = u32::from_le_bytes(bytes.get(60..64)?.try_into().ok()?) as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return None;
    }
    match u16::from_le_bytes(bytes.get(pe + 4..pe + 6)?.try_into().ok()?) {
        0x8664 => Some("x64"),
        0xaa64 => Some("arm64"),
        _ => None,
    }
}

fn source_marker_prefix(kind: &[u8]) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(21 + kind.len());
    prefix.extend_from_slice(b"TALKING_QUILL_");
    prefix.extend_from_slice(b"SOURCE_");
    prefix.extend_from_slice(kind);
    prefix.push(b'=');
    prefix
}

fn has_source_identity(bytes: &[u8], commit: &str, tree: &str) -> bool {
    [(b"COMMIT".as_slice(), commit), (b"TREE".as_slice(), tree)]
        .iter()
        .all(|(kind, expected)| {
            let prefix = source_marker_prefix(kind);
            let offsets: Vec<usize> = bytes
                .windows(prefix.len())
                .enumerate()
                .filter(|(_, window)| *window == prefix)
                .map(|(offset, _)| offset)
                .collect();
            offsets.len() == 1
                && bytes.get(offsets[0] + prefix.len()..offsets[0] + prefix.len() + 40)
                    == Some(expected.as_bytes())
        })
}

pub fn protected_policy_proof(binding: &StablePipeBinding) -> Vec<u8> {
    let mut proof = Vec::with_capacity(136);
    proof.extend_from_slice(b"TQKOWPR1");
    proof.extend_from_slice(binding.manifest_sha256.as_bytes());
    proof.extend_from_slice(binding.release_policy_digest.as_bytes());
    proof.extend_from_slice(binding.release_build_digest.as_bytes());
    proof.extend_from_slice(&binding.credential_binding_digest);
    proof
}

fn canonical_package_layout(manifest: &ReleaseManifest) -> Result<[u8; 32], InstalledReleaseError> {
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

fn hash_layout_field(
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

fn valid_source_identity(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_hex32(value: &str) -> Result<[u8; 32], InstalledReleaseError> {
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

fn path_eq(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

fn role_digest(role: u8, image: [u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"TQKO-WINDOWS-LOCAL-ROLE-V1\0");
    hash.update([role]);
    hash.update(image);
    hash.finalize().into()
}

fn encode_policy(
    architecture: WindowsArchitecture,
    release: [u8; 32],
    gateway: [u8; 32],
    owner: [u8; 32],
    gateway_role: [u8; 32],
    owner_role: [u8; 32],
    predecessor: Option<([u8; 32], [u8; 32], [u8; 32])>,
) -> [u8; 328] {
    let mut bytes = [0_u8; 328];
    bytes[..8].copy_from_slice(b"TQKOPOL1");
    bytes[8..10].copy_from_slice(&1_u16.to_be_bytes());
    bytes[10] = 1;
    bytes[11] = match architecture {
        WindowsArchitecture::X64 => 1,
        WindowsArchitecture::Arm64 => 2,
    };
    bytes[12] = 2;
    bytes[13] = u8::from(predecessor.is_some());
    bytes[16..48].copy_from_slice(&release);
    bytes[48..80].copy_from_slice(&gateway);
    bytes[80..112].copy_from_slice(&owner);
    bytes[112..144].copy_from_slice(&gateway_role);
    bytes[144..176].copy_from_slice(&owner_role);
    for offset in [176, 200] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_be_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&1_u32.to_be_bytes());
        bytes[offset + 8..offset + 16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[offset + 16..offset + 24].copy_from_slice(&1_u64.to_be_bytes());
    }
    if let Some((release, gateway, owner)) = predecessor {
        bytes[224..256].copy_from_slice(&release);
        bytes[256..288].copy_from_slice(&gateway);
        bytes[288..320].copy_from_slice(&owner);
        bytes[320] = 1;
        bytes[321] = match architecture {
            WindowsArchitecture::X64 => 1,
            WindowsArchitecture::Arm64 => 2,
        };
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::{FileIdentity, SourceIdentity};

    fn facts(role: &str, pid: u32, image: u8) -> PeerFacts {
        PeerFacts {
            process_id: pid,
            creation_marker: u64::from(pid) * 10,
            wts_session_id: 4,
            user_sid: vec![1, 2, 3],
            logon_sid: vec![4, 5, 6],
            integrity_rid: 0x2000,
            architecture: WindowsArchitecture::X64,
            canonical_image: PathBuf::from(format!(
                r"C:\Program Files\Talking Quill\resources\helper\{role}"
            )),
            file_identity: FileIdentity {
                volume_serial: 7,
                file_index: u64::from(pid),
            },
            image_sha256: [image; 32],
            source_identity: Some(SourceIdentity {
                commit: "11".repeat(20),
                tree: "22".repeat(20),
            }),
        }
    }

    fn recovery_launcher(architecture: &str) -> Vec<u8> {
        let mut bytes = vec![0_u8; 256];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&128_u32.to_le_bytes());
        bytes[128..132].copy_from_slice(b"PE\0\0");
        let machine = if architecture == "arm64" {
            0xaa64_u16
        } else {
            0x8664_u16
        };
        bytes[132..134].copy_from_slice(&machine.to_le_bytes());
        bytes.extend_from_slice(&source_marker_prefix(b"COMMIT"));
        bytes.extend_from_slice("11".repeat(20).as_bytes());
        bytes.extend_from_slice(&source_marker_prefix(b"TREE"));
        bytes.extend_from_slice("22".repeat(20).as_bytes());
        bytes
    }

    fn manifest_with(
        gateway: u8,
        owner: u8,
        architecture: &str,
        predecessor: serde_json::Value,
    ) -> Vec<u8> {
        let launcher_sha256: [u8; 32] = Sha256::digest(recovery_launcher(architecture)).into();
        let mut value = serde_json::json!({
            "schemaVersion": 1,
            "kind": "talking-quill-local-owner-release",
            "version": "0.0.69",
            "platform": "win",
            "architecture": architecture,
            "ownerMode": "local-unsigned-enabled",
            "packageMode": if predecessor.is_null() { "fresh" } else { "update" },
            "sourceCommit": "11".repeat(20),
            "sourceTree": "22".repeat(20),
            "roles": [
                {"role":"gateway","path":"resources/helper/talking-quill-helper.exe","sha256":hex(&[gateway;32]),"suppressionCapable":false},
                {"role":"owner","path":"resources/helper/talking-quill-keyboard-owner.exe","sha256":hex(&[owner;32]),"suppressionCapable":true},
                {"role":"recovery-launcher","path":"resources/helper/talking-quill-update-recovery-launcher.exe","sha256":hex(&launcher_sha256),"suppressionCapable":false}
            ],
            "predecessor": predecessor,
            "releaseBuildDigest": hex(&[0;32]),
            "packageLayoutDigest": hex(&[0;32]),
            "update": {"channel":format!("latest-{architecture}"),"payload":"tqpkg2","companion":null,"transactionBinding":"source-target-package-sha256-v1","maintenanceInstaller":"native-setup"}
        });
        if predecessor.is_null() {
            value["freshInstall"] = true.into();
        }
        let parsed: ReleaseManifest = serde_json::from_value(value.clone()).unwrap();
        let digest = hex(&canonical_package_layout(&parsed).unwrap());
        value["releaseBuildDigest"] = digest.clone().into();
        value["packageLayoutDigest"] = digest.into();
        serde_json::to_vec(&value).unwrap()
    }

    fn manifest(gateway: u8, owner: u8) -> Vec<u8> {
        manifest_with(gateway, owner, "x64", serde_json::Value::Null)
    }

    fn recovery_launcher_for(gateway: &PeerFacts) -> Vec<u8> {
        recovery_launcher(match gateway.architecture {
            WindowsArchitecture::X64 => "x64",
            WindowsArchitecture::Arm64 => "arm64",
        })
    }

    fn hex(value: &[u8; 32]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn installer_manifest_binds_roles_release_and_layout() {
        let gateway = facts("talking-quill-helper.exe", 10, 1);
        let owner = facts("talking-quill-keyboard-owner.exe", 11, 2);
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        let release = InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &manifest(1, 2),
            &recovery_launcher_for(&gateway),
        )
        .unwrap();
        let parsed: ReleaseManifest = serde_json::from_slice(&manifest(1, 2)).unwrap();
        let canonical = canonical_package_layout(&parsed).unwrap();
        assert_eq!(release.binding.release_build_digest.as_bytes(), &canonical);
        assert_eq!(&release.policy[16..48], &canonical);
        assert_eq!(release.binding.installation_id, hex(&canonical));
    }

    #[test]
    fn windows_policy_binds_the_allowed_predecessor() {
        let gateway = facts("talking-quill-helper.exe", 10, 1);
        let owner = facts("talking-quill-keyboard-owner.exe", 11, 2);
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        let bytes = manifest_with(
            1,
            2,
            "x64",
            serde_json::json!({
                "platform":"win",
                "architecture":"x64",
                "version":"0.0.66",
                "releaseBuildDigest":hex(&[5; 32]),
                "gatewaySha256":hex(&[6; 32]),
                "ownerSha256":hex(&[7; 32])
            }),
        );
        let release = InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &bytes,
            &recovery_launcher_for(&gateway),
        )
        .unwrap();
        assert_eq!(release.policy[13], 1);
        assert_eq!(&release.policy[224..256], &[5; 32]);
        assert_eq!(&release.policy[256..288], &[6; 32]);
        assert_eq!(&release.policy[288..320], &[7; 32]);
    }

    #[test]
    fn arm64_manifest_and_policy_are_native_and_proof_is_not_fake_der() {
        let mut gateway = facts("talking-quill-helper.exe", 12, 1);
        let mut owner = facts("talking-quill-keyboard-owner.exe", 13, 2);
        gateway.architecture = WindowsArchitecture::Arm64;
        owner.architecture = WindowsArchitecture::Arm64;
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        let bytes = manifest_with(1, 2, "arm64", serde_json::Value::Null);
        let release = InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &bytes,
            &recovery_launcher_for(&gateway),
        )
        .unwrap();
        assert_eq!(release.policy[11], 2);
        let proof = protected_policy_proof(&release.binding);
        assert_eq!(&proof[..8], b"TQKOWPR1");
        assert_ne!(proof[0], 0x30);
    }

    #[test]
    fn installed_manifest_binds_source_provenance() {
        let gateway = facts("talking-quill-helper.exe", 16, 1);
        let owner = facts("talking-quill-keyboard-owner.exe", 17, 2);
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        let mut value: serde_json::Value = serde_json::from_slice(&manifest(1, 2)).unwrap();
        value["sourceCommit"] = "33".repeat(20).into();
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            InstalledRelease::from_manifest_bytes(
                &gateway,
                &owner,
                ChannelPurpose::Capture,
                resources,
                &bytes,
                &recovery_launcher_for(&gateway),
            )
            .is_err()
        );
    }

    #[test]
    fn installed_layout_digest_is_recomputed_instead_of_trusted() {
        let gateway = facts("talking-quill-helper.exe", 18, 1);
        let owner = facts("talking-quill-keyboard-owner.exe", 19, 2);
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        let bytes = String::from_utf8(manifest(1, 2))
            .unwrap()
            .replace(
                &hex(&canonical_package_layout(
                    &serde_json::from_slice::<ReleaseManifest>(&manifest(1, 2)).unwrap(),
                )
                .unwrap()),
                &hex(&[9; 32]),
            )
            .into_bytes();
        assert!(
            InstalledRelease::from_manifest_bytes(
                &gateway,
                &owner,
                ChannelPurpose::Capture,
                resources,
                &bytes,
                &recovery_launcher_for(&gateway),
            )
            .is_err()
        );
    }

    #[test]
    fn wrong_hash_role_or_architecture_fails_closed() {
        let gateway = facts("talking-quill-helper.exe", 20, 3);
        let mut owner = facts("talking-quill-keyboard-owner.exe", 21, 4);
        let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
        assert!(
            InstalledRelease::from_manifest_bytes(
                &gateway,
                &owner,
                ChannelPurpose::Capture,
                resources,
                &manifest(3, 5),
                &recovery_launcher_for(&gateway),
            )
            .is_err()
        );
        owner.architecture = WindowsArchitecture::Arm64;
        assert!(
            InstalledRelease::from_manifest_bytes(
                &gateway,
                &owner,
                ChannelPurpose::Capture,
                resources,
                &manifest(3, 4),
                &recovery_launcher_for(&gateway),
            )
            .is_err()
        );
    }
}
