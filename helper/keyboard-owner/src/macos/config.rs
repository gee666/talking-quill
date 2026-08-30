#![cfg(target_os = "macos")]

use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::Deserialize;
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};
use talking_quill_owner_protocol::schema::Architecture;

use super::runtime_directory::MacosRuntimeDirectory;
use super::{
    GATEWAY_SIGNING_IDENTIFIER, LocalSigningIdentity, MacosEndpointError, MacosPeerPolicy,
    OWNER_SIGNING_IDENTIFIER, RequirementHash, RolePolicy, ad_hoc_designated_requirement,
    self_signed_designated_requirement,
};
use super::{KeychainPolicy, KeychainSharingMode};

const MAX_VALIDATED_CONFIG_BYTES: usize = 64 * 1024;

pub struct MacosEndpointConfig {
    pub runtime_directory: MacosRuntimeDirectory,
    pub peer_policy: MacosPeerPolicy,
    pub keychain_policy: KeychainPolicy,
    pub bridge_policy: RolePolicy,
    pub installation_identity_digest: Bytes32,
    pub gateway_release_policy: PolicyBlob,
    pub gateway_release_policy_signature: PolicySignature,
    pub owner_release_policy: PolicyBlob,
    pub owner_release_policy_signature: PolicySignature,
    pub bridge_release_policy: PolicyBlob,
    pub bridge_release_policy_signature: PolicySignature,
    pub architecture: Architecture,
    pub outer_bundle_path: PathBuf,
    pub(crate) validated_owner_executable: Option<OwnedFd>,
    pub(crate) validated_resource_bytes: Vec<u8>,
}

impl std::fmt::Debug for MacosEndpointConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MacosEndpointConfig(<redacted>)")
    }
}

impl MacosEndpointConfig {
    /// Loads the fixed sealed resource. The release-policy signer pin is
    /// compiled into the owner; it is never supplied by the sidecar itself.
    pub fn load_for_current_install() -> Result<Self, MacosEndpointError> {
        Self::load_for_current_role(super::PeerRole::Owner)
    }

    pub fn load_for_current_gateway_install() -> Result<Self, MacosEndpointError> {
        Self::load_for_current_role(super::PeerRole::Gateway)
    }

    fn load_for_current_role(role: super::PeerRole) -> Result<Self, MacosEndpointError> {
        super::native_identity::validate_current_process_code()?;
        let executable = std::env::current_exe().map_err(|_| MacosEndpointError::Configuration)?;
        let macos = executable
            .parent()
            .ok_or(MacosEndpointError::Configuration)?;
        let contents = macos.parent().ok_or(MacosEndpointError::Configuration)?;
        if macos.file_name().and_then(|value| value.to_str()) != Some("MacOS")
            || contents.file_name().and_then(|value| value.to_str()) != Some("Contents")
        {
            return Err(MacosEndpointError::Configuration);
        }
        let outer_bundle_path = match role {
            super::PeerRole::Owner => {
                let owner_app = contents.parent().ok_or(MacosEndpointError::Configuration)?;
                let login_items = owner_app
                    .parent()
                    .ok_or(MacosEndpointError::Configuration)?;
                let library = login_items
                    .parent()
                    .ok_or(MacosEndpointError::Configuration)?;
                let outer_contents = library.parent().ok_or(MacosEndpointError::Configuration)?;
                let outer = outer_contents
                    .parent()
                    .ok_or(MacosEndpointError::Configuration)?;
                if owner_app.file_name().and_then(|value| value.to_str())
                    != Some("Talking Quill Keyboard Owner.app")
                    || login_items.file_name().and_then(|value| value.to_str())
                        != Some("LoginItems")
                    || library.file_name().and_then(|value| value.to_str()) != Some("Library")
                    || outer_contents.file_name().and_then(|value| value.to_str())
                        != Some("Contents")
                    || outer.extension().and_then(|value| value.to_str()) != Some("app")
                {
                    return Err(MacosEndpointError::Configuration);
                }
                outer.to_path_buf()
            }
            super::PeerRole::Gateway => contents
                .parent()
                .map(PathBuf::from)
                .ok_or(MacosEndpointError::Configuration)?,
        };
        let sidecar = outer_bundle_path.join("Contents/Resources/keyboard-owner-r5m.json");
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(sidecar)
            .map_err(|_| MacosEndpointError::Configuration)?;
        let before = file_stat(file.as_raw_fd())?;
        if (before.st_mode & libc::S_IFMT) != libc::S_IFREG || before.st_nlink != 1 {
            return Err(MacosEndpointError::Configuration);
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_VALIDATED_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| MacosEndpointError::Configuration)?;
        if bytes.is_empty() || bytes.len() > MAX_VALIDATED_CONFIG_BYTES {
            return Err(MacosEndpointError::Configuration);
        }
        let after = file_stat(file.as_raw_fd())?;
        if before.st_dev != after.st_dev
            || before.st_ino != after.st_ino
            || before.st_size != after.st_size
        {
            return Err(MacosEndpointError::Configuration);
        }
        // Revalidate running code around the no-follow stable read. The outer
        // policy bytes are independently authenticated below by detached CMS;
        // keeping them outside the nested bundle avoids a CodeDirectory cycle.
        super::native_identity::validate_current_process_code()?;
        let wire: WireConfig =
            serde_json::from_slice(&bytes).map_err(|_| MacosEndpointError::Configuration)?;
        let mut config = Self::from_wire(wire)?;
        config.outer_bundle_path = outer_bundle_path;
        let expected = config.peer_policy.role(role);
        if expected.canonical_executable_path
            != executable
                .canonicalize()
                .map_err(|_| MacosEndpointError::Configuration)?
        {
            return Err(MacosEndpointError::Configuration);
        }
        let expected_gateway = config
            .outer_bundle_path
            .join("Contents/Resources/helper/talking-quill-helper");
        let expected_owner = config.outer_bundle_path.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner");
        let expected_bridge = config
            .outer_bundle_path
            .join("Contents/MacOS/talking-quill-macos-service-bridge");
        if config.peer_policy.gateway.canonical_executable_path != expected_gateway
            || config.peer_policy.owner.canonical_executable_path != expected_owner
            || config.bridge_policy.canonical_executable_path != expected_bridge
        {
            return Err(MacosEndpointError::Configuration);
        }
        let executable =
            super::native_identity::validate_current_process_against_retaining_executable(
                expected,
                config.peer_policy.release_build_digest,
            )?;
        if role == super::PeerRole::Owner {
            config.validated_owner_executable = Some(executable);
        }
        config.validated_resource_bytes = bytes;
        Ok(config)
    }

    pub fn reconcile_startup_maintenance(&self) -> Result<(), MacosEndpointError> {
        let record = super::read_maintenance_record_without_ui()?;
        let policy = self
            .owner_release_policy
            .decode()
            .map_err(|_| MacosEndpointError::Configuration)?;
        if startup_record_requires_clear(
            record,
            self.peer_policy.release_build_digest,
            self.peer_policy.owner.executable_sha256,
            &policy,
        )? {
            super::clear_maintenance_record_without_ui()?;
        }
        Ok(())
    }

    pub(crate) fn load_broker_policy_from_validated_bytes(
        bytes: &[u8],
    ) -> Result<Self, MacosEndpointError> {
        if bytes.is_empty() || bytes.len() > MAX_VALIDATED_CONFIG_BYTES {
            return Err(MacosEndpointError::Configuration);
        }
        let wire: WireConfig =
            serde_json::from_slice(bytes).map_err(|_| MacosEndpointError::Configuration)?;
        Self::from_wire(wire)
    }

    fn from_wire(wire: WireConfig) -> Result<Self, MacosEndpointError> {
        let runtime_directory = MacosRuntimeDirectory::for_current_audit_session()?;
        let release_build_digest = bytes32(&wire.release_build_digest)?;
        let release_policy_signer = compiled_release_policy_signer()?;
        super::native_cms::verify_release_policy_cms(
            &wire.gateway_release_policy,
            &wire.gateway_release_policy_signature,
            release_policy_signer,
        )?;
        super::native_cms::verify_release_policy_cms(
            &wire.owner_release_policy,
            &wire.owner_release_policy_signature,
            release_policy_signer,
        )?;
        super::native_cms::verify_release_policy_cms(
            &wire.bridge_release_policy,
            &wire.bridge_release_policy_signature,
            release_policy_signer,
        )?;
        let gateway = wire.gateway.into_role(GATEWAY_SIGNING_IDENTIFIER, true)?;
        let owner = wire.owner.into_role(OWNER_SIGNING_IDENTIFIER, true)?;
        let bridge = wire
            .bridge
            .into_role("com.talkingquill.app.service-management", false)?;
        let peer_policy = MacosPeerPolicy {
            console_uid: runtime_directory.uid(),
            audit_session_id: runtime_directory.audit_session_id(),
            release_build_digest,
            gateway,
            owner,
        };
        peer_policy.validate()?;
        let keychain_policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::RequirementAcl)?;
        let architecture = match std::env::consts::ARCH {
            "x86_64" => Architecture::X64,
            "aarch64" => Architecture::Arm64,
            _ => return Err(MacosEndpointError::Configuration),
        };
        let gateway_decoded = wire
            .gateway_release_policy
            .decode()
            .map_err(|_| MacosEndpointError::Configuration)?;
        let owner_decoded = wire
            .owner_release_policy
            .decode()
            .map_err(|_| MacosEndpointError::Configuration)?;
        let bridge_decoded = wire
            .bridge_release_policy
            .decode()
            .map_err(|_| MacosEndpointError::Configuration)?;
        if wire.gateway_release_policy != wire.owner_release_policy
            || gateway_decoded != owner_decoded
            || gateway_decoded.platform != talking_quill_owner_protocol::schema::Platform::Macos
            || owner_decoded.platform != talking_quill_owner_protocol::schema::Platform::Macos
            || gateway_decoded.architecture != architecture
            || owner_decoded.architecture != architecture
            || gateway_decoded.release_build_digest != release_build_digest
            || owner_decoded.release_build_digest != release_build_digest
            || gateway_decoded.gateway_sha256 != peer_policy.gateway.executable_sha256
            || gateway_decoded.owner_sha256 != peer_policy.owner.executable_sha256
            || owner_decoded.gateway_sha256 != peer_policy.gateway.executable_sha256
            || owner_decoded.owner_sha256 != peer_policy.owner.executable_sha256
            || gateway_decoded.gateway_signer_policy_digest
                != super::requirement_digest(&peer_policy.gateway.designated_requirement)
            || gateway_decoded.owner_signer_policy_digest
                != super::requirement_digest(&peer_policy.owner.designated_requirement)
            || owner_decoded.gateway_signer_policy_digest
                != super::requirement_digest(&peer_policy.gateway.designated_requirement)
            || owner_decoded.owner_signer_policy_digest
                != super::requirement_digest(&peer_policy.owner.designated_requirement)
            || gateway_decoded.gateway_protocol != expected_protocol_header()
            || gateway_decoded.owner_protocol != expected_protocol_header()
            || owner_decoded.gateway_protocol != expected_protocol_header()
            || owner_decoded.owner_protocol != expected_protocol_header()
            || bridge_decoded.architecture != architecture
            || bridge_decoded.release_build_digest != release_build_digest
            || bridge_decoded.gateway_sha256 != bridge.executable_sha256
            || bridge_decoded.owner_sha256 != bridge.executable_sha256
            || bridge_decoded.gateway_signer_policy_digest
                != super::requirement_digest(&bridge.designated_requirement)
            || bridge_decoded.owner_signer_policy_digest
                != super::requirement_digest(&bridge.designated_requirement)
            || bridge_decoded.predecessor.is_some()
        {
            return Err(MacosEndpointError::Configuration);
        }
        Ok(Self {
            runtime_directory,
            peer_policy,
            keychain_policy,
            bridge_policy: bridge,
            installation_identity_digest: bytes32(&wire.installation_identity_digest)?,
            gateway_release_policy: wire.gateway_release_policy,
            gateway_release_policy_signature: wire.gateway_release_policy_signature,
            owner_release_policy: wire.owner_release_policy,
            owner_release_policy_signature: wire.owner_release_policy_signature,
            bridge_release_policy: wire.bridge_release_policy,
            bridge_release_policy_signature: wire.bridge_release_policy_signature,
            architecture,
            outer_bundle_path: PathBuf::new(),
            validated_owner_executable: None,
            validated_resource_bytes: Vec::new(),
        })
    }
}

fn startup_record_requires_clear(
    record: Option<talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord>,
    release_build: Bytes32,
    owner_sha256: Bytes32,
    policy: &talking_quill_owner_protocol::release_policy::ReleasePolicy,
) -> Result<bool, MacosEndpointError> {
    use talking_quill_owner_protocol::macos_maintenance::{
        MacosMaintenanceOperation, MacosMaintenancePhase,
    };
    let Some(record) = record else {
        return Ok(false);
    };
    match record.phase {
        MacosMaintenancePhase::InProgress => Err(MacosEndpointError::Configuration),
        MacosMaintenancePhase::InstallationComplete => {
            let predecessor = policy
                .predecessor
                .as_ref()
                .ok_or(MacosEndpointError::Configuration)?;
            if !matches!(
                record.operation,
                MacosMaintenanceOperation::Update | MacosMaintenanceOperation::Rollback
            ) || record.target_build != Some(release_build)
                || record.target_owner != Some(owner_sha256)
                || predecessor.release_build_digest != record.source_build
                || policy.release_build_digest != release_build
                || policy.owner_sha256 != owner_sha256
            {
                Err(MacosEndpointError::Configuration)
            } else {
                Ok(true)
            }
        }
        MacosMaintenancePhase::RolledBack => {
            if record.source_build == release_build {
                Ok(true)
            } else {
                Err(MacosEndpointError::Configuration)
            }
        }
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
    fn into_role(
        self,
        identifier: &str,
        capture_authorized: bool,
    ) -> Result<RolePolicy, MacosEndpointError> {
        let (signing_identity, code_directory_hash, designated_requirement) = match self.signing {
            WireSigning::AdHoc { cdhash } => {
                let hash = requirement_hash(&cdhash)?;
                (
                    LocalSigningIdentity::AdHoc {
                        code_directory_hash: hash,
                    },
                    hash,
                    ad_hoc_designated_requirement(identifier, hash),
                )
            }
            WireSigning::LocallyTrustedSelfSigned {
                certificate_sha256,
                certificate_requirement_hash,
                cdhash,
            } => {
                let hash = requirement_hash(&certificate_requirement_hash)?;
                (
                    LocalSigningIdentity::LocallyTrustedSelfSigned {
                        certificate_sha256: bytes32(&certificate_sha256)?,
                        requirement_certificate_hash: hash,
                    },
                    requirement_hash(&cdhash)?,
                    self_signed_designated_requirement(identifier, hash),
                )
            }
        };
        if self.capture_authorized != capture_authorized {
            return Err(MacosEndpointError::Configuration);
        }
        let canonical_executable_path =
            validate_absolute_normal_path(self.canonical_executable_path)?;
        Ok(RolePolicy {
            signing_identifier: identifier.to_owned(),
            designated_requirement,
            canonical_executable_path,
            executable_sha256: bytes32(&self.executable_sha256)?,
            code_directory_hash,
            signing_identity,
            capture_authorized: self.capture_authorized,
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

fn validate_absolute_normal_path(path: PathBuf) -> Result<PathBuf, MacosEndpointError> {
    use std::path::Component;
    let encoded = path.to_string_lossy();
    if !path.is_absolute()
        || encoded
            .split('/')
            .any(|component| matches!(component, "." | ".."))
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(MacosEndpointError::Configuration);
    }
    Ok(path)
}

fn expected_protocol_header() -> talking_quill_owner_protocol::schema::ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

fn compiled_release_policy_signer() -> Result<Bytes32, MacosEndpointError> {
    let value = option_env!("TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256")
        .ok_or(MacosEndpointError::Configuration)?;
    bytes32(value)
}

fn file_stat(fd: libc::c_int) -> Result<libc::stat, MacosEndpointError> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        Err(MacosEndpointError::Configuration)
    } else {
        Ok(stat)
    }
}

fn bytes32(value: &str) -> Result<Bytes32, MacosEndpointError> {
    Ok(Bytes32::new(decode_hex::<32>(value)?))
}

fn requirement_hash(value: &str) -> Result<RequirementHash, MacosEndpointError> {
    Ok(RequirementHash::new(decode_hex::<20>(value)?))
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], MacosEndpointError> {
    if value.len() != N * 2 {
        return Err(MacosEndpointError::Configuration);
    }
    let mut bytes = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(bytes)
}

fn nibble(value: u8) -> Result<u8, MacosEndpointError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(MacosEndpointError::Configuration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talking_quill_owner_protocol::macos_maintenance::{
        MacosMaintenanceOperation, MacosMaintenancePhase, MacosMaintenanceRecord,
    };
    use talking_quill_owner_protocol::release_policy::{
        OwnerMode, ReleasePolicy, ReleasePolicyPredecessor,
    };
    use talking_quill_owner_protocol::schema::{Architecture, Platform, ProtocolHeader};

    fn bytes(value: u8) -> Bytes32 {
        Bytes32::new([value; 32])
    }
    fn policy() -> ReleasePolicy {
        let protocol = ProtocolHeader {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            supported_feature_bits: talking_quill_owner_protocol::FeatureBits::new(3),
            required_feature_bits: talking_quill_owner_protocol::FeatureBits::new(1),
        };
        ReleasePolicy {
            platform: Platform::Macos,
            architecture: Architecture::Arm64,
            owner_mode: OwnerMode::EnabledCandidate,
            release_build_digest: bytes(3),
            gateway_sha256: bytes(4),
            owner_sha256: bytes(5),
            gateway_signer_policy_digest: bytes(6),
            owner_signer_policy_digest: bytes(7),
            gateway_protocol: protocol,
            owner_protocol: protocol,
            predecessor: Some(ReleasePolicyPredecessor {
                release_build_digest: bytes(2),
                gateway_sha256: bytes(8),
                owner_sha256: bytes(9),
                platform: Platform::Macos,
                architecture: Architecture::Arm64,
            }),
        }
    }
    fn record(phase: MacosMaintenancePhase) -> MacosMaintenanceRecord {
        MacosMaintenanceRecord::in_progress(
            MacosMaintenanceOperation::Update,
            bytes(1),
            bytes(2),
            Some(bytes(3)),
            Some(bytes(5)),
            bytes(10),
        )
        .unwrap()
        .with_phase(phase)
    }

    #[test]
    fn startup_denies_in_progress_and_clears_only_exact_completion() {
        let policy = policy();
        assert!(
            startup_record_requires_clear(
                Some(record(MacosMaintenancePhase::InProgress)),
                bytes(3),
                bytes(5),
                &policy
            )
            .is_err()
        );
        assert!(
            startup_record_requires_clear(
                Some(record(MacosMaintenancePhase::InstallationComplete)),
                bytes(3),
                bytes(5),
                &policy
            )
            .unwrap()
        );
        assert!(
            startup_record_requires_clear(
                Some(record(MacosMaintenancePhase::InstallationComplete)),
                bytes(11),
                bytes(5),
                &policy
            )
            .is_err()
        );
        assert!(!startup_record_requires_clear(None, bytes(3), bytes(5), &policy).unwrap());
    }

    #[test]
    fn rolled_back_record_must_name_the_restored_source() {
        let policy = policy();
        assert!(
            startup_record_requires_clear(
                Some(record(MacosMaintenancePhase::RolledBack)),
                bytes(2),
                bytes(9),
                &policy
            )
            .unwrap()
        );
        assert!(
            startup_record_requires_clear(
                Some(record(MacosMaintenancePhase::RolledBack)),
                bytes(3),
                bytes(5),
                &policy
            )
            .is_err()
        );
    }
}
