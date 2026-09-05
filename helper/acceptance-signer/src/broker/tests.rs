use super::*;
use talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture;
use talking_quill_windows_owner_ipc::peer::{FileIdentity, PeerFacts, SourceIdentity};

fn facts() -> PeerFacts {
    PeerFacts {
        process_id: 40,
        creation_marker: 400,
        wts_session_id: 3,
        user_sid: vec![1, 2],
        logon_sid: vec![3, 4],
        integrity_rid: 0x2000,
        architecture: WindowsArchitecture::X64,
        canonical_image: PathBuf::from(r"C:\Program Files\Talking Quill\Talking Quill.exe"),
        file_identity: FileIdentity {
            volume_serial: 9,
            file_index: 12,
        },
        image_sha256: [7; 32],
        source_identity: Some(SourceIdentity {
            commit: "1".repeat(40),
            tree: "2".repeat(40),
        }),
    }
}

#[test]
fn forged_first_client_does_not_consume_the_later_authorized_identity() {
    let expected = facts();
    let mut forged = expected.clone();
    forged.process_id = 41;
    forged.user_sid = vec![9];
    let admitted = [forged, expected.clone()].into_iter().find(|candidate| {
        peer_identity_matches(
            candidate,
            &expected,
            [7; 32],
            &"1".repeat(40),
            &"2".repeat(40),
        )
    });
    assert_eq!(admitted.map(|facts| facts.process_id), Some(40));
}

#[test]
fn terminate_action_remains_visible_after_continue() {
    let (sender, receiver) = std::sync::mpsc::channel();
    sender.send(Ok(ControlAction::Continue)).unwrap();
    sender.send(Ok(ControlAction::Terminate)).unwrap();
    drop(sender);
    let control = Control { receiver };
    assert!(control.wait(unix_ms().unwrap() + 1_000).is_ok());
    assert!(matches!(
        control.poll().unwrap(),
        Some(ControlAction::Terminate)
    ));
}

#[test]
fn pid_reuse_and_time_reversing_ancestry_are_rejected() {
    assert!(creation_chain_reaches_root(
        &[(90, 500), (40, 400)],
        40,
        400
    ));
    assert!(!creation_chain_reaches_root(
        &[(90, 500), (40, 401)],
        40,
        400
    ));
    assert!(!creation_chain_reaches_root(
        &[(90, 300), (40, 400)],
        40,
        400
    ));
}

#[test]
fn sid_logon_session_integrity_and_image_mismatches_are_rejected() {
    let expected = facts();
    for mutate in [
        |facts: &mut PeerFacts| facts.user_sid.push(9),
        |facts: &mut PeerFacts| facts.logon_sid.push(9),
        |facts: &mut PeerFacts| facts.wts_session_id += 1,
        |facts: &mut PeerFacts| facts.integrity_rid += 1,
        |facts: &mut PeerFacts| facts.canonical_image.push("forged.exe"),
        |facts: &mut PeerFacts| facts.file_identity.file_index += 1,
        |facts: &mut PeerFacts| facts.image_sha256[0] ^= 1,
        |facts: &mut PeerFacts| facts.source_identity.as_mut().unwrap().commit = "9".repeat(40),
    ] {
        let mut candidate = expected.clone();
        mutate(&mut candidate);
        assert!(!peer_identity_matches(
            &candidate,
            &expected,
            [7; 32],
            &"1".repeat(40),
            &"2".repeat(40)
        ));
    }
}

#[test]
fn signer_rename_hardlink_reparse_and_replacement_are_blocked() {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).unwrap();
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tmp")
        .join(format!("acceptance-broker-race-{suffix}"));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("signer.exe");
    let replacement = root.join("replacement.exe");
    let linked = root.join("linked.exe");
    std::fs::write(&source, b"trusted").unwrap();
    std::fs::write(&replacement, b"forged").unwrap();
    std::fs::hard_link(&source, &linked).unwrap();
    let mut linked_file = open_locked(&source).unwrap();
    assert!(snapshot(&mut linked_file).is_err());
    drop(linked_file);
    std::fs::remove_file(&linked).unwrap();
    let retained = open_locked(&source).unwrap();
    assert!(std::fs::rename(&replacement, &source).is_err());
    assert!(std::fs::rename(&source, root.join("renamed.exe")).is_err());
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        let reparse = root.join("reparse.exe");
        if symlink_file(&source, &reparse).is_ok() {
            assert!(require_absolute_no_reparse(&reparse).is_err());
        }
    }
    drop(retained);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn request_wire_names_and_unknown_field_rejection_are_preserved() {
    let mut request = serde_json::json!({
        "operation": "sign", "version": 1, "correlation": "a".repeat(32),
        "brokerSha256": "b".repeat(64), "brokerBytes": 1,
        "signerPath": "signer.exe", "signerSha256": "c".repeat(64), "signerBytes": 2,
        "privateKeyPath": "key.der", "payloadHex": "00"
    });
    assert!(matches!(
        serde_json::from_value::<Request>(request.clone()),
        Ok(Request::Sign { .. })
    ));
    request["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<Request>(request).is_err());
}

#[test]
fn hex_bounds_and_case_are_preserved() {
    assert_eq!(decode_hex("00ff", 2), Ok(vec![0, 255]));
    for value in ["", "0", "FF", "000000"] {
        assert!(decode_hex(value, 2).is_err());
    }
    assert!(validate_header(1, &"a".repeat(32)).is_ok());
    assert!(validate_header(2, &"a".repeat(32)).is_err());
}
