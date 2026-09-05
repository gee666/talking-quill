//! HKDF-separated directional keys and handshake proofs.
use super::*;

impl AuthenticationKeys {
    pub fn derive(
        transcript: &Transcript,
        material: &KeyAgreementMaterial,
    ) -> Result<Self, AuthenticationError> {
        if !material.matches(transcript) {
            return Err(AuthenticationError::KeyAgreement);
        }
        let (_, hkdf) = Hkdf::<Sha256>::extract(Some(transcript.hash.as_bytes()), &material.ikm);
        Ok(Self {
            client_proof_key: expand(&hkdf, CLIENT_PROOF_INFO)?,
            owner_proof_key: expand(&hkdf, OWNER_PROOF_INFO)?,
            gateway_frame_key: FrameKey::from_secret(
                Direction::GatewayToOwner,
                expand(&hkdf, GATEWAY_FRAME_INFO)?,
            ),
            owner_frame_key: FrameKey::from_secret(
                Direction::OwnerToGateway,
                expand(&hkdf, OWNER_FRAME_INFO)?,
            ),
        })
    }

    pub fn pending_gateway_session<'a>(
        &'a self,
        transcript: &'a Transcript,
    ) -> Result<PendingAuthenticatedSession<'a>, AuthenticationError> {
        PendingAuthenticatedSession::new(transcript, self)
    }

    pub fn establish_owner_session(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
    ) -> Result<AuthenticatedSession, AuthenticationError> {
        self.verify_client_proof(transcript, client_proof)?;
        Ok(AuthenticatedSession {
            session_id: transcript.session_id,
            owner_instance_id: transcript.owner_instance_id,
            purpose: transcript.purpose,
            authority_ceiling: transcript.authority_ceiling,
            feature_bits: transcript.selected_protocol.feature_bits,
            test_only: false,
            inbound: EnvelopeReceiver::new(
                Direction::GatewayToOwner,
                transcript.session_id,
                self.gateway_frame_key(),
                transcript.purpose,
            )?,
        })
    }

    #[must_use]
    pub const fn gateway_frame_key(&self) -> &FrameKey {
        &self.gateway_frame_key
    }

    #[must_use]
    pub const fn owner_frame_key(&self) -> &FrameKey {
        &self.owner_frame_key
    }

    #[must_use]
    pub fn proofs(&self, transcript: &Transcript) -> AuthenticationProofs {
        let mut client_message = Vec::with_capacity(CLIENT_FINISH_DOMAIN.len() + 32);
        client_message.extend_from_slice(CLIENT_FINISH_DOMAIN);
        client_message.extend_from_slice(transcript.hash.as_bytes());
        let client_proof = hmac(&self.client_proof_key, &client_message);

        let mut owner_message = Vec::with_capacity(OWNER_FINISH_DOMAIN.len() + 64);
        owner_message.extend_from_slice(OWNER_FINISH_DOMAIN);
        owner_message.extend_from_slice(transcript.hash.as_bytes());
        owner_message.extend_from_slice(client_proof.as_bytes());
        let owner_proof = hmac(&self.owner_proof_key, &owner_message);
        AuthenticationProofs {
            client_proof,
            owner_proof,
        }
    }

    pub fn verify_client_proof(
        &self,
        transcript: &Transcript,
        proof: &Bytes32,
    ) -> Result<(), AuthenticationError> {
        let mut message = Vec::with_capacity(CLIENT_FINISH_DOMAIN.len() + 32);
        message.extend_from_slice(CLIENT_FINISH_DOMAIN);
        message.extend_from_slice(transcript.hash.as_bytes());
        verify_hmac(&self.client_proof_key, &message, proof)
    }

    pub fn verify_authenticated_finish(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
        authenticated: &Authenticated,
    ) -> Result<(), AuthenticationError> {
        if authenticated.selected_protocol != transcript.selected_protocol
            || authenticated.purpose != transcript.purpose
            || authenticated.authority_ceiling != transcript.authority_ceiling
        {
            return Err(AuthenticationError::Binding);
        }
        self.verify_owner_proof(transcript, client_proof, &authenticated.owner_proof)
    }

    pub(super) fn verify_owner_proof(
        &self,
        transcript: &Transcript,
        client_proof: &Bytes32,
        owner_proof: &Bytes32,
    ) -> Result<(), AuthenticationError> {
        self.verify_client_proof(transcript, client_proof)?;
        let mut message = Vec::with_capacity(OWNER_FINISH_DOMAIN.len() + 64);
        message.extend_from_slice(OWNER_FINISH_DOMAIN);
        message.extend_from_slice(transcript.hash.as_bytes());
        message.extend_from_slice(client_proof.as_bytes());
        verify_hmac(&self.owner_proof_key, &message, owner_proof)
    }
}

fn expand(hkdf: &Hkdf<Sha256>, info: &[u8]) -> Result<[u8; 32], AuthenticationError> {
    let mut output = [0_u8; 32];
    hkdf.expand(info, &mut output)
        .map_err(|_| AuthenticationError::KeyDerivation)?;
    Ok(output)
}

fn hmac(key: &[u8; 32], message: &[u8]) -> Bytes32 {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(message);
    Bytes32::new(mac.finalize().into_bytes().into())
}

fn verify_hmac(
    key: &[u8; 32],
    message: &[u8],
    expected: &Bytes32,
) -> Result<(), AuthenticationError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("SHA-256 HMAC accepts 32-byte keys");
    mac.update(message);
    mac.verify_slice(expected.as_bytes())
        .map_err(|_| AuthenticationError::Proof)
}
