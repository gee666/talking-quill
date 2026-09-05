//! Handshake authentication. Opaque transcript, key, and session fields stay
//! private here; child modules own transcript construction, policy checks,
//! key agreement, proof derivation, and the authenticated-finish transition.

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

pub struct EphemeralP256Secret {
    secret: SecretKey,
    public: P256PublicKey,
}

impl fmt::Debug for EphemeralP256Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EphemeralP256Secret([REDACTED])")
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

mod key_agreement;
mod keys;
mod policy;
mod session;
mod transcript;
use policy::{validate_policy_pair, validate_policy_self_fields};
#[cfg(test)]
mod tests;
