//! Uninstall and interrupted transaction recovery.
use super::*;

pub(super) fn uninstall(
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

pub(super) fn finish_uninstall_machine_cleanup(
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

pub(super) fn require_uninstall_cleanup_complete(paths: &Paths) -> Result<()> {
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

pub(super) fn retire_and_remove_machine_lock(
    paths: &Paths,
    machine_lock: &mut Option<MachineLock>,
) -> Result<Option<LegacyMutexPair>> {
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    Ok(legacy)
}

pub(super) fn complete_terminal_uninstall(
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

pub(super) fn finalize_uninstall(
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

pub(super) fn retire_terminal_machine_state(
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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RecoveryPlan {
    RestorePredecessor,
    DiscardStaging,
    RemoveFreshCandidate,
    FinishCommit,
    FinishUninstall,
}

pub(super) fn recovery_plan(
    value: &Transaction,
    backup_exists: bool,
    install_exists: bool,
) -> Result<RecoveryPlan> {
    if !matches!(
        value.action.as_str(),
        "install" | "update" | "repair" | "uninstall"
    ) || (value.action == "update" && !value.had_predecessor)
        || (value.action == "repair" && !value.had_predecessor)
        || (value.action == "uninstall"
            && !matches!(
                value.phase.as_str(),
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
            ))
    {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction action is invalid.",
        ));
    }
    match value.phase.as_str() {
        "staging" | "staged" | "prepared" | "publishing"
            if !backup_exists && install_exists == value.had_predecessor =>
        {
            Ok(RecoveryPlan::DiscardStaging)
        }
        "prepared" | "publishing" | "published-before-persist"
            if !value.had_predecessor && !backup_exists && install_exists =>
        {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "staging"
        | "staged"
        | "prepared"
        | "predecessor-moved"
        | "publishing"
        | "published-before-persist"
        | "published"
        | "registered"
            if value.had_predecessor && backup_exists =>
        {
            Ok(RecoveryPlan::RestorePredecessor)
        }
        "published" | "registered"
            if !value.had_predecessor && !backup_exists && install_exists =>
        {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "committed" | "legacy-retiring" | "legacy-retired" if install_exists => {
            Ok(RecoveryPlan::FinishCommit)
        }
        "recovering-restore-predecessor"
            if value.had_predecessor && (backup_exists || install_exists) =>
        {
            Ok(RecoveryPlan::RestorePredecessor)
        }
        "recovering-discard-staging"
            if !backup_exists && install_exists == value.had_predecessor =>
        {
            Ok(RecoveryPlan::DiscardStaging)
        }
        "recovering-remove-fresh" if !value.had_predecessor && !backup_exists => {
            Ok(RecoveryPlan::RemoveFreshCandidate)
        }
        "recovering-finish-commit" if install_exists => Ok(RecoveryPlan::FinishCommit),
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
        | "uninstall-registration-retired" => Ok(RecoveryPlan::FinishUninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction topology is invalid.",
        )),
    }
}

pub(super) fn recover_with_adapter(paths: &Paths, system: &dyn NativeSystemAdapter) -> Result<()> {
    if !path_present(&paths.transaction)? {
        if path_present(&paths.backup)? && !path_present(&paths.install)? {
            durable_rename(&paths.backup, &paths.install)?;
        }
        remove_plain_tree(&paths.staging)?;
        return Ok(());
    }
    assert_plain_file(&paths.transaction)?;
    let value: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
    if value.schema_version != TRANSACTION_SCHEMA {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction schema is invalid.",
        ));
    }
    let plan = recovery_plan(
        &value,
        path_present(&paths.backup)?,
        path_present(&paths.install)?,
    )?;
    let finishing_uninstall = plan == RecoveryPlan::FinishUninstall;
    if finishing_uninstall && value.phase == "uninstall-cleanup-complete" {
        return Ok(());
    }
    let progress_phase = match plan {
        RecoveryPlan::RestorePredecessor => "recovering-restore-predecessor",
        RecoveryPlan::DiscardStaging => "recovering-discard-staging",
        RecoveryPlan::RemoveFreshCandidate => "recovering-remove-fresh",
        RecoveryPlan::FinishCommit => "recovering-finish-commit",
        RecoveryPlan::FinishUninstall => "recovering-finish-uninstall",
    };
    if value.phase != progress_phase {
        write_transaction(
            paths,
            progress_phase,
            transaction_action(&value)?,
            value.had_predecessor,
        )?;
    }
    match plan {
        RecoveryPlan::RestorePredecessor => {
            remove_plain_tree(&paths.staging)?;
            if path_present(&paths.backup)? {
                remove_plain_tree(&paths.install)?;
                durable_rename(&paths.backup, &paths.install)?;
            }
            if !path_present(&paths.install)? {
                return Err(fail(EXIT_REJECTED, "Recovered predecessor is missing."));
            }
            system.register_installed(paths)?;
        }
        RecoveryPlan::DiscardStaging => remove_plain_tree(&paths.staging)?,
        RecoveryPlan::RemoveFreshCandidate => {
            system.unregister_app_path()?;
            system.unregister_uninstall()?;
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.staging)?;
            remove_maintenance_uninstaller(paths)?;
            remove_update_recovery_launcher_residue(paths)?;
            remove_transaction(paths)?;
            system.clear_relaunch_owner(paths)?;
            return Ok(());
        }
        RecoveryPlan::FinishCommit => {
            if value.action == "repair" && path_present(&paths.backup)? {
                restore_repair_controller(paths)?;
            }
            system.register_installed(paths)?;
            system.retire_legacy(paths)?;
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.staging)?;
        }
        RecoveryPlan::FinishUninstall => finish_uninstall_machine_cleanup(paths, system)?,
    }
    if finishing_uninstall {
        Ok(())
    } else {
        remove_transaction(paths)
    }
}

#[cfg(test)]
pub(super) struct InjectedNativeSystem;

#[cfg(test)]
impl NativeSystemAdapter for InjectedNativeSystem {
    fn register_version(&self, _paths: &Paths, _version: &str) -> Result<()> {
        Ok(())
    }
    fn register_installed(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn unregister_app_path(&self) -> Result<()> {
        Ok(())
    }
    fn unregister_uninstall(&self) -> Result<()> {
        Ok(())
    }
    fn retire_legacy(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn clear_update_recovery(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
    fn clear_relaunch_owner(&self, _paths: &Paths) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn recover_with_system(paths: &Paths, update_system_state: bool) -> Result<()> {
    if update_system_state {
        recover_with_adapter(paths, &WindowsNativeSystem)
    } else {
        recover_with_adapter(paths, &InjectedNativeSystem)
    }
}
