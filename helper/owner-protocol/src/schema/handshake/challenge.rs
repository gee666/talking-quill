//! Owner challenge schema and validation.
use super::*;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    #[serde(rename = "type")]
    kind: ChallengeTag,
    #[serde(rename = "ownerProtocol")]
    pub owner_protocol: ProtocolHeader,
    #[serde(rename = "selectedProtocol")]
    pub selected_protocol: SelectedProtocol,
    pub purpose: Purpose,
    #[serde(rename = "authorityCeiling")]
    pub authority_ceiling: AuthorityCeiling,
    #[serde(rename = "ownerNonce")]
    pub owner_nonce: Bytes32,
    #[serde(rename = "sessionId")]
    pub session_id: Bytes32,
    #[serde(rename = "ownerInstanceId")]
    pub owner_instance_id: Bytes32,
    pub platform: Platform,
    pub architecture: Architecture,
    #[serde(rename = "releaseBuildDigest")]
    pub release_build_digest: Bytes32,
    #[serde(rename = "executableSha256")]
    pub executable_sha256: Bytes32,
    #[serde(rename = "installationIdentityDigest")]
    pub installation_identity_digest: Bytes32,
    #[serde(rename = "signerPolicyDigest")]
    pub signer_policy_digest: Bytes32,
    #[serde(rename = "osSessionBindingDigest")]
    pub os_session_binding_digest: Bytes32,
    #[serde(rename = "ownerReleasePolicy")]
    pub owner_release_policy: PolicyBlob,
    #[serde(rename = "ownerReleasePolicySignature")]
    pub owner_release_policy_signature: PolicySignature,
    #[serde(rename = "ownerReleasePolicyDigest")]
    pub owner_release_policy_digest: Bytes32,
    #[serde(rename = "platformCredentialBindingDigest")]
    pub platform_credential_binding_digest: Bytes32,
    #[serde(
        rename = "ownerEphemeralPublicKey",
        deserialize_with = "deserialize_required_option"
    )]
    pub owner_ephemeral_public_key: Option<P256PublicKey>,
}

impl Challenge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_protocol: ProtocolHeader,
        selected_protocol: SelectedProtocol,
        purpose: Purpose,
        authority_ceiling: AuthorityCeiling,
        owner_nonce: Bytes32,
        session_id: Bytes32,
        owner_instance_id: Bytes32,
        platform: Platform,
        architecture: Architecture,
        release_build_digest: Bytes32,
        executable_sha256: Bytes32,
        installation_identity_digest: Bytes32,
        signer_policy_digest: Bytes32,
        os_session_binding_digest: Bytes32,
        owner_release_policy: PolicyBlob,
        owner_release_policy_signature: PolicySignature,
        platform_credential_binding_digest: Bytes32,
        owner_ephemeral_public_key: Option<P256PublicKey>,
    ) -> Result<Self, SchemaError> {
        let owner_release_policy_digest = owner_release_policy.digest();
        let value = Self {
            kind: ChallengeTag::Challenge,
            owner_protocol,
            selected_protocol,
            purpose,
            authority_ceiling,
            owner_nonce,
            session_id,
            owner_instance_id,
            platform,
            architecture,
            release_build_digest,
            executable_sha256,
            installation_identity_digest,
            signer_policy_digest,
            os_session_binding_digest,
            owner_release_policy,
            owner_release_policy_signature,
            owner_release_policy_digest,
            platform_credential_binding_digest,
            owner_ephemeral_public_key,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, SchemaError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| SchemaError::Json)?;
        parse_handshake_json(&bytes)?;
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        self.owner_protocol.validate()?;
        if self.selected_protocol.major != self.owner_protocol.major
            || self.selected_protocol.compatibility_epoch != self.owner_protocol.compatibility_epoch
            || self.selected_protocol.minor > self.owner_protocol.minor
            || self.selected_protocol.feature_bits.get()
                & !self.owner_protocol.supported_feature_bits.get()
                != 0
            || self.selected_protocol.feature_bits.get()
                & self.owner_protocol.required_feature_bits.get()
                != self.owner_protocol.required_feature_bits.get()
        {
            return Err(SchemaError::Protocol(
                ProtocolSelectionError::MissingRequiredFeature,
            ));
        }
        if !self
            .owner_release_policy_digest
            .constant_time_eq(&self.owner_release_policy.digest())
        {
            return Err(SchemaError::PolicyDigest);
        }
        validate_platform_key(self.platform, self.owner_ephemeral_public_key.as_ref())?;
        if purpose_ceiling(self.purpose) != self.authority_ceiling {
            return Err(SchemaError::AuthorityCeiling);
        }
        Ok(())
    }
}

impl fmt::Debug for Challenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Challenge([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ChallengeTag {
    Challenge,
}
