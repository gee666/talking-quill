//! Finalizer deletion ownership, launcher residue, and image matching.
use super::*;

pub(in super::super) fn pending_deletion_is_owned(path: &Path) -> Result<bool> {
    let expected = canonical(path)?;
    let key = open_session_manager(KEY_READ)?;
    let pairs = read_pending_rename_pairs(key)?;
    unsafe { RegCloseKey(key) };
    Ok(pairs.iter().any(|(source, destination)| {
        destination.is_empty() && normalized_pending_source(source) == expected
    }))
}

pub(in super::super) fn remove_update_recovery_launcher_residue(paths: &Paths) -> Result<()> {
    let launcher = paths.program_data.join("Talking Quill Update Recovery");
    if !path_present(&launcher)? {
        return Ok(());
    }
    let identity =
        owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    if !medium_launcher_directory_is_protected(&launcher)? {
        return Err(fail(
            EXIT_REJECTED,
            "Update recovery launcher identity is invalid.",
        ));
    }
    let marker = launcher.join("launcher-tree-identity-v1");
    if path_present(&marker)? {
        verify_atomic_marker(&marker, &identity, MEDIUM_LAUNCHER_FILE_SDDL, None)?;
    } else {
        // Older setup could fail after creating only a protected temporary copy.
        verify_unpublished_launcher_directory(&launcher)?;
    }
    let mut tree = Vec::new();
    collect_finalizer_deletion_paths(&launcher, &mut tree)?;
    for target in tree {
        if target == launcher {
            continue;
        }
        let metadata = fs::symlink_metadata(&target).map_err(io_failure)?;
        if metadata.is_dir() {
            fs::remove_dir(&target).map_err(io_failure)?;
        } else {
            fs::remove_file(&target).map_err(io_failure)?;
        }
    }
    // Keep the exact protected directory as a non-squattable namespace until HKLM Run is
    // flushed. Its identity was retained above and no executable remains in it.
    if owned_tree_identity(&launcher).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?
        != identity
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update launcher directory was replaced.",
        ));
    }
    Ok(())
}

pub(in super::super) fn relocated_uninstall_matches_maintenance(
    target: &Path,
    paths: &Paths,
) -> Result<bool> {
    let canonical_target = std::fs::canonicalize(target).map_err(io_failure)?;
    let canonical_temp = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = canonical_target
        .file_name()
        .and_then(|value| value.to_str());
    Ok(canonical_target.parent() == Some(canonical_temp.as_path())
        && name.is_some_and(|value| {
            value.starts_with(".TalkingQuill-uninstall-")
                && value.ends_with(".exe")
                && value.len() == ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        })
        && path_present(&paths.maintenance_uninstaller)?
        && file_hash(target)? == file_hash(&paths.maintenance_uninstaller)?)
}

pub(in super::super) fn collect_finalizer_deletion_paths(
    path: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(path).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(io_failure)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Finalizer tree contains a reparse point.",
            ));
        }
        if metadata.is_dir() {
            collect_finalizer_deletion_paths(&child, output)?;
        } else if metadata.is_file() {
            output.push(child);
        } else {
            return Err(fail(EXIT_REJECTED, "Finalizer tree entry type is invalid."));
        }
    }
    output.push(path.to_path_buf());
    Ok(())
}
