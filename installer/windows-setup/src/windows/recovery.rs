//! Uninstall and interrupted transaction recovery.
use super::*;

mod uninstall;
pub(super) use uninstall::*;

#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(super) use test_support::*;

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
