//! Durable uninstall authority and finalizer image checks.
use super::*;

pub(in super::super) fn pending_uninstall_transaction(paths: &Paths) -> Result<bool> {
    if !path_present(&paths.transaction)? {
        return Ok(false);
    }
    assert_plain_file(&paths.transaction)?;
    let transaction: Transaction =
        serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
    if transaction.schema_version != TRANSACTION_SCHEMA {
        return Err(fail(
            EXIT_REJECTED,
            "Installer transaction schema is invalid.",
        ));
    }
    Ok(transaction.action == "uninstall"
        && matches!(
            transaction.phase.as_str(),
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
}

pub(in super::super) fn arm_relocated_uninstall_controller(server: &OwnedHandle) -> Result<()> {
    pipe_write(
        server.as_raw_handle(),
        b"TQ-UNINSTALL-JOURNALED",
        None,
        Instant::now() + Duration::from_secs(30),
    )?;
    if pipe_read::<18>(
        server.as_raw_handle(),
        None,
        Instant::now() + Duration::from_secs(30),
    )? != *b"TQ-UNINSTALL-ARMED"
    {
        return Err(fail(
            EXIT_REJECTED,
            "Uninstall controller did not commit mapped-image deletion ownership.",
        ));
    }
    Ok(())
}

pub(in super::super) fn authorize_uninstall_controller(
    paths: &Paths,
    current: &Path,
) -> Result<()> {
    let controller = process_image(parent_process_id()?)?;
    let installed = paths.install.join("Uninstall Talking Quill.exe");
    let expected = if is_uninstall_finalizer(&controller)? {
        controller.clone()
    } else if path_present(&installed)? {
        installed
    } else if path_present(&paths.maintenance_uninstaller)? {
        paths.maintenance_uninstaller.clone()
    } else {
        assert_plain_file(&paths.transaction)?;
        let transaction: Transaction =
            serde_json::from_slice(&fs::read(&paths.transaction).map_err(io_failure)?)
                .map_err(|_| fail(EXIT_REJECTED, "Installer transaction is invalid."))?;
        if transaction.schema_version != TRANSACTION_SCHEMA
            || transaction.action != "uninstall"
            || !matches!(
                transaction.phase.as_str(),
                "uninstall-cleanup-owned"
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
            )
        {
            return Err(fail(
                EXIT_REJECTED,
                "Uninstall cleanup lacks a protected durable authorization.",
            ));
        }
        current.to_owned()
    };
    assert_plain_file(&expected)?;
    if file_hash(&controller)? != file_hash(&expected)? {
        return Err(fail(
            EXIT_REJECTED,
            "Uninstall requires an authenticated exact copy of the installed controller image.",
        ));
    }
    Ok(())
}

pub(in super::super) fn is_uninstall_finalizer(path: &Path) -> Result<bool> {
    if !path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(UNINSTALL_FINALIZER_NAME))
    {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    if !parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.strip_prefix(UNINSTALL_FINALIZER_PREFIX)
                .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
        })
        || !medium_launcher_directory_is_protected(parent)?
    {
        return Ok(false);
    }
    let identity =
        owned_tree_identity(parent).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let marker = parent.join("finalizer-tree-identity-v1");
    Ok(
        marker_security_is_exact(&marker, MEDIUM_FINALIZER_FILE_SDDL)?
            && marker_security_is_exact(path, MEDIUM_FINALIZER_FILE_SDDL)?
            && fs::read_to_string(marker).is_ok_and(|value| value == identity),
    )
}
