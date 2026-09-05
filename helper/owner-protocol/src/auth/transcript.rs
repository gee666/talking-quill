//! Verified handshake binding and canonical transcript bytes.
use super::*;

impl KeyAgreementMode {
    const fn tag(self) -> u8 {
        match self {
            Self::MacosKeychain => 1,
            Self::WindowsStablePipeP256 => 3,
        }
    }
}

impl TranscriptInput {
    pub fn from_verified_handshake<V: HandshakeTrustVerifier>(
        hello: &Hello,
        challenge: &Challenge,
        verifier: &V,
    ) -> Result<Self, AuthenticationError> {
        hello.validate()?;
        challenge.validate()?;
        let selected = hello.protocol.negotiate(challenge.owner_protocol)?;
        if selected != challenge.selected_protocol {
            return Err(AuthenticationError::Selection);
        }
        if hello.purpose != challenge.purpose
            || hello.platform != challenge.platform
            || hello.architecture != challenge.architecture
            || !hello
                .installation_identity_digest
                .constant_time_eq(&challenge.installation_identity_digest)
            || !hello
                .os_session_binding_digest
                .constant_time_eq(&challenge.os_session_binding_digest)
            || !hello
                .platform_credential_binding_digest
                .constant_time_eq(&challenge.platform_credential_binding_digest)
        {
            return Err(AuthenticationError::Binding);
        }
        let (client_policy, owner_policy) = validate_policy_self_fields(hello, challenge)?;
        verifier.verify(hello, challenge, &client_policy, &owner_policy)?;
        validate_policy_pair(hello.purpose, &client_policy, &owner_policy)?;
        let key_agreement_mode = match hello.platform {
            Platform::Macos => KeyAgreementMode::MacosKeychain,
            Platform::Windows => KeyAgreementMode::WindowsStablePipeP256,
        };
        Ok(Self {
            client_protocol: hello.protocol,
            owner_protocol: challenge.owner_protocol,
            selected_protocol: selected,
            purpose: hello.purpose,
            authority_ceiling: challenge.authority_ceiling,
            platform: hello.platform,
            client_architecture: hello.architecture,
            owner_architecture: challenge.architecture,
            client_nonce: hello.client_nonce,
            owner_nonce: challenge.owner_nonce,
            session_id: challenge.session_id,
            owner_instance_id: challenge.owner_instance_id,
            client_release_build_digest: hello.release_build_digest,
            client_executable_digest: hello.executable_sha256,
            owner_release_build_digest: challenge.release_build_digest,
            owner_executable_digest: challenge.executable_sha256,
            installation_identity_digest: hello.installation_identity_digest,
            client_signer_policy_digest: hello.signer_policy_digest,
            owner_signer_policy_digest: challenge.signer_policy_digest,
            os_session_binding_digest: hello.os_session_binding_digest,
            client_release_policy_digest: hello.client_release_policy_digest,
            owner_release_policy_digest: challenge.owner_release_policy_digest,
            platform_credential_binding_digest: hello.platform_credential_binding_digest,
            key_agreement_mode,
            client_p256_public_key: hello.client_ephemeral_public_key.clone(),
            owner_p256_public_key: challenge.owner_ephemeral_public_key.clone(),
        })
    }

    fn validate(&self) -> Result<(), AuthenticationError> {
        self.client_protocol.validate()?;
        self.owner_protocol.validate()?;
        let selected = self.client_protocol.negotiate(self.owner_protocol)?;
        if selected != self.selected_protocol {
            return Err(AuthenticationError::Selection);
        }
        let ceiling_matches = matches!(
            (self.purpose, self.authority_ceiling),
            (Purpose::Observe, AuthorityCeiling::Observer)
                | (Purpose::Capture, AuthorityCeiling::Capture)
                | (Purpose::Maintenance, AuthorityCeiling::Maintenance)
        );
        if !ceiling_matches {
            return Err(AuthenticationError::Binding);
        }
        let keys_match = match self.key_agreement_mode {
            KeyAgreementMode::MacosKeychain => {
                self.platform == Platform::Macos
                    && self.client_p256_public_key.is_none()
                    && self.owner_p256_public_key.is_none()
            }
            KeyAgreementMode::WindowsStablePipeP256 => {
                self.platform == Platform::Windows
                    && self.client_p256_public_key.is_some()
                    && self.owner_p256_public_key.is_some()
            }
        };
        if !keys_match {
            return Err(AuthenticationError::KeyAgreement);
        }
        Ok(())
    }
}

impl Transcript {
    pub fn build(input: &TranscriptInput) -> Result<Self, AuthenticationError> {
        input.validate()?;
        let mut bytes = Vec::with_capacity(712);
        bytes.extend_from_slice(TRANSCRIPT_DOMAIN);
        append_protocol(&mut bytes, input.client_protocol);
        append_protocol(&mut bytes, input.owner_protocol);
        append_selected_protocol(&mut bytes, input.selected_protocol);
        bytes.push(input.purpose.tag());
        bytes.push(input.authority_ceiling.tag());
        bytes.push(input.platform.tag());
        bytes.push(input.client_architecture.tag());
        bytes.push(input.owner_architecture.tag());
        bytes.extend_from_slice(&[0; 3]);
        for value in [
            input.client_nonce,
            input.owner_nonce,
            input.session_id,
            input.owner_instance_id,
            input.client_release_build_digest,
            input.client_executable_digest,
            input.owner_release_build_digest,
            input.owner_executable_digest,
            input.installation_identity_digest,
            input.client_signer_policy_digest,
            input.owner_signer_policy_digest,
            input.os_session_binding_digest,
            input.client_release_policy_digest,
            input.owner_release_policy_digest,
            input.platform_credential_binding_digest,
        ] {
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes.push(input.key_agreement_mode.tag());
        bytes.extend_from_slice(&[0; 3]);
        append_public_key(&mut bytes, input.client_p256_public_key.as_ref());
        append_public_key(&mut bytes, input.owner_p256_public_key.as_ref());
        let hash = Bytes32::new(Sha256::digest(&bytes).into());
        Ok(Self {
            bytes,
            hash,
            selected_protocol: input.selected_protocol,
            purpose: input.purpose,
            authority_ceiling: input.authority_ceiling,
            session_id: input.session_id,
            owner_instance_id: input.owner_instance_id,
            key_agreement_mode: input.key_agreement_mode,
            client_p256_public_key: input.client_p256_public_key.clone(),
            owner_p256_public_key: input.owner_p256_public_key.clone(),
        })
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn hash(&self) -> Bytes32 {
        self.hash
    }
}

fn append_protocol(output: &mut Vec<u8>, protocol: ProtocolHeader) {
    output.extend_from_slice(&protocol.major.to_be_bytes());
    output.extend_from_slice(&protocol.minor.to_be_bytes());
    output.extend_from_slice(&protocol.compatibility_epoch.to_be_bytes());
    output.extend_from_slice(&protocol.supported_feature_bits.get().to_be_bytes());
    output.extend_from_slice(&protocol.required_feature_bits.get().to_be_bytes());
}

fn append_selected_protocol(output: &mut Vec<u8>, protocol: SelectedProtocol) {
    output.extend_from_slice(&protocol.major.to_be_bytes());
    output.extend_from_slice(&protocol.minor.to_be_bytes());
    output.extend_from_slice(&protocol.compatibility_epoch.to_be_bytes());
    output.extend_from_slice(&protocol.feature_bits.get().to_be_bytes());
}

fn append_public_key(output: &mut Vec<u8>, key: Option<&P256PublicKey>) {
    match key {
        Some(key) => output.extend_from_slice(key.as_bytes()),
        None => output.extend_from_slice(&[0; 65]),
    }
}
