#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::Path;

fn main() {
    talking_quill_helper::retain_source_identity();
    let os_arguments: Vec<std::ffi::OsString> = std::env::args_os().collect();
    #[cfg(all(windows, feature = "windows-installed-acceptance"))]
    if os_arguments.get(1).is_some_and(|value| {
        value == "--windows-installed-acceptance-launch-v1"
            || value == "--windows-installed-acceptance-process-guard-v1"
            || value == "--windows-installed-acceptance-broker-v1"
    }) {
        std::process::exit(talking_quill_helper::windows_acceptance_launcher::run(
            &os_arguments[1..],
        ));
    }
    #[cfg(windows)]
    if os_arguments.len() == 2
        && os_arguments
            .get(1)
            .is_some_and(|value| value == "--windows-helper-harness-v1")
    {
        if let Err(error) = talking_quill_helper::windows_harness::run_serialized() {
            talking_quill_helper::report_run_error(&error);
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if os_arguments.get(1).is_some_and(|value| {
        value
            .to_string_lossy()
            .starts_with("--windows-installer-lifecycle-v1=")
    }) {
        std::process::exit(talking_quill_helper::windows_installer::run(
            &os_arguments[1..],
        ));
    }
    #[cfg(windows)]
    if os_arguments.len() == 2
        && os_arguments.get(1).is_some_and(|value| {
            value
                .to_string_lossy()
                .starts_with("--windows-update-bootstrap-v2=")
                || value
                    .to_string_lossy()
                    .starts_with("--windows-update-bootstrap-staged-v2=")
                || value
                    .to_string_lossy()
                    .starts_with("--windows-update-cleanup-v1=")
        })
    {
        std::process::exit(talking_quill_helper::windows_update::run_from_argument(
            &os_arguments[1],
        ));
    }
    #[cfg(target_os = "macos")]
    if os_arguments
        .get(1)
        .is_some_and(|value| value == "--macos-owner-validate-install")
    {
        let valid = os_arguments.len() == 2
            && talking_quill_helper::owner::macos::validate_installed_owner();
        std::process::exit(if valid { 0 } else { 78 });
    }
    #[cfg(target_os = "macos")]
    if os_arguments
        .get(1)
        .is_some_and(|value| value == "--macos-owner-resume-cleanup")
    {
        let result = (os_arguments.len() == 2)
            .then_some(())
            .ok_or(())
            .and_then(|()| {
                talking_quill_helper::owner::macos::resume_committed_uninstall_cleanup()
                    .map_err(|_| ())
            });
        std::process::exit(if result.is_ok() { 0 } else { 76 });
    }
    #[cfg(target_os = "macos")]
    if os_arguments
        .get(1)
        .is_some_and(|value| value == "--macos-owner-probe")
    {
        let result = os_arguments
            .get(2)
            .and_then(|value| value.to_str())
            .filter(|_| os_arguments.len() == 3)
            .ok_or(())
            .and_then(|target| {
                talking_quill_helper::owner::macos::probe_installed_target(target).map_err(|_| ())
            });
        std::process::exit(if result.is_ok() { 0 } else { 77 });
    }
    #[cfg(target_os = "macos")]
    if os_arguments
        .get(1)
        .is_some_and(|value| value == "--macos-owner-finalize")
    {
        let arguments = os_arguments[2..]
            .iter()
            .map(|value| value.clone().into_string())
            .collect::<Result<Vec<_>, _>>();
        let result = arguments.map_err(|_| ()).and_then(|values| {
            talking_quill_helper::owner::macos::run_finalizer(&values).map_err(|_| ())
        });
        std::process::exit(if result.is_ok() { 0 } else { 70 });
    }
    #[cfg(target_os = "macos")]
    if os_arguments
        .get(1)
        .is_some_and(|value| value == "--owner-maintenance")
    {
        let arguments = os_arguments[2..]
            .iter()
            .map(|value| value.clone().into_string())
            .collect::<Result<Vec<_>, _>>();
        let Ok(arguments) = arguments else {
            std::process::exit(
                talking_quill_helper::owner::maintenance_cli::MaintenanceCliExit::Usage.code(),
            );
        };
        if arguments.iter().any(|value| !value.is_ascii()) {
            std::process::exit(
                talking_quill_helper::owner::maintenance_cli::MaintenanceCliExit::Usage.code(),
            );
        }
        let mut connector =
            talking_quill_helper::owner::platform_client::ProductionOwnerConnector::default();
        let exit = talking_quill_helper::owner::maintenance_cli::run_maintenance_only_with_verifier(
            &mut connector,
            &talking_quill_helper::owner::macos::MacosOwnerExitVerifier::default(),
            &std::sync::atomic::AtomicBool::new(false),
            &arguments,
        );
        std::process::exit(exit.code());
    }
    let arguments = match os_arguments
        .into_iter()
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(_) => {
            talking_quill_helper::report_run_error("helper arguments must be Unicode");
            std::process::exit(1);
        }
    };
    let result = if arguments.get(1).map(String::as_str) == Some("--remove-owned-tree") {
        match (arguments.get(2), arguments.get(3), arguments.len()) {
            (Some(path), Some(identity), 4) => {
                talking_quill_helper::owned_tree::remove_owned_tree(Path::new(path), identity)
                    .map_err(|error| error.to_string())
            }
            _ => Err("expected --remove-owned-tree <path> <device:inode>".to_owned()),
        }
    } else if arguments.len() == 1 {
        if let Err(error) = talking_quill_helper::run() {
            talking_quill_helper::report_run_error(&error.to_string());
            std::process::exit(1);
        }
        Ok(())
    } else {
        Err("unknown talking-quill-helper arguments".to_owned())
    };
    if let Err(error) = result {
        talking_quill_helper::report_run_error(&error);
        std::process::exit(1);
    }
}
