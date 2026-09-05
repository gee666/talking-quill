use super::*;

#[test]
fn frozen_cross_language_protocol_vectors_conform() {
    use crate::auth::{KeyAgreementMode, Transcript, TranscriptInput};
    use crate::envelope::{AuthenticatedEnvelope, CorrelationTracker};
    use crate::framing::encode_outer_frame;
    use crate::scalar::{FeatureBits, P256PublicKey};
    use crate::schema::{
        Architecture, Authenticated, AuthorityCeiling, Empty, ErrorBody, ErrorCode, Platform,
        ProtocolHeader, Purpose, Request, Response, SelectedProtocol,
    };

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../tests/fixtures/compatibility/keyboard-owner-v1/owner-protocol-vectors.json"
    ))
    .expect("valid fixture JSON");
    assert_eq!(fixture["fixtureVersion"], 1);
    for vector in fixture["vectors"].as_array().expect("vectors") {
        let input = &vector["input"];
        let expected = &vector["expected"];
        let protocol = |name: &str| -> ProtocolHeader {
            serde_json::from_value(input[name].clone()).expect("protocol header")
        };
        let selected: SelectedProtocol =
            serde_json::from_value(input["selectedProtocol"].clone()).expect("selected");
        let platform = match text(input, "platform") {
            "windows" => Platform::Windows,
            "macos" => Platform::Macos,
            _ => panic!("fixture platform"),
        };
        let architecture = |name: &str| match text(input, name) {
            "x64" => Architecture::X64,
            "arm64" => Architecture::Arm64,
            _ => panic!("fixture architecture"),
        };
        let public = |name: &str| {
            input[name].as_str().map(|value| {
                P256PublicKey::from_sec1_bytes(hex(value).try_into().expect("65-byte public point"))
                    .expect("valid public point")
            })
        };
        let transcript_input = TranscriptInput {
            client_protocol: protocol("clientProtocol"),
            owner_protocol: protocol("ownerProtocol"),
            selected_protocol: selected,
            purpose: Purpose::Capture,
            authority_ceiling: AuthorityCeiling::Capture,
            platform,
            client_architecture: architecture("clientArchitecture"),
            owner_architecture: architecture("ownerArchitecture"),
            client_nonce: hex32(text(input, "clientNonce")),
            owner_nonce: hex32(text(input, "ownerNonce")),
            session_id: hex32(text(input, "sessionId")),
            owner_instance_id: hex32(text(input, "ownerInstanceId")),
            client_release_build_digest: hex32(text(input, "clientReleaseBuildDigest")),
            client_executable_digest: hex32(text(input, "clientExecutableDigest")),
            owner_release_build_digest: hex32(text(input, "ownerReleaseBuildDigest")),
            owner_executable_digest: hex32(text(input, "ownerExecutableDigest")),
            installation_identity_digest: hex32(text(input, "installationIdentityDigest")),
            client_signer_policy_digest: hex32(text(input, "clientSignerPolicyDigest")),
            owner_signer_policy_digest: hex32(text(input, "ownerSignerPolicyDigest")),
            os_session_binding_digest: hex32(text(input, "osSessionBindingDigest")),
            client_release_policy_digest: hex32(text(input, "clientReleasePolicyDigest")),
            owner_release_policy_digest: hex32(text(input, "ownerReleasePolicyDigest")),
            platform_credential_binding_digest: hex32(text(
                input,
                "platformCredentialBindingDigest",
            )),
            key_agreement_mode: match text(input, "keyAgreementMode") {
                "macos_keychain" => KeyAgreementMode::MacosKeychain,
                "windows_stable_pipe_p256" => KeyAgreementMode::WindowsStablePipeP256,
                _ => panic!("fixture key mode"),
            },
            client_p256_public_key: public("clientP256PublicKey"),
            owner_p256_public_key: public("ownerP256PublicKey"),
        };
        assert_eq!(
            transcript_input.selected_protocol.feature_bits,
            FeatureBits::new(1)
        );
        let transcript = Transcript::build(&transcript_input).expect("transcript");
        assert_eq!(transcript.as_bytes(), hex(text(expected, "transcriptHex")));
        for field in 0..15 {
            let mut changed = transcript_input.clone();
            let replacement = Bytes32::new([0xf0 + field; 32]);
            match field {
                0 => changed.client_nonce = replacement,
                1 => changed.owner_nonce = replacement,
                2 => changed.session_id = replacement,
                3 => changed.owner_instance_id = replacement,
                4 => changed.client_release_build_digest = replacement,
                5 => changed.client_executable_digest = replacement,
                6 => changed.owner_release_build_digest = replacement,
                7 => changed.owner_executable_digest = replacement,
                8 => changed.installation_identity_digest = replacement,
                9 => changed.client_signer_policy_digest = replacement,
                10 => changed.owner_signer_policy_digest = replacement,
                11 => changed.os_session_binding_digest = replacement,
                12 => changed.client_release_policy_digest = replacement,
                13 => changed.owner_release_policy_digest = replacement,
                14 => changed.platform_credential_binding_digest = replacement,
                _ => unreachable!(),
            }
            assert_ne!(
                Transcript::build(&changed)
                    .expect("mutated transcript")
                    .hash(),
                transcript.hash(),
                "transcript field {field} was not bound"
            );
        }
        assert_eq!(transcript.hash(), hex32(text(expected, "transcriptSha256")));

        let expected_ikm = hex(text(input, "platformIkm"));
        let material = if platform == Platform::Windows {
            let client_secret = super::EphemeralP256Secret::from_bytes(hex32_array(text(
                input,
                "clientP256PrivateKey",
            )))
            .expect("client private scalar");
            let owner_secret = super::EphemeralP256Secret::from_bytes(hex32_array(text(
                input,
                "ownerP256PrivateKey",
            )))
            .expect("owner private scalar");
            assert_eq!(
                client_secret.public_key(),
                transcript_input
                    .client_p256_public_key
                    .as_ref()
                    .expect("client public")
            );
            assert_eq!(
                owner_secret.public_key(),
                transcript_input
                    .owner_p256_public_key
                    .as_ref()
                    .expect("owner public")
            );
            let client_material = KeyAgreementMaterial::windows_peer(
                super::PeerRole::Gateway,
                &client_secret,
                owner_secret.public_key(),
            )
            .expect("client ECDH");
            let owner_material = KeyAgreementMaterial::windows_peer(
                super::PeerRole::Owner,
                &owner_secret,
                client_secret.public_key(),
            )
            .expect("owner ECDH");
            assert_eq!(client_material.ikm, owner_material.ikm);
            assert_eq!(client_material.ikm, expected_ikm);
            assert_eq!(client_material.ikm, hex(text(expected, "rawP256AffineX")));
            client_material
        } else {
            let mut secret = hex32_array(text(input, "macosSecret"));
            let material = KeyAgreementMaterial::macos(&mut secret);
            assert_eq!(secret, [0; 32]);
            assert_eq!(material.ikm, expected_ikm);
            material
        };
        let keys = AuthenticationKeys::derive(&transcript, &material).expect("keys");
        assert_eq!(
            keys.client_proof_key,
            hex32_array(text(expected, "clientProofKey"))
        );
        assert_eq!(
            keys.owner_proof_key,
            hex32_array(text(expected, "ownerProofKey"))
        );
        assert_eq!(
            *keys.gateway_frame_key.as_bytes(),
            hex32_array(text(expected, "gatewayFrameKey"))
        );
        assert_eq!(
            *keys.owner_frame_key.as_bytes(),
            hex32_array(text(expected, "ownerFrameKey"))
        );
        let proofs = keys.proofs(&transcript);
        assert_eq!(proofs.client_proof, hex32(text(expected, "clientProof")));
        assert_eq!(proofs.owner_proof, hex32(text(expected, "ownerProof")));
        keys.verify_owner_proof(&transcript, &proofs.client_proof, &proofs.owner_proof)
            .expect("proof verification");

        let request = Request::HealthGet(Empty {});
        let frame = &expected["gatewayRequestFrame"];
        let envelope = AuthenticatedEnvelope::request(transcript_input.session_id, 1, &request)
            .expect("envelope");
        assert_eq!(envelope.payload(), text(frame, "payloadUtf8").as_bytes());
        let body = envelope
            .encode_body(keys.gateway_frame_key())
            .expect("encoded body");
        assert_eq!(body, hex(text(frame, "bodyHex")));
        assert_eq!(&body[body.len() - 32..], hex(text(frame, "mac")));
        assert_eq!(
            encode_outer_frame(&body).expect("outer frame"),
            hex(text(frame, "outerFrameHex"))
        );

        let authenticated = Authenticated::new(
            transcript_input.selected_protocol,
            transcript_input.purpose,
            transcript_input.authority_ceiling,
            proofs.owner_proof,
        )
        .expect("authenticated payload");
        let finish_frame = &expected["authenticatedFinishFrame"];
        let finish_envelope = AuthenticatedEnvelope::authenticated_finish(
            transcript_input.session_id,
            &authenticated,
        )
        .expect("finish envelope");
        assert_eq!(
            finish_envelope.payload(),
            text(finish_frame, "payloadUtf8").as_bytes()
        );
        let finish_body = finish_envelope
            .encode_body(keys.owner_frame_key())
            .expect("finish body");
        assert_eq!(finish_body, hex(text(finish_frame, "bodyHex")));

        let mut session = keys
            .pending_gateway_session(&transcript)
            .expect("pending session")
            .accept_authenticated_finish(&finish_body, &proofs.client_proof)
            .expect("authenticated session");
        assert_eq!(session.session_id(), transcript_input.session_id);
        assert_eq!(session.purpose(), Purpose::Capture);

        let response = Response::Error(ErrorBody::new(ErrorCode::Unavailable));
        let owner_frame = &expected["ownerResponseFrame"];
        let owner_envelope = AuthenticatedEnvelope::response_for_request(
            transcript_input.session_id,
            2,
            1,
            &request,
            &response,
        )
        .expect("owner envelope");
        assert_eq!(
            owner_envelope.payload(),
            text(owner_frame, "payloadUtf8").as_bytes()
        );
        let owner_body = owner_envelope
            .encode_body(keys.owner_frame_key())
            .expect("owner body");
        assert_eq!(owner_body, hex(text(owner_frame, "bodyHex")));
        assert_eq!(
            &owner_body[owner_body.len() - 32..],
            hex(text(owner_frame, "mac"))
        );
        assert_eq!(
            encode_outer_frame(&owner_body).expect("owner outer frame"),
            hex(text(owner_frame, "outerFrameHex"))
        );
        let mut correlations = CorrelationTracker::new();
        correlations
            .register_request(1, &request)
            .expect("request correlation");
        assert_eq!(
            session
                .inbound()
                .accept_response(&owner_body, &mut correlations)
                .expect("typed response")
                .1,
            response
        );
        assert!(correlations.is_empty());
    }
}
