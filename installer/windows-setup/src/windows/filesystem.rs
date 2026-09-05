//! Durable filesystem, registry deletion, and basic Windows utilities.
use super::*;

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

pub(super) fn unregister_uninstall() -> Result<()> {
    delete_registry_tree_durable(
        UNINSTALL_KEY,
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        "native uninstall registration",
    )
}

pub(super) fn delete_machine_lock_registry_durable(
    path: &str,
    parent: &str,
    label: &str,
) -> Result<()> {
    delete_registry_tree_durable_in_hive(machine_lock_registry_hive(), path, parent, label)
}

pub(super) fn delete_registry_tree_durable(path: &str, parent: &str, label: &str) -> Result<()> {
    delete_registry_tree_durable_in_hive(HKEY_LOCAL_MACHINE, path, parent, label)
}

pub(super) fn delete_registry_tree_durable_in_hive(
    hive: HKEY,
    path: &str,
    parent: &str,
    label: &str,
) -> Result<()> {
    let status = unsafe { RegDeleteTreeW(hive, wide(OsStr::new(path)).as_ptr()) };
    if status != 0 && status != 2 {
        return Err(fail(EXIT_FAILURE, format!("Cannot remove the {label}.")));
    }
    let mut deleted = ptr::null_mut();
    let observed = unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(path)).as_ptr(),
            0,
            KEY_READ,
            &mut deleted,
        )
    };
    if observed == 0 {
        unsafe { RegCloseKey(deleted) };
        return Err(fail(EXIT_FAILURE, format!("Windows retained the {label}.")));
    }
    if observed != 2 {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot verify removal of the {label}."),
        ));
    }
    let mut parent_key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            hive,
            wide(OsStr::new(parent)).as_ptr(),
            0,
            KEY_READ,
            &mut parent_key,
        )
    } != 0
    {
        return Err(fail(
            EXIT_FAILURE,
            format!("Cannot open the {label} parent."),
        ));
    }
    let flushed = unsafe { RegFlushKey(parent_key) } == 0;
    unsafe { RegCloseKey(parent_key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            format!("Cannot flush removal of the {label}."),
        ))
    }
}

pub(super) fn transaction_action(value: &Transaction) -> Result<Action> {
    match value.action.as_str() {
        "install" => Ok(Action::Install),
        "update" => Ok(Action::Update),
        "repair" => Ok(Action::Repair),
        "uninstall" => Ok(Action::Uninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction action is invalid.",
        )),
    }
}

pub(super) fn write_transaction(
    paths: &Paths,
    phase: &str,
    action: Action,
    had_predecessor: bool,
) -> Result<()> {
    let temporary = paths
        .transaction
        .with_extension(format!("tmp-{}", std::process::id()));
    let action = match action {
        Action::Install => "install",
        Action::Update => "update",
        Action::Repair => "repair",
        Action::Uninstall => "uninstall",
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => {
            return Err(fail(EXIT_REJECTED, "Cleanup cannot create a transaction."));
        }
    };
    let bytes = serde_json::to_vec(&Transaction {
        schema_version: TRANSACTION_SCHEMA,
        phase: phase.into(),
        action: action.into(),
        had_predecessor,
    })
    .map_err(|_| fail(EXIT_FAILURE, "Cannot encode installer transaction."))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    output
        .write_all(&bytes)
        .and_then(|_| output.sync_all())
        .map_err(io_failure)?;
    durable_replace(&temporary, &paths.transaction)
}

pub(super) fn cleanup_transaction_residue(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(pid) = name.strip_prefix(".Talking Quill.native-transaction-v2.tmp-") else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let file = open_plain_handle(&entry.path(), false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
}

pub(super) fn remove_transaction(paths: &Paths) -> Result<()> {
    if paths.transaction.exists() {
        let file = open_plain_handle(&paths.transaction, false, true)?;
        delete_retained(&file)?;
    }
    if paths.maintenance_generation_record.exists() {
        let file = open_plain_handle(&paths.maintenance_generation_record, false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
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

pub(super) fn known_folder(identifier: *const windows_sys::core::GUID) -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(identifier, 0, ptr::null_mut(), &mut raw) } != 0
        || raw.is_null()
    {
        return Err(fail(EXIT_FAILURE, "Windows known-folder lookup failed."));
    }
    let length = unsafe { (0..).position(|index| *raw.add(index) == 0).unwrap_or(0) };
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(value))
}

pub(super) fn token_is_elevated() -> Result<bool> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect the setup token."));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(fail(EXIT_REJECTED, "Cannot read setup elevation."));
    }
    Ok(elevation.TokenIsElevated != 0)
}

pub(super) fn message_box(text: &str, flags: u32) -> i32 {
    let caption = wide(OsStr::new(concat!(
        "Talking Quill ",
        env!("CARGO_PKG_VERSION"),
        " setup"
    )));
    let text = wide(OsStr::new(text));
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            flags | MB_SETFOREGROUND,
        )
    }
}
pub(super) fn report(message: &str) {
    message_box(message, 0x10);
}
pub(super) fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

#[cfg(any(test, feature = "stale-schema2-cleanup"))]
pub(super) fn registry_key_present(root: HKEY, path: &str) -> Result<bool> {
    let mut key = ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(root, wide(OsStr::new(path)).as_ptr(), 0, KEY_READ, &mut key) };
    if status == 0 {
        unsafe { RegCloseKey(key) };
        Ok(true)
    } else if status == 2 {
        Ok(false)
    } else {
        Err(fail(
            EXIT_REJECTED,
            "Cannot inspect stale coordination registry state.",
        ))
    }
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
