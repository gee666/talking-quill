//! Controller image retention, mapped deletion, and relocation identity.
use super::*;

pub(in super::super) fn retain_controller_image(
    path: &Path,
    delete_on_close: bool,
) -> Result<OwnedHandle> {
    let access = FILE_GENERIC_READ | if delete_on_close { DELETE } else { 0 };
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if delete_on_close {
            FILE_FLAG_DELETE_ON_CLOSE
        } else {
            0
        };
    let sharing = FILE_SHARE_READ | FILE_SHARE_DELETE;
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            access,
            sharing,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

pub(in super::super) fn rename_handle(
    handle: std::os::windows::io::RawHandle,
    destination: &Path,
) -> Result<()> {
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(mem::size_of::<u16>())
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let fixed = mem::offset_of!(FILE_RENAME_INFO, FileName);
    let total = fixed
        .checked_add(name_bytes)
        .ok_or_else(|| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
    let mut storage = vec![0_usize; total.div_ceil(mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = u32::try_from(name_bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?;
        ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileRenameInfo,
            info.cast(),
            u32::try_from(total)
                .map_err(|_| fail(EXIT_FAILURE, "Mapped uninstall path is too long."))?,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    Ok(())
}

pub(in super::super) fn rename_retained(handle: &OwnedHandle, destination: &Path) -> Result<()> {
    rename_handle(handle.as_raw_handle(), destination)
}

pub(in super::super) fn arm_mapped_image_deletion(path: &Path) -> Result<()> {
    let rename_handle = open_plain_handle(path, false, true)?;
    let stream = PathBuf::from(format!(":tq-uninstall-{:08x}", std::process::id()));
    rename_retained(&rename_handle, &stream).map_err(|error| {
        fail(
            error.code,
            format!("Mapped image stream rename failed: {}", error.message),
        )
    })?;
    drop(rename_handle);
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_FAILURE,
            format!(
                "Renamed mapped image reopen failed: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let delete_handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            delete_handle.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(io_failure(std::io::Error::last_os_error()));
    }
    drop(delete_handle);
    if path_present(path)? {
        return Err(fail(
            EXIT_FAILURE,
            "Windows did not commit mapped uninstall image deletion.",
        ));
    }
    Ok(())
}

pub(in super::super) fn validate_relocated_uninstall_image(
    relocated: &Path,
    original: &Path,
    expected: &Path,
    maintenance: &Path,
) -> Result<File> {
    let relocated_canonical = std::fs::canonicalize(relocated).map_err(io_failure)?;
    let temp_canonical = std::fs::canonicalize(std::env::temp_dir()).map_err(io_failure)?;
    let name = relocated_canonical
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| fail(EXIT_REJECTED, "Relocated uninstall path is invalid."))?;
    if relocated_canonical.parent() != Some(temp_canonical.as_path())
        || !name.starts_with(".TalkingQuill-uninstall-")
        || !name.ends_with(".exe")
        || name.len() != ".TalkingQuill-uninstall-".len() + 32 + ".exe".len()
        || canonical(original)? != canonical(expected)?
    {
        return Err(fail(EXIT_REJECTED, "Relocated uninstall path is invalid."));
    }
    let original_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(original)
        .map_err(io_failure)?;
    let expected_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(expected)
        .map_err(io_failure)?;
    let mut relocated_file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(relocated)
        .map_err(io_failure)?;
    if file_identity_text(&original_file)? != file_identity_text(&expected_file)?
        || file_hash(original)? != file_hash(expected)?
        || file_hash(original)? != file_hash(maintenance)?
        || hash_reader(&mut relocated_file)? != file_hash(maintenance)?
    {
        return Err(fail(
            EXIT_REJECTED,
            "Relocated uninstall source identity does not match maintenance authority.",
        ));
    }
    drop((original_file, expected_file));
    Ok(relocated_file)
}

pub(in super::super) fn create_relocated_image(source: &Path) -> Result<(PathBuf, File)> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!(".TalkingQuill-uninstall-{suffix}.exe"));
    let mut target = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(&path)
        .map_err(io_failure)?;
    let mut source = File::open(source).map_err(io_failure)?;
    std::io::copy(&mut source, &mut target).map_err(io_failure)?;
    target.sync_all().map_err(io_failure)?;
    Ok((path, target))
}
