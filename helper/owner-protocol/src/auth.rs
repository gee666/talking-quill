use std::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use p256::SecretKey;
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::envelope::{Direction, EnvelopeError, EnvelopeReceiver, FrameKey};
use crate::scalar::{Bytes32, P256PublicKey};
use crate::schema::{
    Architecture, Authenticated, AuthorityCeiling, Challenge, Hello, Platform, ProtocolHeader,
    Purpose, SelectedProtocol,
};

const TRANSCRIPT_DOMAIN: &[u8] = b"TQKO-AUTH-TRANSCRIPT-V1\0";
const CLIENT_FINISH_DOMAIN: &[u8] = b"TQKO CLIENT FINISH V1\0";
const OWNER_FINISH_DOMAIN: &[u8] = b"TQKO OWNER FINISH V1\0";
const CLIENT_PROOF_INFO: &[u8] = b"TQKO client proof v1";
const OWNER_PROOF_INFO: &[u8] = b"TQKO owner proof v1";
const GATEWAY_FRAME_INFO: &[u8] = b"TQKO gateway-to-owner frame v1";
const OWNER_FRAME_INFO: &[u8] = b"TQKO owner-to-gateway frame v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAgreementMode {
    MacosKeychain,
    WindowsStablePipeP256,
}

impl KeyAgreementMode {
    const fn tag(self) -> u8 {
        match self {
            Self::MacosKeychain => 1,
            Self::WindowsStablePipeP256 => 3,
        }
    }
}

#[derive(Clone)]
pub struct TranscriptInput {
    client_protocol: ProtocolHeader,
    owner_protocol: ProtocolHeader,
    selected_protocol: SelectedProtocol,
    purpose: Purpose,
    authority_ceiling: AuthorityCeiling,
    platform: Platform,
    client_architecture: Architecture,
    owner_architecture: Architecture,
    client_nonce: Bytes32,
    owner_nonce: Bytes32,
    session_id: Bytes32,
    owner_instance_id: Bytes32,
    client_release_build_digest: Bytes32,
    client_executable_digest: Bytes32,
    owner_release_build_digest: Bytes32,
    owner_executable_digest: Bytes32,
    installation_identity_digest: Bytes32,
    client_signer_policy_digest: Bytes32,
    owner_signer_policy_digest: Bytes32,
    os_session_binding_digest: Bytes32,
    client_release_policy_digest: Bytes32,
    owner_release_policy_digest: Bytes32,
    platform_credential_binding_digest: Bytes32,
    key_agreement_mode: KeyAgreementMode,
    client_p256_public_key: Option<P256PublicKey>,
    owner_p256_public_key: Option<P256PublicKey>,
}

impl fmt::Debug for TranscriptInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TranscriptInput([REDACTED])")
    }
}

/// Platform transport/authentication code implements this only after binding
/// kernel peer identity, approved code identity, detached CMS signatures, and
/// the private bootstrap credential to the held connection.
pub trait HandshakeTrustVerifier {
    fn verify(
        &self,
        hello: &Hello,
        challenge: &Challenge,
        client_policy: &crate::release_policy::ReleasePolicy,
        owner_policy: &crate::release_policy::ReleasePolicy,
    ) -> Result<(), AuthenticationError>;
}

impl TranscriptInput {
    pub fn from_verified_handshake<V: HandshakeTrustVerifier>(
        hello: &Hello,
        challenge: &Challenge,
        verifier: &V,
    ) -> Result<Self, AuthenticationError> {
        hello.validate()?;
        challenge.validate()?;
        let selected = hello.protocol.negotiate(challenge.owner_protocol)?;
        if selected != challenge.selected_protocol {
            return Err(AuthenticationError::Selection);
        }
        if hello.purpose != challenge.purpose
            || hello.platform != challenge.platform
            || hello.architecture != challenge.architecture
            || !hello
                .installation_identity_digest
                .constant_time_eq(&challenge.installation_identity_digest)
            || !hello
                .os_session_binding_digest
                .constant_time_eq(&challenge.os_session_binding_digest)
            || !hello
                .platform_credential_binding_digest
                .constant_time_eq(&challenge.platform_credential_binding_digest)
        {
            return Err(AuthenticationError::Binding);
        }
        let (client_policy, owner_policy) = validate_policy_self_fields(hello, challenge)?;
        verifier.verify(hello, challenge, &client_policy, &owner_policy)?;
        validate_policy_pair(hello.purpose, &client_policy, &owner_policy)?;
        let key_agreement_mode = match hello.platform {
            Platform::Macos => KeyAgreementMode::MacosKeychain,
            Platform::Windows => KeyAgreementMode::WindowsStablePipeP256,
        };
        Ok(Self {
            client_protocol: hello.protocol,
            owner_protocol: challenge.owner_protocol,
            selected_protocol: selected,
            purpose: hello.purpose,
            authority_ceiling: challenge.authority_ceiling,
            platform: hello.platform,
            client_architecture: hello.architecture,
            owner_architecture: challenge.architecture,
            client_nonce: hello.client_nonce,
            owner_nonce: challenge.owner_nonce,
            session_id: challenge.session_id,
            owner_instance_id: challenge.owner_instance_id,
            client_release_build_digest: hello.release_build_digest,
            client_executable_digest: hello.executable_sha256,
            owner_release_build_digest: challenge.release_build_digest,
            owner_executable_digest: challenge.executable_sha256,
            installation_identity_digest: hello.installation_identity_digest,
            client_signer_policy_digest: hello.signer_policy_digest,
            owner_signer_policy_digest: challenge.signer_policy_digest,
            os_session_binding_digest: hello.os_session_binding_digest,
            client_release_policy_digest: hello.client_release_policy_digest,
            owner_release_policy_digest: challenge.owner_release_policy_digest,
            platform_credential_binding_digest: hello.platform_credential_binding_digest,
            key_agreement_mode,
            client_p256_public_key: hello.client_ephemeral_public_key.clone(),
            owner_p256_public_key: challenge.owner_ephemeral_public_key.clone(),
        })
    }

    fn validate(&self) -> Result<(), AuthenticationError> {
        self.client_protocol.validate()?;
        self.owner_protocol.validate()?;
        let selected = self.client_protocol.negotiate(self.owner_protocol)?;
        if selected != self.selected_protocol {
            return Err(AuthenticationError::Selection);
        }
        let ceiling_matches = matches!(
            (self.purpose, self.authority_ceiling),
            (Purpose::Observe, AuthorityCeiling::Observer)
                | (Purpose::Capture, AuthorityCeiling::Capture)
                | (Purpose::Maintenance, AuthorityCeiling::Maintenance)
        );
        if !ceiling_matches {
            return Err(AuthenticationError::Binding);
        }
        let keys_match = match self.key_agreement_mode {
            KeyAgreementMode::MacosKeychain => {
                self.platform == Platform::Macos
                    && self.client_p256_public_key.is_none()
                    && self.owner_p256_public_key.is_none()
            }
            KeyAgreementMode::WindowsStablePipeP256 => {
                self.platform == Platform::Windows
                    && self.client_p256_public_key.is_some()
                    && self.owner_p256_public_key.is_some()
            }
        };
        if !keys_match {
            return Err(AuthenticationError::KeyAgreement);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Transcript {
    bytes: Vec<u8>,
    hash: Bytes32,
    selected_protocol: SelectedProtocol,
    purpose: Purpose,
    authority_ceiling: AuthorityCeiling,
    session_id: Bytes32,
    owner_instance_id: Bytes32,
    key_agreement_mode: KeyAgreementMode,
    client_p256_public_key: Option<P256PublicKey>,
    owner_p256_public_key: Option<P256PublicKey>,
}

impl fmt::Debug for Transcript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Transcript([REDACTED])")
    }
}

impl Transcript {
    pub fn build(input: &TranscriptInput) -> Result<Self, AuthenticationError> {
        input.validate()?;
        let mut bytes = Vec::with_capacity(712);
        bytes.extend_from_slice(TRANSCRIPT_DOMAIN);
        append_protocol(&mut bytes, input.client_protocol);
        append_protocol(&mut bytes, input.owner_protocol);
        append_selected_protocol(&mut bytes, input.selected_protocol);
        bytes.push(input.purpose.tag());
        bytes.push(input.authority_ceiling.tag());
        bytes.push(input.platform.tag());
        bytes.push(input.client_architecture.tag());
        bytes.push(input.owner_architecture.tag());
        bytes.extend_from_slice(&[0; 3]);
        for value in [
            input.client_nonce,
            input.owner_nonce,
            input.session_id,
            input.owner_instance_id,
            input.client_release_build_digest,
            input.client_executable_digest,
            input.owner_release_build_digest,
            input.owner_executable_digest,
            input.installation_identity_digest,
            input.client_signer_policy_digest,
            input.owner_signer_policy_digest,
            input.os_session_binding_digest,
            input.client_release_policy_digest,
            input.owner_release_policy_digest,
            input.platform_credential_binding_digest,
        ] {
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes.push(input.key_agreement_mode.tag());
        bytes.extend_from_slice(&[0; 3]);
        append_public_key(&mut bytes, input.client_p256_public_key.as_ref());
        append_public_key(&mut bytes, input.owner_p256_public_key.as_ref());
        let hash = Bytes32::new(Sha256::digest(&bytes).into());
        Ok(Self {
            bytes,
            hash,
            selected_protocol: input.selected_protocol,
            purpose: input.purpose,
            authority_ceiling: input.authority_ceiling,
            session_id: input.session_id,
            owner_instance_id: input.owner_instance_id,
            key_agreement_mode: input.key_agreement_mode,
            client_p256_public_key: input.client_p256_public_key.clone(),
            owner_p256_public_key: input.owner_p256_public_key.clone(),
        })
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn hash(&self) -> Bytes32 {
        self.hash
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRole {
    Gateway,
    Owner,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct KeyAgreementMaterial {
    ikm: Vec<u8>,
    #[zeroize(skip)]
    mode: KeyAgreementMode,
    #[zeroize(skip)]
    client_public_key: Option<P256PublicKey>,
    #[zeroize(skip)]
    owner_public_key: Option<P256PublicKey>,
}

impl fmt::Debug for KeyAgreementMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyAgreementMaterial([REDACTED])")
    }
}

impl KeyAgreementMaterial {
    #[must_use]
    pub fn macos(per_install_keychain_secret: &mut [u8; 32]) -> Self {
        let ikm = per_install_keychain_secret.to_vec();
        per_install_keychain_secret.zeroize();
        Self {
            ikm,
            mode: KeyAgreementMode::MacosKeychain,
            client_public_key: None,
            owner_public_key: None,
        }
    }

    /// Windows stable named-pipe mode. Authentication authority comes from the
    /// pipe ACL plus kernel-derived peer process/token/image facts, so the
    /// ephemeral P-256 shared value is the complete key material.
    pub fn windows_peer(
        local_role: PeerRole,
        local_secret: &EphemeralP256Secret,
        peer_public_key: &P256PublicKey,
    ) -> Result<Self, AuthenticationError> {
        let shared = local_secret
            .secret
            .diffie_hellman(&peer_public_key.parsed());
        Self::windows_from_shared(
            shared.raw_secret_bytes().to_vec(),
            KeyAgreementMode::WindowsStablePipeP256,
            local_role,
            local_secret,
            peer_public_key,
        )
    }

    fn windows_from_shared(
        ikm: Vec<u8>,
        mode: KeyAgreementMode,
        local_role: PeerRole,
        local_secret: &EphemeralP256Secret,
        peer_public_key: &P256PublicKey,
    ) -> Result<Self, AuthenticationError> {
        let (client_public_key, owner_public_key) = match local_role {
            PeerRole::Gateway => (local_secret.public.clone(), peer_public_key.clone()),
            PeerRole::Owner => (peer_public_key.clone(), local_secret.public.clone()),
        };
        Ok(Self {
            ikm,
            mode,
            client_public_key: Some(client_public_key),
            owner_public_key: Some(owner_public_key),
        })
    }

    fn matches(&self, transcript: &Transcript) -> bool {
        self.mode == transcript.key_agreement_mode
            && self.client_public_key == transcript.client_p256_public_key
            && self.owner_public_key == transcript.owner_p256_public_key
    }
}

pub struct EphemeralP256Secret {
    secret: SecretKey,
    public: P256PublicKey,
}

impl fmt::Debug for EphemeralP256Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EphemeralP256Secret([REDACTED])")
    }
}

impl EphemeralP256Secret {
    pub fn random() -> Result<Self, AuthenticationError> {
        for _ in 0..128 {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).map_err(|_| AuthenticationError::Random)?;
            if let Ok(secret) = SecretKey::from_slice(&bytes) {
                bytes.zeroize();
                let public = P256PublicKey::from_key(&secret.public_key());
                return Ok(Self { secret, public });
            }
            bytes.zeroize();
        }
        Err(AuthenticationError::Random)
    }

    #[cfg(test)]
    fn from_bytes(mut bytes: [u8; 32]) -> Result<Self, AuthenticationError> {
        let parsed = SecretKey::from_slice(&bytes);
        bytes.zeroize();
        let secret = parsed.map_err(|_| AuthenticationError::KeyAgreement)?;
        let public = P256PublicKey::from_key(&secret.public_key());
        Ok(Self { secret, public })
    }

    #[must_use]
    pub const fn public_key(&self) -> &P256PublicKey {
        &self.public
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AuthenticationKeys {
    client_proof_key: [u8; 32],
    owner_proof_key: [u8; 32],
    gateway_frame_key: FrameKey,
    owner_frame_key: FrameKey,
}

impl fmt::Debug for AuthenticationKeys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticationKeys([REDACTED])")
    }
}

impl AuthenticationKeys {
    pub fn derive(
        transcript: &Transcript,
        material: &KeyAgreementMaterial,
    ) -> Result<Self, AuthenticationError> {
        if !material.matches(transcript) {
            return Err(AuthenticationError::KeyAgreement);
        }
        let (_, hkdf) = Hkdf::<Sha256>::extract(Some(transcript.hash.as_bytes()), &material.ikm);
        Ok(Self {
            client_proof_key: expand(&hkdf, CLIENT_PROOF_INFO)?,
            owner_proof_key: expand(&hkdf, OWNER_PROOF_INFO)?,
            gateway_frame_key: FrameKey::from_secret(
                Direction::GatewayToOwner,
                expand(&hkdf, GATEWAY_FRAME_INFO)?,
            ),
            owner_frame_key: FrameKey::from_secret(
                Direction::OwnerToGateway,
                expand(&hkdf, OWNER_FRAME_INFO)?,
            ),
        })
    }

    pub fn pending_gateway_session<'a>(
        &'a self,
        transcript: &'a Transcript,
    ) -> Result<PendingAuthenticatedSession<'a>, AuthenticationError> {
        PendingAuthenticatedSession::new(transcript, self)
    }

    pub fn establish_owner_session(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
    ) -> Result<AuthenticatedSession, AuthenticationError> {
        self.verify_client_proof(transcript, client_proof)?;
        Ok(AuthenticatedSession {
            session_id: transcript.session_id,
            owner_instance_id: transcript.owner_instance_id,
            purpose: transcript.purpose,
            authority_ceiling: transcript.authority_ceiling,
            feature_bits: transcript.selected_protocol.feature_bits,
            test_only: false,
            inbound: EnvelopeReceiver::new(
                Direction::GatewayToOwner,
                transcript.session_id,
                self.gateway_frame_key(),
                transcript.purpose,
            )?,
        })
    }

    #[must_use]
    pub const fn gateway_frame_key(&self) -> &FrameKey {
        &self.gateway_frame_key
    }

    #[must_use]
    pub const fn owner_frame_key(&self) -> &FrameKey {
        &self.owner_frame_key
    }

    #[must_use]
    pub fn proofs(&self, transcript: &Transcript) -> AuthenticationProofs {
        let mut client_message = Vec::with_capacity(CLIENT_FINISH_DOMAIN.len() + 32);
        client_message.extend_from_slice(CLIENT_FINISH_DOMAIN);
        client_message.extend_from_slice(transcript.hash.as_bytes());
        let client_proof = hmac(&self.client_proof_key, &client_message);

        let mut owner_message = Vec::with_capacity(OWNER_FINISH_DOMAIN.len() + 64);
        owner_message.extend_from_slice(OWNER_FINISH_DOMAIN);
        owner_message.extend_from_slice(transcript.hash.as_bytes());
        owner_message.extend_from_slice(client_proof.as_bytes());
        let owner_proof = hmac(&self.owner_proof_key, &owner_message);
        AuthenticationProofs {
            client_proof,
            owner_proof,
        }
    }

    pub fn verify_client_proof(
        &self,
        transcript: &Transcript,
        proof: &Bytes32,
    ) -> Result<(), AuthenticationError> {
        let mut message = Vec::with_capacity(CLIENT_FINISH_DOMAIN.len() + 32);
        message.extend_from_slice(CLIENT_FINISH_DOMAIN);
        message.extend_from_slice(transcript.hash.as_bytes());
        verify_hmac(&self.client_proof_key, &message, proof)
    }

    pub fn verify_authenticated_finish(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
        authenticated: &Authenticated,
    ) -> Result<(), AuthenticationError> {
        if authenticated.selected_protocol != transcript.selected_protocol
            || authenticated.purpose != transcript.purpose
            || authenticated.authority_ceiling != transcript.authority_ceiling
        {
            return Err(AuthenticationError::Binding);
        }
        self.verify_owner_proof(transcript, client_proof, &authenticated.owner_proof)
    }

    fn verify_owner_proof(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
        owner_proof: &Bytes32,
    ) -> Result<(), AuthenticationError> {
        self.verify_client_proof(transcript, client_proof)?;
        let mut message = Vec::with_capacity(OWNER_FINISH_DOMAIN.len() + 64);
        message.extend_from_slice(OWNER_FINISH_DOMAIN);
        message.extend_from_slice(transcript.hash.as_bytes());
        message.extend_from_slice(client_proof.as_bytes());
        verify_hmac(&self.owner_proof_key, &message, owner_proof)
    }
}

pub struct PendingAuthenticatedSession<'a> {
    transcript: &'a Transcript,
    keys: &'a AuthenticationKeys,
    inbound: EnvelopeReceiver,
}

impl fmt::Debug for PendingAuthenticatedSession<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PendingAuthenticatedSession([REDACTED])")
    }
}

impl<'a> PendingAuthenticatedSession<'a> {
    fn new(
        transcript: &'a Transcript,
        keys: &'a AuthenticationKeys,
    ) -> Result<Self, AuthenticationError> {
        Ok(Self {
            transcript,
            keys,
            inbound: EnvelopeReceiver::new(
                Direction::OwnerToGateway,
                transcript.session_id,
                keys.owner_frame_key(),
                transcript.purpose,
            )?,
        })
    }

    pub fn accept_authenticated_finish(
        mut self,
        body: &[u8],
        client_proof: &Bytes32,
    ) -> Result<AuthenticatedSession, AuthenticationError> {
        let envelope = self.inbound.accept_authenticated_finish(body)?;
        let authenticated = match crate::schema::parse_handshake_json(envelope.payload())? {
            crate::schema::HandshakeMessage::Authenticated(value) => value,
            _ => return Err(AuthenticationError::Binding),
        };
        self.keys
            .verify_authenticated_finish(self.transcript, client_proof, &authenticated)?;
        Ok(AuthenticatedSession {
            session_id: self.transcript.session_id,
            owner_instance_id: self.transcript.owner_instance_id,
            purpose: self.transcript.purpose,
            authority_ceiling: self.transcript.authority_ceiling,
            feature_bits: self.transcript.selected_protocol.feature_bits,
            test_only: false,
            inbound: self.inbound,
        })
    }
}

pub struct AuthenticatedSession {
    session_id: Bytes32,
    owner_instance_id: Bytes32,
    purpose: Purpose,
    authority_ceiling: AuthorityCeiling,
    feature_bits: crate::scalar::FeatureBits,
    test_only: bool,
    inbound: EnvelopeReceiver,
}

impl fmt::Debug for AuthenticatedSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticatedSession([REDACTED])")
    }
}

impl AuthenticatedSession {
    #[cfg(any(test, feature = "test-transport"))]
    pub(crate) fn for_fake_transport(
        session_id: Bytes32,
        owner_instance_id: Bytes32,
        purpose: Purpose,
        inbound_direction: Direction,
        inbound_key: &FrameKey,
        authenticated_finish_consumed: bool,
        feature_bits: crate::scalar::FeatureBits,
    ) -> Result<Self, AuthenticationError> {
        let authority_ceiling = match purpose {
            Purpose::Observe => AuthorityCeiling::Observer,
            Purpose::Capture => AuthorityCeiling::Capture,
            Purpose::Maintenance => AuthorityCeiling::Maintenance,
        };
        Ok(Self {
            session_id,
            owner_instance_id,
            purpose,
            authority_ceiling,
            feature_bits,
            test_only: true,
            inbound: EnvelopeReceiver::new_after_high_water(
                inbound_direction,
                session_id,
                inbound_key,
                purpose,
                authenticated_finish_consumed.then_some(1),
            )?,
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> Bytes32 {
        self.session_id
    }

    #[must_use]
    pub const fn owner_instance_id(&self) -> Bytes32 {
        self.owner_instance_id
    }

    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.purpose
    }

    #[must_use]
    pub const fn authority_ceiling(&self) -> AuthorityCeiling {
        self.authority_ceiling
    }

    #[must_use]
    pub const fn supports_feature(&self, feature: u64) -> bool {
        self.feature_bits.get() & feature == feature
    }

    #[must_use]
    pub(crate) const fn is_test_only(&self) -> bool {
        self.test_only
    }

    #[must_use]
    pub fn inbound(&mut self) -> &mut EnvelopeReceiver {
        &mut self.inbound
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationProofs {
    pub client_proof: Bytes32,
    pub owner_proof: Bytes32,
}

impl fmt::Debug for AuthenticationProofs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticationProofs([REDACTED])")
    }
}

#[derive(Debug, Error)]
pub enum AuthenticationError {
    #[error("owner-protocol handshake schema is invalid")]
    Schema(#[from] crate::schema::SchemaError),
    #[error("owner-protocol selection is invalid")]
    Protocol(#[from] crate::schema::ProtocolSelectionError),
    #[error("owner-protocol transcript selection mismatch")]
    Selection,
    #[error("owner-protocol transcript binding mismatch")]
    Binding,
    #[error("owner-protocol release-policy self fields mismatch")]
    Policy,
    #[error("owner-protocol release-policy pair is unauthorized for the connection purpose")]
    PolicyPair,
    #[error("owner-protocol platform trust verification failed")]
    Trust,
    #[error("owner-protocol key-agreement mode is invalid")]
    KeyAgreement,
    #[error("owner-protocol key derivation failed")]
    KeyDerivation,
    #[error("owner-protocol authentication proof is invalid")]
    Proof,
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    #[error("operating-system randomness unavailable")]
    Random,
}

fn append_protocol(output: &mut Vec<u8>, protocol: ProtocolHeader) {
    output.extend_from_slice(&protocol.major.to_be_bytes());
    output.extend_from_slice(&protocol.minor.to_be_bytes());
    output.extend_from_slice(&protocol.compatibility_epoch.to_be_bytes());
    output.extend_from_slice(&protocol.supported_feature_bits.get().to_be_bytes());
    output.extend_from_slice(&protocol.required_feature_bits.get().to_be_bytes());
}

fn append_selected_protocol(output: &mut Vec<u8>, protocol: SelectedProtocol) {
    output.extend_from_slice(&protocol.major.to_be_bytes());
    output.extend_from_slice(&protocol.minor.to_be_bytes());
    output.extend_from_slice(&protocol.compatibility_epoch.to_be_bytes());
    output.extend_from_slice(&protocol.feature_bits.get().to_be_bytes());
}

fn append_public_key(output: &mut Vec<u8>, key: Option<&P256PublicKey>) {
    match key {
        Some(key) => output.extend_from_slice(key.as_bytes()),
        None => output.extend_from_slice(&[0; 65]),
    }
}

fn expand(hkdf: &Hkdf<Sha256>, info: &[u8]) -> Result<[u8; 32], AuthenticationError> {
    let mut output = [0_u8; 32];
    hkdf.expand(info, &mut output)
        .map_err(|_| AuthenticationError::KeyDerivation)?;
    Ok(output)
}

fn hmac(key: &[u8; 32], message: &[u8]) -> Bytes32 {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(message);
    Bytes32::new(mac.finalize().into_bytes().into())
}

fn verify_hmac(
    key: &[u8; 32],
    message: &[u8],
    expected: &Bytes32,
) -> Result<(), AuthenticationError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(message);
    mac.verify_slice(expected.as_bytes())
        .map_err(|_| AuthenticationError::Proof)
}

fn validate_policy_self_fields(
    hello: &Hello,
    challenge: &Challenge,
) -> Result<
    (
        crate::release_policy::ReleasePolicy,
        crate::release_policy::ReleasePolicy,
    ),
    AuthenticationError,
> {
    let client = hello
        .client_release_policy
        .decode()
        .map_err(|_| AuthenticationError::Policy)?;
    let owner = challenge
        .owner_release_policy
        .decode()
        .map_err(|_| AuthenticationError::Policy)?;
    if client.platform != hello.platform
        || client.architecture != hello.architecture
        || client.gateway_protocol != hello.protocol
        || !client
            .release_build_digest
            .constant_time_eq(&hello.release_build_digest)
        || !client
            .gateway_sha256
            .constant_time_eq(&hello.executable_sha256)
        || !client
            .gateway_signer_policy_digest
            .constant_time_eq(&hello.signer_policy_digest)
        || owner.platform != challenge.platform
        || owner.architecture != challenge.architecture
        || owner.owner_protocol != challenge.owner_protocol
        || !owner
            .release_build_digest
            .constant_time_eq(&challenge.release_build_digest)
        || !owner
            .owner_sha256
            .constant_time_eq(&challenge.executable_sha256)
        || !owner
            .owner_signer_policy_digest
            .constant_time_eq(&challenge.signer_policy_digest)
    {
        return Err(AuthenticationError::Policy);
    }
    Ok((client, owner))
}

fn validate_policy_pair(
    purpose: Purpose,
    client: &crate::release_policy::ReleasePolicy,
    owner: &crate::release_policy::ReleasePolicy,
) -> Result<(), AuthenticationError> {
    let exact = client == owner;
    let client_names_owner_predecessor = client.predecessor.as_ref().is_some_and(|previous| {
        previous.platform == owner.platform
            && previous.architecture == owner.architecture
            && previous.release_build_digest == owner.release_build_digest
            && previous.gateway_sha256 == owner.gateway_sha256
            && previous.owner_sha256 == owner.owner_sha256
    });
    let owner_names_client_predecessor = owner.predecessor.as_ref().is_some_and(|previous| {
        previous.platform == client.platform
            && previous.architecture == client.architecture
            && previous.release_build_digest == client.release_build_digest
            && previous.gateway_sha256 == client.gateway_sha256
            && previous.owner_sha256 == client.owner_sha256
    });
    let compatible = match purpose {
        Purpose::Observe | Purpose::Capture => exact,
        Purpose::Maintenance => {
            exact || client_names_owner_predecessor || owner_names_client_predecessor
        }
    };
    if compatible {
        Ok(())
    } else {
        Err(AuthenticationError::PolicyPair)
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthenticationKeys, KeyAgreementMaterial};
    use crate::scalar::Bytes32;

    #[test]
    fn rfc_5869_sha256_case_one_matches_extract_and_expand() {
        // RFC 5869 case 1 verifies the pinned HKDF implementation independently
        // of the owner transcript construction.
        let ikm = vec![0x0b; 22];
        let salt = hex("000102030405060708090a0b0c");
        let info = hex("f0f1f2f3f4f5f6f7f8f9");
        let (_, hkdf) = hkdf::Hkdf::<sha2::Sha256>::extract(Some(&salt), &ikm);
        let mut okm = [0_u8; 42];
        hkdf.expand(&info, &mut okm)
            .expect("valid RFC output length");
        assert_eq!(
            okm.as_slice(),
            hex("3cb25f25faacd57a90434f64d0362f2a\
                 2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
                 34007208d5b887185865")
        );
    }

    #[test]
    fn proof_verification_rejects_one_bit_change() {
        let transcript = super::Transcript {
            bytes: vec![],
            hash: Bytes32::new([7; 32]),
            selected_protocol: crate::schema::SelectedProtocol {
                major: 1,
                minor: 0,
                compatibility_epoch: 1,
                feature_bits: crate::scalar::FeatureBits::new(1),
            },
            purpose: crate::schema::Purpose::Capture,
            authority_ceiling: crate::schema::AuthorityCeiling::Capture,
            session_id: Bytes32::new([0; 32]),
            owner_instance_id: Bytes32::new([1; 32]),
            key_agreement_mode: super::KeyAgreementMode::MacosKeychain,
            client_p256_public_key: None,
            owner_p256_public_key: None,
        };
        let mut secret = [9; 32];
        let material = KeyAgreementMaterial::macos(&mut secret);
        assert_eq!(secret, [0; 32]);
        let keys = AuthenticationKeys::derive(&transcript, &material).expect("derive");
        let proofs = keys.proofs(&transcript);
        let authenticated = crate::schema::Authenticated::new(
            transcript.selected_protocol,
            transcript.purpose,
            transcript.authority_ceiling,
            proofs.owner_proof,
        )
        .expect("authenticated payload");
        keys.verify_authenticated_finish(&transcript, &proofs.client_proof, &authenticated)
            .expect("valid authenticated finish");
        let finish = crate::envelope::AuthenticatedEnvelope::authenticated_finish(
            transcript.session_id,
            &authenticated,
        )
        .expect("finish")
        .encode_body(keys.owner_frame_key())
        .expect("finish body");
        let session = keys
            .pending_gateway_session(&transcript)
            .expect("pending")
            .accept_authenticated_finish(&finish, &proofs.client_proof)
            .expect("atomic finish");
        assert_eq!(session.purpose(), transcript.purpose);

        let mut changed = *proofs.owner_proof.as_bytes();
        changed[0] ^= 1;
        assert!(
            keys.verify_owner_proof(&transcript, &proofs.client_proof, &Bytes32::new(changed))
                .is_err()
        );
        let wrong = crate::schema::Authenticated::new(
            transcript.selected_protocol,
            transcript.purpose,
            transcript.authority_ceiling,
            Bytes32::new(changed),
        )
        .expect("wrong proof payload");
        let wrong_finish = crate::envelope::AuthenticatedEnvelope::authenticated_finish(
            transcript.session_id,
            &wrong,
        )
        .expect("finish")
        .encode_body(keys.owner_frame_key())
        .expect("finish body");
        assert!(
            keys.pending_gateway_session(&transcript)
                .expect("pending")
                .accept_authenticated_finish(&wrong_finish, &proofs.client_proof)
                .is_err()
        );
    }

    #[test]
    fn invalid_p256_private_scalar_is_rejected() {
        assert!(super::EphemeralP256Secret::from_bytes([0; 32]).is_err());
    }

    #[test]
    fn release_policy_pair_allows_only_exact_or_one_hop_maintenance() {
        use crate::release_policy::{OwnerMode, ReleasePolicy, ReleasePolicyPredecessor};
        use crate::scalar::FeatureBits;
        use crate::schema::{Architecture, Platform, ProtocolHeader, Purpose};

        let header = ProtocolHeader {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            supported_feature_bits: FeatureBits::new(1),
            required_feature_bits: FeatureBits::new(1),
        };
        let policy = |byte: u8| ReleasePolicy {
            platform: Platform::Windows,
            architecture: Architecture::X64,
            owner_mode: OwnerMode::SafeDisabled,
            release_build_digest: Bytes32::new([byte; 32]),
            gateway_sha256: Bytes32::new([byte + 1; 32]),
            owner_sha256: Bytes32::new([byte + 2; 32]),
            gateway_signer_policy_digest: Bytes32::new([4; 32]),
            owner_signer_policy_digest: Bytes32::new([5; 32]),
            gateway_protocol: header,
            owner_protocol: header,
            predecessor: None,
        };
        let old = policy(10);
        let mut new = policy(20);
        assert!(super::validate_policy_pair(Purpose::Capture, &old, &old).is_ok());
        assert!(super::validate_policy_pair(Purpose::Capture, &old, &new).is_err());
        assert!(super::validate_policy_pair(Purpose::Maintenance, &old, &new).is_err());
        new.predecessor = Some(ReleasePolicyPredecessor {
            release_build_digest: old.release_build_digest,
            gateway_sha256: old.gateway_sha256,
            owner_sha256: old.owner_sha256,
            platform: old.platform,
            architecture: old.architecture,
        });
        assert!(super::validate_policy_pair(Purpose::Maintenance, &new, &old).is_ok());
        assert!(super::validate_policy_pair(Purpose::Maintenance, &old, &new).is_ok());
        assert!(super::validate_policy_pair(Purpose::Capture, &new, &old).is_err());
    }

    #[test]
    fn frozen_cross_language_protocol_vectors_conform() {
        use crate::auth::{KeyAgreementMode, Transcript, TranscriptInput};
        use crate::envelope::{AuthenticatedEnvelope, CorrelationTracker};
        use crate::framing::encode_outer_frame;
        use crate::scalar::{FeatureBits, P256PublicKey};
        use crate::schema::{
            Architecture, Authenticated, AuthorityCeiling, Empty, ErrorBody, ErrorCode, Platform,
            ProtocolHeader, Purpose, Request, Response, SelectedProtocol,
        };

        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/compatibility/keyboard-owner-v1/owner-protocol-vectors.json"
        ))
        .expect("valid fixture JSON");
        assert_eq!(fixture["fixtureVersion"], 1);
        for vector in fixture["vectors"].as_array().expect("vectors") {
            let input = &vector["input"];
            let expected = &vector["expected"];
            let protocol = |name: &str| -> ProtocolHeader {
                serde_json::from_value(input[name].clone()).expect("protocol header")
            };
            let selected: SelectedProtocol =
                serde_json::from_value(input["selectedProtocol"].clone()).expect("selected");
            let platform = match text(input, "platform") {
                "windows" => Platform::Windows,
                "macos" => Platform::Macos,
                _ => panic!("fixture platform"),
            };
            let architecture = |name: &str| match text(input, name) {
                "x64" => Architecture::X64,
                "arm64" => Architecture::Arm64,
                _ => panic!("fixture architecture"),
            };
            let public = |name: &str| {
                input[name].as_str().map(|value| {
                    P256PublicKey::from_sec1_bytes(
                        hex(value).try_into().expect("65-byte public point"),
                    )
                    .expect("valid public point")
                })
            };
            let transcript_input = TranscriptInput {
                client_protocol: protocol("clientProtocol"),
                owner_protocol: protocol("ownerProtocol"),
                selected_protocol: selected,
                purpose: Purpose::Capture,
                authority_ceiling: AuthorityCeiling::Capture,
                platform,
                client_architecture: architecture("clientArchitecture"),
                owner_architecture: architecture("ownerArchitecture"),
                client_nonce: hex32(text(input, "clientNonce")),
                owner_nonce: hex32(text(input, "ownerNonce")),
                session_id: hex32(text(input, "sessionId")),
                owner_instance_id: hex32(text(input, "ownerInstanceId")),
                client_release_build_digest: hex32(text(input, "clientReleaseBuildDigest")),
                client_executable_digest: hex32(text(input, "clientExecutableDigest")),
                owner_release_build_digest: hex32(text(input, "ownerReleaseBuildDigest")),
                owner_executable_digest: hex32(text(input, "ownerExecutableDigest")),
                installation_identity_digest: hex32(text(input, "installationIdentityDigest")),
                client_signer_policy_digest: hex32(text(input, "clientSignerPolicyDigest")),
                owner_signer_policy_digest: hex32(text(input, "ownerSignerPolicyDigest")),
                os_session_binding_digest: hex32(text(input, "osSessionBindingDigest")),
                client_release_policy_digest: hex32(text(input, "clientReleasePolicyDigest")),
                owner_release_policy_digest: hex32(text(input, "ownerReleasePolicyDigest")),
                platform_credential_binding_digest: hex32(text(
                    input,
                    "platformCredentialBindingDigest",
                )),
                key_agreement_mode: match text(input, "keyAgreementMode") {
                    "macos_keychain" => KeyAgreementMode::MacosKeychain,
                    "windows_stable_pipe_p256" => KeyAgreementMode::WindowsStablePipeP256,
                    _ => panic!("fixture key mode"),
                },
                client_p256_public_key: public("clientP256PublicKey"),
                owner_p256_public_key: public("ownerP256PublicKey"),
            };
            assert_eq!(
                transcript_input.selected_protocol.feature_bits,
                FeatureBits::new(1)
            );
            let transcript = Transcript::build(&transcript_input).expect("transcript");
            assert_eq!(transcript.as_bytes(), hex(text(expected, "transcriptHex")));
            for field in 0..15 {
                let mut changed = transcript_input.clone();
                let replacement = Bytes32::new([0xf0 + field; 32]);
                match field {
                    0 => changed.client_nonce = replacement,
                    1 => changed.owner_nonce = replacement,
                    2 => changed.session_id = replacement,
                    3 => changed.owner_instance_id = replacement,
                    4 => changed.client_release_build_digest = replacement,
                    5 => changed.client_executable_digest = replacement,
                    6 => changed.owner_release_build_digest = replacement,
                    7 => changed.owner_executable_digest = replacement,
                    8 => changed.installation_identity_digest = replacement,
                    9 => changed.client_signer_policy_digest = replacement,
                    10 => changed.owner_signer_policy_digest = replacement,
                    11 => changed.os_session_binding_digest = replacement,
                    12 => changed.client_release_policy_digest = replacement,
                    13 => changed.owner_release_policy_digest = replacement,
                    14 => changed.platform_credential_binding_digest = replacement,
                    _ => unreachable!(),
                }
                assert_ne!(
                    Transcript::build(&changed)
                        .expect("mutated transcript")
                        .hash(),
                    transcript.hash(),
                    "transcript field {field} was not bound"
                );
            }
            assert_eq!(transcript.hash(), hex32(text(expected, "transcriptSha256")));

            let expected_ikm = hex(text(input, "platformIkm"));
            let material = if platform == Platform::Windows {
                let client_secret = super::EphemeralP256Secret::from_bytes(hex32_array(text(
                    input,
                    "clientP256PrivateKey",
                )))
                .expect("client private scalar");
                let owner_secret = super::EphemeralP256Secret::from_bytes(hex32_array(text(
                    input,
                    "ownerP256PrivateKey",
                )))
                .expect("owner private scalar");
                assert_eq!(
                    client_secret.public_key(),
                    transcript_input
                        .client_p256_public_key
                        .as_ref()
                        .expect("client public")
                );
                assert_eq!(
                    owner_secret.public_key(),
                    transcript_input
                        .owner_p256_public_key
                        .as_ref()
                        .expect("owner public")
                );
                let client_material = KeyAgreementMaterial::windows_peer(
                    super::PeerRole::Gateway,
                    &client_secret,
                    owner_secret.public_key(),
                )
                .expect("client ECDH");
                let owner_material = KeyAgreementMaterial::windows_peer(
                    super::PeerRole::Owner,
                    &owner_secret,
                    client_secret.public_key(),
                )
                .expect("owner ECDH");
                assert_eq!(client_material.ikm, owner_material.ikm);
                assert_eq!(client_material.ikm, expected_ikm);
                assert_eq!(client_material.ikm, hex(text(expected, "rawP256AffineX")));
                client_material
            } else {
                let mut secret = hex32_array(text(input, "macosSecret"));
                let material = KeyAgreementMaterial::macos(&mut secret);
                assert_eq!(secret, [0; 32]);
                assert_eq!(material.ikm, expected_ikm);
                material
            };
            let keys = AuthenticationKeys::derive(&transcript, &material).expect("keys");
            assert_eq!(
                keys.client_proof_key,
                hex32_array(text(expected, "clientProofKey"))
            );
            assert_eq!(
                keys.owner_proof_key,
                hex32_array(text(expected, "ownerProofKey"))
            );
            assert_eq!(
                *keys.gateway_frame_key.as_bytes(),
                hex32_array(text(expected, "gatewayFrameKey"))
            );
            assert_eq!(
                *keys.owner_frame_key.as_bytes(),
                hex32_array(text(expected, "ownerFrameKey"))
            );
            let proofs = keys.proofs(&transcript);
            assert_eq!(proofs.client_proof, hex32(text(expected, "clientProof")));
            assert_eq!(proofs.owner_proof, hex32(text(expected, "ownerProof")));
            keys.verify_owner_proof(&transcript, &proofs.client_proof, &proofs.owner_proof)
                .expect("proof verification");

            let request = Request::HealthGet(Empty {});
            let frame = &expected["gatewayRequestFrame"];
            let envelope = AuthenticatedEnvelope::request(transcript_input.session_id, 1, &request)
                .expect("envelope");
            assert_eq!(envelope.payload(), text(frame, "payloadUtf8").as_bytes());
            let body = envelope
                .encode_body(keys.gateway_frame_key())
                .expect("encoded body");
            assert_eq!(body, hex(text(frame, "bodyHex")));
            assert_eq!(&body[body.len() - 32..], hex(text(frame, "mac")));
            assert_eq!(
                encode_outer_frame(&body).expect("outer frame"),
                hex(text(frame, "outerFrameHex"))
            );

            let authenticated = Authenticated::new(
                transcript_input.selected_protocol,
                transcript_input.purpose,
                transcript_input.authority_ceiling,
                proofs.owner_proof,
            )
            .expect("authenticated payload");
            let finish_frame = &expected["authenticatedFinishFrame"];
            let finish_envelope = AuthenticatedEnvelope::authenticated_finish(
                transcript_input.session_id,
                &authenticated,
            )
            .expect("finish envelope");
            assert_eq!(
                finish_envelope.payload(),
                text(finish_frame, "payloadUtf8").as_bytes()
            );
            let finish_body = finish_envelope
                .encode_body(keys.owner_frame_key())
                .expect("finish body");
            assert_eq!(finish_body, hex(text(finish_frame, "bodyHex")));

            let mut session = keys
                .pending_gateway_session(&transcript)
                .expect("pending session")
                .accept_authenticated_finish(&finish_body, &proofs.client_proof)
                .expect("authenticated session");
            assert_eq!(session.session_id(), transcript_input.session_id);
            assert_eq!(session.purpose(), Purpose::Capture);

            let response = Response::Error(ErrorBody::new(ErrorCode::Unavailable));
            let owner_frame = &expected["ownerResponseFrame"];
            let owner_envelope = AuthenticatedEnvelope::response_for_request(
                transcript_input.session_id,
                2,
                1,
                &request,
                &response,
            )
            .expect("owner envelope");
            assert_eq!(
                owner_envelope.payload(),
                text(owner_frame, "payloadUtf8").as_bytes()
            );
            let owner_body = owner_envelope
                .encode_body(keys.owner_frame_key())
                .expect("owner body");
            assert_eq!(owner_body, hex(text(owner_frame, "bodyHex")));
            assert_eq!(
                &owner_body[owner_body.len() - 32..],
                hex(text(owner_frame, "mac"))
            );
            assert_eq!(
                encode_outer_frame(&owner_body).expect("owner outer frame"),
                hex(text(owner_frame, "outerFrameHex"))
            );
            let mut correlations = CorrelationTracker::new();
            correlations
                .register_request(1, &request)
                .expect("request correlation");
            assert_eq!(
                session
                    .inbound()
                    .accept_response(&owner_body, &mut correlations)
                    .expect("typed response")
                    .1,
                response
            );
            assert!(correlations.is_empty());
        }
    }

    fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
        value[field].as_str().expect("fixture string")
    }

    fn hex32(value: &str) -> Bytes32 {
        Bytes32::new(hex32_array(value))
    }

    fn hex32_array(value: &str) -> [u8; 32] {
        hex(value).try_into().expect("32-byte hex")
    }

    fn hex(value: &str) -> Vec<u8> {
        let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        compact
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex")
            })
            .collect()
    }
}
