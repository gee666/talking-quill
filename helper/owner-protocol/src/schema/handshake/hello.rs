//! Gateway hello schema and validation.
use super::*;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    #[serde(rename = "type")]
    kind: HelloTag,
    pub purpose: Purpose,
    pub protocol: ProtocolHeader,
    #[serde(rename = "clientNonce")]
    pub client_nonce: Bytes32,
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
    #[serde(rename = "clientReleasePolicy")]
    pub client_release_policy: PolicyBlob,
    #[serde(rename = "clientReleasePolicySignature")]
    pub client_release_policy_signature: PolicySignature,
    #[serde(rename = "clientReleasePolicyDigest")]
    pub client_release_policy_digest: Bytes32,
    #[serde(rename = "platformCredentialBindingDigest")]
    pub platform_credential_binding_digest: Bytes32,
    #[serde(
        rename = "clientEphemeralPublicKey",
        deserialize_with = "deserialize_required_option"
    )]
    pub client_ephemeral_public_key: Option<P256PublicKey>,
}

impl Hello {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        purpose: Purpose,
        protocol: ProtocolHeader,
        client_nonce: Bytes32,
        platform: Platform,
        architecture: Architecture,
        release_build_digest: Bytes32,
        executable_sha256: Bytes32,
        installation_identity_digest: Bytes32,
        signer_policy_digest: Bytes32,
        os_session_binding_digest: Bytes32,
        client_release_policy: PolicyBlob,
        client_release_policy_signature: PolicySignature,
        platform_credential_binding_digest: Bytes32,
        client_ephemeral_public_key: Option<P256PublicKey>,
    ) -> Result<Self, SchemaError> {
        let client_release_policy_digest = client_release_policy.digest();
        let value = Self {
            kind: HelloTag::Hello,
            purpose,
            protocol,
            client_nonce,
            platform,
            architecture,
            release_build_digest,
            executable_sha256,
            installation_identity_digest,
            signer_policy_digest,
            os_session_binding_digest,
            client_release_policy,
            client_release_policy_signature,
            client_release_policy_digest,
            platform_credential_binding_digest,
            client_ephemeral_public_key,
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
        self.protocol.validate()?;
        if !self
            .client_release_policy_digest
            .constant_time_eq(&self.client_release_policy.digest())
        {
            return Err(SchemaError::PolicyDigest);
        }
        validate_platform_key(self.platform, self.client_ephemeral_public_key.as_ref())
    }
}

impl fmt::Debug for Hello {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Hello([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HelloTag {
    Hello,
}
