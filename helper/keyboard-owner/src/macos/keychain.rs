use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::{
    AuthorizationPurpose, MacosPeerPolicy, PeerRole, VerifiedPeer, peer_policy_digest,
    requirement_digest,
};

pub const KEYCHAIN_SECRET_BYTES: usize = 32;
pub const KEYCHAIN_SERVICE: &str = "com.talkingquill.app.keyboard-owner";
pub const HANDSHAKE_SECRET_ACCOUNT: &str = "owner-ipc-v1";
pub const MAINTENANCE_LATCH_ACCOUNT: &str = "maintenance-latch-v1";
pub const MAX_MAINTENANCE_LATCH_BYTES: usize = 512;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct KeychainItem([u8; KEYCHAIN_SECRET_BYTES]);

impl KeychainItem {
    pub fn from_bytes(bytes: [u8; KEYCHAIN_SECRET_BYTES]) -> Result<Self, KeychainError> {
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(KeychainError::InvalidItem);
        }
        Ok(Self(bytes))
    }

    /// The secret may be exposed only to the bounded authenticated owner
    /// handshake. It must never enter argv, environment, files, IPC logs, or
    /// renderer-visible diagnostics.
    #[must_use]
    pub const fn expose_to_authenticated_handshake(&self) -> &[u8; KEYCHAIN_SECRET_BYTES] {
        &self.0
    }

    /// Transfers the secret into the protocol HKDF input. The protocol API
    /// zeroizes the returned array immediately after deriving session keys.
    #[must_use]
    pub fn into_authenticated_handshake_secret(mut self) -> [u8; KEYCHAIN_SECRET_BYTES] {
        std::mem::take(&mut self.0)
    }
}

impl std::fmt::Debug for KeychainItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KeychainItem(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeychainSharingMode {
    /// The item ACL contains only the exact enrolled gateway and owner
    /// designated requirements. This is preferred for a locally trusted
    /// self-signed identity and is also valid for exact ad-hoc requirements.
    RequirementAcl,
    /// Compatibility fallback for systems that cannot create the desired ACL
    /// without provisioning. Only the owner reads the item; an already
    /// audit-token/code-identity-verified gateway authenticates through the
    /// owner protocol without obtaining a generic Keychain capability.
    OwnerMediated,
}

#[derive(Clone, Eq, PartialEq)]
pub struct KeychainPolicy {
    sharing_mode: KeychainSharingMode,
    gateway_requirement_digest: talking_quill_owner_protocol::Bytes32,
    owner_requirement_digest: talking_quill_owner_protocol::Bytes32,
    peer_policy_digest: talking_quill_owner_protocol::Bytes32,
}

impl KeychainPolicy {
    pub fn from_peer_policy(
        peer_policy: &MacosPeerPolicy,
        sharing_mode: KeychainSharingMode,
    ) -> Result<Self, KeychainError> {
        peer_policy
            .validate()
            .map_err(|_| KeychainError::InvalidPolicy)?;
        Ok(Self {
            sharing_mode,
            gateway_requirement_digest: requirement_digest(
                &peer_policy.gateway.designated_requirement,
            ),
            owner_requirement_digest: requirement_digest(&peer_policy.owner.designated_requirement),
            peer_policy_digest: peer_policy_digest(peer_policy),
        })
    }

    fn permits_secret_read(&self, peer: &VerifiedPeer<'_>) -> bool {
        if peer.policy_digest() != self.peer_policy_digest
            || !matches!(
                peer.purpose(),
                AuthorizationPurpose::Capture | AuthorizationPurpose::Maintenance
            )
        {
            return false;
        }
        match peer.role() {
            PeerRole::Gateway => {
                self.sharing_mode == KeychainSharingMode::RequirementAcl
                    && peer.requirement_digest() == self.gateway_requirement_digest
            }
            PeerRole::Owner => peer.requirement_digest() == self.owner_requirement_digest,
        }
    }
}

impl std::fmt::Debug for KeychainPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KeychainPolicy(<redacted>)")
    }
}

/// Narrow OS adapter interface for fixed service/account queries. Native
/// implementations must set `kSecUseAuthenticationUIFail`; owner startup and
/// IPC processing must never display a Keychain prompt. Item creation is an
/// installer/explicit user setup operation, not an implicit runtime fallback.
pub trait KeychainStore {
    fn read_owner_handshake_secret_without_ui(&self) -> Result<KeychainItem, KeychainError>;
    fn write_maintenance_latch_without_ui(&mut self, value: &[u8]) -> Result<(), KeychainError>;
}

pub struct KeychainAccess<'a, S: KeychainStore> {
    store: &'a mut S,
    peer: &'a VerifiedPeer<'a>,
    policy: &'a KeychainPolicy,
}

impl<'a, S: KeychainStore> KeychainAccess<'a, S> {
    #[must_use]
    pub const fn new(
        store: &'a mut S,
        peer: &'a VerifiedPeer<'a>,
        policy: &'a KeychainPolicy,
    ) -> Self {
        Self {
            store,
            peer,
            policy,
        }
    }

    pub fn handshake_secret(&self) -> Result<KeychainItem, KeychainError> {
        if !self.policy.permits_secret_read(self.peer) {
            return Err(KeychainError::AccessDenied);
        }
        self.store.read_owner_handshake_secret_without_ui()
    }

    pub fn persist_maintenance_latch(&mut self, value: &[u8]) -> Result<(), KeychainError> {
        if self.peer.role() != PeerRole::Owner
            || self.peer.purpose() != AuthorizationPurpose::Maintenance
            || self.peer.policy_digest() != self.policy.peer_policy_digest
            || value.is_empty()
            || value.len() > MAX_MAINTENANCE_LATCH_BYTES
        {
            return Err(KeychainError::AccessDenied);
        }
        self.store.write_maintenance_latch_without_ui(value)
    }

    // Keychain deletion is intentionally absent. R8-M must add it only behind
    // an unforgeable token returned by the concrete successful SMAppService
    // unregister operation; an owner role or caller-supplied `Ok(())` is not
    // sufficient evidence.
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum KeychainError {
    #[error("Keychain access is denied")]
    AccessDenied,
    #[error("Keychain item is missing")]
    Missing,
    #[error("Keychain item has invalid contents")]
    InvalidItem,
    #[error("Keychain policy is invalid")]
    InvalidPolicy,
    #[error("Keychain operation failed closed without authentication UI")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use talking_quill_owner_protocol::Bytes32;

    use super::*;
    use crate::macos::{
        AuditToken, AuthorizationPurpose, CodeIdentity, GATEWAY_SIGNING_IDENTIFIER,
        LocalSigningIdentity, OWNER_SIGNING_IDENTIFIER, PeerCredentials, PeerEvidence,
        PeerEvidenceProvider, RequirementHash, RolePolicy, self_signed_designated_requirement,
    };

    fn bytes(value: u8) -> Bytes32 {
        Bytes32::new([value; 32])
    }

    fn role(identifier: &str, path: &str, marker: u8) -> RolePolicy {
        let certificate = bytes(8);
        let requirement_certificate_hash = RequirementHash::new([9; 20]);
        RolePolicy {
            signing_identifier: identifier.to_owned(),
            designated_requirement: self_signed_designated_requirement(
                identifier,
                requirement_certificate_hash,
            ),
            canonical_executable_path: if cfg!(windows) {
                PathBuf::from(format!("C:{}", path.replace('/', "\\")))
            } else {
                PathBuf::from(path)
            },
            executable_sha256: bytes(marker),
            code_directory_hash: RequirementHash::new([marker; 20]),
            signing_identity: LocalSigningIdentity::LocallyTrustedSelfSigned {
                certificate_sha256: certificate,
                requirement_certificate_hash,
            },
            capture_authorized: true,
        }
    }

    fn peer_policy() -> MacosPeerPolicy {
        MacosPeerPolicy {
            console_uid: 501,
            audit_session_id: 12,
            release_build_digest: bytes(3),
            gateway: role(GATEWAY_SIGNING_IDENTIFIER, "/Applications/TQ/gateway", 4),
            owner: role(OWNER_SIGNING_IDENTIFIER, "/Applications/TQ/owner", 5),
        }
    }

    struct Evidence {
        identity: CodeIdentity,
        expected_requirement: String,
    }

    impl PeerEvidenceProvider for Evidence {
        fn peer_credentials(&self) -> Result<PeerCredentials, crate::macos::IdentityError> {
            Ok(PeerCredentials { uid: 501, gid: 20 })
        }

        fn peer_pid(&self) -> Result<u32, crate::macos::IdentityError> {
            Ok(44)
        }

        fn peer_audit_token(&self) -> Result<AuditToken, crate::macos::IdentityError> {
            Ok(AuditToken::new([0, 501, 0, 0, 0, 44, 12, 0]))
        }

        fn sec_code_identity(
            &self,
            _token: AuditToken,
        ) -> Result<CodeIdentity, crate::macos::IdentityError> {
            Ok(self.identity.clone())
        }

        fn evaluate_designated_requirement(
            &self,
            _token: AuditToken,
            requirement: &str,
        ) -> Result<bool, crate::macos::IdentityError> {
            Ok(requirement == self.expected_requirement)
        }

        fn connection_binding(&self) -> Result<Bytes32, crate::macos::IdentityError> {
            Ok(bytes(91))
        }
    }

    fn verified(
        policy: &MacosPeerPolicy,
        role: PeerRole,
        purpose: AuthorizationPurpose,
    ) -> VerifiedPeer<'static> {
        let expected = policy.role(role);
        let evidence = Evidence {
            expected_requirement: expected.designated_requirement.clone(),
            identity: CodeIdentity {
                signing_identifier: expected.signing_identifier.clone(),
                canonical_executable_path: expected.canonical_executable_path.clone(),
                executable_sha256: expected.executable_sha256,
                code_directory_hash: expected.code_directory_hash,
                release_build_digest: policy.release_build_digest,
                signing_identity: expected.signing_identity,
                statically_valid: true,
            },
        };
        let evidence = Box::leak(Box::new(evidence));
        PeerEvidence::new(evidence)
            .verify(policy, role, purpose)
            .expect("verified test peer")
    }

    #[derive(Default)]
    struct FakeStore {
        latch: Vec<u8>,
    }

    impl KeychainStore for FakeStore {
        fn read_owner_handshake_secret_without_ui(&self) -> Result<KeychainItem, KeychainError> {
            KeychainItem::from_bytes([7; KEYCHAIN_SECRET_BYTES])
        }

        fn write_maintenance_latch_without_ui(
            &mut self,
            value: &[u8],
        ) -> Result<(), KeychainError> {
            self.latch = value.to_vec();
            Ok(())
        }
    }

    #[test]
    fn requirement_acl_allows_only_verified_gateway_and_owner_to_read() {
        let peer_policy = peer_policy();
        let policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::RequirementAcl)
                .expect("policy");
        for role in [PeerRole::Gateway, PeerRole::Owner] {
            let mut store = FakeStore::default();
            let peer = verified(&peer_policy, role, AuthorizationPurpose::Capture);
            let secret = KeychainAccess::new(&mut store, &peer, &policy)
                .handshake_secret()
                .expect("role-bound secret");
            assert_eq!(secret.expose_to_authenticated_handshake(), &[7; 32]);
        }
    }

    #[test]
    fn owner_mediated_fallback_never_releases_secret_to_gateway() {
        let peer_policy = peer_policy();
        let policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::OwnerMediated)
                .expect("policy");
        let mut store = FakeStore::default();
        let gateway = verified(
            &peer_policy,
            PeerRole::Gateway,
            AuthorizationPurpose::Capture,
        );
        assert!(matches!(
            KeychainAccess::new(&mut store, &gateway, &policy).handshake_secret(),
            Err(KeychainError::AccessDenied)
        ));
        let owner = verified(&peer_policy, PeerRole::Owner, AuthorizationPurpose::Capture);
        assert!(
            KeychainAccess::new(&mut store, &owner, &policy)
                .handshake_secret()
                .is_ok()
        );
    }

    #[test]
    fn gateway_cannot_mutate_latch() {
        let peer_policy = peer_policy();
        let policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::RequirementAcl)
                .expect("policy");
        let mut store = FakeStore::default();
        let gateway = verified(
            &peer_policy,
            PeerRole::Gateway,
            AuthorizationPurpose::Maintenance,
        );
        let mut access = KeychainAccess::new(&mut store, &gateway, &policy);
        assert_eq!(
            access.persist_maintenance_latch(b"transaction"),
            Err(KeychainError::AccessDenied)
        );
    }

    #[test]
    fn owner_latch_is_nonempty_and_bounded() {
        let peer_policy = peer_policy();
        let policy =
            KeychainPolicy::from_peer_policy(&peer_policy, KeychainSharingMode::RequirementAcl)
                .expect("policy");
        let mut store = FakeStore::default();
        let owner = verified(
            &peer_policy,
            PeerRole::Owner,
            AuthorizationPurpose::Maintenance,
        );
        let mut access = KeychainAccess::new(&mut store, &owner, &policy);
        assert_eq!(
            access.persist_maintenance_latch(&[]),
            Err(KeychainError::AccessDenied)
        );
        assert_eq!(
            access.persist_maintenance_latch(&[1; MAX_MAINTENANCE_LATCH_BYTES + 1]),
            Err(KeychainError::AccessDenied)
        );
        access
            .persist_maintenance_latch(b"bounded-latch")
            .expect("owner writes latch");
    }

    #[test]
    fn stale_verified_peer_cannot_be_replayed_against_new_enrollment() {
        let old_peer_policy = peer_policy();
        let gateway = verified(
            &old_peer_policy,
            PeerRole::Gateway,
            AuthorizationPurpose::Capture,
        );
        let mut new_peer_policy = old_peer_policy.clone();
        new_peer_policy.owner.executable_sha256 = bytes(99);
        let policy =
            KeychainPolicy::from_peer_policy(&new_peer_policy, KeychainSharingMode::RequirementAcl)
                .expect("new policy");
        let mut store = FakeStore::default();
        assert!(matches!(
            KeychainAccess::new(&mut store, &gateway, &policy).handshake_secret(),
            Err(KeychainError::AccessDenied)
        ));
    }

    #[test]
    fn all_zero_secret_is_rejected_and_debug_is_redacted() {
        assert!(matches!(
            KeychainItem::from_bytes([0; KEYCHAIN_SECRET_BYTES]),
            Err(KeychainError::InvalidItem)
        ));
        let secret = KeychainItem::from_bytes([1; KEYCHAIN_SECRET_BYTES]).expect("secret");
        assert_eq!(format!("{secret:?}"), "KeychainItem(<redacted>)");
    }
}
