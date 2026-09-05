//! Protected uninstall finalizer publication and registration.
use super::*;

pub(in super::super) fn ensure_uninstall_finalizer_registered(
    current: &Path,
    paths: &Paths,
) -> Result<PathBuf> {
    if let Some(existing) = registered_uninstall_executable()?
        && existing
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(UNINSTALL_FINALIZER_NAME))
        && path_present(&existing)?
        && is_uninstall_finalizer(&existing)?
        && marker_security_is_exact(&existing, MEDIUM_FINALIZER_FILE_SDDL)?
        && file_hash(&existing)? == file_hash(current)?
    {
        return Ok(existing);
    }
    write_transaction(
        paths,
        "uninstall-finalizer-publishing",
        Action::Uninstall,
        true,
    )?;
    let token = new_machine_lock_suffix()?;
    let suffix = new_machine_lock_suffix()?;
    let pending = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PENDING_PREFIX}{token}"));
    let published = paths
        .program_data
        .join(format!("{UNINSTALL_FINALIZER_PREFIX}{suffix}"));
    create_directory_with_security(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    apply_lock_dacl(&pending, MEDIUM_FINALIZER_DIRECTORY_SDDL)?;
    let identity =
        owned_tree_identity(&pending).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    create_or_verify_finalizer_marker(&pending.join("finalizer-tree-identity-v1"), &identity)?;
    let executable = pending.join(UNINSTALL_FINALIZER_NAME);
    let mut source = File::open(current).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&executable)
        .map_err(io_failure)?;
    apply_lock_dacl(&executable, MEDIUM_FINALIZER_FILE_SDDL)?;
    std::io::copy(&mut source, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    if file_hash(current)? != file_hash(&executable)? {
        return Err(fail(EXIT_REJECTED, "Uninstall finalizer copy changed."));
    }
    flush_setup_directory(&pending)?;
    if unsafe {
        MoveFileExW(
            wide(pending.as_os_str()).as_ptr(),
            wide(published.as_os_str()).as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot publish the uninstall finalizer.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    let executable = published.join(UNINSTALL_FINALIZER_NAME);
    register_uninstall_executable(&executable)?;
    write_transaction(
        paths,
        "uninstall-finalizer-published",
        Action::Uninstall,
        true,
    )?;
    Ok(executable)
}

pub(in super::super) fn create_or_verify_finalizer_marker(
    path: &Path,
    identity: &str,
) -> Result<()> {
    create_atomic_marker(path, identity, MEDIUM_FINALIZER_FILE_SDDL)
}
