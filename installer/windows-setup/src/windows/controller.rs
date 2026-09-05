//! Normal-user setup entry, image retention, relocation, and UAC launch.
use super::*;

mod image;
pub(super) use image::*;

mod launch;
pub(super) use launch::*;

pub(super) fn run_inner() -> Result<i32> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_diagnostic_arguments(&arguments) {
        return run_direct_stale_schema2_diagnostic(&arguments)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    let elevated = token_is_elevated()?;
    let relocated = !elevated
        && arguments.iter().any(|value| value == "/TQ-RELOCATED")
        && arguments
            .iter()
            .all(|value| value == "/TQ-RELOCATED" || value == "/S");
    let legacy_predecessor = elevated && legacy_predecessor_arguments(&arguments);
    #[cfg(feature = "stale-schema2-cleanup")]
    let cleanup_requested =
        !elevated && arguments.len() == 1 && arguments[0] == "/TQ-CLEAN-STALE-SCHEMA2";
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let cleanup_requested = false;
    #[cfg(feature = "stale-schema2-cleanup")]
    let direct_cleanup_requested = direct_cleanup_arguments(&arguments, elevated);
    #[cfg(not(feature = "stale-schema2-cleanup"))]
    let direct_cleanup_requested = false;
    if !((arguments.is_empty() || (arguments.len() == 1 && arguments[0] == "/S"))
        || legacy_predecessor
        || relocated
        || cleanup_requested
        || direct_cleanup_requested)
    {
        return Err(fail(EXIT_USAGE, "The native setup accepts only /S."));
    }
    let mut silent = arguments.first().is_some_and(|value| value == "/S") || cleanup_requested;
    #[cfg(feature = "stale-schema2-cleanup")]
    if direct_cleanup_requested {
        return run_direct_elevated_stale_schema2_cleanup()
            .map(|()| 0)
            .map_err(|error| fail(EXIT_REJECTED, error.message));
    }
    if !elevated {
        let current =
            std::env::current_exe().map_err(|error| fail(EXIT_FAILURE, error.to_string()))?;
        #[cfg(feature = "stale-schema2-cleanup")]
        if cleanup_requested {
            let retained = retain_controller_image(&current, false)?;
            let channel =
                ControllerChannel::create(Action::CleanStaleSchema2, true, std::process::id())?;
            let result = elevate(&current, true, &channel, None);
            drop(retained);
            return result;
        }
        let controller_paths = paths()?;
        let retained = retain_controller_image(&current, relocated)?;
        let mut lifecycle_parent = 0;
        let mut relocation_server = None;
        let mut relocated_finalizer = false;
        let mut relocated_identity_guard = None;
        let action = if relocated {
            let original = process_image(parent_process_id()?)?;
            let installed = controller_paths.install.join("Uninstall Talking Quill.exe");
            let expected =
                if canonical(&original)? == canonical(&controller_paths.maintenance_uninstaller)? {
                    controller_paths.maintenance_uninstaller.clone()
                } else if canonical(&original)? == canonical(&installed)? {
                    installed
                } else if is_uninstall_finalizer(&original)? {
                    relocated_finalizer = true;
                    original.clone()
                } else {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Relocated uninstall source is not an authenticated maintenance image.",
                    ));
                };
            relocated_identity_guard = Some(validate_relocated_uninstall_image(
                &current,
                &original,
                &expected,
                &controller_paths.maintenance_uninstaller,
            )?);
            let (action, server, requested_silent, requested_lifecycle_parent) =
                WorkerChannel::connect_and_authenticate(&current, Some(&expected))?;
            silent = requested_silent;
            lifecycle_parent = requested_lifecycle_parent;
            relocation_server = Some(server);
            if action != Action::Uninstall {
                return Err(fail(
                    EXIT_REJECTED,
                    "Relocation requested an invalid operation.",
                ));
            }
            Action::Uninstall
        } else {
            derive_action(&current, &controller_paths)?
        };
        if action == Action::Uninstall && !relocated {
            let _ = request_runtime_exit(&controller_paths, None);
            let (path, lock) = create_relocated_image(&current)?;
            let channel = ControllerChannel::create(Action::Uninstall, silent, std::process::id())?;
            let process = launch_relocated(&path, &channel)?;
            drop(retained);
            let code = channel.wait_relocated_status(&process, &current)?;
            drop(lock);
            return Ok(code);
        }
        if !silent && !confirm_controller(action)? {
            return Ok(ERROR_CANCELLED as i32);
        }
        let delete_profile = !silent
            && action == Action::Uninstall
            && message_box(
                "Also delete your Talking Quill profile?",
                MB_YESNO | MB_ICONQUESTION,
            ) == IDYES;
        if action != Action::Install {
            // The elevated worker retries cleanup and can close administrator processes.
            let _ = request_runtime_exit(
                &controller_paths,
                (lifecycle_parent != 0).then_some(lifecycle_parent),
            );
        }
        let retained = if relocated {
            drop(retained);
            retain_controller_image(&current, true)?
        } else {
            retained
        };
        let arm_deletion = || -> Result<()> {
            let server = relocation_server.as_ref().ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                b"TQ-ARM-DELETE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
            if pipe_read::<1>(
                server.as_raw_handle(),
                None,
                Instant::now() + Duration::from_secs(30),
            )? != [1]
            {
                return Err(fail(
                    EXIT_FAILURE,
                    "Installed uninstall image deletion was not armed.",
                ));
            }
            Ok(())
        };
        let before_accept = (relocated && lifecycle_parent != 0 && !relocated_finalizer)
            .then_some(&arm_deletion as &dyn Fn() -> Result<()>);
        if relocated_finalizer {
            pipe_write(
                relocation_server
                    .as_ref()
                    .ok_or_else(|| fail(EXIT_FAILURE, "Finalizer relocation channel is missing."))?
                    .as_raw_handle(),
                b"TQ-KEEP-IMAGE",
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        let channel = ControllerChannel::create(action, silent, lifecycle_parent)?;
        let result = elevate(&current, silent, &channel, before_accept);
        drop(relocated_identity_guard);
        drop(retained);
        if relocated && lifecycle_parent != 0 {
            let status = result.as_ref().copied().unwrap_or_else(|error| error.code);
            let server = relocation_server.ok_or_else(|| {
                fail(
                    EXIT_FAILURE,
                    "Relocated uninstall status channel is missing.",
                )
            })?;
            pipe_write(
                server.as_raw_handle(),
                &status.to_le_bytes(),
                None,
                Instant::now() + Duration::from_secs(30),
            )?;
        }
        if result.as_ref().is_ok_and(|code| *code == 0) && delete_profile {
            remove_plain_tree(&controller_paths.profile)?;
        }
        return result;
    }
    run_worker(silent, legacy_predecessor)
}

pub(super) fn legacy_predecessor_arguments(arguments: &[OsString]) -> bool {
    if arguments.len() != 5 || arguments[0] != "/S" {
        return false;
    }
    [
        "/TQUPDATE=",
        "/TQGATEWAYHASH=",
        "/TQOWNERHASH=",
        "/TQLAYOUT=",
    ]
    .iter()
    .all(|prefix| {
        arguments
            .iter()
            .skip(1)
            .any(|value| value.to_string_lossy().starts_with(prefix))
    })
}

pub(super) fn confirm_controller(action: Action) -> Result<bool> {
    let text = match action {
        Action::Install => "Install Talking Quill for all users?",
        Action::Update => "Update Talking Quill for all users?",
        Action::Repair => "Repair Talking Quill for all users?",
        Action::Uninstall => {
            "Uninstall Talking Quill?\n\nYour profile is preserved unless you delete it in the application first."
        }
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => "Remove the exact stale schema-2 test residue?",
    };
    let text = format!(
        "{text}\n\nVersion {} for 64-bit Windows.\nWindows will ask for administrator approval to continue.",
        env!("CARGO_PKG_VERSION")
    );
    let response = message_box(&text, MB_OKCANCEL | MB_ICONQUESTION);
    Ok(response == IDOK)
}
