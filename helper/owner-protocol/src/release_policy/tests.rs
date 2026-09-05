use super::{OwnerMode, ReleasePolicy, ReleasePolicyPredecessor};
use crate::scalar::Bytes32;
use crate::schema::{Architecture, Platform, ProtocolHeader};

#[test]
fn all_frozen_release_policy_vectors_conform() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/compatibility/keyboard-owner-v1/release-policy-vectors.json"
    ))
    .expect("valid fixture JSON");
    assert_eq!(fixture["fixtureVersion"], 1);
    for vector in fixture["vectors"].as_array().expect("vectors") {
        let fields = &vector["fields"];
        let policy_platform = platform(text(fields, "platform"));
        let policy_architecture = architecture(text(fields, "architecture"));
        let predecessor = fields["predecessor"].as_object().map(|_| {
            let value = &fields["predecessor"];
            ReleasePolicyPredecessor {
                release_build_digest: digest(text(value, "releaseBuildDigest")),
                gateway_sha256: digest(text(value, "gatewaySha256")),
                owner_sha256: digest(text(value, "ownerSha256")),
                platform: platform(text(value, "platform")),
                architecture: architecture(text(value, "architecture")),
            }
        });
        let policy = ReleasePolicy {
            platform: policy_platform,
            architecture: policy_architecture,
            owner_mode: match text(fields, "ownerMode") {
                "safe_disabled" => OwnerMode::SafeDisabled,
                "enabled_candidate" => OwnerMode::EnabledCandidate,
                _ => panic!("fixture owner mode"),
            },
            release_build_digest: digest(text(fields, "releaseBuildDigest")),
            gateway_sha256: digest(text(fields, "gatewaySha256")),
            owner_sha256: digest(text(fields, "ownerSha256")),
            gateway_signer_policy_digest: digest(text(fields, "gatewaySignerPolicyDigest")),
            owner_signer_policy_digest: digest(text(fields, "ownerSignerPolicyDigest")),
            gateway_protocol: protocol(&fields["gatewayProtocol"]),
            owner_protocol: protocol(&fields["ownerProtocol"]),
            predecessor,
        };
        let encoded = policy.encode().expect("policy encodes");
        assert_eq!(
            encoded.as_bytes().as_slice(),
            hex(text(vector, "expectedHex"))
        );
        assert_eq!(encoded.digest(), digest(text(vector, "expectedSha256")));
        assert_eq!(encoded.decode().expect("policy decodes"), policy);
    }
}

fn protocol(value: &serde_json::Value) -> ProtocolHeader {
    serde_json::from_value(value.clone()).expect("protocol")
}

fn platform(value: &str) -> Platform {
    match value {
        "windows" => Platform::Windows,
        "macos" => Platform::Macos,
        _ => panic!("fixture platform"),
    }
}

fn architecture(value: &str) -> Architecture {
    match value {
        "x64" => Architecture::X64,
        "arm64" => Architecture::Arm64,
        _ => panic!("fixture architecture"),
    }
}

fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field].as_str().expect("fixture string")
}

fn digest(value: &str) -> Bytes32 {
    Bytes32::new(hex(value).try_into().expect("32-byte digest"))
}

fn hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex"))
        .collect()
}
