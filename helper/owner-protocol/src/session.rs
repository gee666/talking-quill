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

#[derive(Clone)]
struct ActiveActivationRoute {
    event: ActivationEvent,
}

struct CaptureEventRoute {
    lease_epoch: u64,
    revoked: bool,
    generation_high_water: u64,
    observation_generation_high_water: u64,
    active_activation: Option<ActiveActivationRoute>,
    session_keys: u8,
    paste_operations: Vec<Bytes32>,
}

impl CaptureEventRoute {
    fn new(lease_epoch: u64) -> Self {
        Self {
            lease_epoch,
            revoked: false,
            generation_high_water: 0,
            observation_generation_high_water: 0,
            active_activation: None,
            session_keys: 0,
            paste_operations: Vec::new(),
        }
    }

    fn clear_ephemeral(&mut self) {
        self.active_activation = None;
        self.session_keys = 0;
    }

    fn accept_event(
        &mut self,
        event: &Event,
        owner_instance: Option<Bytes32>,
    ) -> Result<(), SessionCodecError> {
        if self.revoked {
            return Err(SessionCodecError::EventRoute);
        }
        match event {
            Event::Activation(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || owner_instance != Some(actual.owner_instance_id)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                let generation = actual.activation_generation.get();
                match (actual.phase, actual.held_ms, &self.active_activation) {
                    (Phase::Down, None, None) if generation > self.generation_high_water => {
                        self.generation_high_water = generation;
                        self.active_activation = Some(ActiveActivationRoute {
                            event: actual.clone(),
                        });
                    }
                    (Phase::Up, None, Some(expected))
                        if same_activation_route(&expected.event, actual) =>
                    {
                        self.active_activation = None;
                    }
                    (Phase::Up, Some(_), None) if generation > self.generation_high_water => {
                        self.generation_high_water = generation;
                    }
                    _ => return Err(SessionCodecError::EventRoute),
                }
            }
            Event::RegisteredObservation(actual) => {
                let generation = actual.generation.get();
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || generation <= self.observation_generation_high_water
                {
                    return Err(SessionCodecError::EventRoute);
                }
                self.observation_generation_high_water = generation;
            }
            Event::SessionKey(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch {
                    return Err(SessionCodecError::EventRoute);
                }
                let bit = match actual.key {
                    SessionKey::Escape => 1,
                    SessionKey::Enter => 2,
                };
                match actual.phase {
                    Phase::Down if self.session_keys & bit == 0 => self.session_keys |= bit,
                    Phase::Up if self.session_keys & bit != 0 => self.session_keys &= !bit,
                    _ => return Err(SessionCodecError::EventRoute),
                }
            }
            Event::PasteCommitted(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch
                    || !self
                        .paste_operations
                        .iter()
                        .any(|operation| operation == &actual.operation_id)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                self.paste_operations
                    .retain(|operation| operation != &actual.operation_id);
            }
            Event::AudioDevicesChanged(actual) => {
                if actual.capture_lease_epoch.get() != self.lease_epoch {
                    return Err(SessionCodecError::EventRoute);
                }
            }
            Event::HealthChanged(_) | Event::TerminalDegraded(_) => {}
        }
        Ok(())
    }
}

fn same_activation_route(expected: &ActivationEvent, actual: &ActivationEvent) -> bool {
    expected.capture_lease_epoch == actual.capture_lease_epoch
        && expected.owner_instance_id == actual.owner_instance_id
        && expected.profile_id == actual.profile_id
        && expected.shortcut == actual.shortcut
        && expected.activation_generation == actual.activation_generation
        && expected.target_token == actual.target_token
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

/// Owner-side codec for one already-authenticated connection.
pub struct OwnerSessionCodec {
    session: AuthenticatedSession,
    outbound_key: FrameKey,
    next_outbound_sequence: Option<u64>,
}

impl fmt::Debug for OwnerSessionCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerSessionCodec(<redacted>)")
    }
}

impl OwnerSessionCodec {
    #[must_use]
    pub const fn supports_feature(&self, feature: u64) -> bool {
        self.session.supports_feature(feature)
    }

    /// Creates the post-handshake owner codec. Owner sequence 1 belongs to the
    /// authenticated finish, so the first regular owner frame is sequence 2.
    pub fn new(
        session: AuthenticatedSession,
        outbound_key: &FrameKey,
    ) -> Result<Self, SessionCodecError> {
        if outbound_key.direction() != Direction::OwnerToGateway {
            return Err(SessionCodecError::Direction);
        }
        Ok(Self {
            session,
            outbound_key: outbound_key.duplicate(),
            next_outbound_sequence: Some(2),
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> Bytes32 {
        self.session.session_id()
    }

    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.session.purpose()
    }

    #[must_use]
    pub const fn is_test_only(&self) -> bool {
        self.session.is_test_only()
    }

    pub fn receive_request(&mut self, frame: &[u8]) -> Result<ReceivedRequest, SessionCodecError> {
        let body = decode_outer_frame(frame)?;
        let (envelope, request) = self.session.inbound().accept_request(body)?;
        if request.method() == crate::Method::FrontAppMetadataGet
            && !self.supports_feature(crate::FRONT_APP_METADATA_V1)
        {
            return Err(SessionCodecError::FeatureNotNegotiated);
        }
        Ok(ReceivedRequest { envelope, request })
    }

    pub fn encode_response(
        &mut self,
        request: &ReceivedRequest,
        response: &Response,
    ) -> Result<Vec<u8>, SessionCodecError> {
        let sequence = self.take_outbound_sequence()?;
        let body = AuthenticatedEnvelope::response_for_request(
            self.session.session_id(),
            sequence,
            request.transport_sequence(),
            request.request(),
            response,
        )?
        .encode_body(&self.outbound_key)?;
        Ok(encode_outer_frame(&body)?)
    }

    pub fn encode_event(&mut self, event: &Event) -> Result<Vec<u8>, SessionCodecError> {
        let sequence = self.take_outbound_sequence()?;
        let body = AuthenticatedEnvelope::event(self.session.session_id(), sequence, event)?
            .encode_body(&self.outbound_key)?;
        Ok(encode_outer_frame(&body)?)
    }

    pub fn encode_predecessor_terminal(
        &mut self,
        event: &PredecessorTerminalEvent,
    ) -> Result<Vec<u8>, SessionCodecError> {
        let sequence = self.take_outbound_sequence()?;
        let body = AuthenticatedEnvelope::predecessor_terminal_event(
            self.session.session_id(),
            sequence,
            event,
        )?
        .encode_body(&self.outbound_key)?;
        Ok(encode_outer_frame(&body)?)
    }

    fn take_outbound_sequence(&mut self) -> Result<u64, SessionCodecError> {
        let sequence = self
            .next_outbound_sequence
            .ok_or(SessionCodecError::SequenceExhausted)?;
        self.next_outbound_sequence = sequence.checked_add(1);
        Ok(sequence)
    }
}

/// Gateway-side codec for one already-authenticated connection.
pub struct GatewaySessionCodec {
    session: AuthenticatedSession,
    outbound_key: FrameKey,
    next_outbound_sequence: Option<u64>,
    correlations: CorrelationTracker,
    predecessor_route: Option<PredecessorRouteValidator>,
    capture_event_route: Option<CaptureEventRoute>,
    owner_instance_id: Bytes32,
    pending_paste_operations: BTreeMap<u64, Bytes32>,
}

impl fmt::Debug for GatewaySessionCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GatewaySessionCodec(<redacted>)")
    }
}

impl GatewaySessionCodec {
    #[must_use]
    pub const fn supports_feature(&self, feature: u64) -> bool {
        self.session.supports_feature(feature)
    }

    /// Creates the post-handshake gateway codec. Gateway request sequence starts
    /// at 1 independently of the owner's authenticated finish.
    pub fn new(
        session: AuthenticatedSession,
        outbound_key: &FrameKey,
    ) -> Result<Self, SessionCodecError> {
        if outbound_key.direction() != Direction::GatewayToOwner {
            return Err(SessionCodecError::Direction);
        }
        let owner_instance_id = session.owner_instance_id();
        Ok(Self {
            session,
            outbound_key: outbound_key.duplicate(),
            next_outbound_sequence: Some(1),
            correlations: CorrelationTracker::new(),
            predecessor_route: None,
            capture_event_route: None,
            owner_instance_id,
            pending_paste_operations: BTreeMap::new(),
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> Bytes32 {
        self.session.session_id()
    }

    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.session.purpose()
    }

    #[must_use]
    pub const fn is_test_only(&self) -> bool {
        self.session.is_test_only()
    }

    pub fn encode_request(
        &mut self,
        request: &Request,
    ) -> Result<EncodedRequest, SessionCodecError> {
        if request.method() == crate::Method::FrontAppMetadataGet
            && !self.supports_feature(crate::FRONT_APP_METADATA_V1)
        {
            return Err(SessionCodecError::FeatureNotNegotiated);
        }
        let sequence = self
            .next_outbound_sequence
            .ok_or(SessionCodecError::SequenceExhausted)?;
        self.correlations.register_request(sequence, request)?;
        let body = AuthenticatedEnvelope::request(self.session.session_id(), sequence, request)?
            .encode_body(&self.outbound_key)?;
        let frame = encode_outer_frame(&body)?;
        if let Request::PasteInject(params) = request {
            self.pending_paste_operations
                .insert(sequence, params.operation_id);
        }
        self.next_outbound_sequence = sequence.checked_add(1);
        Ok(EncodedRequest {
            transport_sequence: sequence,
            frame,
        })
    }

    pub fn receive_owner_frame(
        &mut self,
        frame: &[u8],
    ) -> Result<GatewayMessage, SessionCodecError> {
        let body = decode_outer_frame(frame)?;
        let (envelope, message) = self.session.inbound().accept_owner_message(
            body,
            &mut self.correlations,
            self.predecessor_route.as_mut(),
        )?;
        let result = match message {
            OwnerMessage::Response(response) => {
                let correlation = envelope.correlation_sequence();
                if let Response::Success(result) = &response {
                    match result {
                        SuccessResult::LeaseAcquire(acquired) => {
                            self.predecessor_route = Some(PredecessorRouteValidator::new(
                                acquired.capture_lease_id,
                                acquired.capture_lease_epoch.get(),
                            ));
                            self.capture_event_route =
                                Some(CaptureEventRoute::new(acquired.capture_lease_epoch.get()));
                        }
                        SuccessResult::Health(health)
                            if health.owner_instance_id != self.owner_instance_id =>
                        {
                            return Err(SessionCodecError::EventRoute);
                        }
                        SuccessResult::Health(_) => {}
                        SuccessResult::Enabled(enabled) if !enabled.enabled => {
                            if let Some(route) = &mut self.capture_event_route {
                                route.clear_ephemeral();
                            }
                        }
                        SuccessResult::Configuration(_) => {
                            if let Some(route) = &mut self.capture_event_route {
                                route.clear_ephemeral();
                            }
                        }
                        SuccessResult::Paste(result) => {
                            if let Some(operation) =
                                self.pending_paste_operations.remove(&correlation)
                                && matches!(result, PasteResult::Waiting { .. })
                            {
                                let route = self
                                    .capture_event_route
                                    .as_mut()
                                    .ok_or(SessionCodecError::EventRoute)?;
                                if route.paste_operations.len()
                                    >= crate::envelope::MAX_OUTSTANDING_REQUESTS
                                {
                                    return Err(SessionCodecError::EventRoute);
                                }
                                route.paste_operations.push(operation);
                            }
                        }
                        _ => {}
                    }
                } else {
                    self.pending_paste_operations.remove(&correlation);
                }
                GatewayMessage::Response {
                    correlation_sequence: correlation,
                    response,
                }
            }
            OwnerMessage::Event(event) => {
                if matches!(event, Event::RegisteredObservation(_))
                    && !self.supports_feature(crate::REGISTERED_INPUT_OBSERVABILITY_V1)
                {
                    return Err(SessionCodecError::EventRoute);
                }
                if let Event::HealthChanged(health) = &event
                    && health.owner_instance_id != self.owner_instance_id
                {
                    return Err(SessionCodecError::EventRoute);
                }
                if is_capture_scoped_event(&event)
                    || self
                        .capture_event_route
                        .as_ref()
                        .is_some_and(|route| route.revoked)
                {
                    self.capture_event_route
                        .as_mut()
                        .ok_or(SessionCodecError::EventRoute)?
                        .accept_event(&event, Some(self.owner_instance_id))?;
                }
                GatewayMessage::Event(event)
            }
            OwnerMessage::PredecessorTerminal(event) => {
                if matches!(event, PredecessorTerminalEvent::LeaseRevoked { .. }) {
                    self.capture_event_route
                        .as_mut()
                        .ok_or(SessionCodecError::EventRoute)?
                        .revoked = true;
                }
                GatewayMessage::PredecessorTerminal(event)
            }
        };
        Ok(result)
    }

    #[must_use]
    pub fn outstanding_requests(&self) -> usize {
        self.correlations.len()
    }
}

const fn is_capture_scoped_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Activation(_)
            | Event::RegisteredObservation(_)
            | Event::SessionKey(_)
            | Event::PasteCommitted(_)
            | Event::AudioDevicesChanged(_)
    )
}

/// Authentication material that can create only test-branded sessions. It
/// deliberately bypasses platform trust and handshake I/O and is compiled only
/// for this crate's tests or the explicit, default-off `test-transport` feature.
#[cfg(any(test, feature = "test-transport"))]
pub struct FakeAuthenticatedMaterial {
    session_id: Bytes32,
    owner_instance_id: Bytes32,
    purpose: Purpose,
    gateway_key: FrameKey,
    owner_key: FrameKey,
    feature_bits: crate::FeatureBits,
}

#[cfg(any(test, feature = "test-transport"))]
impl fmt::Debug for FakeAuthenticatedMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FakeAuthenticatedMaterial(<redacted>)")
    }
}

#[cfg(any(test, feature = "test-transport"))]
impl FakeAuthenticatedMaterial {
    #[must_use]
    pub fn new(
        session_id: Bytes32,
        purpose: Purpose,
        gateway_key: [u8; 32],
        owner_key: [u8; 32],
    ) -> Self {
        Self::new_with_owner_instance(session_id, session_id, purpose, gateway_key, owner_key)
    }

    #[must_use]
    pub fn new_with_owner_instance(
        session_id: Bytes32,
        owner_instance_id: Bytes32,
        purpose: Purpose,
        gateway_key: [u8; 32],
        owner_key: [u8; 32],
    ) -> Self {
        Self {
            session_id,
            owner_instance_id,
            purpose,
            gateway_key: FrameKey::from_secret(Direction::GatewayToOwner, gateway_key),
            owner_key: FrameKey::from_secret(Direction::OwnerToGateway, owner_key),
            feature_bits: crate::FeatureBits::new(crate::BASE_V1),
        }
    }

    #[must_use]
    pub fn with_features(mut self, feature_bits: u64) -> Self {
        self.feature_bits = crate::FeatureBits::new(feature_bits | crate::BASE_V1);
        self
    }

    pub fn codecs(&self) -> Result<(GatewaySessionCodec, OwnerSessionCodec), SessionCodecError> {
        let gateway_session = AuthenticatedSession::for_fake_transport(
            self.session_id,
            self.owner_instance_id,
            self.purpose,
            Direction::OwnerToGateway,
            &self.owner_key,
            true,
            self.feature_bits,
        )?;
        let owner_session = AuthenticatedSession::for_fake_transport(
            self.session_id,
            self.owner_instance_id,
            self.purpose,
            Direction::GatewayToOwner,
            &self.gateway_key,
            false,
            self.feature_bits,
        )?;
        Ok((
            GatewaySessionCodec::new(gateway_session, &self.gateway_key)?,
            OwnerSessionCodec::new(owner_session, &self.owner_key)?,
        ))
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
