//! Durable filesystem, registry deletion, and basic Windows utilities.
use super::*;

mod registry;
pub(super) use registry::*;

mod journal;
pub(super) use journal::*;

mod platform;
pub(super) use platform::*;

pub(super) fn flush_file(path: &Path) -> Result<()> {
    // FlushFileBuffers requires a handle opened for writing on Windows.
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            fail(
                EXIT_FAILURE,
                format!("Cannot save {}: {error}", path.display()),
            )
        })
}

pub(super) fn remove_machine_lock_residue(paths: &Paths, suffix: &str) -> Result<()> {
    validate_machine_lock_suffix(suffix)?;
    let path = paths
        .program_data
        .join(format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}"));
    if !path_present(&path)? {
        return Ok(());
    }
    verify_machine_lock_tree(&path)?;
    let identity =
        owned_tree_identity(&path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    remove_owned_tree(&path, &identity).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    flush_setup_directory(&paths.program_data)
}

pub(super) fn create_plain_directories(root: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| fail(EXIT_REJECTED, "Package escaped staging."))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if !current.exists() {
            fs::create_dir(&current).map_err(io_failure)?;
        }
        assert_plain_directory(&current)?;
    }
    Ok(())
}

pub(super) fn open_plain_handle(path: &Path, directory: bool, delete: bool) -> Result<OwnedHandle> {
    let access = FILE_GENERIC_READ | if delete { DELETE } else { 0 };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
        || tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || directory != (tag.FileAttributes & 0x10 != 0)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Installer object identity is not a plain expected file type.",
        ));
    }
    Ok(handle)
}

pub(super) fn delete_retained(handle: &OwnedHandle) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(())
}

pub(super) fn path_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_failure(error)),
    }
}

pub(super) fn remove_plain_tree(path: &Path) -> Result<()> {
    if !path_present(path)? {
        return Ok(());
    }
    let identity =
        owned_tree_identity(path).map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
    remove_owned_tree(path, &identity).map_err(|error| fail(EXIT_REJECTED, error.to_string()))
}

pub(super) fn assert_plain_absent(path: &Path) -> Result<()> {
    if path_present(path)? {
        Err(fail(EXIT_REJECTED, "Installer staging already exists."))
    } else {
        Ok(())
    }
}
pub(super) fn assert_plain_directory(path: &Path) -> Result<()> {
    open_plain_handle(path, true, false).map(|_| ())
}
pub(super) fn assert_plain_file(path: &Path) -> Result<()> {
    open_plain_handle(path, false, false).map(|_| ())
}
pub(super) fn durable_rename(from: &Path, to: &Path) -> Result<()> {
    move_file(from, to, MOVEFILE_WRITE_THROUGH)
}
pub(super) fn durable_replace(from: &Path, to: &Path) -> Result<()> {
    move_file(from, to, MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING)
}
pub(super) fn move_file(from: &Path, to: &Path, flags: u32) -> Result<()> {
    if unsafe {
        MoveFileExW(
            wide(from.as_os_str()).as_ptr(),
            wide(to.as_os_str()).as_ptr(),
            flags,
        )
    } == 0
    {
        Err(io_failure(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

pub(super) fn canonical(path: &Path) -> Result<String> {
    Ok(fs::canonicalize(path)
        .map_err(io_failure)?
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase())
}

pub(super) fn io_failure(error: std::io::Error) -> SetupError {
    fail(EXIT_FAILURE, error.to_string())
}

#[cfg(test)]
mod flush_tests {
    use super::*;

    #[test]
    fn copied_file_can_be_flushed_on_windows() {
        let path = std::env::temp_dir().join(format!("tq-file-flush-{}", std::process::id()));
        fs::write(&path, b"durable").unwrap();
        flush_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"durable");
        fs::remove_file(path).unwrap();
    }
}
