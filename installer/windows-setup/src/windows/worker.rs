//! Elevated package validation and transaction dispatch.
use super::*;

pub(super) fn run_worker(_silent: bool, legacy_predecessor: bool) -> Result<i32> {
    let current = std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
    // A normal elevated worker proves its controller before touching attacker-sized package data.
    let authenticated_controller = if legacy_predecessor {
        None
    } else {
        Some(WorkerChannel::connect_and_authenticate(&current, None)?)
    };
    let result = (|| {
        let requested_action = authenticated_controller.as_ref().map(|value| value.0);
        #[cfg(feature = "stale-schema2-cleanup")]
        if requested_action == Some(Action::CleanStaleSchema2) {
            reclaim_exact_schema2_orphan(true, true)?;
            return Ok(0);
        }
        let mut image =
            File::open(&current).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let length = image
            .metadata()
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
            .len();
        let package = package::parse(&mut image, length).map_err(|error| {
            fail(
                EXIT_REJECTED,
                format!("TQPKG2 validation failed: {error:?}"),
            )
        })?;
        let expected_architecture = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "unsupported"
        };
        if package.manifest.architecture != expected_architecture {
            return Err(fail(
                EXIT_REJECTED,
                "TQPKG2 architecture does not match the native setup image.",
            ));
        }
        // Normal installation uses the retained machine lock and transaction
        // recovery below. The schema-two fixture cleanup is an explicit diagnostic
        // command, not a prerequisite for installing or repairing the application.
        let mut paths = paths()?;
        let predecessor_policy_epoch = if package.manifest.package_mode == "fresh"
            && matches!(requested_action, Some(Action::Install | Action::Repair))
        {
            // A user-launched, authenticated repair may recreate a missing lifecycle
            // lock. A downloaded update remains bound to its predecessor's policy.
            1
        } else {
            installed_recovery_policy_epoch(&paths)?
        };
        let mut machine_lock = Some(MachineLock::acquire(
            &paths,
            120_000,
            predecessor_policy_epoch,
        )?);
        let state_root = paths
            .transaction
            .parent()
            .ok_or_else(|| fail(EXIT_FAILURE, "Installer state root is invalid."))?;
        assert_plain_directory(state_root)?;
        cleanup_transaction_residue(state_root)?;
        let finishing_existing_uninstall = pending_uninstall_transaction(&paths)?;
        // A pending uninstall is already durable machine authority. Validate the signed package and
        // predecessor arguments, but defer installed-state predecessor authentication until recovery
        // only when a predecessor still exists.
        let predecessor_authorized = if legacy_predecessor {
            validate_predecessor_arguments(&package, &current)?;
            if finishing_existing_uninstall {
                false
            } else {
                authenticate_predecessor_helper(&package, &paths)?;
                true
            }
        } else {
            false
        };
        let uninstall_authorized = if requested_action == Some(Action::Uninstall) {
            authorize_uninstall_controller(&paths, &current)?;
            true
        } else {
            false
        };
        let system = WindowsNativeSystem;
        // A relocated controller must stop mapping the installed image before finish-uninstall
        // recovery can delete it. The existing protected journal is sufficient durable authority.
        if finishing_existing_uninstall
            && let Some((Action::Uninstall, server, _, lifecycle_parent)) =
                authenticated_controller.as_ref()
            && *lifecycle_parent != 0
        {
            arm_relocated_uninstall_controller(server)?;
        }
        // Authentication and package validation happen before recovery. Once the machine lock is held,
        // every installed entry point must finish durable recovery before arming or deriving a new action.
        if finishing_existing_uninstall {
            ensure_uninstall_finalizer_registered(&current, &paths)?;
        }
        recover_with_adapter(&paths, &system)?;
        recover_terminal_recovery_tombstones(&paths)?;
        if finishing_existing_uninstall {
            complete_terminal_uninstall(&paths, &system, &current, &mut machine_lock)?;
            if requested_action == Some(Action::Uninstall) {
                return Ok(0);
            }
            // Terminal retirement removes the predecessor and its generation. Rebuild paths so a
            // continued fresh install cannot recreate an image already owned by reboot deletion.
            paths = self::paths()?;
            machine_lock = Some(MachineLock::acquire(
                &paths,
                120_000,
                installed_recovery_policy_epoch(&paths)?,
            )?);
            let recovered_action = derive_action(&current, &paths)?;
            if requested_action.is_some_and(|requested| {
                !recovered_action_is_authorized(
                    requested,
                    recovered_action,
                    &package.manifest.package_mode,
                )
            }) {
                return Err(fail(
                    EXIT_REJECTED,
                    "Recovered machine state does not match the authenticated setup request.",
                ));
            }
        }
        if uninstall_authorized
            && !path_present(&paths.transaction)?
            && !path_present(&paths.install)?
        {
            // Journal-absent recovery must honor the durable terminal owner before touching its Run
            // target. The exact maintenance image finishes residue, retires Run, then self-unlinks.
            if let Some(record) = read_terminal_uninstall_record(&paths)? {
                let maintenance_image =
                    canonical(&current)? == canonical(&paths.maintenance_uninstaller)?;
                let authenticated_relocated_image = authenticated_controller.is_some()
                    && relocated_uninstall_matches_maintenance(&current, &paths)?;
                if (!maintenance_image && !authenticated_relocated_image)
                    || hex_hash(&file_hash(&current)?) != record.maintenance_sha256
                {
                    return Err(fail(EXIT_REJECTED, "Terminal uninstall owner is invalid."));
                }
                if matches!(record.phase.as_str(), "armed" | "machine-retired") {
                    resume_terminal_service(&paths, &record)?;
                }
                drop(machine_lock.take());
                wait_for_terminal_service_retirement(&paths, &record.generation)?;
                return Ok(0);
            }
            system.unregister_app_path()?;
            system.unregister_uninstall()?;
            clear_update_recovery(&paths)?;
            clear_legacy_profile_relaunch_owners(&paths)?;
            remove_update_recovery_launcher_residue(&paths)?;
            remove_uninstall_finalizer_residue(&paths)?;
            let legacy = retire_and_remove_machine_lock(&paths, &mut machine_lock)?;
            arm_mapped_image_deletion(&current)?;
            drop(legacy);
            clear_machine_relaunch_owner(&paths)?;
            return Ok(0);
        }
        if let Some((Action::Uninstall, server, _, lifecycle_parent)) =
            authenticated_controller.as_ref()
            && *lifecycle_parent != 0
        {
            write_transaction(&paths, "uninstall-armed", Action::Uninstall, true)?;
            arm_relocated_uninstall_controller(server)?;
        }
        let mut action = if uninstall_authorized {
            Action::Uninstall
        } else {
            let derived = derive_action(&current, &paths)?;
            if requested_action.is_some_and(|requested| {
                !recovered_action_is_authorized(requested, derived, &package.manifest.package_mode)
            }) {
                return Err(fail(
                    EXIT_REJECTED,
                    "Recovered machine state does not match the authenticated setup request.",
                ));
            }
            derived
        };
        if action != Action::Uninstall {
            action =
                authorize_package_mode(&package, &paths, &current, predecessor_authorized, action)?;
        }
        if action != Action::Install || paths.install.exists() {
            request_runtime_exit(
                &paths,
                authenticated_controller
                    .as_ref()
                    .and_then(|value| (value.3 != 0).then_some(value.3)),
            )?;
        }
        match action {
            Action::Install | Action::Update | Action::Repair => {
                install(&mut image, &package, &current, &paths, action, &system)
            }
            Action::Uninstall => {
                write_transaction(&paths, "uninstalling", Action::Uninstall, true)?;
                let original_controller = authenticated_controller
                    .as_ref()
                    .ok_or_else(|| {
                        fail(
                            EXIT_REJECTED,
                            "Uninstall controller authentication is missing.",
                        )
                    })?
                    .3;
                uninstall(&paths, &system, original_controller != 0, &mut machine_lock)
            }
            #[cfg(feature = "stale-schema2-cleanup")]
            Action::CleanStaleSchema2 => {
                return Err(fail(
                    EXIT_REJECTED,
                    "Cleanup reached the installer transaction path.",
                ));
            }
        }?;
        if action == Action::Uninstall {
            complete_terminal_uninstall(&paths, &system, &current, &mut machine_lock)?;
        }
        Ok(0)
    })();
    if let Some((_, channel, _, _)) = authenticated_controller.as_ref() {
        pipe_write(
            channel.as_raw_handle(),
            &encode_completion(&result),
            None,
            Instant::now() + Duration::from_secs(5),
        )?;
        // The controller receives the precise error and owns the only dialog.
        return Ok(result.unwrap_or_else(|error| error.code));
    }
    result
}

pub(super) trait NativeSystemAdapter {
    fn register_version(&self, paths: &Paths, version: &str) -> Result<()>;
    fn register_installed(&self, paths: &Paths) -> Result<()>;
    fn unregister_app_path(&self) -> Result<()>;
    fn unregister_uninstall(&self) -> Result<()>;
    fn retire_legacy(&self, paths: &Paths) -> Result<()>;
    fn clear_update_recovery(&self, paths: &Paths) -> Result<()>;
    fn clear_relaunch_owner(&self, paths: &Paths) -> Result<()>;
}

pub(super) struct WindowsNativeSystem;

impl NativeSystemAdapter for WindowsNativeSystem {
    fn register_version(&self, paths: &Paths, version: &str) -> Result<()> {
        register_uninstall(paths, version)?;
        register_app_path(paths)
    }
    fn register_installed(&self, paths: &Paths) -> Result<()> {
        register_installed_uninstall(paths)
    }
    fn unregister_app_path(&self) -> Result<()> {
        unregister_app_path()
    }
    fn unregister_uninstall(&self) -> Result<()> {
        unregister_uninstall()
    }
    fn retire_legacy(&self, paths: &Paths) -> Result<()> {
        retire_legacy_authority(paths)
    }
    fn clear_update_recovery(&self, paths: &Paths) -> Result<()> {
        clear_update_recovery(paths)
    }
    fn clear_relaunch_owner(&self, paths: &Paths) -> Result<()> {
        clear_machine_relaunch_owner(paths)
    }
}

pub(super) fn installed_recovery_policy_epoch(paths: &Paths) -> Result<u8> {
    let helper = paths
        .install
        .join("resources/helper/talking-quill-helper.exe");
    if !path_present(&helper)? {
        return Ok(1);
    }
    assert_plain_file(&helper)?;
    let bytes = fs::read(helper).map_err(io_failure)?;
    const PREFIX: &[u8] = b"TALKING_QUILL_WINDOWS_RECOVERY_POLICY_EPOCH=";
    let matches = bytes
        .windows(PREFIX.len() + 1)
        .filter_map(|window| {
            window
                .strip_prefix(PREFIX)
                .map(|value| value[0])
                .filter(u8::is_ascii_digit)
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Ok(1);
    }
    if matches.iter().any(|value| *value != matches[0]) {
        return Err(fail(
            EXIT_REJECTED,
            "Installed recovery policy epoch is invalid.",
        ));
    }
    Ok(matches[0] - b'0')
}

fn recovered_action_is_authorized(
    requested: Action,
    recovered: Action,
    package_mode: &str,
) -> bool {
    requested == recovered
        || (package_mode == "fresh"
            && matches!(requested, Action::Install | Action::Repair)
            && matches!(recovered, Action::Install | Action::Repair))
}

#[cfg(test)]
mod action_tests;
