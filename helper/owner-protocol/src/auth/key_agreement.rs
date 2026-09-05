//! Platform key material and validated ephemeral P-256 secrets.
use super::*;

impl KeyAgreementMaterial {
    #[must_use]
    pub fn macos(per_install_keychain_secret: &mut [u8; 32]) -> Self {
        let ikm = per_install_keychain_secret.to_vec();
        per_install_keychain_secret.zeroize();
        Self {
            ikm,
            mode: KeyAgreementMode::MacosKeychain,
            client_public_key: None,
            owner_public_key: None,
        }
    }

    /// Windows stable named-pipe mode. Authentication authority comes from the
    /// pipe ACL plus kernel-derived peer process/token/image facts, so the
    /// ephemeral P-256 shared value is the complete key material.
    pub fn windows_peer(
        local_role: PeerRole,
        local_secret: &EphemeralP256Secret,
        peer_public_key: &P256PublicKey,
    ) -> Result<Self, AuthenticationError> {
        let shared = local_secret
            .secret
            .diffie_hellman(&peer_public_key.parsed());
        Self::windows_from_shared(
            shared.raw_secret_bytes().to_vec(),
            KeyAgreementMode::WindowsStablePipeP256,
            local_role,
            local_secret,
            peer_public_key,
        )
    }

    fn windows_from_shared(
        ikm: Vec<u8>,
        mode: KeyAgreementMode,
        local_role: PeerRole,
        local_secret: &EphemeralP256Secret,
        peer_public_key: &P256PublicKey,
    ) -> Result<Self, AuthenticationError> {
        let (client_public_key, owner_public_key) = match local_role {
            PeerRole::Gateway => (local_secret.public.clone(), peer_public_key.clone()),
            PeerRole::Owner => (peer_public_key.clone(), local_secret.public.clone()),
        };
        Ok(Self {
            ikm,
            mode,
            client_public_key: Some(client_public_key),
            owner_public_key: Some(owner_public_key),
        })
    }

    pub(super) fn matches(&self, transcript: &Transcript) -> bool {
        self.mode == transcript.key_agreement_mode
            && self.client_public_key == transcript.client_p256_public_key
            && self.owner_public_key == transcript.owner_p256_public_key
    }
}

impl EphemeralP256Secret {
    pub fn random() -> Result<Self, AuthenticationError> {
        for _ in 0..128 {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).map_err(|_| AuthenticationError::Random)?;
            if let Ok(secret) = SecretKey::from_slice(&bytes) {
                bytes.zeroize();
                let public = P256PublicKey::from_key(&secret.public_key());
                return Ok(Self { secret, public });
            }
            bytes.zeroize();
        }
        Err(AuthenticationError::Random)
    }

    #[cfg(test)]
    pub(super) fn from_bytes(mut bytes: [u8; 32]) -> Result<Self, AuthenticationError> {
        let parsed = SecretKey::from_slice(&bytes);
        bytes.zeroize();
        let secret = parsed.map_err(|_| AuthenticationError::KeyAgreement)?;
        let public = P256PublicKey::from_key(&secret.public_key());
        Ok(Self { secret, public })
    }

    #[must_use]
    pub const fn public_key(&self) -> &P256PublicKey {
        &self.public
    }
}
