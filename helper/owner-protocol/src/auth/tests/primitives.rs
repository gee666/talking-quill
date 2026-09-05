use super::*;

#[test]
fn rfc_5869_sha256_case_one_matches_extract_and_expand() {
    // RFC 5869 case 1 verifies the pinned HKDF implementation independently
    // of the owner transcript construction.
    let ikm = vec![0x0b; 22];
    let salt = hex("000102030405060708090a0b0c");
    let info = hex("f0f1f2f3f4f5f6f7f8f9");
    let (_, hkdf) = hkdf::Hkdf::<sha2::Sha256>::extract(Some(&salt), &ikm);
    let mut okm = [0_u8; 42];
    hkdf.expand(&info, &mut okm)
        .expect("valid RFC output length");
    assert_eq!(
        okm.as_slice(),
        hex("3cb25f25faacd57a90434f64d0362f2a\
             2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
             34007208d5b887185865")
    );
}

#[test]
fn proof_verification_rejects_one_bit_change() {
    let transcript = super::Transcript {
        bytes: vec![],
        hash: Bytes32::new([7; 32]),
        selected_protocol: crate::schema::SelectedProtocol {
            major: 1,
            minor: 0,
            compatibility_epoch: 1,
            feature_bits: crate::scalar::FeatureBits::new(1),
        },
        purpose: crate::schema::Purpose::Capture,
        authority_ceiling: crate::schema::AuthorityCeiling::Capture,
        session_id: Bytes32::new([0; 32]),
        owner_instance_id: Bytes32::new([1; 32]),
        key_agreement_mode: super::KeyAgreementMode::MacosKeychain,
        client_p256_public_key: None,
        owner_p256_public_key: None,
    };
    let mut secret = [9; 32];
    let material = KeyAgreementMaterial::macos(&mut secret);
    assert_eq!(secret, [0; 32]);
    let keys = AuthenticationKeys::derive(&transcript, &material).expect("derive");
    let proofs = keys.proofs(&transcript);
    let authenticated = crate::schema::Authenticated::new(
        transcript.selected_protocol,
        transcript.purpose,
        transcript.authority_ceiling,
        proofs.owner_proof,
    )
    .expect("authenticated payload");
    keys.verify_authenticated_finish(&transcript, &proofs.client_proof, &authenticated)
        .expect("valid authenticated finish");
    let finish = crate::envelope::AuthenticatedEnvelope::authenticated_finish(
        transcript.session_id,
        &authenticated,
    )
    .expect("finish")
    .encode_body(keys.owner_frame_key())
    .expect("finish body");
    let session = keys
        .pending_gateway_session(&transcript)
        .expect("pending")
        .accept_authenticated_finish(&finish, &proofs.client_proof)
        .expect("atomic finish");
    assert_eq!(session.purpose(), transcript.purpose);

    let mut changed = *proofs.owner_proof.as_bytes();
    changed[0] ^= 1;
    assert!(
        keys.verify_owner_proof(&transcript, &proofs.client_proof, &Bytes32::new(changed))
            .is_err()
    );
    let wrong = crate::schema::Authenticated::new(
        transcript.selected_protocol,
        transcript.purpose,
        transcript.authority_ceiling,
        Bytes32::new(changed),
    )
    .expect("wrong proof payload");
    let wrong_finish =
        crate::envelope::AuthenticatedEnvelope::authenticated_finish(transcript.session_id, &wrong)
            .expect("finish")
            .encode_body(keys.owner_frame_key())
            .expect("finish body");
    assert!(
        keys.pending_gateway_session(&transcript)
            .expect("pending")
            .accept_authenticated_finish(&wrong_finish, &proofs.client_proof)
            .is_err()
    );
}

#[test]
fn invalid_p256_private_scalar_is_rejected() {
    assert!(super::EphemeralP256Secret::from_bytes([0; 32]).is_err());
}

#[test]
fn fixed_p256_ecdh_known_answer_matches_both_protocol_roles() {
    // RFC 5903 Section 8.1, NIST P-256 ECDH.
    let client_secret = super::EphemeralP256Secret::from_bytes(hex32_array(
        "c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433",
    ))
    .expect("client private scalar");
    let owner_secret = super::EphemeralP256Secret::from_bytes(hex32_array(
        "c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53",
    ))
    .expect("owner private scalar");
    let client_public = P256PublicKey::from_sec1_bytes(
        hex(concat!(
            "04dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c3772581180",
            "5271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3"
        ))
        .try_into()
        .expect("65-byte client public point"),
    )
    .expect("client public point");
    let owner_public = P256PublicKey::from_sec1_bytes(
        hex(concat!(
            "04d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf63",
            "56fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab"
        ))
        .try_into()
        .expect("65-byte owner public point"),
    )
    .expect("owner public point");
    assert_eq!(client_secret.public_key(), &client_public);
    assert_eq!(owner_secret.public_key(), &owner_public);

    let gateway =
        KeyAgreementMaterial::windows_peer(super::PeerRole::Gateway, &client_secret, &owner_public)
            .expect("gateway ECDH");
    let owner =
        KeyAgreementMaterial::windows_peer(super::PeerRole::Owner, &owner_secret, &client_public)
            .expect("owner ECDH");
    let expected = hex("d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de");
    assert_eq!(gateway.ikm, expected);
    assert_eq!(owner.ikm, expected);
}
