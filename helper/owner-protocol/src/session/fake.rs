//! Explicitly test-branded authentication material.
use super::*;

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
