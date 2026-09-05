//! Explicit diagnostic entry point for legacy schema-two state.
use super::*;

mod image;
pub(super) use image::*;

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn run_direct_stale_schema2_diagnostic(arguments: &[OsString]) -> Result<i32> {
    let mut diagnostic = StaleSchema2Diagnostic::open()?;
    run_direct_stale_schema2_diagnostic_inner(&mut diagnostic, arguments)
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn run_direct_stale_schema2_diagnostic_inner(
    diagnostic: &mut StaleSchema2Diagnostic,
    arguments: &[OsString],
) -> Result<i32> {
    diagnostic_stage(diagnostic, "request.exact-argv", || {
        let argv_utf16 = arguments
            .iter()
            .map(|argument| argument.encode_wide().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        Ok(((), serde_json::json!({ "argvUtf16": argv_utf16 })))
    })?;
    diagnostic_stage(diagnostic, "token.identity", || {
        let elevated = token_is_elevated()?;
        let integrity_rid = peer_identity::process_integrity(std::process::id())?;
        if !direct_cleanup_token_is_authorized(elevated, integrity_rid) {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnosis requires a high elevated token.",
            ));
        }
        Ok((
            (),
            serde_json::json!({
                "elevated": elevated,
                "integrityRid": integrity_rid,
                "processId": std::process::id(),
            }),
        ))
    })?;
    let (current, kernel_image) = diagnostic_stage(diagnostic, "self.path", || {
        let current = std::env::current_exe().map_err(io_failure)?;
        let kernel_image = process_image(std::process::id())?;
        if canonical(&current)? != canonical(&kernel_image)? {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic process image does not match its self path.",
            ));
        }
        let evidence = serde_json::json!({
            "current": current.to_string_lossy(),
            "kernel": kernel_image.to_string_lossy(),
        });
        Ok(((current, kernel_image), evidence))
    })?;
    let mut retained_image = diagnostic_stage(diagnostic, "self.open", || {
        let image = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&current)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic image cannot be retained."))?;
        Ok((image, serde_json::json!({ "retained": true })))
    })?;
    diagnostic_stage(diagnostic, "self.acl", || {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic image has no parent."))?;
        if !staged_path_is_protected(parent, true)?
            || !protected_file_handle_acl_is_exact(&retained_image)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image is not administrator protected.",
            ));
        }
        Ok(((), serde_json::json!({ "protected": true })))
    })?;
    let identity = diagnostic_stage(diagnostic, "self.identity", || {
        let identity = file_identity_text(&retained_image)?;
        let path_image = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&kernel_image)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic process image changed."))?;
        if file_identity_text(&path_image)? != identity {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic process image identity changed.",
            ));
        }
        Ok((
            identity.clone(),
            serde_json::json!({ "fileIdentity": identity }),
        ))
    })?;
    let expected_hash = diagnostic_stage(diagnostic, "self.sha256", || {
        let digest = hash_reader(&mut retained_image)?;
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        Ok((digest, serde_json::json!({ "sha256": hex_hash(&digest) })))
    })?;
    let package = diagnostic_stage(diagnostic, "package.tqpkg2", || {
        let length = retained_image.metadata().map_err(io_failure)?.len();
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        let package = package::parse(&mut retained_image, length).map_err(|error| {
            fail(
                EXIT_REJECTED,
                format!("Diagnostic TQPKG2 validation failed: {error:?}"),
            )
        })?;
        Ok((
            package,
            serde_json::json!({ "parser": "rust", "format": "TQPKG2", "schemaVersion": 2 }),
        ))
    })?;
    diagnostic_stage(diagnostic, "package.source-binding", || {
        let (source_commit, source_tree) = direct_cleanup_source_identity()?;
        let expected_architecture = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "unsupported"
        };
        if package.manifest.source_commit != source_commit
            || package.manifest.source_tree != source_tree
            || package.manifest.architecture != expected_architecture
            || package.manifest.package_mode != "stale-schema2-cleanup"
            || package.manifest.predecessor.is_some()
            || package.manifest.fault_phase.is_some()
        {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image does not match its compiled source identity.",
            ));
        }
        Ok((
            (),
            serde_json::json!({
                "architecture": package.manifest.architecture,
                "packageMode": package.manifest.package_mode,
                "sourceCommit": package.manifest.source_commit,
                "sourceTree": package.manifest.source_tree,
            }),
        ))
    })?;
    let audit_path = diagnostic_stage(diagnostic, "audit.environment", || {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| fail(EXIT_REJECTED, "TQ_STALE_SCHEMA2_AUDIT_PATH is required."))?;
        Ok((
            path.clone(),
            serde_json::json!({ "path": path.to_string_lossy() }),
        ))
    })?;
    let audit_parent = diagnostic_stage(diagnostic, "audit.path", || {
        if !audit_path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale cleanup audit path must be absolute.",
            ));
        }
        let parent = audit_path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Audit path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        Ok((parent, serde_json::json!({ "absolute": true })))
    })?;
    let audit_file = diagnostic_stage(diagnostic, "audit.open", || {
        let file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&audit_path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected cleanup audit file.",
                )
            })?;
        Ok((file, serde_json::json!({ "opened": true })))
    })?;
    let audit_file = diagnostic_stage(diagnostic, "audit.acl", || {
        if !protected_file_handle_acl_is_exact(&audit_file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Cleanup audit is not administrator protected.",
            ));
        }
        Ok((audit_file, serde_json::json!({ "protected": true })))
    })?;
    let mut audit = diagnostic_stage(diagnostic, "audit.initialize", || {
        let audit = StaleCleanupAudit::from_retained(audit_file, audit_parent)?;
        let evidence = serde_json::json!({ "auditIdentity": audit.identity });
        Ok((audit, evidence))
    })?;
    diagnostic_stage(diagnostic, "audit.event", || {
        let empty = retained_binding(&[], "diagnostic-start");
        audit.record("diagnostic-start", &empty, &empty)?;
        Ok(((), serde_json::json!({ "stage": "diagnostic-start" })))
    })?;
    let legacy = diagnostic_stage(diagnostic, "mutex.availability", || {
        let mutex = LegacyMutexPair::acquire()?;
        Ok((mutex, serde_json::json!({ "available": true })))
    })?;
    let (program_files, program_data, system) =
        diagnostic_stage(diagnostic, "paths.known-folders", || {
            let program_files = known_folder(&FOLDERID_ProgramFiles)?;
            let program_data = known_folder(&FOLDERID_ProgramData)?;
            let system = known_folder(&FOLDERID_System)?;
            Ok((
                (program_files, program_data, system),
                serde_json::json!({ "resolved": true }),
            ))
        })?;
    let publication = diagnostic_stage(diagnostic, "registry.inventory", || {
        let publication = exact_machine_lock_publication()?;
        let subkeys = registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
        let values = registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
        if let Some(publication) = publication.as_ref() {
            exact_cleanup_registry(&publication.suffix)?;
        }
        let evidence = serde_json::json!({
            "machineLockSuffix": publication.as_ref().map(|value| &value.suffix),
            "subkeys": subkeys,
            "values": values,
            "aclAdmission": publication.as_ref().map(|value| match value.acl_admission {
                StaleRegistryAclAdmission::LegacyExactParent => "legacy-exact-parent",
                StaleRegistryAclAdmission::Hardened => "hardened",
            }),
            "parentDescriptor": publication.as_ref().map(|value| &value.parent_sddl),
            "childDescriptor": publication.as_ref().map(|value| &value.child_sddl),
        });
        Ok((publication, evidence))
    })?;
    let active = diagnostic_stage(diagnostic, "active-state.inventory", || {
        let proof = active_state_proof(&program_files, &program_data, &system, true, false)?;
        Ok((proof.clone(), serde_json::json!({ "proofSha256": proof })))
    })?;
    let Some(publication) = publication else {
        let binding = retained_binding(&[], "no-machine-lock-publication");
        diagnostic_stage(diagnostic, "image.stability", || {
            retained_image
                .seek(SeekFrom::Start(0))
                .map_err(io_failure)?;
            if hash_reader(&mut retained_image)? != expected_hash {
                return Err(fail(
                    EXIT_REJECTED,
                    "Diagnostic image changed during inspection.",
                ));
            }
            Ok(((), serde_json::json!({ "fileIdentity": identity })))
        })?;
        diagnostic_stage(diagnostic, "audit.event", || {
            audit.record("diagnostic-complete", &binding, &active)?;
            Ok(((), serde_json::json!({ "stage": "diagnostic-complete" })))
        })?;
        diagnostic_stage(diagnostic, "diagnostic.complete", || {
            Ok((
                (),
                serde_json::json!({ "bindingSha256": binding, "state": "absent" }),
            ))
        })?;
        drop(legacy);
        return Ok(0);
    };
    let suffix = publication.suffix.clone();
    let lock_directory = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    let lifecycle = diagnostic_stage(diagnostic, "lifecycle-lock.availability", || {
        let object =
            RetainedStaleObject::open_lifecycle(&lock_directory.join("recovery-state-v1.lock"))?;
        object.verify_protected_acl()?;
        let evidence = serde_json::json!({ "fileIdentity": object.identity });
        Ok((object, evidence))
    })?;
    let recovery = program_data.join("Talking Quill Update Recovery");
    let orphan_lock_only = !path_present(&recovery)?;
    let (lock_root, mut lock_tree_identity, mut lock_file_identity, mut publication_pending) =
        diagnostic_stage(diagnostic, "fixture.identity", || {
            for (path, directory) in [
                (&lock_directory, true),
                (&lock_directory.join("recovery-state-v1.lock"), false),
                (&lock_directory.join("lock-tree-identity-v1"), false),
                (&lock_directory.join("recovery-state-v1.identity-v1"), false),
            ] {
                if !staged_path_is_protected(path, directory)? {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Machine lifecycle ACL inventory is not exact.",
                    ));
                }
            }
            let lock_root = RetainedStaleObject::open(&lock_directory, true)?;
            let mut lock_tree_identity =
                RetainedStaleObject::open(&lock_directory.join("lock-tree-identity-v1"), false)?;
            let mut lock_file_identity = RetainedStaleObject::open(
                &lock_directory.join("recovery-state-v1.identity-v1"),
                false,
            )?;
            for object in [&lock_root, &lock_tree_identity, &lock_file_identity] {
                object.verify_protected_acl()?;
            }
            let mut publication_pending = if orphan_lock_only {
                let path = lock_directory.join("publication-pending-v1");
                if !staged_path_is_protected(&path, false)? {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Published machine lock marker ACL is not exact.",
                    ));
                }
                let object = RetainedStaleObject::open(&path, false)?;
                object.verify_protected_acl()?;
                Some(object)
            } else {
                None
            };
            let expected_names = if orphan_lock_only {
                vec![
                    "lock-tree-identity-v1".to_owned(),
                    "publication-pending-v1".to_owned(),
                    "recovery-state-v1.identity-v1".to_owned(),
                    "recovery-state-v1.lock".to_owned(),
                ]
            } else {
                vec![
                    "lock-tree-identity-v1".to_owned(),
                    "recovery-state-v1.identity-v1".to_owned(),
                    "recovery-state-v1.lock".to_owned(),
                ]
            };
            let expected_publication = format!("{suffix}:{}", lock_root.identity);
            if lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
                || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
                || publication_pending.as_mut().is_some_and(|marker| {
                    marker.read_all().ok().as_deref() != Some(expected_publication.as_bytes())
                })
                || lock_root.names()? != expected_names
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Retained stale lock identity is not exact.",
                ));
            }
            Ok((
                (
                    lock_root,
                    lock_tree_identity,
                    lock_file_identity,
                    publication_pending,
                ),
                serde_json::json!({
                    "lifecycle": lifecycle.identity,
                }),
            ))
        })?;
    if orphan_lock_only {
        exact_stale_coordination_inventory(&program_data, &suffix, false)?;
        let publication_pending = publication_pending
            .as_mut()
            .expect("orphan topology retains the publication marker");
        let expected_publication = format!("{suffix}:{}", lock_root.identity);
        if publication_pending.read_all()? != expected_publication.as_bytes() {
            return Err(fail(
                EXIT_REJECTED,
                "Published machine lock marker content is not exact.",
            ));
        }
        let binding = retained_binding(
            &[
                &lifecycle,
                &lock_root,
                &lock_tree_identity,
                &lock_file_identity,
                &*publication_pending,
            ],
            &suffix,
        );
        diagnostic_stage(diagnostic, "image.stability", || {
            std::thread::sleep(Duration::from_millis(750));
            if publication_pending.read_all()? != expected_publication.as_bytes() {
                return Err(fail(
                    EXIT_REJECTED,
                    "Published machine lock marker changed during inspection.",
                ));
            }
            for object in [
                &lifecycle,
                &lock_root,
                &lock_tree_identity,
                &lock_file_identity,
                &*publication_pending,
            ] {
                object.verify()?;
                object.verify_protected_acl()?;
            }
            exact_stale_coordination_inventory(&program_data, &suffix, false)?;
            retained_image
                .seek(SeekFrom::Start(0))
                .map_err(io_failure)?;
            if hash_reader(&mut retained_image)? != expected_hash {
                return Err(fail(
                    EXIT_REJECTED,
                    "Diagnostic image changed during inspection.",
                ));
            }
            Ok(((), serde_json::json!({ "fileIdentity": identity })))
        })?;
        diagnostic_stage(diagnostic, "audit.event", || {
            audit.record("diagnostic-complete", &binding, &active)?;
            Ok(((), serde_json::json!({ "stage": "diagnostic-complete" })))
        })?;
        diagnostic_stage(diagnostic, "diagnostic.complete", || {
            Ok((
                (),
                serde_json::json!({
                    "bindingSha256": binding,
                    "state": "exact-orphan-lock-only",
                }),
            ))
        })?;
        drop(legacy);
        return Ok(0);
    }
    exact_stale_coordination_inventory(&program_data, &suffix, true)?;
    let (pending, generation_guard, relaunch_guard, recovery_guard, bytes) =
        diagnostic_stage(diagnostic, "fixture.identity", || {
            let recovery = program_data.join("Talking Quill Update Recovery");
            let relaunch = recovery.join("Relaunch Records");
            let generation =
                relaunch.join(format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}"));
            let mut pending =
                RetainedStaleObject::open(&generation.join(SYNTHETIC_SCHEMA2_PENDING), false)?;
            let generation_guard = RetainedStaleObject::open(&generation, true)?;
            let relaunch_guard = RetainedStaleObject::open(&relaunch, true)?;
            let recovery_guard = RetainedStaleObject::open(&recovery, true)?;
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
            let bytes = pending.read_all()?;
            if lock_tree_identity.read_all()? != lock_root.identity.as_bytes()
                || lock_file_identity.read_all()? != lifecycle.identity.as_bytes()
                || generation_guard.names()? != [SYNTHETIC_SCHEMA2_PENDING]
                || relaunch_guard.names()?
                    != [format!("1594b190881d1328-{SYNTHETIC_SCHEMA2_GENERATION}")]
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
                    "Retained stale fixture identity is not exact.",
                ));
            }
            let evidence = serde_json::json!({
                "lifecycle": lifecycle.identity,
                "lockRoot": lock_root.identity,
                "pending": pending.identity,
            });
            Ok((
                (
                    pending,
                    generation_guard,
                    relaunch_guard,
                    recovery_guard,
                    bytes,
                ),
                evidence,
            ))
        })?;
    diagnostic_stage(diagnostic, "fixture.sha256", || {
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes != SYNTHETIC_SCHEMA2_BYTES || hex_hash(&digest) != SYNTHETIC_SCHEMA2_SHA256 {
            return Err(fail(
                EXIT_REJECTED,
                "Retained stale fixture hash is not exact.",
            ));
        }
        Ok((
            (),
            serde_json::json!({ "sha256": hex_hash(&digest), "size": bytes.len() }),
        ))
    })?;
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
    diagnostic_stage(diagnostic, "image.stability", || {
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        if hash_reader(&mut retained_image)? != expected_hash {
            return Err(fail(
                EXIT_REJECTED,
                "Diagnostic image changed during inspection.",
            ));
        }
        Ok(((), serde_json::json!({ "fileIdentity": identity })))
    })?;
    diagnostic_stage(diagnostic, "audit.event", || {
        audit.record("diagnostic-complete", &binding, &active)?;
        Ok(((), serde_json::json!({ "stage": "diagnostic-complete" })))
    })?;
    diagnostic_stage(diagnostic, "diagnostic.complete", || {
        Ok((
            (),
            serde_json::json!({ "bindingSha256": binding, "state": "exact-schema2-fixture" }),
        ))
    })?;
    drop(legacy);
    Ok(0)
}
