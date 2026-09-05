use super::{
    MEDIUM_LAUNCHER_DIRECTORY_SDDL, PersistedRelaunchRecord, RECOVERY_LAUNCHER_PENDING_PREFIX,
    RUN_ONCE_VALUE_PREFIX, StagedDirectoryGuard, UpdateAuthorization, UpdateCandidate,
    UpdatePredecessor, UpdateRole, authorization_transcript, canonical_candidate_layout,
    create_directory_with_sddl, create_restricted_directory, decode_base64,
    decode_pending_delete_pairs, publish_relaunch_record, read_persisted_relaunch_record,
    reclaim_incomplete_launcher_directories, reclaim_incomplete_recovery_directories,
    recovery_value_name, remove_relaunch_record_directory, validate_generation,
    write_persisted_relaunch_record,
};

#[cfg(feature = "machine-lock-test-namespace")]
#[test]
fn production_machine_lock_constructor_admits_and_retires_exact_published_tree() {
    let root = super::machine_lock_program_data().unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    let seeded_lock = super::machine_lock_file(1).unwrap();
    let state = super::RecoveryStateLock::acquire_for_epoch(3).unwrap();
    assert_eq!(state.path, seeded_lock);
    let directory = state.path.parent().unwrap().to_path_buf();
    let suffix = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(super::MACHINE_LOCK_DIRECTORY_PREFIX))
        .unwrap();
    let directory_identity = super::owned_tree_identity(&directory).unwrap();
    let expected_publication = format!("{suffix}:{directory_identity}");
    assert_eq!(
        std::fs::read_to_string(directory.join("publication-pending-v1")).unwrap(),
        expected_publication
    );
    let mut names = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "lock-tree-identity-v1",
            "publication-pending-v1",
            "recovery-state-v1.identity-v1",
            "recovery-state-v1.lock",
        ]
    );
    assert!(super::has_exact_security(&directory, super::MACHINE_LOCK_DIRECTORY_SDDL).unwrap());
    for name in &names {
        assert!(
            super::has_exact_security(&directory.join(name), super::MACHINE_LOCK_FILE_SDDL)
                .unwrap()
        );
    }
    state.retire().unwrap();
    assert!(!directory.exists());
    let key_path = super::machine_lock_registry_key().unwrap();
    let mut key = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            super::RegOpenKeyExW(
                super::machine_lock_registry_hive(),
                super::wide_nul(std::path::Path::new(&key_path))
                    .unwrap()
                    .as_ptr(),
                0,
                super::KEY_READ,
                &mut key,
            )
        },
        2
    );
}

#[test]
fn pending_delete_pairs_preserve_final_empty_destinations_and_terminators() {
    let encode = |values: &[&str], final_terminator: bool| {
        let mut data = Vec::new();
        for value in values {
            data.extend(value.encode_utf16());
            data.push(0);
        }
        if final_terminator {
            data.push(0);
        }
        data
    };
    assert_eq!(
        decode_pending_delete_pairs(&encode(&[r"\??\C:\old.exe", ""], true)).unwrap(),
        vec![(r"\??\C:\old.exe".into(), String::new())]
    );
    assert_eq!(
        decode_pending_delete_pairs(&encode(
            &[r"\??\C:\a.exe", "", r"\??\C:\b.exe", r"\??\C:\c.exe"],
            true,
        ))
        .unwrap(),
        vec![
            (r"\??\C:\a.exe".into(), String::new()),
            (r"\??\C:\b.exe".into(), r"\??\C:\c.exe".into()),
        ]
    );
    assert!(decode_pending_delete_pairs(&encode(&[r"\??\C:\old.exe", ""], false)).is_err());
    assert!(decode_pending_delete_pairs(&[0]).is_err());
    assert_eq!(decode_pending_delete_pairs(&[0, 0]).unwrap(), Vec::new());
}

#[test]
fn relaunch_record_marker_and_each_phase_are_power_loss_safe() {
    let generation = super::new_recovery_generation().unwrap();
    let identity = super::current_relaunch_identity().unwrap();
    let root = std::env::temp_dir().join(format!("tq-relaunch-record-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    *super::TEST_RELAUNCH_ROOT.lock().unwrap() = Some(root.clone());
    let mut record = PersistedRelaunchRecord {
        schema_version: 3,
        generation: generation.clone(),
        user_sid: identity.user_sid,
        logon_sid: identity.logon_sid,
        request: "--windows-update-bootstrap-v2=dGVzdA==".into(),
        nonce: "11".repeat(16),
        source_version: "0.0.69".into(),
        target_version: "0.0.70".into(),
        phase: "armed".into(),
        completed_version: None,
        recovery_generation: generation.clone(),
        predecessor: super::InstalledManifest {
            version: "0.0.69".into(),
            platform: "win32".into(),
            architecture: "x64".into(),
            source_commit: "a".repeat(40),
            source_tree: "b".repeat(40),
            release_build_digest: "c".repeat(64),
            roles: Vec::new(),
        },
    };
    publish_relaunch_record(&record).unwrap();
    for phase in [
        "setup-started",
        "setup-complete",
        "launch-started",
        "app-ready",
    ] {
        record.phase = phase.into();
        if phase == "setup-complete" {
            record.completed_version = Some("0.0.69".into());
        }
        write_persisted_relaunch_record(
            &super::relaunch_generation_directory(&generation).unwrap(),
            &record,
        )
        .unwrap();
        assert_eq!(
            read_persisted_relaunch_record(&generation).unwrap().phase,
            phase
        );
    }
    record.schema_version = 2;
    write_persisted_relaunch_record(
        &super::relaunch_generation_directory(&generation).unwrap(),
        &record,
    )
    .unwrap();
    assert!(read_persisted_relaunch_record(&generation).is_err());
    record.schema_version = 3;
    write_persisted_relaunch_record(
        &super::relaunch_generation_directory(&generation).unwrap(),
        &record,
    )
    .unwrap();
    remove_relaunch_record_directory(&super::relaunch_generation_directory(&generation).unwrap())
        .unwrap();
    *super::TEST_RELAUNCH_ROOT.lock().unwrap() = None;
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn terminated_bootstrap_publish_seams_leave_only_reclaimable_owned_residue() {
    let root_variable = "TQ_TERMINATED_BOOTSTRAP_TEST_ROOT";
    let seam_variable = "TQ_TERMINATED_BOOTSTRAP_TEST_SEAM";
    if let (Some(root), Some(seam)) = (
        std::env::var_os(root_variable),
        std::env::var_os(seam_variable),
    ) {
        let root = std::path::PathBuf::from(root);
        let seam = seam.to_string_lossy();
        let pending = root.join(format!(
            ".Talking Quill.update-bootstrap-pending-{}",
            "11".repeat(16)
        ));
        create_restricted_directory(&pending).unwrap();
        let mut guard = StagedDirectoryGuard::new(pending.clone()).unwrap();
        if seam != "directory-created" {
            guard.persist_identity_marker().unwrap();
        }
        if matches!(seam.as_ref(), "payload-durable" | "directory-published") {
            let mut payload = std::fs::File::create(pending.join("payload")).unwrap();
            std::io::Write::write_all(&mut payload, b"durable payload").unwrap();
            payload.sync_all().unwrap();
        }
        if seam == "directory-published" {
            guard
                .publish(root.join(".Talking Quill.update-bootstrap-1122334455667788"))
                .unwrap();
        }
        let mut marker = std::fs::File::create(root.join(format!("{seam}.durable"))).unwrap();
        std::io::Write::write_all(&mut marker, seam.as_bytes()).unwrap();
        marker.sync_all().unwrap();
        std::mem::forget(guard);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    for seam in [
        "directory-created",
        "identity-durable",
        "payload-durable",
        "directory-published",
    ] {
        let root = std::env::temp_dir().join(format!(
            "tq-terminated-bootstrap-recovery-{}-{seam}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("terminated_bootstrap_publish_seams_leave_only_reclaimable_owned_residue")
            .arg("--nocapture")
            .env(root_variable, &root)
            .env(seam_variable, seam)
            .spawn()
            .unwrap();
        let durable = root.join(format!("{seam}.durable"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !durable.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(durable.exists(), "{seam}");
        child.kill().unwrap();
        child.wait().unwrap();
        reclaim_incomplete_recovery_directories(&root).unwrap();
        assert_eq!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".Talking Quill.update-bootstrap-"))
                .count(),
            0,
            "{seam}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[test]
fn launcher_pending_directories_are_reclaimed_before_the_identity_marker() {
    let root = std::env::temp_dir().join(format!(
        "tq-launcher-pending-recovery-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    for token in ["11".repeat(16), "22".repeat(16)] {
        let pending = root.join(format!("{RECOVERY_LAUNCHER_PENDING_PREFIX}{token}"));
        create_directory_with_sddl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap();
        super::apply_restricted_dacl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap();
        assert!(super::has_exact_security(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL).unwrap());
    }
    reclaim_incomplete_launcher_directories(&root).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminated_generation_commit_seams_never_publish_an_incomplete_run_record() {
    let root_variable = "TQ_GENERATION_COMMIT_TEST_ROOT";
    let seam_variable = "TQ_GENERATION_COMMIT_TEST_SEAM";
    let operations = ["retry", "binding", "active", "run"];
    if let (Some(root), Some(seam)) = (
        std::env::var_os(root_variable),
        std::env::var_os(seam_variable),
    ) {
        let root = std::path::PathBuf::from(root);
        let seam = seam.to_string_lossy();
        for operation in operations {
            let mut file = std::fs::File::create(root.join(operation)).unwrap();
            std::io::Write::write_all(&mut file, operation.as_bytes()).unwrap();
            file.sync_all().unwrap();
            if operation == seam {
                std::fs::File::create(root.join("durable"))
                    .unwrap()
                    .sync_all()
                    .unwrap();
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
        unreachable!();
    }
    for seam in operations {
        let root = std::env::temp_dir().join(format!(
            "tq-generation-commit-{}-{seam}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("terminated_generation_commit_seams_never_publish_an_incomplete_run_record")
            .arg("--nocapture")
            .env(root_variable, &root)
            .env(seam_variable, seam)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join("durable").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(root.join("durable").exists(), "{seam}");
        child.kill().unwrap();
        child.wait().unwrap();
        if root.join("run").exists() {
            for prerequisite in ["retry", "binding", "active"] {
                assert!(root.join(prerequisite).exists(), "{seam}: {prerequisite}");
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn every_generation_commit_interruption_preserves_publish_order() {
    let operations = ["retry", "binding", "active", "run"];
    for interrupted_after in 0..=operations.len() {
        let durable = &operations[..interrupted_after];
        if durable.contains(&"active") {
            assert!(durable.contains(&"retry"));
            assert!(durable.contains(&"binding"));
        }
        if durable.contains(&"run") {
            assert_eq!(durable, operations);
        }
    }
    let generation = "00112233445566778899aabbccddeeff";
    let cleanup = format!("--windows-update-cleanup-v1=1122334455667788:{generation}");
    let resume = format!("--windows-update-resume-v2={generation}");
    assert!(cleanup.ends_with(generation));
    assert!(resume.ends_with(generation));
}

#[test]
fn run_once_generations_are_unique_and_stale_deletion_cannot_name_a_successor() {
    let first = "00112233445566778899aabbccddeeff";
    let second = "ffeeddccbbaa99887766554433221100";
    let first_name = recovery_value_name(first).unwrap();
    let second_name = recovery_value_name(second).unwrap();
    assert_ne!(first_name, second_name);
    assert!(first_name.starts_with(RUN_ONCE_VALUE_PREFIX));
    assert!(second_name.starts_with(RUN_ONCE_VALUE_PREFIX));
    let mut simulated_registry = std::collections::BTreeMap::from([
        (first_name.clone(), "running"),
        (second_name.clone(), "successor"),
    ]);
    simulated_registry.remove(&first_name);
    assert_eq!(simulated_registry.get(&second_name), Some(&"successor"));
    assert!(validate_generation(first).is_ok());
    assert!(validate_generation("0011").is_err());
}

#[test]
fn prelaunch_guard_removes_the_exact_abandoned_tree() {
    let root =
        std::env::temp_dir().join(format!("tq-staged-bootstrap-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    {
        let _guard = StagedDirectoryGuard::new(root.clone()).unwrap();
        std::fs::write(root.join("partial"), b"partial").unwrap();
    }
    assert!(!root.exists());
}

fn candidate() -> UpdateCandidate {
    UpdateCandidate {
        version: "1.2.3".into(),
        platform: "win".into(),
        architecture: "x64".into(),
        owner_mode: "local-unsigned-enabled".into(),
        package_mode: "update".into(),
        source_commit: "66".repeat(20),
        source_tree: "77".repeat(20),
        release_build_digest: String::new(),
        package_layout_digest: String::new(),
        package_sha256: "aa".repeat(32),
        channel: "latest-x64".into(),
        transaction_binding: "source-target-package-sha256-v1".into(),
        roles: vec![
            UpdateRole {
                role: "gateway".into(),
                path: "resources/helper/talking-quill-helper.exe".into(),
                sha256: "11".repeat(32),
                suppression_capable: false,
            },
            UpdateRole {
                role: "owner".into(),
                path: "resources/helper/talking-quill-keyboard-owner.exe".into(),
                sha256: "22".repeat(32),
                suppression_capable: true,
            },
            UpdateRole {
                role: "recovery-launcher".into(),
                path: "resources/helper/talking-quill-update-recovery-launcher.exe".into(),
                sha256: "33".repeat(32),
                suppression_capable: false,
            },
        ],
        predecessor: UpdatePredecessor {
            platform: "win".into(),
            architecture: "x64".into(),
            version: "1.2.2".into(),
            release_build_digest: "33".repeat(32),
            gateway_sha256: "44".repeat(32),
            owner_sha256: "55".repeat(32),
        },
        authorization: UpdateAuthorization {
            scheme: "p256-sha256-v1".into(),
            signature: String::new(),
            verification_key_sha256: None,
        },
    }
}

#[test]
fn candidate_role_and_predecessor_layout_matches_the_javascript_contract() {
    assert_eq!(
        canonical_candidate_layout(&candidate()).unwrap(),
        "edd8c82885e70192cf07b0fbf006e74734ed8009df89a8fe0fe8529369156d30"
    );
}

#[test]
fn signed_candidate_authorization_rejects_mutated_outer_bytes() {
    use p256::ecdsa::{SigningKey, signature::Signer, signature::Verifier};

    let mut value = candidate();
    let signing_key = SigningKey::from_bytes((&[7_u8; 32]).into()).unwrap();
    let signature: p256::ecdsa::Signature =
        signing_key.sign(&authorization_transcript(&value).unwrap());
    signing_key
        .verifying_key()
        .verify(&authorization_transcript(&value).unwrap(), &signature)
        .unwrap();
    value.package_sha256 = "ab".repeat(32);
    assert!(
        signing_key
            .verifying_key()
            .verify(&authorization_transcript(&value).unwrap(), &signature)
            .is_err()
    );
}

#[test]
fn base64_request_decoder_is_strict() {
    assert_eq!(
        decode_base64("eyJ2ZXJzaW9uIjoxfQ==").unwrap(),
        br#"{"version":1}"#
    );
    assert!(decode_base64("abc").is_err());
    assert!(decode_base64("AA=A").is_err());
}
