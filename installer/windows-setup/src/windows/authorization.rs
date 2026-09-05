//! Package, predecessor, and installed-image authorization.
use super::*;

pub(super) fn process_image(pid: u32) -> Result<PathBuf> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the setup peer process.",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    process_image_from_handle(&process)
}
pub(super) fn hash_reader(file: &mut File) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io_failure)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}
pub(super) fn file_hash(path: &Path) -> Result<[u8; 32]> {
    hash_reader(&mut File::open(path).map_err(io_failure)?)
}
pub(super) fn retained_file_hash(path: &Path) -> Result<(File, [u8; 32])> {
    let mut file = File::from(open_plain_handle(path, false, false)?);
    let hash = hash_reader(&mut file)?;
    Ok((file, hash))
}

pub(super) fn pending_uninstall_transaction(paths: &Paths) -> Result<bool> {
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

pub(super) fn arm_relocated_uninstall_controller(server: &OwnedHandle) -> Result<()> {
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

pub(super) fn authorize_uninstall_controller(paths: &Paths, current: &Path) -> Result<()> {
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

pub(super) fn validate_predecessor_arguments(
    package: &ParsedPackage,
    current: &Path,
) -> Result<()> {
    let predecessor = package
        .manifest
        .predecessor
        .as_ref()
        .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
    let expected = [
        ("/TQUPDATE=", hex_hash(&file_hash(current)?)),
        ("/TQGATEWAYHASH=", predecessor.gateway_sha256.clone()),
        ("/TQOWNERHASH=", predecessor.owner_sha256.clone()),
        ("/TQLAYOUT=", predecessor.release_build_digest.clone()),
    ];
    let arguments: Vec<String> = std::env::args_os()
        .skip(2)
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    if expected.iter().any(|(prefix, value)| {
        !arguments
            .iter()
            .any(|argument| argument == &format!("{prefix}{value}"))
    }) {
        return Err(fail(
            EXIT_REJECTED,
            "Authenticated predecessor arguments do not bind the exact package and installed identities.",
        ));
    }
    Ok(())
}

pub(super) fn staged_path_is_protected(path: &Path, directory: bool) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect staged helper protection.",
        ));
    }
    let mut text = ptr::null_mut();
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(descriptor.cast()) };
    if converted == 0 || text.is_null() {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode staged helper protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { LocalFree(text.cast()) };
    let expected = if directory {
        [
            "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)",
        ]
    } else {
        [
            "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)",
        ]
    };
    Ok(expected
        .iter()
        .any(|value| sddl.eq_ignore_ascii_case(value)))
}

pub(super) fn medium_launcher_directory_is_protected(path: &Path) -> Result<bool> {
    let mut descriptor = ptr::null_mut();
    if unsafe {
        GetNamedSecurityInfoW(
            wide(path.as_os_str()).as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    } != 0
        || descriptor.is_null()
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect launcher staging protection.",
        ));
    }
    let mut text = ptr::null_mut();
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
        || text.is_null()
    {
        unsafe { LocalFree(descriptor.cast()) };
        return Err(fail(
            EXIT_REJECTED,
            "Cannot encode launcher staging protection.",
        ));
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        LocalFree(text.cast());
        LocalFree(descriptor.cast());
    }
    Ok(value.eq_ignore_ascii_case("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)"))
}

pub(super) fn authenticate_predecessor_helper(
    package: &ParsedPackage,
    paths: &Paths,
) -> Result<()> {
    if package.manifest.package_mode != "update" {
        return Err(fail(
            EXIT_REJECTED,
            "A medium setup controller is required.",
        ));
    }
    let previous = package
        .manifest
        .predecessor
        .as_ref()
        .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
    let parent = parent_process_id()?;
    let parent_image = process_image(parent)?;
    let (_parent_image_lock, parent_hash) = retained_file_hash(&parent_image)?;
    let installed_gateway = paths
        .install
        .join("resources/helper/talking-quill-helper.exe");
    let parent_is_installed = path_present(&installed_gateway)?
        && canonical(&parent_image)? == canonical(&installed_gateway)?;
    let parent_is_staged = parent_image
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("talking-quill-update-bootstrap.exe"))
        && parent_image
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| {
                canonical(root).ok().as_deref() == canonical(&paths.program_data).ok().as_deref()
            })
        && parent_image
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                let prefix = ".Talking Quill.update-bootstrap-";
                name.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            })
        && parent_image.parent().is_some_and(|parent| {
            staged_path_is_protected(parent, true).unwrap_or(false)
                && staged_path_is_protected(&parent_image, false).unwrap_or(false)
        });
    let installed_hash_matches = if parent_is_installed {
        parent_hash == file_hash(&installed_gateway)?
    } else {
        true
    };
    if (!parent_is_installed && !parent_is_staged)
        || !installed_hash_matches
        || hex_hash(&parent_hash) != previous.gateway_sha256
    {
        return Err(fail(
            EXIT_REJECTED,
            "Update was not invoked by the exact authenticated predecessor helper.",
        ));
    }
    Ok(())
}

pub(super) fn parent_process_id() -> Result<u32> {
    process_parent_id(std::process::id())
}

pub(super) fn process_parent_id(process_id: u32) -> Result<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot inspect the update parent process.",
        ));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while available {
        if entry.th32ProcessID == process_id {
            return Ok(entry.th32ParentProcessID);
        }
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Err(fail(
        EXIT_REJECTED,
        "The update parent process is unavailable.",
    ))
}

pub(super) fn hex_bytes(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn hex_hash(value: &[u8; 32]) -> String {
    hex_bytes(value)
}

pub(super) fn installed_matches_target(
    package: &ParsedPackage,
    paths: &Paths,
    candidate: &Path,
) -> Result<bool> {
    let installed_path = paths
        .install
        .join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&installed_path)?;
    let installed: serde_json::Value =
        serde_json::from_slice(&fs::read(installed_path).map_err(io_failure)?)
            .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
    let role = |name: &str| {
        installed
            .get("roles")
            .and_then(|value| value.as_array())
            .and_then(|roles| {
                roles
                    .iter()
                    .find(|role| role.get("role").and_then(|value| value.as_str()) == Some(name))
            })
            .and_then(|role| role.get("sha256"))
            .and_then(|value| value.as_str())
    };
    let installed_setup = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&installed_setup)?;
    let exact_setup = file_hash(candidate)? == file_hash(&installed_setup)?;
    // Fault-bearing packages are non-promotable process-test artifacts. Their crash
    // seam is package-bound (never command-line authority) and can only target the
    // exact installed release identity.
    let acceptance_fault = cfg!(feature = "acceptance-faults")
        && package.manifest.fault_phase.is_some()
        && package.manifest.package_mode == "repair";
    Ok((exact_setup || acceptance_fault)
        && installed.get("version").and_then(|value| value.as_str())
            == Some(package.manifest.version.as_str())
        && installed
            .get("architecture")
            .and_then(|value| value.as_str())
            == Some(package.manifest.architecture.as_str())
        && installed
            .get("releaseBuildDigest")
            .and_then(|value| value.as_str())
            == Some(package.manifest.target.release_build_digest.as_str())
        && role("gateway") == Some(package.manifest.target.gateway_sha256.as_str())
        && role("owner") == Some(package.manifest.target.owner_sha256.as_str())
        && role("recovery-launcher")
            == Some(package.manifest.target.recovery_launcher_sha256.as_str()))
}

pub(super) fn authorize_package_mode(
    package: &ParsedPackage,
    paths: &Paths,
    candidate: &Path,
    predecessor_authorized: bool,
    requested: Action,
) -> Result<Action> {
    // The installed image is its own production repair authority. A renamed exact copy
    // may repair the same target without introducing a separately trusted repair binary.
    let installed_setup = paths.install.join("Uninstall Talking Quill.exe");
    if requested == Action::Repair
        && paths.install.exists()
        && installed_setup.exists()
        && file_hash(candidate)? == file_hash(&installed_setup)?
        && installed_matches_target(package, paths, candidate)?
    {
        return Ok(Action::Repair);
    }
    match package.manifest.package_mode.as_str() {
        // A manually launched full installer authorizes replacement of old or damaged files.
        // Downloaded updates still require their verified predecessor.
        "fresh" => Ok(if paths.install.exists() {
            Action::Repair
        } else {
            Action::Install
        }),
        "repair"
            if requested == Action::Repair
                && paths.install.exists()
                && installed_matches_target(package, paths, candidate)? =>
        {
            Ok(Action::Repair)
        }
        "update" if paths.install.exists() && predecessor_authorized => {
            let previous = package
                .manifest
                .predecessor
                .as_ref()
                .ok_or_else(|| fail(EXIT_REJECTED, "Update predecessor is missing."))?;
            let installed_path = paths
                .install
                .join("resources/keyboard-owner-release-v1.json");
            assert_plain_file(&installed_path)?;
            let installed: serde_json::Value =
                serde_json::from_slice(&fs::read(installed_path).map_err(io_failure)?)
                    .map_err(|_| fail(EXIT_REJECTED, "Installed release identity is invalid."))?;
            let role = |name: &str| {
                installed
                    .get("roles")
                    .and_then(|value| value.as_array())
                    .and_then(|roles| {
                        roles.iter().find(|role| {
                            role.get("role").and_then(|value| value.as_str()) == Some(name)
                        })
                    })
                    .and_then(|role| role.get("sha256"))
                    .and_then(|value| value.as_str())
            };
            if installed.get("version").and_then(|value| value.as_str()) != Some(&previous.version)
                || installed
                    .get("architecture")
                    .and_then(|value| value.as_str())
                    != Some(&package.manifest.architecture)
                || installed
                    .get("releaseBuildDigest")
                    .and_then(|value| value.as_str())
                    != Some(&previous.release_build_digest)
                || role("gateway") != Some(&previous.gateway_sha256)
                || role("owner") != Some(&previous.owner_sha256)
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Update does not authorize the exact installed predecessor.",
                ));
            }
            Ok(Action::Update)
        }
        _ => Err(fail(
            EXIT_REJECTED,
            "Package mode does not match independently derived machine state.",
        )),
    }
}

pub(super) fn is_uninstall_finalizer(path: &Path) -> Result<bool> {
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

pub(super) fn derive_action(current: &Path, paths: &Paths) -> Result<Action> {
    let maintenance = canonical(current)
        .ok()
        .zip(canonical(&paths.maintenance_uninstaller).ok())
        .is_some_and(|(current, maintenance)| current == maintenance);
    let finalizer = is_uninstall_finalizer(current)?;
    if maintenance
        || finalizer
        || current
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Uninstall Talking Quill.exe"))
    {
        let parent = current
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Invalid installed setup path."))?;
        if !maintenance && !finalizer && canonical(parent)? != canonical(&paths.install)? {
            return Err(fail(
                EXIT_REJECTED,
                "Uninstall image is outside the installed tree.",
            ));
        }
        return Ok(Action::Uninstall);
    }
    if pending_uninstall_transaction(paths)? {
        Ok(Action::Install)
    } else if paths.install.exists() {
        Ok(Action::Repair)
    } else {
        Ok(Action::Install)
    }
}
