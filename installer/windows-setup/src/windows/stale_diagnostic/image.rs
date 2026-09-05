//! Retained direct cleanup image authentication and source binding.
use super::*;

#[cfg(feature = "stale-schema2-cleanup")]
pub(in super::super) fn open_authenticated_direct_cleanup_image() -> Result<(File, [u8; 32])> {
    let current = std::env::current_exe().map_err(io_failure)?;
    let kernel_image = process_image(std::process::id())?;
    if canonical(&current)? != canonical(&kernel_image)? {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup process image does not match its self path.",
        ));
    }
    let mut image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&current)
        .map_err(|_| fail(EXIT_REJECTED, "Direct cleanup image cannot be retained."))?;
    let parent = current
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Direct cleanup image has no parent."))?;
    if !staged_path_is_protected(parent, true)? || !protected_file_handle_acl_is_exact(&image)? {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup image is not administrator protected.",
        ));
    }
    let identity = file_identity_text(&image)?;
    let length = image.metadata().map_err(io_failure)?.len();
    let digest = hash_reader(&mut image)?;
    image.seek(SeekFrom::Start(0)).map_err(io_failure)?;
    let package = package::parse(&mut image, length).map_err(|error| {
        fail(
            EXIT_REJECTED,
            format!("Direct cleanup TQPKG2 validation failed: {error:?}"),
        )
    })?;
    let (source_commit, source_tree) = direct_cleanup_source_identity()?;
    let expected_architecture = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unsupported"
    };
    if package.manifest.source_commit != source_commit
        || package.manifest.source_tree != source_tree
        || package.manifest.architecture != expected_architecture
        || package.manifest.package_mode != "stale-schema2-cleanup"
        || package.manifest.predecessor.is_some()
        || package.manifest.fault_phase.is_some()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup image does not match its compiled source identity.",
        ));
    }
    let path_image = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&kernel_image)
        .map_err(|_| fail(EXIT_REJECTED, "Direct cleanup process image changed."))?;
    if file_identity_text(&path_image)? != identity {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup process image identity changed.",
        ));
    }
    Ok((image, digest))
}
