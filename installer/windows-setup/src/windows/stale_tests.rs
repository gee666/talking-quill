use super::tests::CHANNEL_TEST_LOCK;
use super::*;

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn direct_stale_cleanup_rejects_wrong_arguments_and_token_modes() {
    let command = OsString::from("/TQ-CLEAN-STALE-SCHEMA2");
    assert!(direct_cleanup_arguments(
        std::slice::from_ref(&command),
        true
    ));
    assert!(!direct_cleanup_arguments(
        std::slice::from_ref(&command),
        false
    ));
    assert!(!direct_cleanup_arguments(&[], true));
    assert!(!direct_cleanup_arguments(
        &[command.clone(), OsString::from("/S")],
        true,
    ));
    assert!(!direct_cleanup_arguments(&[OsString::from("/S")], true));
    assert!(direct_cleanup_token_is_authorized(true, 0x3000));
    assert!(direct_cleanup_token_is_authorized(true, 0x4000));
    assert!(!direct_cleanup_token_is_authorized(false, 0x3000));
    assert!(!direct_cleanup_token_is_authorized(true, 0x2fff));
    let diagnostic = OsString::from("/TQ-DIAGNOSE-STALE-SCHEMA2");
    assert!(direct_diagnostic_arguments(std::slice::from_ref(
        &diagnostic
    )));
    assert!(!direct_diagnostic_arguments(&[]));
    assert!(!direct_diagnostic_arguments(&[
        diagnostic,
        OsString::from("/S")
    ]));
    assert_eq!(STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.len(), 26);
    let unique = STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.len());
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn exact_current_inherited_registry_descriptor_requires_its_parent() {
    let legacy = STALE_REGISTRY_LEGACY_SDDL.to_ascii_uppercase();
    let descriptor = descriptor_from_sddl(STALE_REGISTRY_LEGACY_SDDL, EXIT_REJECTED).unwrap();
    assert_eq!(
        stale_registry_acl_admission(&legacy, &legacy, &descriptor, &descriptor).unwrap(),
        Some(StaleRegistryAclAdmission::LegacyExactParent)
    );

    let parent_mismatch =
        descriptor_from_sddl(STALE_REGISTRY_HARDENED_SDDL, EXIT_REJECTED).unwrap();
    assert_eq!(
        stale_registry_acl_admission(
            &STALE_REGISTRY_HARDENED_SDDL.to_ascii_uppercase(),
            &legacy,
            &parent_mismatch,
            &descriptor,
        )
        .unwrap(),
        None
    );
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_registry_acl_reorder_and_extra_ace_are_rejected() {
    let legacy = STALE_REGISTRY_LEGACY_SDDL.to_ascii_uppercase();
    let reordered = STALE_REGISTRY_LEGACY_SDDL.replace(
        "(A;CIID;KR;;;BU)(A;CIID;KA;;;BA)",
        "(A;CIID;KA;;;BA)(A;CIID;KR;;;BU)",
    );
    let reordered_descriptor = descriptor_from_sddl(&reordered, EXIT_REJECTED).unwrap();
    assert_eq!(
        stale_registry_acl_admission(
            &legacy,
            &reordered.to_ascii_uppercase(),
            &reordered_descriptor,
            &reordered_descriptor,
        )
        .unwrap(),
        None
    );

    let extra = format!("{STALE_REGISTRY_HARDENED_SDDL}(A;CI;KR;;;BU)");
    let extra_descriptor = descriptor_from_sddl(&extra, EXIT_REJECTED).unwrap();
    assert_eq!(
        stale_registry_acl_admission(
            &extra.to_ascii_uppercase(),
            &extra.to_ascii_uppercase(),
            &extra_descriptor,
            &extra_descriptor,
        )
        .unwrap(),
        None
    );
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_registry_value_and_suffix_inventory_is_exact() {
    assert!(validate_machine_lock_suffix("78bd88811b14faf1e11ba59620088aa0").is_ok());
    assert!(validate_machine_lock_suffix("78BD88811b14faf1e11ba59620088aa0").is_err());
    assert!(validate_machine_lock_suffix("78bd88811b14faf1e11ba59620088aa").is_err());
    assert_ne!(
        vec![MACHINE_LOCK_REGISTRY_VALUE, "Unexpected"],
        vec![MACHINE_LOCK_REGISTRY_VALUE]
    );
    assert_ne!(REG_EXPAND_SZ, REG_SZ);
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn hardened_registry_descriptor_is_structurally_exact() {
    let exact = descriptor_from_sddl(STALE_REGISTRY_HARDENED_SDDL, EXIT_REJECTED).unwrap();
    assert!(hardened_registry_descriptor_is_exact(&exact).unwrap());
    for invalid in [
        "O:BAG:BAD:P(A;CI;KA;;;BA)(A;CI;KA;;;SY)",
        "O:SYG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)",
        "O:BAG:SYD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)",
        "O:BAG:BAD:(A;CI;KA;;;SY)(A;CI;KA;;;BA)",
        "O:BAG:BAD:P(A;CI;KR;;;SY)(A;CI;KA;;;BA)",
        "O:BAG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KR;;;BU)",
    ] {
        let descriptor = descriptor_from_sddl(invalid, EXIT_REJECTED).unwrap();
        assert!(!hardened_registry_descriptor_is_exact(&descriptor).unwrap());
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn direct_stale_cleanup_rejects_missing_relative_and_unprotected_audits() {
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
    assert!(StaleCleanupAudit::open().is_err());
    unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", "audit.jsonl") };
    assert!(StaleCleanupAudit::open().is_err());

    let root =
        std::env::temp_dir().join(format!("tq-stale-unprotected-audit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let path = root.join("audit.jsonl");
    fs::write(&path, b"").unwrap();
    unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path) };
    assert!(StaleCleanupAudit::open().is_err());
    unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_retained_handles_block_mutation_and_prove_zero() {
    let root = std::env::temp_dir().join(format!("tq-stale-retained-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let child = root.join("fixture");
    fs::write(&child, b"exact").unwrap();
    let child_guard = RetainedStaleObject::open(&child, false).unwrap();
    apply_lock_dacl(&root, MACHINE_LOCK_DIRECTORY_SDDL).unwrap();
    let root_guard = RetainedStaleObject::open(&root, true).unwrap();
    assert!(OpenOptions::new().write(true).open(&child).is_err());
    let mutation = root.join("mutation");
    let mutation_attempt = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&mutation);
    child_guard.verify().unwrap();
    root_guard.verify().unwrap();
    child_guard.delete().unwrap();
    if mutation_attempt.is_ok() {
        drop(mutation_attempt);
        assert!(root_guard.delete().is_err());
        assert!(
            mutation.exists(),
            "an unretained mutation must never be deleted"
        );
        fs::remove_file(mutation).unwrap();
        fs::remove_dir(root).unwrap();
    } else {
        root_guard.delete().unwrap();
        assert!(!root.exists() && !child.exists());
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_concurrent_installer_lifecycle_lock_is_rejected() {
    if let (Some(lock), Some(ready)) = (
        std::env::var_os("TQ_STALE_LOCK_CHILD"),
        std::env::var_os("TQ_STALE_LOCK_READY"),
    ) {
        let _guard = RetainedStaleObject::open(Path::new(&lock), false).unwrap();
        fs::write(ready, b"ready").unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let root = std::env::temp_dir().join(format!("tq-stale-lock-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let lock = root.join("recovery-state-v1.lock");
    let ready = root.join("ready");
    fs::write(&lock, b"").unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "windows::stale_tests::stale_concurrent_installer_lifecycle_lock_is_rejected",
            "--nocapture",
        ])
        .env("TQ_STALE_LOCK_CHILD", &lock)
        .env("TQ_STALE_LOCK_READY", &ready)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists());
    assert!(RetainedStaleObject::open(&lock, false).is_err());
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn namespace_supervisor_claim_rejects_pid_and_identity_spoofing() {
    let original = std::env::var("TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM").unwrap();
    let mut claim: serde_json::Value = serde_json::from_str(&original).unwrap();
    claim["pid"] = serde_json::Value::from(std::process::id());
    assert!(authenticate_namespace_supervisor_claim(&claim.to_string()).is_err());
    let mut claim: serde_json::Value = serde_json::from_str(&original).unwrap();
    claim["creationTime"] = serde_json::Value::from(1_u64);
    assert!(authenticate_namespace_supervisor_claim(&claim.to_string()).is_err());
    let mut claim: serde_json::Value = serde_json::from_str(&original).unwrap();
    claim["imageSha256"] = serde_json::Value::String("00".repeat(32));
    assert!(authenticate_namespace_supervisor_claim(&claim.to_string()).is_err());
    assert!(authenticate_namespace_supervisor_claim(&original).is_ok());
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_active_process_state_is_rejected() {
    if std::env::var_os("TQ_STALE_ACTIVE_CHILD").is_some() {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let root = std::env::temp_dir().join(format!("tq-stale-active-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let image = root.join("Talking Quill Active Test.exe");
    fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
    let mut child = Command::new(&image)
        .args([
            "--exact",
            "windows::stale_tests::stale_active_process_state_is_rejected",
            "--nocapture",
        ])
        .env("TQ_STALE_ACTIVE_CHILD", "1")
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));
    assert!(!no_talking_quill_process_except_authenticated_pair(true).unwrap());
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_cleanup_authorization_is_authenticated_cross_process() {
    if std::env::var_os("TQ_STALE_AUTH_CHILD").is_some() {
        let image = std::env::current_exe().unwrap();
        let (action, _, silent, parent) =
            WorkerChannel::connect_and_authenticate(&image, None).unwrap();
        assert!(action == Action::CleanStaleSchema2 && silent && parent != 0);
        return;
    }
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let channel =
        ControllerChannel::create(Action::CleanStaleSchema2, true, std::process::id()).unwrap();
    let image = std::env::current_exe().unwrap();
    let mut child = Command::new(&image)
        .args([
            "--exact",
            "windows::stale_tests::stale_cleanup_authorization_is_authenticated_cross_process",
            "--nocapture",
        ])
        .env("TQ_STALE_AUTH_CHILD", "1")
        .spawn()
        .unwrap();
    let mut duplicate = ptr::null_mut();
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                child.as_raw_handle(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        },
        0
    );
    let shell = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let worker = channel.authenticate(&shell, &image, None).unwrap();
    assert_eq!(unsafe { GetProcessId(worker.as_raw_handle()) }, child.id());
    assert!(child.wait().unwrap().success());
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn forced_post_inspected_and_post_commit_rejections_keep_one_audit_chain() {
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    for forced_stage in ["post-inspected", "post-commit-intent"] {
        let root = std::env::temp_dir().join(format!(
            "tq-stale-forced-rejection-{}-{forced_stage}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("audit.jsonl");
        let diagnostic_path = root.join("diagnostic.jsonl");
        fs::write(&path, b"").unwrap();
        fs::write(&diagnostic_path, b"").unwrap();
        apply_lock_dacl(&path, MACHINE_LOCK_FILE_SDDL).unwrap();
        apply_lock_dacl(&diagnostic_path, MACHINE_LOCK_FILE_SDDL).unwrap();
        unsafe {
            std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path);
            std::env::set_var("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH", &diagnostic_path);
            std::env::set_var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE", forced_stage);
        }
        let mut audit = StaleCleanupAudit::open().unwrap();
        let mut diagnostic = StaleSchema2Diagnostic::open().unwrap();
        let binding = "ab".repeat(32);
        let proof = "cd".repeat(32);
        audit.record("inspected", &binding, &proof).unwrap();
        if forced_stage == "post-commit-intent" {
            audit.record("commit-intent", &binding, &proof).unwrap();
        }
        let forced = force_stale_cleanup_rejection(forced_stage).unwrap_err();
        assert!(record_post_audit_cleanup_rejection(&mut diagnostic, &mut audit, forced,).is_err());
        unsafe {
            std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH");
            std::env::remove_var("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH");
            std::env::remove_var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE");
        }
        drop(diagnostic);
        drop(audit);
        let events = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.first().unwrap()["stage"], "inspected");
        assert_eq!(
            events.last().unwrap()["stage"],
            "cleanup.rejected.after-audit"
        );
        let operation = events.first().unwrap()["operationId"].as_str().unwrap();
        assert!(
            events
                .iter()
                .all(|event| event["operationId"].as_str() == Some(operation))
        );
        for pair in events.windows(2) {
            assert_eq!(pair[1]["previousSha256"], pair[0]["eventSha256"]);
        }
        let diagnostic_events = fs::read_to_string(&diagnostic_path).unwrap();
        assert!(diagnostic_events.contains("cleanup.rejected.after-audit"));
        assert!(diagnostic_events.contains("\"outcome\":\"rejected\""));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_audit_is_flushed_and_identity_bound() {
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let root = std::env::temp_dir().join(format!("tq-stale-audit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let path = root.join("audit.jsonl");
    fs::write(&path, b"").unwrap();
    apply_lock_dacl(&path, MACHINE_LOCK_FILE_SDDL).unwrap();
    unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &path) };
    let mut audit = StaleCleanupAudit::open().unwrap();
    unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
    audit
        .record("commit-intent", &"ab".repeat(32), &"cd".repeat(32))
        .unwrap();
    audit
        .record("completed", &"ab".repeat(32), &"ef".repeat(32))
        .unwrap();
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content.lines().count(), 2);
    assert!(content.contains("commit-intent") && content.contains("completed"));
    assert!(content.contains(&"ab".repeat(32)) && content.contains(&"ef".repeat(32)));
    drop(audit);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_no_publication_active_residue_cannot_complete_audit() {
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let root = std::env::temp_dir().join(format!("tq-stale-no-publication-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let program_files = root.join("Program Files");
    let program_data = root.join("ProgramData");
    let system = root.join("System32");
    fs::create_dir_all(program_files.join("Talking Quill")).unwrap();
    fs::create_dir(&program_data).unwrap();
    fs::create_dir_all(system.join("Tasks")).unwrap();
    let audit_path = root.join("audit.jsonl");
    fs::write(&audit_path, b"").unwrap();
    apply_lock_dacl(&audit_path, MACHINE_LOCK_FILE_SDDL).unwrap();
    unsafe { std::env::set_var("TQ_STALE_SCHEMA2_AUDIT_PATH", &audit_path) };
    let mut audit = StaleCleanupAudit::open().unwrap();
    unsafe { std::env::remove_var("TQ_STALE_SCHEMA2_AUDIT_PATH") };
    let binding = retained_binding(&[], "no-machine-lock-publication");
    assert!(
        complete_stale_cleanup_zero_state(
            &mut audit,
            &binding,
            &program_files,
            &program_data,
            &system,
            true,
        )
        .is_err()
    );
    drop(audit);
    assert!(fs::read(&audit_path).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn orphan_lock_only_coordination_inventory_rejects_every_mixed_topology() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp/machine-lock-tests/orphan-inventory")
        .join(machine_lock_test_id().unwrap())
        .join("w");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let suffix = "11".repeat(16);
    let lock = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    fs::create_dir(&lock).unwrap();
    exact_stale_coordination_inventory(&root, &suffix, false).unwrap();

    let recovery = root.join("Talking Quill Update Recovery");
    fs::create_dir(&recovery).unwrap();
    assert!(exact_stale_coordination_inventory(&root, &suffix, false).is_err());
    exact_stale_coordination_inventory(&root, &suffix, true).unwrap();
    fs::remove_dir(&recovery).unwrap();

    for extra in [
        format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{}", "22".repeat(16)),
        format!("{MACHINE_LOCK_PENDING_PREFIX}{}", "33".repeat(16)),
        format!(
            ".Talking Quill.machine-lifecycle-retained-{}",
            "44".repeat(16)
        ),
    ] {
        let path = root.join(extra);
        fs::create_dir(&path).unwrap();
        assert!(exact_stale_coordination_inventory(&root, &suffix, false).is_err());
        fs::remove_dir(path).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "stale-schema2-cleanup")]
#[test]
fn stale_interrupted_registry_last_never_claims_zero() {
    let root = std::env::temp_dir().join(format!("tq-stale-registry-last-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let registry_publication = root.join("RecoveryStateLockV1");
    fs::write(&registry_publication, b"suffix").unwrap();
    let fixture = root.join("fixture");
    fs::create_dir(&fixture).unwrap();
    let lifecycle_path = fixture.join("recovery-state-v1.lock");
    fs::write(&lifecycle_path, b"").unwrap();
    let lifecycle = RetainedStaleObject::open_lifecycle(&lifecycle_path).unwrap();
    let fixture_guard = RetainedStaleObject::open(&fixture, true).unwrap();
    let mut lifecycle = lifecycle;
    let lifecycle_identity = lifecycle.identity.clone();
    let retained = root.join("retained-lifecycle.lock");
    lifecycle.rename(&retained).unwrap();
    lifecycle.mark_posix_deleted().unwrap();
    fixture_guard.delete().unwrap();
    assert!(
        registry_publication.exists(),
        "registry-last interruption must remain observable while lifecycle authority remains"
    );
    fs::remove_file(&registry_publication).unwrap();
    assert_eq!(
        file_identity_text(&lifecycle.file).unwrap(),
        lifecycle_identity
    );
    lifecycle.finish_deleted().unwrap();
    assert!(!fixture.exists() && !registry_publication.exists() && !retained.exists());
    fs::remove_dir(root).unwrap();
}
