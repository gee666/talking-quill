use std::collections::BTreeMap;
use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::framing::MAX_BODY_LENGTH;
use crate::scalar::Bytes32;
use crate::schema::{
    Event, Method, PredecessorTerminalEvent, Purpose, Request, Response, SessionMode, SuccessResult,
};

const FRAME_DOMAIN: &[u8] = b"TQKO-FRAME-V1\0";
const ENVELOPE_MAGIC: &[u8; 4] = b"TQKO";
const ENVELOPE_VERSION: u8 = 1;
const FIXED_WITHOUT_MAC: usize = 60;
const MAC_LENGTH: usize = 32;
pub const ENVELOPE_OVERHEAD: usize = FIXED_WITHOUT_MAC + MAC_LENGTH;
pub const MAX_PAYLOAD_LENGTH: usize = MAX_BODY_LENGTH - ENVELOPE_OVERHEAD;
pub const MAX_OUTSTANDING_REQUESTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EnvelopeKind {
    Request = 1,
    Response = 2,
    Event = 3,
    PredecessorTerminalEvent = 4,
}

impl EnvelopeKind {
    fn from_tag(tag: u8) -> Result<Self, EnvelopeError> {
        match tag {
            1 => Ok(Self::Request),
            2 => Ok(Self::Response),
            3 => Ok(Self::Event),
            4 => Ok(Self::PredecessorTerminalEvent),
            _ => Err(EnvelopeError::Kind),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    GatewayToOwner = 1,
    OwnerToGateway = 2,
}

impl Direction {
    fn from_tag(tag: u8) -> Result<Self, EnvelopeError> {
        match tag {
            1 => Ok(Self::GatewayToOwner),
            2 => Ok(Self::OwnerToGateway),
            _ => Err(EnvelopeError::Direction),
        }
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct FrameKey {
    #[zeroize(skip)]
    direction: Direction,
    bytes: [u8; 32],
}

impl FrameKey {
    /// Constructs a directional key from already-authenticated key material.
    /// Normal protocol users obtain these from [`crate::AuthenticationKeys`].
    #[must_use]
    pub const fn from_secret(direction: Direction, bytes: [u8; 32]) -> Self {
        Self { direction, bytes }
    }

    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    pub(crate) const fn duplicate(&self) -> Self {
        Self::from_secret(self.direction, self.bytes)
    }
}

impl fmt::Debug for FrameKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FrameKey([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedEnvelope {
    kind: EnvelopeKind,
    direction: Direction,
    session_id: Bytes32,
    transport_sequence: u64,
    correlation_sequence: u64,
    payload: Vec<u8>,
}

impl fmt::Debug for AuthenticatedEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticatedEnvelope([REDACTED])")
    }
}

impl AuthenticatedEnvelope {
    pub(crate) fn new(
        kind: EnvelopeKind,
        direction: Direction,
        session_id: Bytes32,
        transport_sequence: u64,
        correlation_sequence: u64,
        payload: Vec<u8>,
    ) -> Result<Self, EnvelopeError> {
        if transport_sequence == 0 || payload.len() > MAX_PAYLOAD_LENGTH {
            return Err(EnvelopeError::Length);
        }
        std::str::from_utf8(&payload).map_err(|_| EnvelopeError::Utf8)?;
        let envelope = Self {
            kind,
            direction,
            session_id,
            transport_sequence,
            correlation_sequence,
            payload,
        };
        envelope.validate_regular_shape()?;
        Ok(envelope)
    }

    pub fn request(
        session_id: Bytes32,
        transport_sequence: u64,
        request: &Request,
    ) -> Result<Self, EnvelopeError> {
        Self::new(
            EnvelopeKind::Request,
            Direction::GatewayToOwner,
            session_id,
            transport_sequence,
            0,
            request
                .to_json()
                .map_err(|_| EnvelopeError::PayloadSchema)?,
        )
    }

    pub fn response_for_request(
        session_id: Bytes32,
        transport_sequence: u64,
        request_transport_sequence: u64,
        request: &Request,
        response: &Response,
    ) -> Result<Self, EnvelopeError> {
        if !ResponseExpectation::from_request(request).validates(response) {
            return Err(EnvelopeError::CorrelationState(CorrelationError::Mismatch));
        }
        Self::new(
            EnvelopeKind::Response,
            Direction::OwnerToGateway,
            session_id,
            transport_sequence,
            request_transport_sequence,
            response
                .to_json()
                .map_err(|_| EnvelopeError::PayloadSchema)?,
        )
    }

    pub fn event(
        session_id: Bytes32,
        transport_sequence: u64,
        event: &crate::schema::Event,
    ) -> Result<Self, EnvelopeError> {
        Self::new(
            EnvelopeKind::Event,
            Direction::OwnerToGateway,
            session_id,
            transport_sequence,
            0,
            event.to_json().map_err(|_| EnvelopeError::PayloadSchema)?,
        )
    }

    pub fn predecessor_terminal_event(
        session_id: Bytes32,
        transport_sequence: u64,
        event: &crate::schema::PredecessorTerminalEvent,
    ) -> Result<Self, EnvelopeError> {
        Self::new(
            EnvelopeKind::PredecessorTerminalEvent,
            Direction::OwnerToGateway,
            session_id,
            transport_sequence,
            0,
            event.to_json().map_err(|_| EnvelopeError::PayloadSchema)?,
        )
    }

    /// Constructs the sole response/correlation-zero exception.
    pub fn authenticated_finish(
        session_id: Bytes32,
        authenticated: &crate::schema::Authenticated,
    ) -> Result<Self, EnvelopeError> {
        let payload = authenticated
            .to_json()
            .map_err(|_| EnvelopeError::PayloadSchema)?;
        Ok(Self {
            kind: EnvelopeKind::Response,
            direction: Direction::OwnerToGateway,
            session_id,
            transport_sequence: 1,
            correlation_sequence: 0,
            payload,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> EnvelopeKind {
        self.kind
    }

    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    #[must_use]
    pub const fn session_id(&self) -> Bytes32 {
        self.session_id
    }

    #[must_use]
    pub const fn transport_sequence(&self) -> u64 {
        self.transport_sequence
    }

    #[must_use]
    pub const fn correlation_sequence(&self) -> u64 {
        self.correlation_sequence
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn validate_regular_shape(&self) -> Result<(), EnvelopeError> {
        let valid_role = matches!(
            (self.direction, self.kind),
            (Direction::GatewayToOwner, EnvelopeKind::Request)
                | (Direction::OwnerToGateway, EnvelopeKind::Response)
                | (Direction::OwnerToGateway, EnvelopeKind::Event)
                | (
                    Direction::OwnerToGateway,
                    EnvelopeKind::PredecessorTerminalEvent
                )
        );
        let valid_correlation = match self.kind {
            EnvelopeKind::Request
            | EnvelopeKind::Event
            | EnvelopeKind::PredecessorTerminalEvent => self.correlation_sequence == 0,
            EnvelopeKind::Response => self.correlation_sequence != 0,
        };
        if !valid_role {
            Err(EnvelopeError::Direction)
        } else if !valid_correlation {
            Err(EnvelopeError::Correlation)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum EnvelopeError {
    #[error("owner-protocol authenticated envelope length is invalid")]
    Length,
    #[error("owner-protocol authenticated envelope header is invalid")]
    Header,
    #[error("owner-protocol authenticated envelope kind is invalid")]
    Kind,
    #[error("owner-protocol authenticated envelope direction is invalid")]
    Direction,
    #[error("owner-protocol authenticated envelope flags are invalid")]
    Flags,
    #[error("owner-protocol authenticated envelope session is invalid")]
    Session,
    #[error("owner-protocol authenticated envelope MAC is invalid")]
    Mac,
    #[error("owner-protocol authenticated envelope payload is not UTF-8")]
    Utf8,
    #[error("owner-protocol authenticated envelope correlation is invalid")]
    Correlation,
    #[error("owner-protocol authenticated envelope payload schema is invalid")]
    PayloadSchema,
    #[error("owner-protocol method is unavailable to this connection purpose")]
    MethodAuthority,
    #[error(transparent)]
    Sequence(#[from] SequenceError),
    #[error(transparent)]
    CorrelationState(#[from] CorrelationError),
    #[error(transparent)]
    TerminalRoute(#[from] TerminalRouteError),
}

mod capability;
mod correlation;
mod predecessor;
mod receiver;
mod sequence;
mod wire;
pub use capability::{CapabilityKind, CapabilitySequenceError, CapabilitySequenceValidator};
use correlation::ResponseExpectation;
pub use correlation::{CorrelationError, CorrelationTracker};
pub use predecessor::{PredecessorRouteValidator, TerminalRouteError};
pub use receiver::{EnvelopeReceiver, OwnerMessage};
pub use sequence::{SequenceError, SequenceValidator};
#[cfg(test)]
use wire::frame_mac;
#[cfg(test)]
mod tests;
