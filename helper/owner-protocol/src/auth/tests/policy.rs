use super::*;

#[test]
fn release_policy_pair_allows_only_exact_or_one_hop_maintenance() {
    use crate::release_policy::{OwnerMode, ReleasePolicy, ReleasePolicyPredecessor};
    use crate::scalar::FeatureBits;
    use crate::schema::{Architecture, Platform, ProtocolHeader, Purpose};

    let header = ProtocolHeader {
        major: 1,
        minor: 0,
        compatibility_epoch: 1,
        supported_feature_bits: FeatureBits::new(1),
        required_feature_bits: FeatureBits::new(1),
    };
    let policy = |byte: u8| ReleasePolicy {
        platform: Platform::Windows,
        architecture: Architecture::X64,
        owner_mode: OwnerMode::SafeDisabled,
        release_build_digest: Bytes32::new([byte; 32]),
        gateway_sha256: Bytes32::new([byte + 1; 32]),
        owner_sha256: Bytes32::new([byte + 2; 32]),
        gateway_signer_policy_digest: Bytes32::new([4; 32]),
        owner_signer_policy_digest: Bytes32::new([5; 32]),
        gateway_protocol: header,
        owner_protocol: header,
        predecessor: None,
    };
    let old = policy(10);
    let mut new = policy(20);
    assert!(super::validate_policy_pair(Purpose::Capture, &old, &old).is_ok());
    assert!(super::validate_policy_pair(Purpose::Capture, &old, &new).is_err());
    assert!(super::validate_policy_pair(Purpose::Maintenance, &old, &new).is_err());
    new.predecessor = Some(ReleasePolicyPredecessor {
        release_build_digest: old.release_build_digest,
        gateway_sha256: old.gateway_sha256,
        owner_sha256: old.owner_sha256,
        platform: old.platform,
        architecture: old.architecture,
    });
    assert!(super::validate_policy_pair(Purpose::Maintenance, &new, &old).is_ok());
    assert!(super::validate_policy_pair(Purpose::Maintenance, &old, &new).is_ok());
    assert!(super::validate_policy_pair(Purpose::Capture, &new, &old).is_err());
}
