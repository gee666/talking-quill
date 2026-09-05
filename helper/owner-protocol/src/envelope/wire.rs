//! Exact authenticated binary layout and MAC verification.
use super::*;

impl AuthenticatedEnvelope {
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

pub(super) fn frame_mac(key: &[u8; 32], body_length: u32, authenticated_bytes: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(FRAME_DOMAIN);
    mac.update(&body_length.to_be_bytes());
    mac.update(authenticated_bytes);
    mac.finalize().into_bytes().into()
}
