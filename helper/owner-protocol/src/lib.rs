#![forbid(unsafe_code)]
//! Platform-neutral implementation of Keyboard Owner protocol v1.
//!
//! This crate owns only strict wire schemas, framing, transcript construction,
//! authentication primitives, and transport/correlation validation. It has no
//! endpoint, service, native-input, process-launch, or packaging code.

pub mod auth;
pub mod client;
pub mod envelope;
#[cfg(any(test, feature = "test-transport"))]
pub mod fake_transport;
pub mod framing;
pub mod macos_maintenance;
pub mod release_policy;
pub mod scalar;
pub mod schema;
pub mod session;
pub mod transport;

pub use auth::{
    AuthenticatedSession, AuthenticationError, AuthenticationKeys, AuthenticationProofs,
    EphemeralP256Secret, HandshakeTrustVerifier, KeyAgreementMaterial, PeerRole,
    PendingAuthenticatedSession, Transcript, TranscriptInput,
};
pub use envelope::{
    AuthenticatedEnvelope, CapabilityKind, CapabilitySequenceValidator, CorrelationTracker,
    Direction, EnvelopeKind, EnvelopeReceiver, FrameKey, OwnerMessage, PredecessorRouteValidator,
    SequenceValidator,
};
pub use framing::{decode_outer_frame, encode_outer_frame, read_outer_frame};
pub use release_policy::{ReleasePolicy, ReleasePolicyPredecessor};
pub use scalar::{Bytes32, Counter, FeatureBits, P256PublicKey, U64String};
pub use schema::{
    Authenticated, Challenge, Hello, Method, ProtocolHeader, Request, Response, SelectedProtocol,
};
#[cfg(any(test, feature = "test-transport"))]
pub use session::FakeAuthenticatedMaterial;
pub use session::{
    EncodedRequest, GatewayMessage, GatewaySessionCodec, OwnerSessionCodec, ReceivedRequest,
    SessionCodecError,
};
pub use transport::{
    FlushReceipt, OrderedTransport, Progress as TransportProgress, ReceiveResult,
    StreamOrderedTransport, TransportError,
};

/// The implemented, still non-runnable structural slice.
pub const STRUCTURAL_STAGE: &str = "b1-protocol-v1";
/// Protocol major implemented by this crate.
pub const PROTOCOL_MAJOR: u16 = 1;
/// Initial compatibility epoch.
pub const COMPATIBILITY_EPOCH: u32 = 1;
/// Baseline owner-protocol v1 capability. Every v1 peer requires this bit.
pub const BASE_V1: u64 = 1;
/// Negotiated exact display metadata response on `front_app.metadata.get`.
pub const FRONT_APP_METADATA_V1: u64 = 1 << 1;
/// Negotiated privacy-safe registered-input boundary aggregates.
pub const REGISTERED_INPUT_OBSERVABILITY_V1: u64 = 1 << 2;
/// Features implemented by the production v1 endpoints. Additive capabilities
/// remain optional so frozen BASE-only v1 peers retain feature negotiation.
pub const PRODUCTION_V1_SUPPORTED_FEATURES: u64 =
    BASE_V1 | FRONT_APP_METADATA_V1 | REGISTERED_INPUT_OBSERVABILITY_V1;
pub const PRODUCTION_V1_REQUIRED_FEATURES: u64 = BASE_V1;

/// One canonical live/release-policy header for production owner-protocol v1.
#[must_use]
pub const fn production_v1_protocol_header() -> ProtocolHeader {
    ProtocolHeader {
        major: PROTOCOL_MAJOR,
        minor: 0,
        compatibility_epoch: COMPATIBILITY_EPOCH,
        supported_feature_bits: FeatureBits::new(PRODUCTION_V1_SUPPORTED_FEATURES),
        required_feature_bits: FeatureBits::new(PRODUCTION_V1_REQUIRED_FEATURES),
    }
}

#[cfg(test)]
mod production_header_tests {
    use super::*;

    #[test]
    fn production_header_validates_and_preserves_optional_v1_negotiation() {
        let production = production_v1_protocol_header();
        production.validate().expect("production header");
        assert_eq!(production.supported_feature_bits.get(), 0x7);
        assert_eq!(production.required_feature_bits.get(), BASE_V1);
        assert_eq!(
            production
                .negotiate(production)
                .expect("same production peer")
                .feature_bits
                .get(),
            PRODUCTION_V1_SUPPORTED_FEATURES
        );
        let frozen_base_peer = ProtocolHeader {
            supported_feature_bits: FeatureBits::new(BASE_V1),
            required_feature_bits: FeatureBits::new(BASE_V1),
            ..production
        };
        assert_eq!(
            production
                .negotiate(frozen_base_peer)
                .expect("frozen BASE-only peer remains compatible")
                .feature_bits
                .get(),
            BASE_V1
        );
    }
}
