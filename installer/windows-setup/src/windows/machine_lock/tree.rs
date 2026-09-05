//! Machine lock tree initialization, verification, and reclamation.
use super::*;

pub(in super::super) fn initialize_machine_lock_tree(
    directory: &Path,
    directory_identity: &str,
) -> Result<()> {
    let lock = directory.join("recovery-state-v1.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&lock)
        .map_err(io_failure)?;
    apply_lock_dacl(&lock, machine_lock_file_sddl())?;
    file.sync_all().map_err(io_failure)?;
    let identity = file_identity_text(&file)?;
    drop(file);
    create_or_verify_lock_marker(&lock.with_extension("identity-v1"), &identity)?;
    create_or_verify_lock_marker(&directory.join("lock-tree-identity-v1"), directory_identity)
}

pub(in super::super) fn verify_machine_lock_tree(directory: &Path) -> Result<PathBuf> {
    if !marker_security_is_exact(directory, machine_lock_directory_sddl())? {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory is not protected.",
        ));
    }
    let identity = owned_tree_identity(directory).map_err(|_| {
        fail(
            EXIT_REJECTED,
            "Machine lock directory identity is unavailable.",
        )
    })?;
    if fs::read_to_string(directory.join("lock-tree-identity-v1")).map_err(io_failure)? != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Machine lock directory identity changed.",
        ));
    }
    let lock = directory.join("recovery-state-v1.lock");
    if !marker_security_is_exact(&lock, machine_lock_file_sddl())? {
        return Err(fail(EXIT_REJECTED, "Machine lock file is not protected."));
    }
    Ok(lock)
}

pub(in super::super) fn reclaim_retired_machine_lock_directory(
    program_data: &Path,
    suffix: &str,
) -> Result<()> {
    let path = program_data.join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path_present(&path)? {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity =
        owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match remove_owned_tree(&path, &identity) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(fail(EXIT_FAILURE, error.to_string())),
        }
    }
}

pub(in super::super) fn reclaim_unpublished_machine_lock_directories(
    program_data: &Path,
) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|name| name.strip_prefix(MACHINE_LOCK_DIRECTORY_PREFIX))
        else {
            continue;
        };
        if validate_machine_lock_suffix(suffix).is_err() {
            continue;
        }
        let path = entry.path();
        if !marker_security_is_exact(&path, machine_lock_directory_sddl())? {
            continue;
        }
        let identity =
            owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        let marker = path.join("publication-pending-v1");
        let expected = format!("{suffix}:{identity}");
        if fs::read_to_string(marker).is_ok_and(|value| value == expected) {
            remove_owned_tree(&path, &identity)
                .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        }
    }
    Ok(())
}

pub(in super::super) fn reclaim_machine_lock_pending(program_data: &Path) -> Result<()> {
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        if !name
            .to_str()
            .and_then(|value| value.strip_prefix(MACHINE_LOCK_PENDING_PREFIX))
            .is_some_and(|suffix| validate_machine_lock_suffix(suffix).is_ok())
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_failure)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !marker_security_is_exact(&path, machine_lock_directory_sddl())?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Pending machine lock identity is unavailable.",
            )
        })?;
        remove_owned_tree(&path, &identity)
            .map_err(|_| fail(EXIT_FAILURE, "Cannot reclaim pending machine lock state."))?;
    }
    Ok(())
}
