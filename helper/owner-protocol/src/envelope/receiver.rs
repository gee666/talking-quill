//! Authenticate and validate payloads before advancing receive state.
use super::*;

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
