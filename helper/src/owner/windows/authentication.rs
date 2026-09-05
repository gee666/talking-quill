//! Installed binding trust and authenticated gateway handshake.

use std::time::{Duration, Instant};

use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::auth::{
    EphemeralP256Secret, HandshakeTrustVerifier, KeyAgreementMaterial, PeerRole,
};
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature, ReleasePolicy};
use talking_quill_owner_protocol::schema::{
    Architecture, Challenge, Hello, Platform, ProtocolHeader, Purpose,
};
use talking_quill_windows_owner_ipc::channel::{ChannelPurpose, StablePipeBinding};
use talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture;

use super::super::client::{ConnectError, ConnectedOwner};
use super::super::handshake::{GatewayHandshakeError, authenticate_gateway};
use super::{AuthenticatedLocalOwner, LocalChildStream};

struct ExactStablePipeTrust<'a> {
    binding: &'a StablePipeBinding,
}
impl HandshakeTrustVerifier for ExactStablePipeTrust<'_> {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &ReleasePolicy,
        owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        let b = self.binding;
        let expected_policy_proof =
            talking_quill_windows_owner_ipc::installed::protected_policy_proof(b);
        if hello.platform != Platform::Windows
            || challenge.platform != Platform::Windows
            || hello.purpose != purpose(b.purpose)
            || challenge.purpose != purpose(b.purpose)
            || hello.architecture != architecture(b.architecture)
            || challenge.architecture != architecture(b.architecture)
            || hello.executable_sha256 != digest(b.gateway_sha256.as_bytes())
            || challenge.executable_sha256 != digest(b.owner_sha256.as_bytes())
            || hello.release_build_digest != digest(b.release_build_digest.as_bytes())
            || challenge.release_build_digest != digest(b.release_build_digest.as_bytes())
            || hello.installation_identity_digest != install_digest(&b.installation_id)
            || challenge.installation_identity_digest != install_digest(&b.installation_id)
            || hello.signer_policy_digest != digest(b.gateway_role_digest.as_bytes())
            || challenge.signer_policy_digest != digest(b.owner_role_digest.as_bytes())
            || hello.client_release_policy_digest != digest(b.release_policy_digest.as_bytes())
            || challenge.owner_release_policy_digest != digest(b.release_policy_digest.as_bytes())
            || hello.client_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || challenge.owner_release_policy_signature.as_proof_bytes() != expected_policy_proof
            || hello.os_session_binding_digest != session_digest(b)
            || challenge.os_session_binding_digest != session_digest(b)
            || hello.platform_credential_binding_digest != Bytes32::new(b.credential_binding_digest)
            || challenge.platform_credential_binding_digest
                != Bytes32::new(b.credential_binding_digest)
            || client_policy.gateway_sha256 != hello.executable_sha256
            || owner_policy.owner_sha256 != challenge.executable_sha256
            || client_policy != owner_policy
        {
            return Err(talking_quill_owner_protocol::AuthenticationError::Trust);
        }
        Ok(())
    }
}

fn protocol_header() -> ProtocolHeader {
    talking_quill_owner_protocol::production_v1_protocol_header()
}
fn purpose(value: ChannelPurpose) -> Purpose {
    match value {
        ChannelPurpose::Observe => Purpose::Observe,
        ChannelPurpose::Capture => Purpose::Capture,
    }
}
fn architecture(value: WindowsArchitecture) -> Architecture {
    match value {
        WindowsArchitecture::X64 => Architecture::X64,
        WindowsArchitecture::Arm64 => Architecture::Arm64,
    }
}
fn digest(value: &[u8; 32]) -> Bytes32 {
    Bytes32::new(*value)
}
fn install_digest(value: &str) -> Bytes32 {
    domain_digest(b"", value)
}
fn domain_digest(domain: &[u8], value: &str) -> Bytes32 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(value.as_bytes());
    Bytes32::new(hash.finalize().into())
}
#[cfg(feature = "windows-installed-acceptance")]
fn lower_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn session_digest(binding: &StablePipeBinding) -> Bytes32 {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"TQKO-WINDOWS-SESSION-V1\0");
    hash.update(binding.session.user_sid_digest);
    hash.update(binding.session.logon_sid_digest);
    hash.update(binding.session.wts_session_id.to_be_bytes());
    hash.update(binding.session.integrity_rid.to_be_bytes());
    hash.update(binding.owner_integrity_rid.to_be_bytes());
    Bytes32::new(hash.finalize().into())
}

pub(super) fn authenticate_local_peer(
    binding: StablePipeBinding,
    release_policy: Vec<u8>,
    stream: LocalChildStream,
) -> Result<AuthenticatedLocalOwner, ConnectError> {
    let policy = PolicyBlob::from_bytes(
        release_policy
            .try_into()
            .map_err(|_| ConnectError::Authentication)?,
    )
    .map_err(|_| ConnectError::Authentication)?;
    let signature = PolicySignature::from_windows_manifest_proof(
        talking_quill_windows_owner_ipc::installed::protected_policy_proof(&binding),
    )
    .map_err(|_| ConnectError::Authentication)?;
    let ephemeral = EphemeralP256Secret::random().map_err(|_| ConnectError::Authentication)?;
    let hello = Hello::new(
        Purpose::Capture,
        protocol_header(),
        Bytes32::random().map_err(|_| ConnectError::Authentication)?,
        Platform::Windows,
        architecture(binding.architecture),
        digest(binding.release_build_digest.as_bytes()),
        digest(binding.gateway_sha256.as_bytes()),
        install_digest(&binding.installation_id),
        digest(binding.gateway_role_digest.as_bytes()),
        session_digest(&binding),
        policy,
        signature,
        Bytes32::new(binding.credential_binding_digest),
        Some(ephemeral.public_key().clone()),
    )
    .map_err(|_| ConnectError::Authentication)?;
    let verifier = ExactStablePipeTrust { binding: &binding };
    let client = authenticate_gateway(
        stream,
        hello,
        &verifier,
        |challenge| {
            let peer = challenge
                .owner_ephemeral_public_key
                .as_ref()
                .ok_or(GatewayHandshakeError::Authentication)?;
            KeyAgreementMaterial::windows_peer(PeerRole::Gateway, &ephemeral, peer)
                .map_err(|_| GatewayHandshakeError::Authentication)
        },
        Instant::now() + Duration::from_secs(3),
    )
    .map_err(handshake_connect_error)?;
    #[cfg(feature = "windows-installed-acceptance")]
    let acceptance_observability = crate::gateway::AcceptanceEndpointObservability {
        endpoint_version: 2,
        peer_authenticated: true,
        release_build_digest: lower_hex(binding.release_build_digest.as_bytes()),
        manifest_sha256: lower_hex(binding.manifest_sha256.as_bytes()),
        gateway: crate::gateway::AcceptanceEndpointPeerFacts {
            process_id: binding.gateway_process_id,
            creation_marker: binding.gateway_creation_marker.to_string(),
            integrity_rid: binding.session.integrity_rid,
            session_id: binding.session.wts_session_id,
            user_sid_hash: lower_hex(&binding.session.user_sid_digest),
        },
        owner: crate::gateway::AcceptanceEndpointPeerFacts {
            process_id: binding.owner_process_id,
            creation_marker: binding.owner_creation_marker.to_string(),
            integrity_rid: binding.owner_integrity_rid,
            session_id: binding.session.wts_session_id,
            user_sid_hash: lower_hex(&binding.session.user_sid_digest),
        },
    };
    Ok(AuthenticatedLocalOwner {
        connected: ConnectedOwner {
            client,
            build_id: binding.build_id,
        },
        #[cfg(feature = "windows-installed-acceptance")]
        acceptance_observability,
    })
}

fn handshake_connect_error(error: GatewayHandshakeError) -> ConnectError {
    match error {
        GatewayHandshakeError::Timeout
        | GatewayHandshakeError::PeerClosed
        | GatewayHandshakeError::Io
        | GatewayHandshakeError::Transport => ConnectError::Unavailable,
        GatewayHandshakeError::Framing
        | GatewayHandshakeError::Protocol
        | GatewayHandshakeError::Authentication => ConnectError::Authentication,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_handshake_is_retryable_but_invalid_identity_is_not() {
        for error in [
            GatewayHandshakeError::Timeout,
            GatewayHandshakeError::PeerClosed,
            GatewayHandshakeError::Io,
            GatewayHandshakeError::Transport,
        ] {
            assert!(matches!(
                handshake_connect_error(error),
                ConnectError::Unavailable
            ));
        }
        for error in [
            GatewayHandshakeError::Framing,
            GatewayHandshakeError::Protocol,
            GatewayHandshakeError::Authentication,
        ] {
            assert!(matches!(
                handshake_connect_error(error),
                ConnectError::Authentication
            ));
        }
    }
}
