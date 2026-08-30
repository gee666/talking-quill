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

    pub fn encode_body(&self, frame_key: &FrameKey) -> Result<Vec<u8>, EnvelopeError> {
        let authenticated_finish = self.kind == EnvelopeKind::Response
            && self.direction == Direction::OwnerToGateway
            && self.transport_sequence == 1
            && self.correlation_sequence == 0
            && matches!(
                crate::schema::parse_handshake_json(&self.payload),
                Ok(crate::schema::HandshakeMessage::Authenticated(_))
            );
        if !authenticated_finish {
            self.validate_regular_shape()?;
        }
        if frame_key.direction != self.direction {
            return Err(EnvelopeError::Direction);
        }
        let payload_length =
            u32::try_from(self.payload.len()).map_err(|_| EnvelopeError::Length)?;
        let body_length = ENVELOPE_OVERHEAD
            .checked_add(self.payload.len())
            .ok_or(EnvelopeError::Length)?;
        if body_length > MAX_BODY_LENGTH {
            return Err(EnvelopeError::Length);
        }
        let body_length_u32 = u32::try_from(body_length).map_err(|_| EnvelopeError::Length)?;
        let mut body = Vec::with_capacity(body_length);
        body.extend_from_slice(ENVELOPE_MAGIC);
        body.push(ENVELOPE_VERSION);
        body.push(self.kind as u8);
        body.push(self.direction as u8);
        body.push(0);
        body.extend_from_slice(self.session_id.as_bytes());
        body.extend_from_slice(&self.transport_sequence.to_be_bytes());
        body.extend_from_slice(&self.correlation_sequence.to_be_bytes());
        body.extend_from_slice(&payload_length.to_be_bytes());
        body.extend_from_slice(&self.payload);
        let mac = frame_mac(frame_key.as_bytes(), body_length_u32, &body);
        body.extend_from_slice(&mac);
        Ok(body)
    }

    /// Verifies structure, direction, session, and MAC before exposing payload.
    /// Sequence/correlation state is intentionally handled by the dedicated
    /// validators after this cryptographic check.
    pub(crate) fn decode_body(
        body: &[u8],
        expected_direction: Direction,
        expected_session_id: &Bytes32,
        frame_key: &FrameKey,
    ) -> Result<Self, EnvelopeError> {
        let envelope =
            Self::decode_body_inner(body, expected_direction, expected_session_id, frame_key)?;
        envelope.validate_regular_shape()?;
        Ok(envelope)
    }

    pub(crate) fn decode_authenticated_finish(
        body: &[u8],
        expected_session_id: &Bytes32,
        owner_frame_key: &FrameKey,
    ) -> Result<Self, EnvelopeError> {
        let envelope = Self::decode_body_inner(
            body,
            Direction::OwnerToGateway,
            expected_session_id,
            owner_frame_key,
        )?;
        if envelope.kind != EnvelopeKind::Response
            || envelope.transport_sequence != 1
            || envelope.correlation_sequence != 0
            || !matches!(
                crate::schema::parse_handshake_json(&envelope.payload),
                Ok(crate::schema::HandshakeMessage::Authenticated(_))
            )
        {
            return Err(EnvelopeError::Correlation);
        }
        Ok(envelope)
    }

    fn decode_body_inner(
        body: &[u8],
        expected_direction: Direction,
        expected_session_id: &Bytes32,
        frame_key: &FrameKey,
    ) -> Result<Self, EnvelopeError> {
        if frame_key.direction != expected_direction {
            return Err(EnvelopeError::Direction);
        }
        if !(ENVELOPE_OVERHEAD..=MAX_BODY_LENGTH).contains(&body.len()) {
            return Err(EnvelopeError::Length);
        }
        let payload_length =
            u32::from_be_bytes(body[56..60].try_into().expect("u32 field")) as usize;
        if payload_length > MAX_PAYLOAD_LENGTH || ENVELOPE_OVERHEAD + payload_length != body.len() {
            return Err(EnvelopeError::Length);
        }
        let mac_offset = FIXED_WITHOUT_MAC + payload_length;
        let body_length = u32::try_from(body.len()).map_err(|_| EnvelopeError::Length)?;
        let expected_mac = &body[mac_offset..];
        let mut mac = Hmac::<Sha256>::new_from_slice(frame_key.as_bytes())
            .expect("SHA-256 HMAC accepts 32-byte keys");
        mac.update(FRAME_DOMAIN);
        mac.update(&body_length.to_be_bytes());
        mac.update(&body[..mac_offset]);
        mac.verify_slice(expected_mac)
            .map_err(|_| EnvelopeError::Mac)?;

        if &body[0..4] != ENVELOPE_MAGIC || body[4] != ENVELOPE_VERSION {
            return Err(EnvelopeError::Header);
        }
        let kind = EnvelopeKind::from_tag(body[5])?;
        let direction = Direction::from_tag(body[6])?;
        if direction != expected_direction {
            return Err(EnvelopeError::Direction);
        }
        if body[7] != 0 {
            return Err(EnvelopeError::Flags);
        }
        let session_id = Bytes32::new(body[8..40].try_into().expect("32-byte session field"));
        if !session_id.constant_time_eq(expected_session_id) {
            return Err(EnvelopeError::Session);
        }
        let transport_sequence = u64::from_be_bytes(body[40..48].try_into().expect("u64 field"));
        if transport_sequence == 0 {
            return Err(EnvelopeError::Sequence(SequenceError::Wrapped));
        }
        let correlation_sequence = u64::from_be_bytes(body[48..56].try_into().expect("u64 field"));
        let payload = body[FIXED_WITHOUT_MAC..mac_offset].to_vec();
        std::str::from_utf8(&payload).map_err(|_| EnvelopeError::Utf8)?;
        Ok(Self {
            kind,
            direction,
            session_id,
            transport_sequence,
            correlation_sequence,
            payload,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SequenceValidator {
    next: Option<u64>,
}

impl Default for SequenceValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceValidator {
    #[must_use]
    pub const fn new() -> Self {
        Self { next: Some(1) }
    }

    /// Restores a validator from a trusted local high-water mark. This is also
    /// useful for bounded model tests near `u64::MAX`; high-water is never read
    /// from an untrusted frame.
    #[must_use]
    pub(crate) const fn from_high_water(high_water: Option<u64>) -> Self {
        let next = match high_water {
            None => Some(1),
            Some(u64::MAX) => None,
            Some(value) => Some(value + 1),
        };
        Self { next }
    }

    pub fn check(&self, sequence: u64) -> Result<(), SequenceError> {
        let Some(expected) = self.next else {
            return Err(SequenceError::Wrapped);
        };
        if sequence == 0 {
            return Err(SequenceError::Wrapped);
        }
        if sequence < expected {
            return Err(SequenceError::Duplicate);
        }
        if sequence > expected {
            return Err(SequenceError::Skipped);
        }
        Ok(())
    }

    pub fn accept(&mut self, sequence: u64) -> Result<(), SequenceError> {
        self.check(sequence)?;
        self.next = sequence.checked_add(1);
        Ok(())
    }

    #[must_use]
    pub const fn next(&self) -> Option<u64> {
        self.next
    }
}

pub struct EnvelopeReceiver {
    direction: Direction,
    session_id: Bytes32,
    frame_key: FrameKey,
    purpose: Purpose,
    sequence: SequenceValidator,
}

impl fmt::Debug for EnvelopeReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EnvelopeReceiver([REDACTED])")
    }
}

impl EnvelopeReceiver {
    pub(crate) fn new(
        direction: Direction,
        session_id: Bytes32,
        frame_key: &FrameKey,
        purpose: Purpose,
    ) -> Result<Self, EnvelopeError> {
        Self::new_after_high_water(direction, session_id, frame_key, purpose, None)
    }

    pub(crate) fn new_after_high_water(
        direction: Direction,
        session_id: Bytes32,
        frame_key: &FrameKey,
        purpose: Purpose,
        high_water: Option<u64>,
    ) -> Result<Self, EnvelopeError> {
        if frame_key.direction() != direction {
            return Err(EnvelopeError::Direction);
        }
        Ok(Self {
            direction,
            session_id,
            frame_key: frame_key.duplicate(),
            purpose,
            sequence: SequenceValidator::from_high_water(high_water),
        })
    }

    pub fn accept_request(
        &mut self,
        body: &[u8],
    ) -> Result<(AuthenticatedEnvelope, Request), EnvelopeError> {
        let envelope = self.decode_expected(body, EnvelopeKind::Request)?;
        self.sequence.check(envelope.transport_sequence)?;
        let request = crate::schema::parse_request_json(envelope.payload())
            .map_err(|_| EnvelopeError::PayloadSchema)?;
        if !request.method().allowed_for(self.purpose) {
            return Err(EnvelopeError::MethodAuthority);
        }
        self.sequence.accept(envelope.transport_sequence)?;
        Ok((envelope, request))
    }

    pub fn accept_response(
        &mut self,
        body: &[u8],
        correlations: &mut CorrelationTracker,
    ) -> Result<(AuthenticatedEnvelope, Response), EnvelopeError> {
        let envelope = self.decode_expected(body, EnvelopeKind::Response)?;
        self.sequence.check(envelope.transport_sequence)?;
        let method = correlations
            .expected_method(envelope.correlation_sequence)
            .map_err(EnvelopeError::CorrelationState)?;
        let response = crate::schema::parse_response_json(method, envelope.payload())
            .map_err(|_| EnvelopeError::PayloadSchema)?;
        correlations
            .accept_response(envelope.correlation_sequence, &response)
            .map_err(EnvelopeError::CorrelationState)?;
        self.sequence.accept(envelope.transport_sequence)?;
        Ok((envelope, response))
    }

    pub fn accept_event(
        &mut self,
        body: &[u8],
    ) -> Result<(AuthenticatedEnvelope, crate::schema::Event), EnvelopeError> {
        let envelope = self.decode_expected(body, EnvelopeKind::Event)?;
        self.sequence.check(envelope.transport_sequence)?;
        let event = crate::schema::parse_event_json(envelope.payload())
            .map_err(|_| EnvelopeError::PayloadSchema)?;
        if !event.allowed_for(self.purpose) {
            return Err(EnvelopeError::MethodAuthority);
        }
        self.sequence.accept(envelope.transport_sequence)?;
        Ok((envelope, event))
    }

    pub fn accept_predecessor_terminal_event(
        &mut self,
        body: &[u8],
        route: &mut PredecessorRouteValidator,
    ) -> Result<
        (
            AuthenticatedEnvelope,
            crate::schema::PredecessorTerminalEvent,
        ),
        EnvelopeError,
    > {
        let envelope = self.decode_expected(body, EnvelopeKind::PredecessorTerminalEvent)?;
        self.sequence.check(envelope.transport_sequence)?;
        let event = crate::schema::parse_predecessor_terminal_json(envelope.payload())
            .map_err(|_| EnvelopeError::PayloadSchema)?;
        route.accept(&event).map_err(EnvelopeError::TerminalRoute)?;
        self.sequence.accept(envelope.transport_sequence)?;
        Ok((envelope, event))
    }

    /// Authenticates and validates one mixed owner-to-gateway frame before
    /// advancing the shared directional transport sequence. The frame kind is
    /// never exposed before its MAC, session, direction, schema, correlation,
    /// and (for predecessor events) immutable route all validate.
    pub fn accept_owner_message(
        &mut self,
        body: &[u8],
        correlations: &mut CorrelationTracker,
        predecessor_route: Option<&mut PredecessorRouteValidator>,
    ) -> Result<(AuthenticatedEnvelope, OwnerMessage), EnvelopeError> {
        let envelope = AuthenticatedEnvelope::decode_body(
            body,
            self.direction,
            &self.session_id,
            &self.frame_key,
        )?;
        if self.direction != Direction::OwnerToGateway {
            return Err(EnvelopeError::Direction);
        }
        self.sequence.check(envelope.transport_sequence)?;
        let message = match envelope.kind {
            EnvelopeKind::Response => {
                let method = correlations
                    .expected_method(envelope.correlation_sequence)
                    .map_err(EnvelopeError::CorrelationState)?;
                let response = crate::schema::parse_response_json(method, envelope.payload())
                    .map_err(|_| EnvelopeError::PayloadSchema)?;
                correlations
                    .accept_response(envelope.correlation_sequence, &response)
                    .map_err(EnvelopeError::CorrelationState)?;
                OwnerMessage::Response(response)
            }
            EnvelopeKind::Event => {
                let event = crate::schema::parse_event_json(envelope.payload())
                    .map_err(|_| EnvelopeError::PayloadSchema)?;
                if !event.allowed_for(self.purpose) {
                    return Err(EnvelopeError::MethodAuthority);
                }
                OwnerMessage::Event(event)
            }
            EnvelopeKind::PredecessorTerminalEvent => {
                let event = crate::schema::parse_predecessor_terminal_json(envelope.payload())
                    .map_err(|_| EnvelopeError::PayloadSchema)?;
                let route = predecessor_route
                    .ok_or(EnvelopeError::TerminalRoute(TerminalRouteError::Invalid))?;
                route.accept(&event).map_err(EnvelopeError::TerminalRoute)?;
                OwnerMessage::PredecessorTerminal(event)
            }
            EnvelopeKind::Request => return Err(EnvelopeError::Direction),
        };
        self.sequence.accept(envelope.transport_sequence)?;
        Ok((envelope, message))
    }

    pub(crate) fn accept_authenticated_finish(
        &mut self,
        body: &[u8],
    ) -> Result<AuthenticatedEnvelope, EnvelopeError> {
        if self.direction != Direction::OwnerToGateway || self.sequence.next() != Some(1) {
            return Err(EnvelopeError::Correlation);
        }
        let envelope = AuthenticatedEnvelope::decode_authenticated_finish(
            body,
            &self.session_id,
            &self.frame_key,
        )?;
        self.sequence.accept(1)?;
        Ok(envelope)
    }

    fn decode_expected(
        &self,
        body: &[u8],
        kind: EnvelopeKind,
    ) -> Result<AuthenticatedEnvelope, EnvelopeError> {
        let envelope = AuthenticatedEnvelope::decode_body(
            body,
            self.direction,
            &self.session_id,
            &self.frame_key,
        )?;
        if envelope.kind != kind {
            return Err(EnvelopeError::Kind);
        }
        Ok(envelope)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerMessage {
    Response(Response),
    Event(Event),
    PredecessorTerminal(PredecessorTerminalEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    Capture,
    Maintenance,
}

pub struct CapabilitySequenceValidator {
    kind: CapabilityKind,
    capability_id: Bytes32,
    capability_epoch: u64,
    sequence: SequenceValidator,
}

impl fmt::Debug for CapabilitySequenceValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CapabilitySequenceValidator([REDACTED])")
    }
}

impl CapabilitySequenceValidator {
    pub fn new(
        kind: CapabilityKind,
        capability_id: Bytes32,
        capability_epoch: u64,
    ) -> Result<Self, CapabilitySequenceError> {
        if capability_epoch == 0 || capability_id.as_bytes().iter().all(|byte| *byte == 0) {
            return Err(CapabilitySequenceError::InvalidCapability);
        }
        Ok(Self {
            kind,
            capability_id,
            capability_epoch,
            sequence: SequenceValidator::new(),
        })
    }

    /// Consumes a command sequence before semantic state validation. Any error
    /// is a capability protocol fault and leaves the high-water unchanged.
    pub fn accept(&mut self, request: &Request) -> Result<(), CapabilitySequenceError> {
        let (kind, capability_id, capability_epoch, command_sequence) = match request {
            Request::LeaseRenew(value)
            | Request::SessionReconcileOff(value)
            | Request::LeaseRelease(value)
            | Request::OwnerExitWhenNeutral(value)
            | Request::RuntimeRollback(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::SessionSetMode(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::CaptureReplaceConfiguration(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::CaptureSetEnabled(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::PasteInject(value) => (
                CapabilityKind::Capture,
                &value.capture_lease_id,
                value.capture_lease_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::MaintenanceRenew(value) => (
                CapabilityKind::Maintenance,
                &value.maintenance_capability_id,
                value.maintenance_capability_epoch.get(),
                value.command_sequence.get(),
            ),
            Request::MaintenancePrepare(value) => (
                CapabilityKind::Maintenance,
                &value.maintenance_capability_id,
                value.maintenance_capability_epoch.get(),
                value.command_sequence.get(),
            ),
            _ => return Err(CapabilitySequenceError::NotCapabilityCommand),
        };
        if kind != self.kind
            || capability_id != &self.capability_id
            || capability_epoch != self.capability_epoch
        {
            return Err(CapabilitySequenceError::WrongCapability);
        }
        self.sequence
            .accept(command_sequence)
            .map_err(CapabilitySequenceError::Sequence)
    }
}

pub struct PredecessorRouteValidator {
    capture_lease_id: Bytes32,
    capture_lease_epoch: u64,
    terminal_sequence: SequenceValidator,
    revoked: bool,
    final_seen: bool,
}

impl fmt::Debug for PredecessorRouteValidator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PredecessorRouteValidator([REDACTED])")
    }
}

impl PredecessorRouteValidator {
    #[must_use]
    pub fn new(capture_lease_id: Bytes32, capture_lease_epoch: u64) -> Self {
        Self {
            capture_lease_id,
            capture_lease_epoch,
            terminal_sequence: SequenceValidator::new(),
            revoked: false,
            final_seen: false,
        }
    }

    pub fn accept(
        &mut self,
        event: &crate::schema::PredecessorTerminalEvent,
    ) -> Result<(), TerminalRouteError> {
        use crate::schema::PredecessorTerminalEvent as Event;
        let (lease_id, lease_epoch, sequence, is_revoked, is_final) = match event {
            Event::LeaseRevoked {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                true,
                false,
            ),
            Event::LeaseDraining {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                false,
                false,
            ),
            Event::LeaseNeutral {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            }
            | Event::LeaseUnavailable {
                capture_lease_id,
                capture_lease_epoch,
                terminal_sequence,
                ..
            } => (
                capture_lease_id,
                capture_lease_epoch.get(),
                terminal_sequence.get(),
                false,
                true,
            ),
        };
        if self.final_seen
            || lease_id != &self.capture_lease_id
            || lease_epoch != self.capture_lease_epoch
            || is_revoked == self.revoked
            || (!self.revoked && !is_revoked)
            || self.terminal_sequence.next() != Some(sequence)
        {
            return Err(TerminalRouteError::Invalid);
        }
        self.terminal_sequence
            .accept(sequence)
            .map_err(|_| TerminalRouteError::Invalid)?;
        self.revoked |= is_revoked;
        self.final_seen |= is_final;
        Ok(())
    }

    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.final_seen
    }
}

enum ResponseExpectation {
    Method(Method),
    SessionMode(Method, SessionMode),
    Configuration(U64Expectation),
    Enabled(bool),
    Paste(Bytes32),
}

struct U64Expectation {
    method: Method,
    value: u64,
}

impl fmt::Debug for ResponseExpectation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponseExpectation([REDACTED])")
    }
}

impl ResponseExpectation {
    fn from_request(request: &Request) -> Self {
        match request {
            Request::SessionReconcileOff(_) => {
                Self::SessionMode(request.method(), SessionMode::Off)
            }
            Request::SessionSetMode(params) => Self::SessionMode(request.method(), params.mode),
            Request::CaptureReplaceConfiguration(params) => Self::Configuration(U64Expectation {
                method: request.method(),
                value: params.revision.get(),
            }),
            Request::CaptureSetEnabled(params) => Self::Enabled(params.enabled),
            Request::PasteInject(params) => Self::Paste(params.operation_id),
            _ => Self::Method(request.method()),
        }
    }

    const fn method(&self) -> Method {
        match self {
            Self::Method(method) | Self::SessionMode(method, _) => *method,
            Self::Configuration(value) => value.method,
            Self::Enabled(_) => Method::CaptureSetEnabled,
            Self::Paste(_) => Method::PasteInject,
        }
    }

    fn validates(&self, response: &Response) -> bool {
        let Response::Success(result) = response else {
            return true;
        };
        match (self, result) {
            (Self::SessionMode(_, expected), SuccessResult::SessionMode(actual)) => {
                *expected == actual.mode
            }
            (Self::Configuration(expected), SuccessResult::Configuration(actual)) => {
                expected.value == actual.revision.get()
            }
            (Self::Enabled(expected), SuccessResult::Enabled(actual)) => {
                *expected == actual.enabled
            }
            (Self::Paste(expected), SuccessResult::Paste(actual)) => match actual {
                crate::schema::PasteResult::ClipboardOnly { .. } => true,
                crate::schema::PasteResult::Waiting { operation_id }
                | crate::schema::PasteResult::Committed { operation_id }
                | crate::schema::PasteResult::Indeterminate { operation_id } => {
                    expected == operation_id
                }
            },
            (Self::Method(expected), actual) => success_matches_method(*expected, actual),
            _ => false,
        }
    }
}

fn success_matches_method(method: Method, result: &SuccessResult) -> bool {
    matches!(
        (method, result),
        (Method::LeaseAcquire, SuccessResult::LeaseAcquire(_))
            | (
                Method::MaintenanceAcquire,
                SuccessResult::MaintenanceAcquire(_)
            )
            | (Method::HealthGet, SuccessResult::Health(_))
            | (Method::PermissionsGet, SuccessResult::Permissions(_))
            | (Method::ObservabilityGet, SuccessResult::Observability(_))
            | (Method::FrontAppGet, SuccessResult::FrontApp(_))
            | (
                Method::FrontAppMetadataGet,
                SuccessResult::FrontAppMetadata(_),
            )
            | (Method::LeaseRenew, SuccessResult::Renew(_))
            | (Method::MaintenanceRenew, SuccessResult::Renew(_))
            | (
                Method::LeaseRelease | Method::OwnerExitWhenNeutral,
                SuccessResult::Release(_)
            )
            | (Method::RuntimeRollback, SuccessResult::Rollback(_))
            | (
                Method::MaintenancePrepare,
                SuccessResult::MaintenancePrepare(_)
            )
    )
}

#[derive(Default)]
pub struct CorrelationTracker {
    outstanding: BTreeMap<u64, ResponseExpectation>,
}

impl CorrelationTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            outstanding: BTreeMap::new(),
        }
    }

    pub fn register_request(
        &mut self,
        transport_sequence: u64,
        request: &Request,
    ) -> Result<(), CorrelationError> {
        if transport_sequence == 0 {
            return Err(CorrelationError::Zero);
        }
        if self.outstanding.contains_key(&transport_sequence) {
            return Err(CorrelationError::Duplicate);
        }
        if self.outstanding.len() >= MAX_OUTSTANDING_REQUESTS {
            return Err(CorrelationError::Capacity);
        }
        self.outstanding.insert(
            transport_sequence,
            ResponseExpectation::from_request(request),
        );
        Ok(())
    }

    pub fn expected_method(&self, correlation_sequence: u64) -> Result<Method, CorrelationError> {
        if correlation_sequence == 0 {
            return Err(CorrelationError::Zero);
        }
        self.outstanding
            .get(&correlation_sequence)
            .map(ResponseExpectation::method)
            .ok_or(CorrelationError::Unknown)
    }

    pub fn accept_response(
        &mut self,
        correlation_sequence: u64,
        response: &Response,
    ) -> Result<Method, CorrelationError> {
        let expected =
            self.outstanding
                .get(&correlation_sequence)
                .ok_or(if correlation_sequence == 0 {
                    CorrelationError::Zero
                } else {
                    CorrelationError::Unknown
                })?;
        if !expected.validates(response) {
            return Err(CorrelationError::Mismatch);
        }
        let method = expected.method();
        self.outstanding.remove(&correlation_sequence);
        Ok(method)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.outstanding.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outstanding.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SequenceError {
    #[error("duplicate or stale owner-protocol sequence")]
    Duplicate,
    #[error("skipped owner-protocol sequence")]
    Skipped,
    #[error("zero or wrapped owner-protocol sequence")]
    Wrapped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CapabilitySequenceError {
    #[error("owner-protocol capability identity is invalid")]
    InvalidCapability,
    #[error("owner-protocol request is not capability-sequenced")]
    NotCapabilityCommand,
    #[error("owner-protocol command presents the wrong capability or epoch")]
    WrongCapability,
    #[error(transparent)]
    Sequence(SequenceError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum TerminalRouteError {
    #[error("owner-protocol predecessor terminal route is invalid")]
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum CorrelationError {
    #[error("owner-protocol correlation cannot be zero")]
    Zero,
    #[error("owner-protocol request correlation is duplicated")]
    Duplicate,
    #[error("owner-protocol response correlation is unknown")]
    Unknown,
    #[error("owner-protocol response does not match its correlated request")]
    Mismatch,
    #[error("owner-protocol outstanding request capacity exceeded")]
    Capacity,
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

fn frame_mac(key: &[u8; 32], body_length: u32, authenticated_bytes: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(FRAME_DOMAIN);
    mac.update(&body_length.to_be_bytes());
    mac.update(authenticated_bytes);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::{
        AuthenticatedEnvelope, CorrelationTracker, Direction, EnvelopeError, EnvelopeKind,
        EnvelopeReceiver, FrameKey, PredecessorRouteValidator, SequenceError, SequenceValidator,
        frame_mac,
    };
    use crate::scalar::{Bytes32, FeatureBits};
    use crate::schema::{
        Authenticated, AuthorityCeiling, Empty, ErrorBody, ErrorCode, Method, Purpose, Request,
        Response, SelectedProtocol, parse_predecessor_terminal_json,
    };

    #[test]
    fn sequence_accepts_maximum_once_and_never_wraps() {
        let mut sequence = SequenceValidator::from_high_water(Some(u64::MAX - 1));
        sequence.accept(u64::MAX).expect("maximum accepted once");
        assert_eq!(sequence.next(), None);
        assert_eq!(sequence.accept(u64::MAX), Err(SequenceError::Wrapped));
        assert_eq!(sequence.accept(1), Err(SequenceError::Wrapped));
    }

    #[test]
    fn authenticated_receiver_rejects_every_byte_mutation() {
        let session = Bytes32::new([3; 32]);
        let key = FrameKey::from_secret(Direction::GatewayToOwner, [4; 32]);
        let request = Request::HealthGet(Empty {});
        let body = AuthenticatedEnvelope::request(session, 1, &request)
            .expect("envelope")
            .encode_body(&key)
            .expect("body");
        for index in 0..body.len() {
            let mut changed = body.clone();
            changed[index] ^= 1;
            let mut receiver =
                EnvelopeReceiver::new(Direction::GatewayToOwner, session, &key, Purpose::Capture)
                    .expect("receiver");
            assert!(receiver.accept_request(&changed).is_err(), "byte {index}");
        }
    }

    #[test]
    fn valid_mac_structural_and_schema_faults_are_rejected() {
        let session = Bytes32::new([5; 32]);
        let key = FrameKey::from_secret(Direction::GatewayToOwner, [6; 32]);
        let body = AuthenticatedEnvelope::request(session, 1, &Request::HealthGet(Empty {}))
            .expect("envelope")
            .encode_body(&key)
            .expect("body");
        let resign = |mut changed: Vec<u8>| {
            let mac_offset = changed.len() - 32;
            let mac = frame_mac(
                key.as_bytes(),
                u32::try_from(changed.len()).expect("length"),
                &changed[..mac_offset],
            );
            changed[mac_offset..].copy_from_slice(&mac);
            changed
        };
        let mut corpus = Vec::new();
        let mut flags = body.clone();
        flags[7] = 1;
        corpus.push(resign(flags));
        let mut zero_sequence = body.clone();
        zero_sequence[40..48].fill(0);
        corpus.push(resign(zero_sequence));
        let mut wrong_kind = body.clone();
        wrong_kind[5] = EnvelopeKind::Event as u8;
        corpus.push(resign(wrong_kind));
        let mut request_correlation = body.clone();
        request_correlation[48..56].copy_from_slice(&1_u64.to_be_bytes());
        corpus.push(resign(request_correlation));
        let mut invalid_json = body.clone();
        let payload_length =
            u32::from_be_bytes(invalid_json[56..60].try_into().expect("length")) as usize;
        invalid_json[60..60 + payload_length].fill(b' ');
        corpus.push(resign(invalid_json));

        for malformed in corpus {
            let mut receiver =
                EnvelopeReceiver::new(Direction::GatewayToOwner, session, &key, Purpose::Capture)
                    .expect("receiver");
            assert!(receiver.accept_request(&malformed).is_err());
        }
    }

    #[test]
    fn skipped_transport_sequence_does_not_mutate_correlation_or_terminal_route() {
        let session = Bytes32::new([7; 32]);
        let owner_key = FrameKey::from_secret(Direction::OwnerToGateway, [8; 32]);
        let authenticated = Authenticated::new(
            SelectedProtocol {
                major: 1,
                minor: 0,
                compatibility_epoch: 1,
                feature_bits: FeatureBits::new(1),
            },
            Purpose::Capture,
            AuthorityCeiling::Capture,
            Bytes32::new([9; 32]),
        )
        .expect("authenticated");
        let finish = AuthenticatedEnvelope::authenticated_finish(session, &authenticated)
            .expect("finish")
            .encode_body(&owner_key)
            .expect("finish body");
        let mut receiver = EnvelopeReceiver::new(
            Direction::OwnerToGateway,
            session,
            &owner_key,
            Purpose::Capture,
        )
        .expect("receiver");
        receiver
            .accept_authenticated_finish(&finish)
            .expect("finish accepted");
        let request = Request::HealthGet(Empty {});
        let response = Response::Error(ErrorBody::new(ErrorCode::Unavailable));
        let skipped =
            AuthenticatedEnvelope::response_for_request(session, 3, 1, &request, &response)
                .expect("response")
                .encode_body(&owner_key)
                .expect("body");
        let current =
            AuthenticatedEnvelope::response_for_request(session, 2, 1, &request, &response)
                .expect("response")
                .encode_body(&owner_key)
                .expect("body");
        let mut correlations = CorrelationTracker::new();
        correlations.register_request(1, &request).expect("request");
        assert!(matches!(
            receiver.accept_response(&skipped, &mut correlations),
            Err(EnvelopeError::Sequence(SequenceError::Skipped))
        ));
        assert_eq!(correlations.expected_method(1), Ok(Method::HealthGet));
        receiver
            .accept_response(&current, &mut correlations)
            .expect("current response");

        let lease_id = Bytes32::new([0; 32]);
        let terminal = parse_predecessor_terminal_json(
            br#"{"event":"lease.revoked","captureLeaseId":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","captureLeaseEpoch":"1","terminalSequence":"1","reason":"release"}"#,
        )
        .expect("terminal");
        let skipped_terminal =
            AuthenticatedEnvelope::predecessor_terminal_event(session, 2, &terminal)
                .expect("terminal envelope")
                .encode_body(&owner_key)
                .expect("terminal body");
        let current_terminal =
            AuthenticatedEnvelope::predecessor_terminal_event(session, 1, &terminal)
                .expect("terminal envelope")
                .encode_body(&owner_key)
                .expect("terminal body");
        let mut terminal_receiver = EnvelopeReceiver::new(
            Direction::OwnerToGateway,
            session,
            &owner_key,
            Purpose::Capture,
        )
        .expect("receiver");
        let mut route = PredecessorRouteValidator::new(lease_id, 1);
        assert!(matches!(
            terminal_receiver.accept_predecessor_terminal_event(&skipped_terminal, &mut route),
            Err(EnvelopeError::Sequence(SequenceError::Skipped))
        ));
        terminal_receiver
            .accept_predecessor_terminal_event(&current_terminal, &mut route)
            .expect("route remained unchanged");
    }
}
