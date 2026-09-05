use std::fs::File;
use std::io::Read;

use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::auth::HandshakeTrustVerifier;
use talking_quill_owner_protocol::release_policy::ReleasePolicy;
use talking_quill_owner_protocol::schema::{Challenge, Hello, Platform, ProtocolHeader};

use crate::owner::client::ConnectError;

use super::InstalledConfig;

pub(super) struct ExactOwnerTrust<'a> {
    pub(super) config: &'a InstalledConfig,
}
impl HandshakeTrustVerifier for ExactOwnerTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let expected = self
            .config
            .owner_release_policy
            .decode()
            .map_err(|_| talking_quill_owner_protocol::AuthenticationError::Trust)?;
        if challenge.platform != Platform::Macos
            || challenge.architecture != self.config.architecture
            || challenge.release_build_digest != self.config.release_build_digest
            || challenge.executable_sha256 != self.config.owner.executable_sha256
            || challenge.installation_identity_digest != self.config.installation_identity_digest
            || challenge.owner_release_policy != self.config.owner_release_policy
            || challenge.owner_release_policy_signature
                != self.config.owner_release_policy_signature
            || hello.platform_credential_binding_digest
                != challenge.platform_credential_binding_digest
            || owner_policy != &expected
            || client_policy.gateway_sha256 != self.config.gateway.executable_sha256
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

pub(super) fn validate_gateway_bytes(config: &InstalledConfig) -> Result<(), ConnectError> {
    let current = std::env::current_exe().map_err(|_| ConnectError::Authentication)?;
    let canonical = current
        .canonicalize()
        .map_err(|_| ConnectError::Authentication)?;
    if canonical != config.gateway.canonical_executable_path {
        return Err(ConnectError::Authentication);
    }
    let mut file = File::open(canonical).map_err(|_| ConnectError::Authentication)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ConnectError::Authentication)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if hash.finalize().as_slice() != config.gateway.executable_sha256.as_bytes() {
        return Err(ConnectError::Authentication);
    }
    Ok(())
}

pub(super) fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}

pub(super) fn audit_binding(token: [u32; 8], uid: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-token-binding/v1\0");
    for word in token {
        digest.update(word.to_be_bytes());
    }
    digest.update(uid.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}
pub(super) fn os_session_digest(uid: u32, audit: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-session/v1\0");
    digest.update(uid.to_be_bytes());
    digest.update(audit.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}
pub(super) fn requirement_digest(requirement: &str) -> Bytes32 {
    Bytes32::new(Sha256::digest(requirement.as_bytes()).into())
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
