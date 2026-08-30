use std::{fmt, marker::PhantomData, path::PathBuf};

#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;

use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use thiserror::Error;

pub const GATEWAY_SIGNING_IDENTIFIER: &str = "com.talkingquill.app.helper";
pub const OWNER_SIGNING_IDENTIFIER: &str = "com.talkingquill.app.keyboard-owner";
const MAX_REQUIREMENT_BYTES: usize = 2_048;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerRole {
    Gateway,
    Owner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationPurpose {
    Capture,
    Maintenance,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AuditToken([u32; 8]);

impl AuditToken {
    #[must_use]
    pub const fn new(words: [u32; 8]) -> Self {
        Self(words)
    }

    #[must_use]
    pub const fn effective_uid(self) -> u32 {
        self.0[1]
    }

    #[must_use]
    pub const fn pid(self) -> u32 {
        self.0[5]
    }

    #[must_use]
    pub const fn audit_session_id(self) -> u32 {
        self.0[6]
    }

    #[must_use]
    #[allow(dead_code)] // consumed by the R5-M native audit-token bridge
    pub(crate) const fn words(self) -> [u32; 8] {
        self.0
    }
}

impl fmt::Debug for AuditToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuditToken(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerCredentials {
    pub uid: u32,
    pub gid: u32,
}

/// The two local signing identities supported by personal macOS builds.
/// Neither variant implies Developer ID, an Apple Team ID, or notarization.
/// Hash form accepted by macOS's requirement language for `cdhash` and
/// certificate `H\"...\"` operands. It is intentionally distinct from the
/// SHA-256 enrollment hashes used for executable bytes and certificates.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RequirementHash([u8; 20]);

impl RequirementHash {
    #[must_use]
    pub const fn new(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl fmt::Debug for RequirementHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequirementHash(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum LocalSigningIdentity {
    /// An ad-hoc signature is bound to the exact CodeDirectory hash. Updating
    /// the executable changes this identity and may require the user to grant
    /// TCC permissions again.
    AdHoc {
        code_directory_hash: RequirementHash,
    },
    /// A locally generated certificate explicitly trusted by the user. SHA-256
    /// remains the enrollment identity; the SHA-1 form is retained solely for
    /// Security.framework's requirement-language certificate operand.
    LocallyTrustedSelfSigned {
        certificate_sha256: Bytes32,
        requirement_certificate_hash: RequirementHash,
    },
}

impl fmt::Debug for LocalSigningIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdHoc { .. } => formatter.write_str("LocalSigningIdentity::AdHoc(<redacted>)"),
            Self::LocallyTrustedSelfSigned { .. } => {
                formatter.write_str("LocalSigningIdentity::LocallyTrustedSelfSigned(<redacted>)")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TccPersistenceExpectation {
    /// Permissions can persist across replacements only while the signing
    /// certificate, identifier, bundle location, and designated requirement
    /// remain unchanged.
    StableLocalRequirement,
    /// A changed ad-hoc CodeDirectory is a changed identity. Startup must stay
    /// disabled until macOS reports the new identity is authorized.
    ReauthorizationAfterIdentityChange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TccAuthorization {
    Granted,
    NotDetermined,
    Denied,
}

#[derive(Clone, Eq, PartialEq)]
pub struct CodeIdentity {
    pub signing_identifier: String,
    pub canonical_executable_path: PathBuf,
    pub executable_sha256: Bytes32,
    pub code_directory_hash: RequirementHash,
    pub release_build_digest: Bytes32,
    pub signing_identity: LocalSigningIdentity,
    pub statically_valid: bool,
}

impl fmt::Debug for CodeIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CodeIdentity(<redacted>)")
    }
}

/// Evidence obtained from the same accepted Unix socket. The native provider
/// must bind `getpeereid`, `LOCAL_PEERPID`/`LOCAL_PEERTOKEN`, and
/// `SecCodeCopyGuestWithAttributes` to that socket. A path-only lookup or a
/// uid-only fallback is not valid production evidence.
pub trait PeerEvidenceProvider {
    fn peer_credentials(&self) -> Result<PeerCredentials, IdentityError>;
    fn peer_pid(&self) -> Result<u32, IdentityError>;
    fn peer_audit_token(&self) -> Result<AuditToken, IdentityError>;
    fn sec_code_identity(&self, token: AuditToken) -> Result<CodeIdentity, IdentityError>;
    /// Compile and evaluate the enrolled requirement against the exact SecCode
    /// guest resolved from `token`; reporting requirement text is insufficient.
    fn evaluate_designated_requirement(
        &self,
        token: AuditToken,
        requirement: &str,
    ) -> Result<bool, IdentityError>;
    /// Client-computable digest of the audit token and console UID obtained
    /// from this accepted socket. Socket-local kernel evidence and SecCode are
    /// still verified independently before this value is accepted.
    fn connection_binding(&self) -> Result<Bytes32, IdentityError>;
}

pub struct PeerEvidence<'a, P: PeerEvidenceProvider> {
    provider: &'a P,
}

impl<'a, P: PeerEvidenceProvider> PeerEvidence<'a, P> {
    #[must_use]
    pub const fn new(provider: &'a P) -> Self {
        Self { provider }
    }

    pub fn verify(
        &self,
        policy: &MacosPeerPolicy,
        role: PeerRole,
        purpose: AuthorizationPurpose,
    ) -> Result<VerifiedPeer<'a>, IdentityError> {
        policy.validate()?;
        let credentials = self.provider.peer_credentials()?;
        let peer_pid = self.provider.peer_pid()?;
        let token = self.provider.peer_audit_token()?;
        if credentials.uid != policy.console_uid || token.effective_uid() != policy.console_uid {
            return Err(IdentityError::WrongUser);
        }
        if peer_pid == 0 || token.pid() != peer_pid {
            return Err(IdentityError::PeerTokenMismatch);
        }
        if token.audit_session_id() != policy.audit_session_id {
            return Err(IdentityError::WrongAuditSession);
        }

        let identity = self.provider.sec_code_identity(token)?;
        let expected = policy.role(role);
        if !identity.statically_valid {
            return Err(IdentityError::InvalidCode);
        }
        if !self
            .provider
            .evaluate_designated_requirement(token, &expected.designated_requirement)?
        {
            return Err(IdentityError::InvalidCode);
        }
        if identity.signing_identifier != expected.signing_identifier
            || identity.canonical_executable_path != expected.canonical_executable_path
            || identity.code_directory_hash != expected.code_directory_hash
            || identity.release_build_digest != policy.release_build_digest
            || identity.signing_identity != expected.signing_identity
        {
            return Err(IdentityError::IdentityMismatch);
        }
        if purpose == AuthorizationPurpose::Capture && !expected.capture_authorized {
            return Err(IdentityError::PurposeDenied);
        }

        let connection_binding = self.provider.connection_binding()?;
        if is_zero(connection_binding) {
            return Err(IdentityError::ConnectionBindingUnavailable);
        }

        Ok(VerifiedPeer {
            role,
            purpose,
            pid: peer_pid,
            uid: credentials.uid,
            audit_session_id: token.audit_session_id(),
            executable_sha256: identity.executable_sha256,
            release_build_digest: identity.release_build_digest,
            requirement_digest: requirement_digest(&expected.designated_requirement),
            policy_digest: peer_policy_digest(policy),
            connection_binding,
            evidence_lifetime: PhantomData,
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct RolePolicy {
    pub signing_identifier: String,
    pub designated_requirement: String,
    pub canonical_executable_path: PathBuf,
    /// Signed corroborating artifact metadata; accepted code authority is the
    /// audit-token SecCode, designated requirement, and exact CDHash.
    pub executable_sha256: Bytes32,
    pub code_directory_hash: RequirementHash,
    pub signing_identity: LocalSigningIdentity,
    /// Immediately previous enrolled owners can be maintenance-only. A role
    /// policy with this bit clear can never receive capture authority.
    pub capture_authorized: bool,
}

#[derive(Clone, Eq, PartialEq)]
pub struct MacosPeerPolicy {
    pub console_uid: u32,
    pub audit_session_id: u32,
    pub release_build_digest: Bytes32,
    pub gateway: RolePolicy,
    pub owner: RolePolicy,
}

impl MacosPeerPolicy {
    pub(crate) fn role(&self, role: PeerRole) -> &RolePolicy {
        match role {
            PeerRole::Gateway => &self.gateway,
            PeerRole::Owner => &self.owner,
        }
    }

    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.audit_session_id == 0
            || is_zero(self.release_build_digest)
            || self.gateway.signing_identifier != GATEWAY_SIGNING_IDENTIFIER
            || self.owner.signing_identifier != OWNER_SIGNING_IDENTIFIER
            || !absolute_path_has_only_normal_components(&self.gateway.canonical_executable_path)
            || !absolute_path_has_only_normal_components(&self.owner.canonical_executable_path)
            || self.gateway.canonical_executable_path == self.owner.canonical_executable_path
        {
            return Err(IdentityError::InvalidPolicy);
        }
        for role in [&self.gateway, &self.owner] {
            validate_requirement(&role.designated_requirement)?;
            if is_zero(role.executable_sha256)
                || role
                    .code_directory_hash
                    .as_bytes()
                    .iter()
                    .all(|byte| *byte == 0)
                || !requirement_matches_signing_identity(
                    &role.designated_requirement,
                    &role.signing_identifier,
                    role.signing_identity,
                )
            {
                return Err(IdentityError::InvalidPolicy);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn tcc_persistence_expectation(&self) -> TccPersistenceExpectation {
        if matches!(
            self.owner.signing_identity,
            LocalSigningIdentity::LocallyTrustedSelfSigned { .. }
        ) {
            TccPersistenceExpectation::StableLocalRequirement
        } else {
            TccPersistenceExpectation::ReauthorizationAfterIdentityChange
        }
    }

    /// TCC is intentionally observation-only here. Talking Quill does not
    /// grant, reset, or bypass Accessibility/Input Monitoring/Event Post
    /// consent. The user must act in System Settings and startup stays disabled
    /// until every permission required by the native adapter is reported.
    pub fn require_user_mediated_tcc(
        &self,
        accessibility: TccAuthorization,
        input_monitoring: TccAuthorization,
        event_post: TccAuthorization,
    ) -> Result<(), IdentityError> {
        if [accessibility, input_monitoring, event_post]
            .into_iter()
            .all(|status| status == TccAuthorization::Granted)
        {
            Ok(())
        } else {
            Err(IdentityError::UserMediatedTccRequired)
        }
    }
}

impl fmt::Debug for MacosPeerPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MacosPeerPolicy(<redacted>)")
    }
}

#[derive(Eq, PartialEq)]
pub struct VerifiedPeer<'e> {
    role: PeerRole,
    purpose: AuthorizationPurpose,
    pid: u32,
    uid: u32,
    audit_session_id: u32,
    executable_sha256: Bytes32,
    release_build_digest: Bytes32,
    requirement_digest: Bytes32,
    policy_digest: Bytes32,
    connection_binding: Bytes32,
    evidence_lifetime: PhantomData<&'e ()>,
}

impl VerifiedPeer<'_> {
    #[must_use]
    pub const fn role(&self) -> PeerRole {
        self.role
    }

    #[must_use]
    pub const fn purpose(&self) -> AuthorizationPurpose {
        self.purpose
    }

    #[must_use]
    pub(crate) const fn requirement_digest(&self) -> Bytes32 {
        self.requirement_digest
    }

    pub(crate) const fn policy_digest(&self) -> Bytes32 {
        self.policy_digest
    }

    #[must_use]
    pub const fn connection_binding(&self) -> Bytes32 {
        self.connection_binding
    }
}

impl fmt::Debug for VerifiedPeer<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedPeer(<redacted>)")
    }
}

#[must_use]
pub fn ad_hoc_designated_requirement(
    identifier: &str,
    code_directory_hash: RequirementHash,
) -> String {
    format!(
        "identifier \"{}\" and cdhash H\"{}\" and not anchor apple",
        identifier,
        requirement_hex(code_directory_hash)
    )
}

#[must_use]
pub fn self_signed_designated_requirement(
    identifier: &str,
    requirement_certificate_hash: RequirementHash,
) -> String {
    format!(
        "identifier \"{}\" and anchor trusted and certificate leaf = H\"{}\" and certificate root = H\"{}\" and not anchor apple",
        identifier,
        requirement_hex(requirement_certificate_hash),
        requirement_hex(requirement_certificate_hash)
    )
}

/// Protocol-v1 macOS credential binding. A gateway computes this from its own
/// audit token; the owner recomputes it from `LOCAL_PEERTOKEN` and
/// `getpeereid`. Fresh handshake nonces/proofs provide per-connection replay
/// resistance without requiring the client to know a server-side socket inode.
#[must_use]
pub fn audit_token_connection_binding(token: AuditToken, uid: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-token-binding/v1\0");
    for word in token.words() {
        digest.update(word.to_be_bytes());
    }
    digest.update(uid.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}

pub fn requirement_digest(requirement: &str) -> Bytes32 {
    Bytes32::new(Sha256::digest(requirement.as_bytes()).into())
}

#[must_use]
pub fn peer_policy_digest(policy: &MacosPeerPolicy) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-peer-policy/v1\0");
    digest.update(policy.console_uid.to_be_bytes());
    digest.update(policy.audit_session_id.to_be_bytes());
    digest.update(policy.release_build_digest.as_bytes());
    for role in [&policy.gateway, &policy.owner] {
        digest_part(&mut digest, role.signing_identifier.as_bytes());
        digest_part(&mut digest, role.designated_requirement.as_bytes());
        #[cfg(target_os = "macos")]
        digest_part(
            &mut digest,
            role.canonical_executable_path.as_os_str().as_bytes(),
        );
        #[cfg(not(target_os = "macos"))]
        digest_part(
            &mut digest,
            role.canonical_executable_path.to_string_lossy().as_bytes(),
        );
        digest.update(role.executable_sha256.as_bytes());
        digest.update(role.code_directory_hash.as_bytes());
        match role.signing_identity {
            LocalSigningIdentity::AdHoc {
                code_directory_hash,
            } => {
                digest.update([0]);
                digest.update(code_directory_hash.as_bytes());
            }
            LocalSigningIdentity::LocallyTrustedSelfSigned {
                certificate_sha256,
                requirement_certificate_hash,
            } => {
                digest.update([1]);
                digest.update(certificate_sha256.as_bytes());
                digest.update(requirement_certificate_hash.as_bytes());
            }
        }
        digest.update([u8::from(role.capture_authorized)]);
    }
    Bytes32::new(digest.finalize().into())
}

fn digest_part(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

fn requirement_matches_signing_identity(
    requirement: &str,
    identifier: &str,
    identity: LocalSigningIdentity,
) -> bool {
    let expected = match identity {
        LocalSigningIdentity::AdHoc {
            code_directory_hash,
        } => ad_hoc_designated_requirement(identifier, code_directory_hash),
        LocalSigningIdentity::LocallyTrustedSelfSigned {
            requirement_certificate_hash,
            ..
        } => self_signed_designated_requirement(identifier, requirement_certificate_hash),
    };
    requirement == expected
}

fn absolute_path_has_only_normal_components(path: &std::path::Path) -> bool {
    use std::path::Component;
    let encoded = path.to_string_lossy();
    path.is_absolute()
        && !encoded
            .split(['/', '\\'])
            .any(|component| matches!(component, "." | ".."))
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

fn validate_requirement(requirement: &str) -> Result<(), IdentityError> {
    if requirement.is_empty()
        || requirement.len() > MAX_REQUIREMENT_BYTES
        || requirement
            .bytes()
            .any(|byte| byte == 0 || (byte.is_ascii_control() && byte != b'\t'))
    {
        return Err(IdentityError::InvalidRequirement);
    }
    Ok(())
}

fn is_zero(value: Bytes32) -> bool {
    value.as_bytes().iter().all(|byte| *byte == 0)
}

fn requirement_hex(value: RequirementHash) -> String {
    hex_bytes(value.as_bytes())
}

fn hex_bytes(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IdentityError {
    #[error("kernel peer credentials are unavailable")]
    PeerCredentialsUnavailable,
    #[error("kernel peer PID is unavailable")]
    PeerPidUnavailable,
    #[error("kernel audit token is unavailable")]
    AuditTokenUnavailable,
    #[error("accepted-socket connection binding is unavailable")]
    ConnectionBindingUnavailable,
    #[error("peer token does not match the kernel peer PID")]
    PeerTokenMismatch,
    #[error("peer belongs to the wrong user")]
    WrongUser,
    #[error("peer belongs to the wrong GUI audit session")]
    WrongAuditSession,
    #[error("Security.framework rejected the peer code")]
    InvalidCode,
    #[error("peer code identity does not exactly match local enrollment")]
    IdentityMismatch,
    #[error("macOS peer policy is invalid")]
    InvalidPolicy,
    #[error("designated requirement is invalid")]
    InvalidRequirement,
    #[error("the enrolled role is maintenance-only")]
    PurposeDenied,
    #[error("explicit user-mediated macOS privacy permission is required")]
    UserMediatedTccRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: u32 = 501;
    const PID: u32 = 42;
    const SESSION: u32 = 77;

    fn bytes(value: u8) -> Bytes32 {
        Bytes32::new([value; 32])
    }

    fn requirement_hash(value: u8) -> RequirementHash {
        RequirementHash::new([value; 20])
    }

    fn role(identifier: &str, path: &str, marker: u8, capture_authorized: bool) -> RolePolicy {
        let certificate = bytes(9);
        let requirement_certificate_hash = requirement_hash(10);
        RolePolicy {
            signing_identifier: identifier.to_owned(),
            designated_requirement: self_signed_designated_requirement(
                identifier,
                requirement_certificate_hash,
            ),
            canonical_executable_path: test_absolute_path(path),
            executable_sha256: bytes(marker),
            code_directory_hash: requirement_hash(marker),
            signing_identity: LocalSigningIdentity::LocallyTrustedSelfSigned {
                certificate_sha256: certificate,
                requirement_certificate_hash,
            },
            capture_authorized,
        }
    }

    fn test_absolute_path(path: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("C:{}", path.replace('/', "\\")))
        } else {
            PathBuf::from(path)
        }
    }

    fn policy() -> MacosPeerPolicy {
        MacosPeerPolicy {
            console_uid: UID,
            audit_session_id: SESSION,
            release_build_digest: bytes(3),
            gateway: role(
                GATEWAY_SIGNING_IDENTIFIER,
                "/Applications/Talking Quill.app/Contents/MacOS/helper",
                4,
                true,
            ),
            owner: role(
                OWNER_SIGNING_IDENTIFIER,
                "/Applications/Talking Quill.app/Contents/Library/LoginItems/owner",
                5,
                true,
            ),
        }
    }

    #[derive(Clone)]
    struct FakeEvidence {
        credentials: PeerCredentials,
        pid: u32,
        token: AuditToken,
        identity: CodeIdentity,
        expected_requirement: String,
        requirement_error: bool,
        connection_binding: Bytes32,
    }

    impl FakeEvidence {
        fn gateway(policy: &MacosPeerPolicy) -> Self {
            let role = &policy.gateway;
            Self {
                credentials: PeerCredentials { uid: UID, gid: 20 },
                pid: PID,
                token: AuditToken::new([0, UID, 0, 0, 0, PID, SESSION, 0]),
                expected_requirement: role.designated_requirement.clone(),
                requirement_error: false,
                connection_binding: bytes(88),
                identity: CodeIdentity {
                    signing_identifier: role.signing_identifier.clone(),
                    canonical_executable_path: role.canonical_executable_path.clone(),
                    executable_sha256: role.executable_sha256,
                    code_directory_hash: role.code_directory_hash,
                    release_build_digest: policy.release_build_digest,
                    signing_identity: role.signing_identity,
                    statically_valid: true,
                },
            }
        }
    }

    impl PeerEvidenceProvider for FakeEvidence {
        fn peer_credentials(&self) -> Result<PeerCredentials, IdentityError> {
            Ok(self.credentials)
        }

        fn peer_pid(&self) -> Result<u32, IdentityError> {
            Ok(self.pid)
        }

        fn peer_audit_token(&self) -> Result<AuditToken, IdentityError> {
            Ok(self.token)
        }

        fn sec_code_identity(&self, _token: AuditToken) -> Result<CodeIdentity, IdentityError> {
            Ok(self.identity.clone())
        }

        fn evaluate_designated_requirement(
            &self,
            _token: AuditToken,
            requirement: &str,
        ) -> Result<bool, IdentityError> {
            if self.requirement_error {
                Err(IdentityError::InvalidCode)
            } else {
                Ok(requirement == self.expected_requirement)
            }
        }

        fn connection_binding(&self) -> Result<Bytes32, IdentityError> {
            Ok(self.connection_binding)
        }
    }

    #[test]
    fn locally_trusted_identity_is_accepted_without_team_or_developer_id() {
        let policy = policy();
        let evidence = FakeEvidence::gateway(&policy);
        let verified = PeerEvidence::new(&evidence)
            .verify(&policy, PeerRole::Gateway, AuthorizationPurpose::Capture)
            .expect("locally enrolled identity");
        assert_eq!(verified.role(), PeerRole::Gateway);
        assert_eq!(
            policy.tcc_persistence_expectation(),
            TccPersistenceExpectation::StableLocalRequirement
        );
    }

    #[test]
    fn exact_ad_hoc_identity_is_supported_but_declares_tcc_reauthorization() {
        let mut policy = policy();
        let cdhash = requirement_hash(11);
        policy.owner.signing_identity = LocalSigningIdentity::AdHoc {
            code_directory_hash: cdhash,
        };
        policy.owner.designated_requirement =
            ad_hoc_designated_requirement(OWNER_SIGNING_IDENTIFIER, cdhash);
        policy.validate().expect("valid ad-hoc policy");
        assert!(
            policy
                .owner
                .designated_requirement
                .contains("not anchor apple")
        );
        assert_eq!(
            policy.tcc_persistence_expectation(),
            TccPersistenceExpectation::ReauthorizationAfterIdentityChange
        );
    }

    #[test]
    fn developer_id_and_non_self_signed_requirements_are_not_valid_local_policy() {
        let mut policy = policy();
        policy.owner.designated_requirement = format!(
            "identifier \"{}\" and anchor apple generic and certificate leaf[subject.OU] = \"TEAMID1234\"",
            OWNER_SIGNING_IDENTIFIER
        );
        assert_eq!(policy.validate(), Err(IdentityError::InvalidPolicy));

        policy.owner.designated_requirement = format!(
            "identifier \"{}\" and anchor trusted and certificate leaf = H\"{}\" and not anchor apple",
            OWNER_SIGNING_IDENTIFIER,
            requirement_hex(requirement_hash(10))
        );
        assert_eq!(policy.validate(), Err(IdentityError::InvalidPolicy));

        let generated =
            self_signed_designated_requirement(OWNER_SIGNING_IDENTIFIER, requirement_hash(10));
        assert!(generated.contains("certificate root"));
        assert!(generated.contains("not anchor apple"));
    }

    #[test]
    fn audit_token_binding_is_client_computable_and_token_exact() {
        let token = AuditToken::new([1, 501, 2, 3, 4, 44, 12, 5]);
        assert_eq!(
            audit_token_connection_binding(token, 501),
            audit_token_connection_binding(token, 501)
        );
        assert_ne!(
            audit_token_connection_binding(token, 501),
            audit_token_connection_binding(AuditToken::new([1, 501, 2, 3, 4, 45, 12, 5]), 501)
        );
        assert_ne!(
            audit_token_connection_binding(token, 501),
            audit_token_connection_binding(token, 502)
        );
    }

    #[test]
    fn enrolled_paths_reject_dot_and_parent_components() {
        let mut value = policy();
        value.gateway.canonical_executable_path = test_absolute_path("/Applications/TQ/../gateway");
        assert_eq!(value.validate(), Err(IdentityError::InvalidPolicy));
        value.gateway.canonical_executable_path = test_absolute_path("/Applications/./gateway");
        assert_eq!(value.validate(), Err(IdentityError::InvalidPolicy));
    }

    #[test]
    fn adversarial_peer_mutations_fail_closed() {
        let policy = policy();
        let baseline = FakeEvidence::gateway(&policy);
        let cases = [
            {
                let mut value = baseline.clone();
                value.credentials.uid += 1;
                value
            },
            {
                let mut value = baseline.clone();
                value.token.0[5] += 1;
                value
            },
            {
                let mut value = baseline.clone();
                value.token.0[6] += 1;
                value
            },
            {
                let mut value = baseline.clone();
                value.identity.signing_identifier = OWNER_SIGNING_IDENTIFIER.into();
                value
            },
            {
                let mut value = baseline.clone();
                value.identity.signing_identity = LocalSigningIdentity::AdHoc {
                    code_directory_hash: requirement_hash(77),
                };
                value
            },
            {
                let mut value = baseline.clone();
                value.identity.canonical_executable_path = test_absolute_path("/tmp/replaced");
                value
            },
            {
                let mut value = baseline.clone();
                value.expected_requirement.push_str(" or true");
                value
            },
            {
                let mut value = baseline.clone();
                value.identity.statically_valid = false;
                value
            },
        ];
        for evidence in cases {
            assert!(
                PeerEvidence::new(&evidence)
                    .verify(&policy, PeerRole::Gateway, AuthorizationPurpose::Capture)
                    .is_err()
            );
        }
    }

    #[test]
    fn requirement_evaluation_errors_and_missing_connection_binding_fail_closed() {
        let policy = policy();
        let mut evidence = FakeEvidence::gateway(&policy);
        evidence.requirement_error = true;
        assert_eq!(
            PeerEvidence::new(&evidence).verify(
                &policy,
                PeerRole::Gateway,
                AuthorizationPurpose::Capture,
            ),
            Err(IdentityError::InvalidCode)
        );

        evidence.requirement_error = false;
        evidence.connection_binding = Bytes32::new([0; 32]);
        assert_eq!(
            PeerEvidence::new(&evidence).verify(
                &policy,
                PeerRole::Gateway,
                AuthorizationPurpose::Capture,
            ),
            Err(IdentityError::ConnectionBindingUnavailable)
        );
    }

    #[test]
    fn raw_executable_sha_is_corroboration_not_peer_code_authority() {
        let policy = policy();
        let mut evidence = FakeEvidence::gateway(&policy);
        evidence.identity.executable_sha256 = bytes(99);
        assert!(
            PeerEvidence::new(&evidence)
                .verify(&policy, PeerRole::Gateway, AuthorizationPurpose::Capture)
                .is_ok()
        );
        evidence.identity.code_directory_hash = requirement_hash(99);
        assert!(
            PeerEvidence::new(&evidence)
                .verify(&policy, PeerRole::Gateway, AuthorizationPurpose::Capture)
                .is_err()
        );
    }

    #[test]
    fn previous_enrolled_hash_can_be_maintenance_only() {
        let mut policy = policy();
        policy.gateway.capture_authorized = false;
        let evidence = FakeEvidence::gateway(&policy);
        assert_eq!(
            PeerEvidence::new(&evidence).verify(
                &policy,
                PeerRole::Gateway,
                AuthorizationPurpose::Capture
            ),
            Err(IdentityError::PurposeDenied)
        );
        assert!(
            PeerEvidence::new(&evidence)
                .verify(
                    &policy,
                    PeerRole::Gateway,
                    AuthorizationPurpose::Maintenance
                )
                .is_ok()
        );
    }

    #[test]
    fn tcc_is_never_inferred_or_programmatically_granted() {
        let policy = policy();
        assert_eq!(
            policy.require_user_mediated_tcc(
                TccAuthorization::Granted,
                TccAuthorization::NotDetermined,
                TccAuthorization::Granted
            ),
            Err(IdentityError::UserMediatedTccRequired)
        );
        assert!(
            policy
                .require_user_mediated_tcc(
                    TccAuthorization::Granted,
                    TccAuthorization::Granted,
                    TccAuthorization::Granted
                )
                .is_ok()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn generated_local_requirements_compile_with_native_security_tools() {
        use std::{fs, process::Command};

        fs::create_dir_all("tmp").expect("project tmp directory");
        let output_path = format!("tmp/r3m-requirement-{}.bin", std::process::id());
        for requirement in [
            ad_hoc_designated_requirement(
                GATEWAY_SIGNING_IDENTIFIER,
                RequirementHash::new([1; 20]),
            ),
            self_signed_designated_requirement(
                OWNER_SIGNING_IDENTIFIER,
                RequirementHash::new([2; 20]),
            ),
        ] {
            let output = Command::new("/usr/bin/csreq")
                .args(["-r", &requirement, "-b", &output_path])
                .output()
                .expect("compile native requirement");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            fs::remove_file(&output_path).expect("remove compiled requirement");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_test_binary_exposes_a_codesign_designated_requirement() {
        use std::process::Command;

        let executable = std::env::current_exe().expect("test executable path");
        let output = Command::new("/usr/bin/codesign")
            .args(["--display", "--requirements", "-"])
            .arg(executable)
            .output()
            .expect("run native codesign inspection");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let evidence = String::from_utf8_lossy(&output.stderr);
        let requirement = evidence
            .lines()
            .find_map(|line| {
                line.split_once("designated =>")
                    .map(|(_, value)| value.trim())
            })
            .filter(|value| !value.is_empty())
            .expect("native designated requirement");
        let evaluation = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", &format!("-R={requirement}")])
            .arg(std::env::current_exe().expect("test executable path"))
            .output()
            .expect("evaluate native designated requirement");
        assert!(
            evaluation.status.success(),
            "{}",
            String::from_utf8_lossy(&evaluation.stderr)
        );
    }
}
