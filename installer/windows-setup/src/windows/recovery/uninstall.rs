//! Uninstall cleanup and terminal service ownership handoff.
use super::*;

pub(in super::super) fn uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    defer_mapped_controller_cleanup: bool,
    machine_lock: &mut Option<MachineLock>,
) -> Result<()> {
    write_transaction(paths, "uninstalling", Action::Uninstall, true)?;
    // Keep both authenticated recovery entry points durable until a relocated cleanup worker
    // has committed all machine cleanup and reported success to its supervising worker.
    system.register_installed(paths)?;
    write_transaction(paths, "uninstall-cleanup-owned", Action::Uninstall, true)?;
    ensure_uninstall_finalizer_registered(&std::env::current_exe().map_err(io_failure)?, paths)?;
    system.retire_legacy(paths)?;
    system.clear_update_recovery(paths)?;
    if defer_mapped_controller_cleanup && path_present(&paths.install)? {
        remove_plain_tree(&paths.backup)?;
        durable_rename(&paths.install, &paths.backup)?;
        write_transaction(paths, "uninstall-quarantined", Action::Uninstall, true)?;
        remove_plain_tree(&paths.staging)?;
        // Hand the exclusive machine lock to the authenticated child. A zero exit is its durable
        // completion report. Reacquire before clearing terminal recovery records.
        drop(machine_lock.take());
        launch_same_token_uninstall_cleanup(&std::env::current_exe().map_err(io_failure)?)?;
        *machine_lock = Some(MachineLock::acquire(
            paths,
            120_000,
            installed_recovery_policy_epoch(paths)?,
        )?);
        if !path_present(&paths.transaction)? && !path_present(&paths.install)? {
            return Ok(());
        }
        require_uninstall_cleanup_complete(paths)?;
        return Ok(());
    }
    finish_uninstall_machine_cleanup(paths, system)
}

pub(in super::super) fn finish_uninstall_machine_cleanup(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
) -> Result<()> {
    write_transaction(
        paths,
        "recovering-finish-uninstall",
        Action::Uninstall,
        true,
    )?;
    system.retire_legacy(paths)?;
    system.clear_update_recovery(paths)?;
    remove_plain_tree(&paths.install)?;
    remove_plain_tree(&paths.backup)?;
    remove_plain_tree(&paths.staging)?;
    write_transaction(paths, "uninstall-cleanup-complete", Action::Uninstall, true)
}

pub(in super::super) fn require_uninstall_cleanup_complete(paths: &Paths) -> Result<()> {
    assert_plain_file(&paths.transaction)?;
    let value: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer cleanup journal is invalid."))?;
    if value.schema_version != TRANSACTION_SCHEMA
        || value.action != "uninstall"
        || value.phase != "uninstall-cleanup-complete"
    {
        return Err(fail(
            EXIT_REJECTED,
            "Relocated uninstall cleanup did not commit completion.",
        ));
    }
    Ok(())
}

pub(in super::super) fn retire_and_remove_machine_lock(
    paths: &Paths,
    machine_lock: &mut Option<MachineLock>,
) -> Result<Option<LegacyMutexPair>> {
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    Ok(legacy)
}

pub(in super::super) fn complete_terminal_uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
    machine_lock: &mut Option<MachineLock>,
) -> Result<()> {
    require_uninstall_cleanup_complete(paths)?;
    let generation = if let Some(record) = read_terminal_uninstall_record(paths)? {
        if record.phase != "cleanup-complete" {
            resume_terminal_service(paths, &record)?;
        }
        record.generation
    } else {
        finalize_uninstall(paths, system, current)?
    };
    // The service cannot acquire the lifecycle lock until its creator releases it.
    drop(machine_lock.take());
    wait_for_terminal_service_retirement(paths, &generation)
}

pub(in super::super) fn finalize_uninstall(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
) -> Result<String> {
    require_uninstall_cleanup_complete(paths)?;
    // Publish and start the protected SCM owner before retiring any callable registration.
    register_uninstall_executable(&paths.maintenance_uninstaller)?;
    let terminal_generation = publish_terminal_uninstall_record(paths, current)?;
    let _ = system;
    Ok(terminal_generation)
}

pub(in super::super) fn retire_terminal_machine_state(
    paths: &Paths,
    system: &dyn NativeSystemAdapter,
    current: &Path,
    generation: &str,
) -> Result<()> {
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if record.generation != generation {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall generation is invalid.",
        ));
    }
    if record.phase == "armed" {
        if path_present(&paths.transaction)? {
            recover_with_adapter(paths, system)?;
            require_uninstall_cleanup_complete(paths)?;
        }
        write_transaction(
            paths,
            "uninstall-app-path-retiring",
            Action::Uninstall,
            true,
        )?;
        system.unregister_app_path()?;
        write_transaction(paths, "uninstall-app-path-retired", Action::Uninstall, true)?;
        register_uninstall_executable(&paths.maintenance_uninstaller)?;
        let _ = current;
        // Keep the journal and registered maintenance command as recovery authority while the
        // service performs filesystem cleanup.
        write_transaction(paths, "uninstall-cleanup-complete", Action::Uninstall, true)?;
        write_terminal_uninstall_phase(paths, generation, "machine-retired")?;
    }
    let retired = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if retired.generation != generation || retired.phase != "machine-retired" {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall machine retirement is invalid.",
        ));
    }
    // The maintenance registration remains callable until it observes stopped-success and
    // retires the service and image.
    register_uninstall_executable(&paths.maintenance_uninstaller)
}
