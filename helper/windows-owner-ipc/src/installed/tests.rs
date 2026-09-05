use super::files::source_marker_prefix;
use super::*;
use crate::peer::{FileIdentity, SourceIdentity};
use std::path::PathBuf;

fn facts(role: &str, pid: u32, image: u8) -> PeerFacts {
    PeerFacts {
        process_id: pid,
        creation_marker: u64::from(pid) * 10,
        wts_session_id: 4,
        user_sid: vec![1, 2, 3],
        logon_sid: vec![4, 5, 6],
        integrity_rid: 0x2000,
        architecture: WindowsArchitecture::X64,
        canonical_image: PathBuf::from(format!(
            r"C:\Program Files\Talking Quill\resources\helper\{role}"
        )),
        file_identity: FileIdentity {
            volume_serial: 7,
            file_index: u64::from(pid),
        },
        image_sha256: [image; 32],
        source_identity: Some(SourceIdentity {
            commit: "11".repeat(20),
            tree: "22".repeat(20),
        }),
    }
}

fn recovery_launcher(architecture: &str) -> Vec<u8> {
    let mut bytes = vec![0_u8; 256];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&128_u32.to_le_bytes());
    bytes[128..132].copy_from_slice(b"PE\0\0");
    let machine = if architecture == "arm64" {
        0xaa64_u16
    } else {
        0x8664_u16
    };
    bytes[132..134].copy_from_slice(&machine.to_le_bytes());
    bytes.extend_from_slice(&source_marker_prefix(b"COMMIT"));
    bytes.extend_from_slice("11".repeat(20).as_bytes());
    bytes.extend_from_slice(&source_marker_prefix(b"TREE"));
    bytes.extend_from_slice("22".repeat(20).as_bytes());
    bytes
}

fn manifest_with(
    gateway: u8,
    owner: u8,
    architecture: &str,
    predecessor: serde_json::Value,
) -> Vec<u8> {
    let launcher_sha256: [u8; 32] = Sha256::digest(recovery_launcher(architecture)).into();
    let mut value = serde_json::json!({
        "schemaVersion": 1,
        "kind": "talking-quill-local-owner-release",
        "version": "0.0.69",
        "platform": "win",
        "architecture": architecture,
        "ownerMode": "local-unsigned-enabled",
        "packageMode": if predecessor.is_null() { "fresh" } else { "update" },
        "sourceCommit": "11".repeat(20),
        "sourceTree": "22".repeat(20),
        "roles": [
            {"role":"gateway","path":"resources/helper/talking-quill-helper.exe","sha256":hex(&[gateway;32]),"suppressionCapable":false},
            {"role":"owner","path":"resources/helper/talking-quill-keyboard-owner.exe","sha256":hex(&[owner;32]),"suppressionCapable":true},
            {"role":"recovery-launcher","path":"resources/helper/talking-quill-update-recovery-launcher.exe","sha256":hex(&launcher_sha256),"suppressionCapable":false}
        ],
        "predecessor": predecessor,
        "releaseBuildDigest": hex(&[0;32]),
        "packageLayoutDigest": hex(&[0;32]),
        "update": {"channel":format!("latest-{architecture}"),"payload":"tqpkg2","companion":null,"transactionBinding":"source-target-package-sha256-v1","maintenanceInstaller":"native-setup"}
    });
    if predecessor.is_null() {
        value["freshInstall"] = true.into();
    }
    let parsed: ReleaseManifest = serde_json::from_value(value.clone()).unwrap();
    let digest = hex(&canonical_package_layout(&parsed).unwrap());
    value["releaseBuildDigest"] = digest.clone().into();
    value["packageLayoutDigest"] = digest.into();
    serde_json::to_vec(&value).unwrap()
}

fn manifest(gateway: u8, owner: u8) -> Vec<u8> {
    manifest_with(gateway, owner, "x64", serde_json::Value::Null)
}

fn recovery_launcher_for(gateway: &PeerFacts) -> Vec<u8> {
    recovery_launcher(match gateway.architecture {
        WindowsArchitecture::X64 => "x64",
        WindowsArchitecture::Arm64 => "arm64",
    })
}

fn hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn installer_manifest_binds_roles_release_and_layout() {
    let gateway = facts("talking-quill-helper.exe", 10, 1);
    let owner = facts("talking-quill-keyboard-owner.exe", 11, 2);
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    let release = InstalledRelease::from_manifest_bytes(
        &gateway,
        &owner,
        ChannelPurpose::Capture,
        resources,
        &manifest(1, 2),
        &recovery_launcher_for(&gateway),
    )
    .unwrap();
    let parsed: ReleaseManifest = serde_json::from_slice(&manifest(1, 2)).unwrap();
    let canonical = canonical_package_layout(&parsed).unwrap();
    assert_eq!(release.binding.release_build_digest.as_bytes(), &canonical);
    assert_eq!(&release.policy[16..48], &canonical);
    assert_eq!(release.binding.installation_id, hex(&canonical));
}

#[test]
fn windows_policy_binds_the_allowed_predecessor() {
    let gateway = facts("talking-quill-helper.exe", 10, 1);
    let owner = facts("talking-quill-keyboard-owner.exe", 11, 2);
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    let bytes = manifest_with(
        1,
        2,
        "x64",
        serde_json::json!({
            "platform":"win",
            "architecture":"x64",
            "version":"0.0.66",
            "releaseBuildDigest":hex(&[5; 32]),
            "gatewaySha256":hex(&[6; 32]),
            "ownerSha256":hex(&[7; 32])
        }),
    );
    let release = InstalledRelease::from_manifest_bytes(
        &gateway,
        &owner,
        ChannelPurpose::Capture,
        resources,
        &bytes,
        &recovery_launcher_for(&gateway),
    )
    .unwrap();
    assert_eq!(release.policy[13], 1);
    assert_eq!(&release.policy[224..256], &[5; 32]);
    assert_eq!(&release.policy[256..288], &[6; 32]);
    assert_eq!(&release.policy[288..320], &[7; 32]);
}

#[test]
fn arm64_manifest_and_policy_are_native_and_proof_is_not_fake_der() {
    let mut gateway = facts("talking-quill-helper.exe", 12, 1);
    let mut owner = facts("talking-quill-keyboard-owner.exe", 13, 2);
    gateway.architecture = WindowsArchitecture::Arm64;
    owner.architecture = WindowsArchitecture::Arm64;
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    let bytes = manifest_with(1, 2, "arm64", serde_json::Value::Null);
    let release = InstalledRelease::from_manifest_bytes(
        &gateway,
        &owner,
        ChannelPurpose::Capture,
        resources,
        &bytes,
        &recovery_launcher_for(&gateway),
    )
    .unwrap();
    assert_eq!(release.policy[11], 2);
    let proof = protected_policy_proof(&release.binding);
    assert_eq!(&proof[..8], b"TQKOWPR1");
    assert_ne!(proof[0], 0x30);
}

#[test]
fn installed_manifest_binds_source_provenance() {
    let gateway = facts("talking-quill-helper.exe", 16, 1);
    let owner = facts("talking-quill-keyboard-owner.exe", 17, 2);
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    let mut value: serde_json::Value = serde_json::from_slice(&manifest(1, 2)).unwrap();
    value["sourceCommit"] = "33".repeat(20).into();
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(
        InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &bytes,
            &recovery_launcher_for(&gateway),
        )
        .is_err()
    );
}

#[test]
fn installed_layout_digest_is_recomputed_instead_of_trusted() {
    let gateway = facts("talking-quill-helper.exe", 18, 1);
    let owner = facts("talking-quill-keyboard-owner.exe", 19, 2);
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    let bytes = String::from_utf8(manifest(1, 2))
        .unwrap()
        .replace(
            &hex(&canonical_package_layout(
                &serde_json::from_slice::<ReleaseManifest>(&manifest(1, 2)).unwrap(),
            )
            .unwrap()),
            &hex(&[9; 32]),
        )
        .into_bytes();
    assert!(
        InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &bytes,
            &recovery_launcher_for(&gateway),
        )
        .is_err()
    );
}

#[test]
fn wrong_hash_role_or_architecture_fails_closed() {
    let gateway = facts("talking-quill-helper.exe", 20, 3);
    let mut owner = facts("talking-quill-keyboard-owner.exe", 21, 4);
    let resources = Path::new(r"C:\Program Files\Talking Quill\resources");
    assert!(
        InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &manifest(3, 5),
            &recovery_launcher_for(&gateway),
        )
        .is_err()
    );
    owner.architecture = WindowsArchitecture::Arm64;
    assert!(
        InstalledRelease::from_manifest_bytes(
            &gateway,
            &owner,
            ChannelPurpose::Capture,
            resources,
            &manifest(3, 4),
            &recovery_launcher_for(&gateway),
        )
        .is_err()
    );
}
