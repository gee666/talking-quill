//! Atomic authenticated finish and established inbound session authority.
use super::*;

impl<'a> PendingAuthenticatedSession<'a> {
    pub(super) fn new(
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
