use super::*;
pub(super) static CHANNEL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn atomic_marker_rename_reopens_the_same_identity_on_windows() {
    let root = std::env::temp_dir().join(format!("tq-marker-publication-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let marker = root.join("identity-v1");
    create_atomic_marker(&marker, "expected", MACHINE_LOCK_FILE_SDDL).unwrap();
    verify_atomic_marker(&marker, "expected", MACHINE_LOCK_FILE_SDDL, None).unwrap();
    assert_eq!(fs::read_to_string(&marker).unwrap(), "expected");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

fn transaction(phase: &str, action: &str, had_predecessor: bool) -> Transaction {
    Transaction {
        schema_version: TRANSACTION_SCHEMA,
        phase: phase.into(),
        action: action.into(),
        had_predecessor,
    }
}

#[test]
fn every_durable_phase_has_a_bounded_recovery_direction() {
    for phase in ["staging", "staged", "prepared"] {
        assert_eq!(
            recovery_plan(&transaction(phase, "install", false), false, false).unwrap(),
            RecoveryPlan::DiscardStaging,
            "{phase}"
        );
        assert_eq!(
            recovery_plan(&transaction(phase, "repair", true), false, true).unwrap(),
            RecoveryPlan::DiscardStaging,
            "{phase}"
        );
    }
    for phase in [
        "predecessor-moved",
        "publishing",
        "published-before-persist",
        "published",
        "registered",
    ] {
        for action in ["install", "update", "repair"] {
            assert_eq!(
                recovery_plan(
                    &transaction(phase, action, true),
                    true,
                    phase != "predecessor-moved"
                )
                .unwrap(),
                RecoveryPlan::RestorePredecessor,
                "{action}:{phase}"
            );
        }
    }
    for phase in [
        "prepared",
        "publishing",
        "published-before-persist",
        "published",
        "registered",
    ] {
        assert_eq!(
            recovery_plan(&transaction(phase, "install", false), false, true).unwrap(),
            RecoveryPlan::RemoveFreshCandidate,
            "{phase}"
        );
    }
    for phase in ["committed", "legacy-retiring", "legacy-retired"] {
        assert_eq!(
            recovery_plan(&transaction(phase, "repair", true), true, true).unwrap(),
            RecoveryPlan::FinishCommit,
            "{phase}"
        );
    }
    for phase in ["uninstall-armed", "uninstalling", "uninstall-quarantined"] {
        assert_eq!(
            recovery_plan(&transaction(phase, "uninstall", true), true, true).unwrap(),
            RecoveryPlan::FinishUninstall,
            "{phase}"
        );
    }
    assert!(recovery_plan(&transaction("prepared", "repair", true), false, false).is_err());
    assert!(recovery_plan(&transaction("unknown", "repair", true), true, true).is_err());
}

#[test]
fn delayed_deletion_pairs_preserve_empty_destinations_and_exact_order() {
    let mut data = Vec::new();
    for value in [r"\??\C:\recovery\child.exe", "", r"\??\C:\recovery", ""] {
        data.extend(value.encode_utf16());
        data.push(0);
    }
    data.push(0);
    assert_eq!(
        decode_pending_rename_pairs(&data).unwrap(),
        vec![
            (r"\??\C:\recovery\child.exe".into(), String::new()),
            (r"\??\C:\recovery".into(), String::new()),
        ]
    );
    let malformed: Vec<u16> = "source-without-terminator".encode_utf16().collect();
    assert!(decode_pending_rename_pairs(&malformed).is_err());
    let mut missing_final = Vec::new();
    for value in [r"\??\C:\final.exe", ""] {
        missing_final.extend(value.encode_utf16());
        missing_final.push(0);
    }
    assert!(decode_pending_rename_pairs(&missing_final).is_err());
    assert!(decode_pending_rename_pairs(&[0]).is_err());
    assert_eq!(decode_pending_rename_pairs(&[0, 0]).unwrap(), Vec::new());
    let mut rename = Vec::new();
    for value in [r"\??\C:\source.exe", r"\??\C:\destination.exe"] {
        rename.extend(value.encode_utf16());
        rename.push(0);
    }
    rename.push(0);
    assert_eq!(
        decode_pending_rename_pairs(&rename).unwrap(),
        vec![(
            r"\??\C:\source.exe".into(),
            r"\??\C:\destination.exe".into()
        )]
    );
}

#[test]
fn stale_legacy_registration_accepts_missing_files_only_in_its_own_directory() {
    let paths = test_paths(&std::env::temp_dir().join("tq-legacy-registration-test"));
    assert!(owned_legacy_executable(&paths.legacy_authority.join("missing.exe"), &paths).unwrap());
    assert!(
        !owned_legacy_executable(
            &paths
                .legacy_authority
                .with_extension("other")
                .join("missing.exe"),
            &paths
        )
        .unwrap()
    );
    assert!(
        !owned_legacy_executable(&paths.legacy_authority.join("..").join("other.exe"), &paths)
            .unwrap()
    );
    assert_eq!(
        service_executable("\"C:\\Program Files\\Talking Quill\\service.exe\" --run"),
        Some(PathBuf::from(r"C:\Program Files\Talking Quill\service.exe"))
    );
}

#[test]
fn finalizer_cleanup_is_child_before_parent() {
    let root = std::env::temp_dir().join(format!(
        "tq-terminal-delete-plan-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let child = nested.join("child");
    fs::write(&child, b"child").unwrap();
    let launcher = root.join("launcher.exe");
    fs::write(&launcher, b"launcher").unwrap();
    let mut plan = Vec::new();
    collect_finalizer_deletion_paths(&root, &mut plan).unwrap();
    let child_index = plan.iter().position(|path| path == &child).unwrap();
    let nested_index = plan.iter().position(|path| path == &nested).unwrap();
    let root_index = plan.iter().position(|path| path == &root).unwrap();
    assert!(child_index < nested_index && nested_index < root_index);
    assert_eq!(plan.last(), Some(&root));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn orphaned_machine_lock_directory_is_reclaimable_after_publication_loss() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp/machine-lock-tests/windows-setup-unit")
        .join(machine_lock_test_id().unwrap())
        .join("w");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let suffix = "44".repeat(16);
    let directory = root.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    fs::create_dir(&directory).unwrap();
    let identity = owned_tree_identity(&directory).unwrap();
    if create_or_verify_lock_marker(
        &directory.join("publication-pending-v1"),
        &format!("{suffix}:{identity}"),
    )
    .is_err()
    {
        fs::remove_dir_all(root).unwrap();
        return;
    }
    initialize_machine_lock_tree(&directory, &identity).unwrap();
    apply_lock_dacl(&directory, machine_lock_directory_sddl()).unwrap();
    reclaim_unpublished_machine_lock_directories(&root).unwrap();
    assert!(!directory.exists());
    fs::remove_dir(root).unwrap();
}

#[test]
fn terminal_uninstall_crash_transitions_recover_only_in_order() {
    assert_eq!(
        terminal_uninstall_recovery_step("armed", true).unwrap(),
        TerminalUninstallRecoveryStep::RetireMachine
    );
    assert!(terminal_uninstall_recovery_step("armed", false).is_err());
    assert_eq!(
        terminal_uninstall_recovery_step("machine-retired", true).unwrap(),
        TerminalUninstallRecoveryStep::FinishCleanup
    );
    for phase in [
        "cleanup-complete",
        "final-launcher-owned",
        "maintenance-deletion-owned",
        "maintenance-deleted",
        "uninstall-unregistered",
        "journal-removed",
    ] {
        for journal_present in [true, false] {
            assert_eq!(
                terminal_uninstall_recovery_step(phase, journal_present).unwrap(),
                TerminalUninstallRecoveryStep::CleanupComplete,
                "{phase}:{journal_present}"
            );
        }
    }
}

#[test]
fn terminal_uninstall_record_schema_is_strict() {
    let valid = br#"{"schemaVersion":3,"generation":"11111111111111111111111111111111","phase":"armed","maintenanceSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","uninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\"","quietUninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\" /S","serviceName":"TalkingQuillTerminalCleanup-11111111111111111111111111111111","serviceImage":"C:\\ProgramData\\.Talking Quill Terminal Cleanup-11111111111111111111111111111111.exe","serviceSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","serviceFileIdentity":"1:2","recordFileIdentity":"3:4"}"#;
    let record: TerminalUninstallRecord = serde_json::from_slice(valid).unwrap();
    assert_eq!(record.phase, "armed");
    let unknown = br#"{"schemaVersion":3,"generation":"11111111111111111111111111111111","phase":"armed","maintenanceSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","uninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\"","quietUninstallCommand":"\"C:\\Program Files\\Talking Quill Maintenance.exe\" /S","serviceName":"TalkingQuillTerminalCleanup-11111111111111111111111111111111","serviceImage":"C:\\ProgramData\\cleanup.exe","serviceSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","serviceFileIdentity":"1:2","recordFileIdentity":"3:4","path":"C:\\untrusted.exe"}"#;
    assert!(serde_json::from_slice::<TerminalUninstallRecord>(unknown).is_err());
}

#[test]
fn legacy_profile_records_remove_only_exact_generations() {
    let root =
        std::env::temp_dir().join(format!("tq-profile-relaunch-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let generation = "22".repeat(16);
    let directory = root.join(&generation);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("relaunch-record-v1.json"),
        format!(
            "{{\"schemaVersion\":1,\"generation\":\"{generation}\",\"request\":\"--windows-update-bootstrap-v2=dGVzdA==\"}}"
        ),
    )
    .unwrap();
    remove_legacy_profile_relaunch_records(&root).unwrap();
    assert!(!root.exists());

    fs::create_dir_all(root.join("not-owned")).unwrap();
    fs::create_dir_all(root.join("33".repeat(16))).unwrap();
    fs::write(
        root.join("33".repeat(16)).join("relaunch-record-v1.json"),
        b"malformed user data",
    )
    .unwrap();
    remove_legacy_profile_relaunch_records(&root).unwrap();
    assert!(root.join("not-owned").exists());
    assert!(root.join("33".repeat(16)).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn normal_relocated_uninstall_binds_original_and_maintenance_identity() {
    let suffix = "11".repeat(16);
    let root =
        std::env::temp_dir().join(format!("tq-relocated-source-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let maintenance = root.join(format!("Talking Quill Maintenance-{suffix}.exe"));
    let installed = root.join("Uninstall Talking Quill.exe");
    let relocated = std::env::temp_dir().join(format!(".TalkingQuill-uninstall-{suffix}.exe"));
    let _ = fs::remove_file(&relocated);
    fs::copy(std::env::current_exe().unwrap(), &maintenance).unwrap();
    fs::copy(&maintenance, &installed).unwrap();
    fs::copy(&maintenance, &relocated).unwrap();
    validate_relocated_uninstall_image(&relocated, &installed, &installed, &maintenance).unwrap();
    fs::write(&installed, b"replaced").unwrap();
    assert!(
        validate_relocated_uninstall_image(&relocated, &installed, &installed, &maintenance)
            .is_err()
    );
    fs::remove_file(relocated).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mapped_uninstall_image_is_kernel_owned_before_machine_cleanup() {
    if let Some(marker) = std::env::var_os("TQ_SETUP_MAPPED_DELETE_MARKER") {
        let current = std::env::current_exe().unwrap();
        arm_mapped_image_deletion(&current).unwrap();
        fs::write(marker, b"armed").unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let root =
        std::env::temp_dir().join(format!("tq-mapped-uninstall-delete-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let image = root.join("Uninstall Talking Quill.exe");
    fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
    let marker = root.join("armed");
    let mut child = Command::new(&image)
        .args([
            "--exact",
            "windows::tests::mapped_uninstall_image_is_kernel_owned_before_machine_cleanup",
            "--nocapture",
        ])
        .env("TQ_SETUP_MAPPED_DELETE_MARKER", &marker)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    assert!(
        !image.exists(),
        "mapped image must be unlinked before success"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn controller_worker_cross_process_authenticates() {
    if std::env::var_os("TQ_SETUP_CHANNEL_CHILD").is_some() {
        let image = std::env::current_exe().unwrap();
        let (action, _, silent, lifecycle_parent) =
            WorkerChannel::connect_and_authenticate(&image, None).unwrap();
        assert_eq!(lifecycle_parent, 0);
        assert!(action == Action::Repair && silent);
        return;
    }
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let channel = ControllerChannel::create(Action::Repair, true, 0).unwrap();
    let image = std::env::current_exe().unwrap();
    let mut child = Command::new(&image)
        .args([
            "--exact",
            "windows::tests::controller_worker_cross_process_authenticates",
            "--nocapture",
        ])
        .env("TQ_SETUP_CHANNEL_CHILD", "1")
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
    let child_handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let retained = channel.authenticate(&child_handle, &image, None).unwrap();
    assert_eq!(
        unsafe { GetProcessId(retained.as_raw_handle()) },
        child.id()
    );
    assert!(child.wait().unwrap().success());
}

fn test_paths(root: &Path) -> Paths {
    Paths {
        install: root.join("Talking Quill"),
        staging: root.join("staging"),
        backup: root.join("backup"),
        transaction: root.join("transaction.json"),
        maintenance_generation_record: root.join("maintenance-generation-v1"),
        maintenance_uninstaller: root
            .join(format!("Talking Quill Maintenance-{}.exe", "11".repeat(16))),
        recovery_launcher: root.join(format!(
            "Talking Quill Update Recovery/talking-quill-update-recovery-launcher-{}.exe",
            "11".repeat(16)
        )),
        profile: root.join("profile"),
        legacy_authority: root.join("legacy"),
        legacy_quarantine: root.join("quarantine"),
        legacy_task_file: root.join("task"),
        program_data: root.join("program-data"),
    }
}

#[test]
fn pending_uninstall_reselection_is_process_stable_without_a_lifecycle_parent() {
    if let Some(root) = std::env::var_os("TQ_PENDING_UNINSTALL_RESELECT_CHILD") {
        let paths = test_paths(Path::new(&root));
        let lifecycle_parent = std::env::var("TQ_PENDING_UNINSTALL_LIFECYCLE_PARENT")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(matches!(lifecycle_parent, 0 | 42));
        assert!(
            derive_action(&std::env::current_exe().unwrap(), &paths).unwrap() == Action::Install
        );
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "tq-pending-uninstall-reselect-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let paths = test_paths(&root);
    fs::write(
        &paths.transaction,
        br#"{"schemaVersion":2,"phase":"uninstall-cleanup-complete","action":"uninstall","hadPredecessor":true}"#,
    )
    .unwrap();
    let image = std::env::current_exe().unwrap();
    for lifecycle_parent in [0, 42] {
        let status = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::pending_uninstall_reselection_is_process_stable_without_a_lifecycle_parent",
                "--nocapture",
            ])
            .env("TQ_PENDING_UNINSTALL_RESELECT_CHILD", &root)
            .env(
                "TQ_PENDING_UNINSTALL_LIFECYCLE_PARENT",
                lifecycle_parent.to_string(),
            )
            .status()
            .unwrap();
        assert!(status.success());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn controller_worker_kill_recovers_every_durable_phase() {
    if std::env::var_os("TQ_SETUP_SECOND_CLEANUP_CHILD").is_some() {
        let image = std::env::current_exe().unwrap();
        let (action, _, silent, lifecycle_parent) =
            WorkerChannel::connect_and_authenticate(&image, None).unwrap();
        assert!(action == Action::Uninstall && silent && lifecycle_parent == 0);
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if let (Some(root), Some(phase)) = (
        std::env::var_os("TQ_SETUP_FAULT_ROOT"),
        std::env::var_os("TQ_SETUP_FAULT_PHASE"),
    ) {
        let image = std::env::current_exe().unwrap();
        let (_action, _, _, _) = WorkerChannel::connect_and_authenticate(&image, None).unwrap();
        let paths = test_paths(Path::new(&root));
        let phase = phase.to_string_lossy();
        let uninstalling = matches!(
            phase.as_ref(),
            "uninstall-armed"
                | "uninstalling"
                | "uninstall-cleanup-owned"
                | "uninstall-quarantined"
                | "recovering-finish-uninstall"
                | "uninstall-cleanup-complete"
                | "uninstall-finalizer-publishing"
                | "uninstall-finalizer-published"
                | "uninstall-finalizer-deletion-owned"
                | "uninstall-terminal-committing"
                | "uninstall-app-path-retiring"
                | "uninstall-app-path-retired"
                | "uninstall-registration-retiring"
                | "uninstall-registration-retired"
                | "uninstall-cleanup-elevation"
        );
        let had_predecessor = !uninstalling;
        let journal_phase = if phase == "uninstall-cleanup-elevation" {
            "uninstall-quarantined"
        } else {
            phase.as_ref()
        };
        match phase.as_ref() {
            "staging" | "staged" | "prepared" => {
                fs::create_dir(&paths.install).unwrap();
                fs::write(paths.install.join("identity"), b"predecessor").unwrap();
                fs::create_dir(&paths.staging).unwrap();
                fs::write(paths.staging.join("identity"), b"candidate").unwrap();
            }
            "predecessor-moved" | "publishing" => {
                fs::create_dir(&paths.backup).unwrap();
                fs::write(paths.backup.join("identity"), b"predecessor").unwrap();
                fs::create_dir(&paths.staging).unwrap();
                fs::write(paths.staging.join("identity"), b"candidate").unwrap();
            }
            "published-before-persist"
            | "published"
            | "registered"
            | "committed"
            | "legacy-retiring"
            | "legacy-retired" => {
                fs::create_dir(&paths.backup).unwrap();
                fs::write(paths.backup.join("identity"), b"predecessor").unwrap();
                fs::create_dir(&paths.install).unwrap();
                fs::write(paths.install.join("identity"), b"candidate").unwrap();
            }
            "uninstall-armed" | "uninstalling" | "uninstall-cleanup-owned" => {
                fs::create_dir(&paths.install).unwrap();
                fs::write(paths.install.join("identity"), b"candidate").unwrap();
            }
            "uninstall-quarantined"
            | "recovering-finish-uninstall"
            | "uninstall-cleanup-elevation" => {
                fs::create_dir(&paths.backup).unwrap();
                fs::write(paths.backup.join("identity"), b"candidate").unwrap();
            }
            "uninstall-cleanup-complete"
            | "uninstall-finalizer-publishing"
            | "uninstall-finalizer-published"
            | "uninstall-finalizer-deletion-owned"
            | "uninstall-terminal-committing"
            | "uninstall-app-path-retiring"
            | "uninstall-app-path-retired"
            | "uninstall-registration-retiring"
            | "uninstall-registration-retired" => {}
            _ => unreachable!(),
        }
        write_transaction(
            &paths,
            journal_phase,
            if uninstalling {
                Action::Uninstall
            } else {
                Action::Update
            },
            had_predecessor,
        )
        .unwrap();
        if phase == "uninstall-cleanup-elevation" {
            let channel = ControllerChannel::create(Action::Uninstall, true, 0).unwrap();
            let mut child = Command::new(&image)
                .args([
                    "--exact",
                    "windows::tests::controller_worker_kill_recovers_every_durable_phase",
                    "--nocapture",
                ])
                .env("TQ_SETUP_SECOND_CLEANUP_CHILD", "1")
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
            let cleanup = channel.authenticate(&shell, &image, None).unwrap();
            fs::create_dir(&paths.program_data).unwrap();
            fs::write(
                paths.program_data.join("cleanup-pid"),
                child.id().to_string(),
            )
            .unwrap();
            drop(cleanup);
            let _ = child.wait();
        } else {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let image = std::env::current_exe().unwrap();
    for phase in [
        "staging",
        "staged",
        "prepared",
        "predecessor-moved",
        "publishing",
        "published-before-persist",
        "published",
        "registered",
        "committed",
        "legacy-retiring",
        "legacy-retired",
        "uninstall-armed",
        "uninstalling",
        "uninstall-cleanup-owned",
        "uninstall-quarantined",
        "recovering-finish-uninstall",
        "uninstall-cleanup-complete",
        "uninstall-finalizer-publishing",
        "uninstall-finalizer-published",
        "uninstall-finalizer-deletion-owned",
        "uninstall-terminal-committing",
        "uninstall-app-path-retiring",
        "uninstall-app-path-retired",
        "uninstall-registration-retiring",
        "uninstall-registration-retired",
        "uninstall-cleanup-elevation",
    ] {
        let root =
            std::env::temp_dir().join(format!("tq-setup-fault-{}-{phase}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let uninstalling = matches!(
            phase,
            "uninstall-armed"
                | "uninstalling"
                | "uninstall-cleanup-owned"
                | "uninstall-quarantined"
                | "recovering-finish-uninstall"
                | "uninstall-cleanup-complete"
                | "uninstall-finalizer-publishing"
                | "uninstall-finalizer-published"
                | "uninstall-finalizer-deletion-owned"
                | "uninstall-terminal-committing"
                | "uninstall-app-path-retiring"
                | "uninstall-app-path-retired"
                | "uninstall-registration-retiring"
                | "uninstall-registration-retired"
                | "uninstall-cleanup-elevation"
        );
        let channel = ControllerChannel::create(
            if uninstalling {
                Action::Uninstall
            } else {
                Action::Repair
            },
            true,
            if uninstalling { std::process::id() } else { 0 },
        )
        .unwrap();
        let mut child = Command::new(&image)
            .args([
                "--exact",
                "windows::tests::controller_worker_kill_recovers_every_durable_phase",
                "--nocapture",
            ])
            .env("TQ_SETUP_FAULT_ROOT", &root)
            .env("TQ_SETUP_FAULT_PHASE", phase)
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
        let transaction = root.join("transaction.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !transaction.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(transaction.exists(), "{phase}");
        if phase == "uninstall-cleanup-elevation" {
            let marker = root.join("program-data/cleanup-pid");
            while !marker.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let pid: u32 = fs::read_to_string(marker).unwrap().parse().unwrap();
            let cleanup = unsafe {
                OwnedHandle::from_raw_handle(OpenProcess(
                    windows_sys::Win32::System::Threading::PROCESS_TERMINATE | SYNCHRONIZE,
                    0,
                    pid,
                ))
            };
            assert_ne!(unsafe { TerminateProcess(cleanup.as_raw_handle(), 197) }, 0);
            assert_eq!(
                unsafe { WaitForSingleObject(cleanup.as_raw_handle(), 30_000) },
                WAIT_OBJECT_0
            );
        }
        assert_ne!(unsafe { TerminateProcess(worker.as_raw_handle(), 197) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(worker.as_raw_handle(), 30_000) },
            WAIT_OBJECT_0
        );
        let _ = child.wait();
        let paths = test_paths(&root);
        recover_with_system(&paths, false).unwrap();
        if uninstalling {
            require_uninstall_cleanup_complete(&paths).unwrap();
            remove_transaction(&paths).unwrap();
            assert!(!paths.install.exists(), "{phase}");
        } else {
            let expected = if matches!(phase, "committed" | "legacy-retiring" | "legacy-retired") {
                b"candidate".as_slice()
            } else {
                b"predecessor".as_slice()
            };
            assert_eq!(
                fs::read(paths.install.join("identity")).unwrap(),
                expected,
                "{phase}"
            );
        }
        assert!(
            !paths.backup.exists() && !paths.staging.exists() && !paths.transaction.exists(),
            "{phase}"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}

#[test]
fn recovery_progress_states_accept_every_terminal_topology() {
    let root = std::env::temp_dir().join(format!("tq-recovery-progress-{}", std::process::id()));
    for (phase, action, had_predecessor, installed) in [
        ("recovering-restore-predecessor", Action::Update, true, true),
        ("recovering-discard-staging", Action::Update, true, true),
        ("recovering-remove-fresh", Action::Install, false, false),
        ("recovering-finish-commit", Action::Update, true, true),
        (
            "recovering-finish-uninstall",
            Action::Uninstall,
            true,
            false,
        ),
    ] {
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let paths = test_paths(&root);
        if installed {
            fs::create_dir(&paths.install).unwrap();
            fs::write(paths.install.join("identity"), b"terminal").unwrap();
        }
        write_transaction(&paths, phase, action, had_predecessor).unwrap();
        recover_with_system(&paths, false).unwrap();
        if action == Action::Uninstall {
            require_uninstall_cleanup_complete(&paths).unwrap();
            remove_transaction(&paths).unwrap();
        }
        assert!(!paths.transaction.exists(), "{phase}");
        assert_eq!(paths.install.exists(), installed, "{phase}");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn machine_lock_registry_deletion_delegates_to_the_configured_hive() {
    let _test_lock = CHANNEL_TEST_LOCK.lock().unwrap();
    let parent = machine_lock_registry_parent().unwrap();
    let path = format!(r"{parent}\RegistryDelegateTest");
    let mut key = ptr::null_mut();
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                machine_lock_registry_hive(),
                wide(OsStr::new(&path)).as_ptr(),
                0,
                ptr::null_mut(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(unsafe { RegCloseKey(key) }, 0);

    delete_machine_lock_registry_durable(&path, &parent, "test machine-lock key").unwrap();

    assert!(!registry_key_present(machine_lock_registry_hive(), &path).unwrap());
}

#[test]
fn delegated_handle_validation_never_owns_or_closes_the_callers_handle() {
    assert!(duplicate_delegated_process_handle(0, std::process::id()).is_err());
    assert!(duplicate_delegated_process_handle(u64::MAX, std::process::id()).is_err());
    let event =
        unsafe { OwnedHandle::from_raw_handle(CreateEventW(ptr::null(), 1, 0, ptr::null())) };
    let value = event.as_raw_handle() as usize as u64;
    assert!(duplicate_delegated_process_handle(value, std::process::id()).is_err());
    let mut flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(event.as_raw_handle(), &mut flags) },
        0
    );
}

#[test]
fn pipe_proof_binds_nonce_both_peers_and_image() {
    let nonce = [1_u8; 32];
    let image = [2_u8; 32];
    let expected = channel_proof(&nonce, 10, 11, &image);
    assert_ne!(expected, channel_proof(&nonce, 11, 10, &image));
    assert_ne!(expected, channel_proof(&[3_u8; 32], 10, 11, &image));
    assert_ne!(expected, channel_proof(&nonce, 10, 11, &[4_u8; 32]));
}

#[test]
fn p256_transcript_authenticates_roles_frames_and_both_ephemeral_keys() {
    let controller = ephemeral_secret().unwrap();
    let worker = ephemeral_secret().unwrap();
    let controller_public = controller.public_key().to_sec1_bytes();
    let worker_public = worker.public_key().to_sec1_bytes();
    let left = diffie_hellman(
        controller.to_nonzero_scalar(),
        worker.public_key().as_affine(),
    );
    let right = diffie_hellman(
        worker.to_nonzero_scalar(),
        controller.public_key().as_affine(),
    );
    assert_eq!(left.raw_secret_bytes(), right.raw_secret_bytes());
    let nonce = [7_u8; 32];
    let image = [9_u8; 32];
    let frame = [2_u8, 1];
    let proof = authenticated_proof(
        left.raw_secret_bytes(),
        &nonce,
        10,
        11,
        &image,
        &controller_public,
        &worker_public,
        b"controller",
        &frame,
    );
    assert_ne!(
        proof,
        authenticated_proof(
            right.raw_secret_bytes(),
            &nonce,
            10,
            11,
            &image,
            &controller_public,
            &worker_public,
            b"worker",
            &frame
        )
    );
    assert_ne!(
        proof,
        authenticated_proof(
            right.raw_secret_bytes(),
            &nonce,
            10,
            11,
            &image,
            &controller_public,
            &worker_public,
            b"controller",
            &[1, 1]
        )
    );
    assert!(deadline_ms(Instant::now() - Duration::from_millis(1)).is_err());
}
