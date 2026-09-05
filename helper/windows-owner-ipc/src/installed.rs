#![cfg(windows)]

use std::path::Path;

mod files;
mod manifest;
mod policy;

use files::{has_source_identity, pe_architecture, read_locked_file, read_locked_manifest};
use manifest::{ReleaseManifest, canonical_package_layout, parse_hex32, valid_source_identity};
pub use policy::protected_policy_proof;
use policy::{encode_policy, role_digest};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::channel::{ChannelPurpose, SessionBinding, StablePipeBinding};
use crate::image_policy::{Sha256Digest, WindowsArchitecture};
use crate::peer::{PeerFacts, peer_facts_match_session};

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

fn path_eq(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests;
