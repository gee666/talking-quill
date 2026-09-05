//! Owner-side authenticated request receiver and ordered response writer.
use super::*;

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
