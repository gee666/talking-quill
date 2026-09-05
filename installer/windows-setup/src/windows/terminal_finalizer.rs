//! Finalize uninstall while retaining crash recovery evidence.
use super::*;

mod tombstones;
pub(super) use tombstones::*;

mod relaunch;
pub(super) use relaunch::*;

mod residue;
pub(super) use residue::*;

pub(super) fn finish_terminal_uninstall(paths: &Paths) -> Result<()> {
    let record = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?;
    if !matches!(
        record.phase.as_str(),
        "cleanup-complete"
            | "final-launcher-owned"
            | "maintenance-deletion-owned"
            | "maintenance-deleted"
            | "uninstall-unregistered"
            | "journal-removed"
    ) {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal uninstall cleanup is not complete.",
        ));
    }
    if path_present(&paths.transaction)? {
        require_uninstall_cleanup_complete(paths)?;
    }
    if !registry_key_absent(&format!(
        r"SYSTEM\CurrentControlSet\Services\{}",
        record.service_name
    ))? || path_present(Path::new(&record.service_image))?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal service retirement is incomplete.",
        ));
    }
    let mut machine_lock = Some(MachineLock::acquire(
        paths,
        120_000,
        installed_recovery_policy_epoch(paths)?,
    )?);
    clear_update_recovery(paths)?;
    clear_legacy_profile_relaunch_owners(paths)?;
    remove_uninstall_finalizer_residue(paths)?;
    let root = terminal_uninstall_root(paths);
    remove_plain_tree(&root.join("Relaunch Records"))?;
    require_machine_relaunch_owner(paths)?;
    if record.phase == "cleanup-complete" {
        publish_terminal_final_launcher(paths, &record.generation)?;
        write_terminal_uninstall_phase(paths, &record.generation, "final-launcher-owned")?;
        terminal_maintenance_crash_at("post-final-launcher-ownership");
    }
    let published_phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if published_phase == "final-launcher-owned" {
        terminal_maintenance_crash_at("pre-maintenance-deletion-ownership");
        if hex_hash(&file_hash(&paths.maintenance_uninstaller)?) != record.maintenance_sha256 {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance image is invalid.",
            ));
        }
        schedule_terminal_service_deletion(&paths.maintenance_uninstaller)?;
        write_terminal_uninstall_phase(paths, &record.generation, "maintenance-deletion-owned")?;
        terminal_maintenance_crash_at("post-maintenance-deletion-ownership");
    }
    let mut phase = read_terminal_uninstall_record(paths)?
        .ok_or_else(|| fail(EXIT_REJECTED, "Terminal uninstall owner is missing."))?
        .phase;
    if phase == "maintenance-deletion-owned" {
        if !path_present(&paths.maintenance_uninstaller)?
            || !pending_deletion_is_owned(&paths.maintenance_uninstaller)?
        {
            return Err(fail(
                EXIT_REJECTED,
                "Terminal maintenance deletion ownership is invalid.",
            ));
        }
        publish_terminal_final_launcher(paths, &record.generation)?;
        unregister_uninstall()?;
        write_terminal_uninstall_phase(paths, &record.generation, "uninstall-unregistered")?;
        terminal_maintenance_crash_at("post-uninstall-unregister");
        phase = "uninstall-unregistered".into();
    }
    if phase == "uninstall-unregistered" {
        remove_transaction(paths)?;
        write_terminal_uninstall_phase(paths, &record.generation, "journal-removed")?;
        terminal_maintenance_crash_at("post-journal-removal");
        phase = "journal-removed".into();
    }
    if phase != "journal-removed" {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal maintenance phase is invalid.",
        ));
    }
    let identity =
        owned_tree_identity(&root).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&root)?
        || !fs::read_to_string(root.join("launcher-tree-identity-v1"))
            .is_ok_and(|value| value == identity)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery root identity is invalid.",
        ));
    }
    let tombstone = terminal_recovery_tombstone(paths, &record.generation)?;
    if path_present(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal recovery tombstone already exists.",
        ));
    }
    fs::rename(&root, &tombstone).map_err(io_failure)?;
    flush_setup_directory(&paths.program_data)?;
    terminal_maintenance_crash_at("post-root-tombstone-rename");
    remove_terminal_recovery_tombstone(paths, &tombstone)?;
    if path_present(&paths.maintenance_uninstaller)? {
        arm_mapped_image_deletion(&paths.maintenance_uninstaller)?;
    }
    terminal_maintenance_crash_at("post-maintenance-posix-delete");
    let final_launcher = terminal_final_launcher(paths, &record.generation)?;
    if !pending_deletion_is_owned(&final_launcher)? {
        schedule_terminal_service_deletion(&final_launcher)?;
    }
    if !pending_deletion_is_owned(&tombstone)? {
        schedule_empty_terminal_tombstone_deletion(&tombstone)?;
    }
    if !pending_deletion_is_owned(&final_launcher)? || !pending_deletion_is_owned(&tombstone)? {
        return Err(fail(
            EXIT_REJECTED,
            "Terminal final deletion ownership is invalid.",
        ));
    }
    terminal_maintenance_crash_at("post-final-deletion-ownership");
    terminal_maintenance_crash_at("post-final-launcher-posix-delete");
    let legacy = machine_lock.as_mut().and_then(MachineLock::take_legacy);
    let suffix = retire_machine_lock_publication(paths)?;
    drop(machine_lock.take());
    remove_machine_lock_residue(paths, &suffix)?;
    drop(legacy);
    terminal_maintenance_crash_at("pre-machine-relaunch-owner-clear");
    clear_machine_relaunch_owner(paths)?;
    terminal_maintenance_crash_at("post-machine-relaunch-owner-clear");
    if path_present(&final_launcher)? {
        arm_mapped_image_deletion(&final_launcher)?;
    }
    if path_present(&tombstone)? {
        fs::remove_dir(&tombstone).map_err(io_failure)?;
        flush_setup_directory(&paths.program_data)?;
    }
    terminal_maintenance_crash_at("post-owner-clear-posix-cleanup");
    Ok(())
}
