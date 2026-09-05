use super::*;

#[test]
fn maintenance_hex_is_canonical_bytes_not_a_text_hash() {
    let encoded = (0_u8..32)
        .map(|value| format!("{value:02x}"))
        .collect::<String>();
    let decoded = maintenance_digest(b"ignored-domain", &encoded).unwrap();
    let expected: [u8; 32] = (0_u8..32).collect::<Vec<_>>().try_into().unwrap();
    assert_eq!(decoded.as_bytes(), &expected);
    assert!(maintenance_digest(b"ignored-domain", &encoded.to_uppercase()).is_err());
    assert!(maintenance_digest(b"ignored-domain", "abcd").is_err());
}
