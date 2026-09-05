//! Reclaim authenticated stale schema-two state.
use super::*;

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn force_stale_cleanup_rejection(stage: &str) -> Result<()> {
    if std::env::var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE").as_deref() == Ok(stage) {
        Err(fail(
            EXIT_REJECTED,
            format!("Forced stale schema-2 cleanup rejection at {stage}."),
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn record_post_audit_cleanup_rejection(
    diagnostic: &mut StaleSchema2Diagnostic,
    audit: &mut StaleCleanupAudit,
    error: SetupError,
) -> Result<()> {
    let stage = "cleanup.rejected.after-audit";
    let evidence = serde_json::json!({ "error": error.message, "exitCode": EXIT_REJECTED });
    let diagnostic_result = diagnostic.record(stage, "rejected", evidence);
    let empty = retained_binding(&[], stage);
    let audit_result = audit.record(stage, &empty, &empty);
    if let Err(audit_error) = audit_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection audit failed: {}",
                error.message, audit_error.message
            ),
        ));
    }
    if let Err(diagnostic_error) = diagnostic_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection diagnostic failed: {}",
                error.message, diagnostic_error.message
            ),
        ));
    }
    Err(fail(EXIT_REJECTED, error.message))
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn run_direct_elevated_stale_schema2_cleanup() -> Result<()> {
    let mut diagnostic = StaleSchema2Diagnostic::open()?;
    let token = token_is_elevated().and_then(|elevated| {
        peer_identity::process_integrity(std::process::id()).map(|integrity| (elevated, integrity))
    });
    let (elevated, integrity_rid) = match token {
        Ok(value) if direct_cleanup_token_is_authorized(value.0, value.1) => {
            diagnostic.record(
                "token.identity",
                "passed",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1 }),
            )?;
            value
        }
        Ok(value) => {
            let error = "Direct stale cleanup requires a high elevated token.";
            diagnostic.record(
                "token.identity",
                "rejected",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1, "error": error }),
            )?;
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error }),
            )?;
            return Err(fail(EXIT_REJECTED, error));
        }
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    debug_assert!(direct_cleanup_token_is_authorized(elevated, integrity_rid));
    let image = open_authenticated_direct_cleanup_image();
    let (mut retained_image, expected_hash) = match image {
        Ok(value) => value,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    // Retain a protected append handle so a later cleanup failure can be recorded durably.
    let audit = StaleCleanupAudit::open();
    let mut audit = match audit {
        Ok(audit) => audit,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    let operation = (|| -> Result<()> {
        reclaim_exact_schema2_orphan_with_audit(true, false, &mut audit)?;
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        if hash_reader(&mut retained_image)? != expected_hash {
            return Err(fail(
                EXIT_REJECTED,
                "Direct cleanup image changed during the operation.",
            ));
        }
        Ok(())
    })();
    if let Err(error) = operation {
        return record_post_audit_cleanup_rejection(&mut diagnostic, &mut audit, error);
    }
    Ok(())
}

pub(super) fn reclaim_exact_schema2_orphan_v2(
    developer_command: bool,
    authenticated_parent: bool,
    audit: &mut StaleCleanupAudit,
) -> Result<()> {
    if !token_is_elevated()? {
        return Err(fail(EXIT_REJECTED, "Stale cleanup requires elevation."));
    }
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = known_folder(&FOLDERID_System)?;
    let legacy = LegacyMutexPair::acquire()?;
    if !developer_command && std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH").is_none() {
        return Err(fail(
            EXIT_REJECTED,
            "Production orphan reclaim requires an administrator audit path.",
        ));
    }
    let Some(publication) = exact_machine_lock_publication_for_mutation()? else {
        let binding = retained_binding(&[], "no-machine-lock-publication");
        complete_stale_cleanup_zero_state(
            audit,
            &binding,
            &program_files,
            &program_data,
            &system,
            authenticated_parent,
        )?;
        drop(legacy);
        return Ok(());
    };
    let suffix = publication.suffix.clone();
    validate_machine_lock_suffix(&suffix)?;
    let lock_directory = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    let lifecycle =
        RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?;
    lifecycle.verify_protected_acl()?;
    exact_cleanup_registry(&suffix)?;

    // The lifecycle file is retained before any process, role, service, task, Run, registration,
    // journal, terminal, Program Files, or ProgramData activity inspection.
    if !staged_path_is_protected(&lock_directory, true)?
        || !staged_path_is_protected(&lock_directory.join("recovery-state-v1.lock"), false)?
        || !staged_path_is_protected(&lock_directory.join("lock-tree-identity-v1"), false)?
        || !staged_path_is_protected(&lock_directory.join("recovery-state-v1.identity-v1"), false)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lifecycle ACL inventory is not exact.",
        ));
    }
    let lock_root = RetainedStaleObject::open(&lock_directory, true)?;
    let mut lock_tree_identity =
        RetainedStaleObject::open(&lock_directory.join("lock-tree-identity-v1"), false)?;
    let mut lock_file_identity =
        RetainedStaleObject::open(&lock_directory.join("recovery-state-v1.identity-v1"), false)?;
    for object in [&lock_root, &lock_tree_identity, &lock_file_identity] {
        object.verify_protected_acl()?;
    }
    let recovery = program_data.join("Talking Quill Update Recovery");
    if !path_present(&recovery)? {
        exact_stale_coordination_inventory(&program_data, &suffix, false)?;
        let publication_pending_path = lock_directory.join("publication-pending-v1");
        if !staged_path_is_protected(&publication_pending_path, false)? {
            return Err(fail(
                EXIT_REJECTED,
                "Published machine lock marker ACL is not exact.",
            ));
        }
        let mut publication_pending = RetainedStaleObject::open(&publication_pending_path, false)?;
        publication_pending.verify_protected_acl()?;
        let expected_publication = format!("{suffix}:{}", lock_root.identity);
        if lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
            || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
            || publication_pending.read_all()? != expected_publication.as_bytes()
            || lock_root.names()?
                != [
                    "lock-tree-identity-v1",
                    "publication-pending-v1",
                    "recovery-state-v1.identity-v1",
                    "recovery-state-v1.lock",
                ]
        {
            return Err(fail(
                EXIT_REJECTED,
                "Retained orphan lock identity is not exact.",
            ));
        }
        let admission = active_state_proof(
            &program_files,
            &program_data,
            &system,
            true,
            authenticated_parent,
        )?;
        for object in [
            &lifecycle,
            &lock_root,
            &lock_tree_identity,
            &lock_file_identity,
            &publication_pending,
        ] {
            object.verify()?;
            object.verify_protected_acl()?;
        }
        let binding = retained_binding(
            &[
                &lifecycle,
                &lock_root,
                &lock_tree_identity,
                &lock_file_identity,
                &publication_pending,
            ],
            &suffix,
        );
        audit.record("inspected", &binding, &admission)?;
        #[cfg(feature = "stale-schema2-cleanup")]
        force_stale_cleanup_rejection("post-inspected")?;
        std::thread::sleep(Duration::from_millis(750));
        if publication_pending.read_all()? != expected_publication.as_bytes() {
            return Err(fail(
                EXIT_REJECTED,
                "Published machine lock marker changed during stability wait.",
            ));
        }
        for object in [
            &lifecycle,
            &lock_root,
            &lock_tree_identity,
            &lock_file_identity,
            &publication_pending,
        ] {
            object.verify()?;
            object.verify_protected_acl()?;
        }
        if lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
            || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
            || lock_root.names()?
                != [
                    "lock-tree-identity-v1",
                    "publication-pending-v1",
                    "recovery-state-v1.identity-v1",
                    "recovery-state-v1.lock",
                ]
        {
            return Err(fail(
                EXIT_REJECTED,
                "Retained orphan lock changed during stability wait.",
            ));
        }
        exact_stale_coordination_inventory(&program_data, &suffix, false)?;
        let second = active_state_proof(
            &program_files,
            &program_data,
            &system,
            true,
            authenticated_parent,
        )?;
        exact_cleanup_registry(&suffix)?;
        audit.record("commit-intent", &binding, &second)?;
        #[cfg(feature = "stale-schema2-cleanup")]
        force_stale_cleanup_rejection("post-commit-intent")?;
        for (registry_key, registry_path) in [
            (publication.parent, r"Software\Talking Quill"),
            (publication.child, MACHINE_LOCK_REGISTRY_KEY),
        ] {
            protect_stale_registry_key(registry_key, registry_path)?;
        }
        if exact_machine_lock_publication()?
            .as_ref()
            .map(|current| current.suffix.as_str())
            != Some(&suffix)
        {
            return Err(fail(
                EXIT_REJECTED,
                "Orphan machine lifecycle publication changed before mutation.",
            ));
        }
        exact_stale_coordination_inventory(&program_data, &suffix, false)?;
        if publication_pending.read_all()? != expected_publication.as_bytes()
            || lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
            || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
            || lock_root.names()?
                != [
                    "lock-tree-identity-v1",
                    "publication-pending-v1",
                    "recovery-state-v1.identity-v1",
                    "recovery-state-v1.lock",
                ]
        {
            return Err(fail(
                EXIT_REJECTED,
                "Retained orphan lock changed before mutation.",
            ));
        }
        for object in [
            &lifecycle,
            &lock_root,
            &lock_tree_identity,
            &lock_file_identity,
            &publication_pending,
        ] {
            object.verify()?;
            object.verify_protected_acl()?;
        }
        publication_pending.delete()?;
        lock_tree_identity.delete()?;
        lock_file_identity.delete()?;
        let mut lifecycle = lifecycle;
        let lifecycle_identity = lifecycle.identity.clone();
        let retained_lifecycle_path = program_data.join(format!(
            ".Talking Quill.machine-lifecycle-retained-{suffix}"
        ));
        lifecycle.rename(&retained_lifecycle_path)?;
        lifecycle.mark_posix_deleted()?;
        lock_root.delete()?;
        flush_setup_directory(&program_data)?;
        exact_cleanup_registry(&suffix)?;
        delete_registry_tree_durable(
            MACHINE_LOCK_REGISTRY_KEY,
            r"Software\Talking Quill",
            "stale machine lifecycle publication",
        )?;
        if registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
            || registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?
                != Some(Vec::new())
        {
            return Err(fail(
                EXIT_REJECTED,
                "Registry parent gained unknown content.",
            ));
        }
        delete_registry_tree_durable(
            r"Software\Talking Quill",
            r"Software",
            "empty Talking Quill registry parent",
        )?;
        if file_identity_text(&lifecycle.file)? != lifecycle_identity {
            return Err(fail(
                EXIT_REJECTED,
                "Retained lifecycle authority changed during registry deletion.",
            ));
        }
        lifecycle.finish_deleted()?;
        flush_setup_directory(&program_data)?;
        complete_stale_cleanup_zero_state(
            audit,
            &binding,
            &program_files,
            &program_data,
            &system,
            authenticated_parent,
        )?;
        drop(legacy);
        return Ok(());
    }
    exact_stale_coordination_inventory(&program_data, &suffix, true)?;
    let relaunch = recovery.join("Relaunch Records");
    let generation = relaunch.join(format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}"));
    let mut pending =
        RetainedStaleObject::open(&generation.join(SYNTHETIC_SCHEMA2_PENDING), false)?;
    let admission = active_state_proof(
        &program_files,
        &program_data,
        &system,
        true,
        authenticated_parent,
    )?;
    // The pending file and lifecycle lock are exclusive before this first mutation. Harden each
    // parent from the inside out, then retain it without sharing. Exact handle inventories below
    // reject anything that appeared before hardening.
    apply_lock_dacl(&generation, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let generation_guard = RetainedStaleObject::open(&generation, true)?;
    apply_lock_dacl(&relaunch, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let relaunch_guard = RetainedStaleObject::open(&relaunch, true)?;
    apply_lock_dacl(&recovery, MACHINE_LOCK_DIRECTORY_SDDL)?;
    let recovery_guard = RetainedStaleObject::open(&recovery, true)?;
    let bytes = pending.read_all()?;
    let lock_root_marker = String::from_utf8(lock_tree_identity.read_all()?)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lock root marker is not UTF-8."))?;
    let lock_file_marker = String::from_utf8(lock_file_identity.read_all()?)
        .map_err(|_| fail(EXIT_REJECTED, "Machine lifecycle marker is not UTF-8."))?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes != SYNTHETIC_SCHEMA2_BYTES
        || lock_root_marker != lock_root.identity
        || lock_file_marker != lifecycle.identity
        || hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256
        || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
        || relaunch_guard.names()? != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
        || recovery_guard.names()? != ["Relaunch Records"]
        || lock_root.names()?
            != [
                "lock-tree-identity-v1",
                "recovery-state-v1.identity-v1",
                "recovery-state-v1.lock",
            ]
    {
        return Err(fail(
            EXIT_REJECTED,
            "Retained stale fixture inventory is not exact.",
        ));
    }
    for object in [
        &lifecycle,
        &lock_root,
        &lock_tree_identity,
        &lock_file_identity,
        &pending,
        &generation_guard,
        &relaunch_guard,
        &recovery_guard,
    ] {
        object.verify()?;
    }
    let objects = [
        &lifecycle,
        &lock_root,
        &lock_tree_identity,
        &lock_file_identity,
        &pending,
        &generation_guard,
        &relaunch_guard,
        &recovery_guard,
    ];
    let binding = retained_binding(&objects, &suffix);
    audit.record("inspected", &binding, &admission)?;
    #[cfg(feature = "stale-schema2-cleanup")]
    force_stale_cleanup_rejection("post-inspected")?;
    std::thread::sleep(Duration::from_millis(750));
    for object in objects {
        object.verify()?;
    }
    if pending.read_all()? != SYNTHETIC_SCHEMA2_BYTES
        || lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
        || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
        || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
        || relaunch_guard.names()? != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
        || recovery_guard.names()? != ["Relaunch Records"]
        || lock_root.names()?
            != [
                "lock-tree-identity-v1",
                "recovery-state-v1.identity-v1",
                "recovery-state-v1.lock",
            ]
    {
        return Err(fail(
            EXIT_REJECTED,
            "Retained fixture changed during the stability wait.",
        ));
    }
    let second = active_state_proof(
        &program_files,
        &program_data,
        &system,
        true,
        authenticated_parent,
    )?;
    exact_stale_coordination_inventory(&program_data, &suffix, true)?;
    exact_cleanup_registry(&suffix)?;
    audit.record("commit-intent", &binding, &second)?;
    #[cfg(feature = "stale-schema2-cleanup")]
    force_stale_cleanup_rejection("post-commit-intent")?;

    // Repeat registry identity after the second active proof and before the first mutation.
    if exact_machine_lock_publication()?
        .as_ref()
        .map(|publication| publication.suffix.as_str())
        != Some(&suffix)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lifecycle publication changed before mutation.",
        ));
    }
    for (registry_key, registry_path) in [
        (publication.parent, r"Software\Talking Quill"),
        (publication.child, MACHINE_LOCK_REGISTRY_KEY),
    ] {
        protect_stale_registry_key(registry_key, registry_path)?;
    }

    pending.delete()?;
    generation_guard.delete()?;
    relaunch_guard.delete()?;
    recovery_guard.delete()?;
    lock_tree_identity.delete()?;
    lock_file_identity.delete()?;
    // Move the retained lifecycle file out of its tree, mark it delete-pending, and keep that
    // same identity handle through registry-last deletion. This lets the now-empty lock tree be
    // handle-deleted without releasing lifecycle authority.
    let mut lifecycle = lifecycle;
    let lifecycle_identity = lifecycle.identity.clone();
    let retained_lifecycle_path = program_data.join(format!(
        ".Talking Quill.machine-lifecycle-retained-{suffix}"
    ));
    lifecycle.rename(&retained_lifecycle_path)?;
    lifecycle.mark_posix_deleted()?;
    lock_root.delete()?;
    flush_setup_directory(&program_data)?;
    exact_cleanup_registry(&suffix)?;
    delete_registry_tree_durable(
        MACHINE_LOCK_REGISTRY_KEY,
        r"Software\Talking Quill",
        "stale machine lifecycle publication",
    )?;
    if registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
        || registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
    {
        return Err(fail(
            EXIT_REJECTED,
            "Registry parent gained unknown content.",
        ));
    }
    delete_registry_tree_durable(
        r"Software\Talking Quill",
        r"Software",
        "empty Talking Quill registry parent",
    )?;
    if file_identity_text(&lifecycle.file)? != lifecycle_identity {
        return Err(fail(
            EXIT_REJECTED,
            "Retained lifecycle authority changed during registry deletion.",
        ));
    }
    lifecycle.finish_deleted()?;
    flush_setup_directory(&program_data)?;
    complete_stale_cleanup_zero_state(
        audit,
        &binding,
        &program_files,
        &program_data,
        &system,
        authenticated_parent,
    )?;
    drop(legacy);
    Ok(())
}

pub(super) fn reclaim_exact_schema2_orphan_with_audit(
    developer_command: bool,
    authenticated_parent: bool,
    audit: &mut StaleCleanupAudit,
) -> Result<()> {
    reclaim_exact_schema2_orphan_v2(developer_command, authenticated_parent, audit)
}

pub(super) fn reclaim_exact_schema2_orphan(
    developer_command: bool,
    authenticated_parent: bool,
) -> Result<()> {
    let mut audit = StaleCleanupAudit::open()?;
    reclaim_exact_schema2_orphan_with_audit(developer_command, authenticated_parent, &mut audit)
}
