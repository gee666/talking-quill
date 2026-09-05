//! Ordered authenticated session codecs shared by owner clients and servers.
//!
//! The codecs hide directional transport-sequence allocation and keep response
//! correlation plus predecessor routing in the same state that authenticates
//! incoming envelopes. They perform no I/O and own no OS endpoint.

use std::collections::BTreeMap;
use std::fmt;

use thiserror::Error;

use crate::auth::{AuthenticatedSession, AuthenticationError};
use crate::envelope::{
    AuthenticatedEnvelope, CorrelationTracker, Direction, EnvelopeError, FrameKey, OwnerMessage,
    PredecessorRouteValidator,
};
use crate::framing::{FramingError, decode_outer_frame, encode_outer_frame};
use crate::scalar::Bytes32;
use crate::schema::{
    ActivationEvent, Event, PasteResult, Phase, PredecessorTerminalEvent, Purpose, Request,
    Response, SessionKey, SuccessResult,
};

/// One request accepted atomically by the authenticated owner receiver.
pub struct ReceivedRequest {
    envelope: AuthenticatedEnvelope,
    request: Request,
}

impl fmt::Debug for ReceivedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReceivedRequest(<redacted>)")
    }
}

impl ReceivedRequest {
    #[must_use]
    pub const fn transport_sequence(&self) -> u64 {
        self.envelope.transport_sequence()
    }

    #[must_use]
    pub const fn request(&self) -> &Request {
        &self.request
    }

    #[must_use]
    pub fn into_request(self) -> Request {
        self.request
    }
}

/// One owner-to-gateway message accepted by the mixed authenticated receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayMessage {
    Response {
        correlation_sequence: u64,
        response: Response,
    },
    Event(Event),
    PredecessorTerminal(PredecessorTerminalEvent),
}

/// Result of encoding a request. Its sequence is the immutable correlation
/// handle for the one response; callers must never retransmit it.
pub struct EncodedRequest {
    transport_sequence: u64,
    frame: Vec<u8>,
}

impl fmt::Debug for EncodedRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncodedRequest(<redacted>)")
    }
}

impl EncodedRequest {
    #[must_use]
    pub const fn transport_sequence(&self) -> u64 {
        self.transport_sequence
    }

    #[must_use]
    pub fn into_frame(self) -> Vec<u8> {
        self.frame
    }
}

#[derive(Debug, Error)]
pub enum SessionCodecError {
    #[error("owner-protocol event is outside the current capture route")]
    EventRoute,
    #[error("owner-protocol session direction is invalid")]
    Direction,
    #[error("owner-protocol feature was not negotiated")]
    FeatureNotNegotiated,
    #[error("owner-protocol directional transport sequence exhausted")]
    SequenceExhausted,
    #[error(transparent)]
    Framing(#[from] FramingError),
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    #[error(transparent)]
    Authentication(#[from] AuthenticationError),
    #[error(transparent)]
    Correlation(#[from] crate::envelope::CorrelationError),
}

mod capture_route;
mod gateway;
mod owner;
use capture_route::{CaptureEventRoute, is_capture_scoped_event};
pub use gateway::GatewaySessionCodec;
pub use owner::OwnerSessionCodec;
#[cfg(any(test, feature = "test-transport"))]
mod fake;
#[cfg(any(test, feature = "test-transport"))]
pub use fake::FakeAuthenticatedMaterial;
