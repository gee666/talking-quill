//! Stage and validate a release before replacing the installed application.
use super::*;

pub(super) fn install(
    image: &mut File,
    package: &ParsedPackage,
    current: &Path,
    paths: &Paths,
    action: Action,
    system: &dyn NativeSystemAdapter,
) -> Result<()> {
    assert_plain_absent(&paths.staging)?;
    let had_predecessor = path_present(&paths.install)?;
    write_transaction(paths, "staging", action, had_predecessor)?;
    fs::create_dir(&paths.staging).map_err(io_failure)?;
    assert_plain_directory(&paths.staging)?;
    for entry in &package.manifest.files {
        let destination = paths.staging.join(entry.path.replace('/', "\\"));
        let parent = destination
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Package path has no parent."))?;
        create_plain_directories(&paths.staging, parent)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(io_failure)?;
        package::extract_file(image, package, entry, &mut output)
            .map_err(|error| fail(EXIT_REJECTED, format!("Package block failed: {error:?}")))?;
        output.sync_all().map_err(io_failure)?;
    }
    validate_staged_release_identity(package, &paths.staging)?;
    let uninstaller_path = paths.staging.join("Uninstall Talking Quill.exe");
    let mut uninstaller = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&uninstaller_path)
        .map_err(io_failure)?;
    let mut setup_image = File::open(current).map_err(io_failure)?;
    std::io::copy(&mut setup_image, &mut uninstaller).map_err(io_failure)?;
    uninstaller.sync_all().map_err(io_failure)?;
    drop(uninstaller);
    if file_hash(&uninstaller_path)? != file_hash(current)? {
        return Err(fail(
            EXIT_REJECTED,
            "Staged uninstaller verification failed.",
        ));
    }
    write_transaction(paths, "staged", action, had_predecessor)?;
    crash_at(package, "staged");
    write_transaction(paths, "prepared", action, had_predecessor)?;
    crash_at(package, "prepared");
    if had_predecessor {
        durable_rename(&paths.install, &paths.backup)?;
        write_transaction(paths, "predecessor-moved", action, had_predecessor)?;
        crash_at(package, "predecessorMoved");
    }
    write_transaction(paths, "publishing", action, had_predecessor)?;
    crash_at(package, "publishing");
    durable_rename(&paths.staging, &paths.install)?;
    write_transaction(paths, "published-before-persist", action, had_predecessor)?;
    crash_at(package, "publishedBeforePersist");
    write_transaction(paths, "published", action, had_predecessor)?;
    crash_at(package, "published");
    ensure_maintenance_uninstaller(paths)?;
    ensure_machine_relaunch_owner_installed(paths)?;
    system.register_version(paths, &package.manifest.version)?;
    write_transaction(paths, "registered", action, had_predecessor)?;
    crash_at(package, "registered");
    write_transaction(paths, "committed", action, had_predecessor)?;
    crash_at(package, "committed");
    write_transaction(paths, "legacy-retiring", action, had_predecessor)?;
    crash_at(package, "legacyRetiring");
    system.retire_legacy(paths)?;
    write_transaction(paths, "legacy-retired", action, had_predecessor)?;
    crash_at(package, "legacyRetired");
    remove_plain_tree(&paths.backup)?;
    remove_transaction(paths)?;
    Ok(())
}

pub(super) fn validate_staged_release_identity(
    package: &ParsedPackage,
    staging: &Path,
) -> Result<()> {
    let path = staging.join("resources/keyboard-owner-release-v1.json");
    assert_plain_file(&path)?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).map_err(io_failure)?)
        .map_err(|_| fail(EXIT_REJECTED, "Staged release identity is invalid."))?;
    let role = |name: &str| {
        value
            .get("roles")
            .and_then(|roles| roles.as_array())
            .and_then(|roles| {
                roles
                    .iter()
                    .find(|role| role.get("role").and_then(|role| role.as_str()) == Some(name))
            })
            .and_then(|role| role.get("sha256"))
            .and_then(|hash| hash.as_str())
    };
    let roles_match = value
        .get("roles")
        .and_then(|roles| roles.as_array())
        .is_some_and(|roles| {
            let expected = [
                (
                    "gateway",
                    "resources/helper/talking-quill-helper.exe",
                    false,
                ),
                (
                    "owner",
                    "resources/helper/talking-quill-keyboard-owner.exe",
                    true,
                ),
                (
                    "recovery-launcher",
                    "resources/helper/talking-quill-update-recovery-launcher.exe",
                    false,
                ),
            ];
            roles.len() == expected.len()
                && roles.iter().zip(expected).all(|(role, expected)| {
                    role.get("role").and_then(|value| value.as_str()) == Some(expected.0)
                        && role.get("path").and_then(|value| value.as_str()) == Some(expected.1)
                        && role
                            .get("suppressionCapable")
                            .and_then(|value| value.as_bool())
                            == Some(expected.2)
                })
        });
    let predecessor_matches = match (&package.manifest.predecessor, value.get("predecessor")) {
        (None, Some(previous)) => previous.is_null(),
        (Some(expected), Some(previous)) => {
            previous.get("version").and_then(|item| item.as_str())
                == Some(expected.version.as_str())
                && previous
                    .get("releaseBuildDigest")
                    .and_then(|item| item.as_str())
                    == Some(expected.release_build_digest.as_str())
                && previous.get("gatewaySha256").and_then(|item| item.as_str())
                    == Some(expected.gateway_sha256.as_str())
                && previous.get("ownerSha256").and_then(|item| item.as_str())
                    == Some(expected.owner_sha256.as_str())
        }
        _ => false,
    };
    let acceptance_repair = cfg!(feature = "acceptance-faults")
        && package.manifest.fault_phase.is_some()
        && package.manifest.package_mode == "repair";
    if !roles_match
        || value.get("version").and_then(|item| item.as_str())
            != Some(package.manifest.version.as_str())
        || value.get("architecture").and_then(|item| item.as_str())
            != Some(package.manifest.architecture.as_str())
        || value.get("sourceCommit").and_then(|item| item.as_str())
            != Some(package.manifest.source_commit.as_str())
        || value.get("sourceTree").and_then(|item| item.as_str())
            != Some(package.manifest.source_tree.as_str())
        || (!acceptance_repair
            && value.get("packageMode").and_then(|item| item.as_str())
                != Some(package.manifest.package_mode.as_str()))
        || value
            .get("releaseBuildDigest")
            .and_then(|item| item.as_str())
            != Some(package.manifest.target.release_build_digest.as_str())
        || role("gateway") != Some(package.manifest.target.gateway_sha256.as_str())
        || role("owner") != Some(package.manifest.target.owner_sha256.as_str())
        || role("recovery-launcher")
            != Some(package.manifest.target.recovery_launcher_sha256.as_str())
        || (!acceptance_repair && !predecessor_matches)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Staged release identity does not match TQPKG2.",
        ));
    }
    for relative in [
        "resources/helper/talking-quill-helper.exe",
        "resources/helper/talking-quill-keyboard-owner.exe",
        "resources/helper/talking-quill-update-recovery-launcher.exe",
    ] {
        validate_staged_native_role(
            &staging.join(relative),
            &package.manifest.architecture,
            &package.manifest.source_commit,
            &package.manifest.source_tree,
        )?;
    }
    Ok(())
}

pub(super) fn validate_staged_native_role(
    path: &Path,
    architecture: &str,
    source_commit: &str,
    source_tree: &str,
) -> Result<()> {
    let bytes = fs::read(path).map_err(io_failure)?;
    let pe = bytes
        .get(60..64)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .map(|value| value as usize);
    let machine = pe.and_then(|offset| {
        (bytes.get(offset..offset + 4) == Some(b"PE\0\0"))
            .then(|| bytes.get(offset + 4..offset + 6))
            .flatten()
            .and_then(|value| value.try_into().ok())
            .map(u16::from_le_bytes)
    });
    let expected_machine = match architecture {
        "x64" => 0x8664,
        "arm64" => 0xaa64,
        _ => 0,
    };
    let marker_is_exact = |name: &str, expected: &str| {
        let prefix = format!("{name}=");
        let matches: Vec<&[u8]> = bytes
            .windows(prefix.len())
            .enumerate()
            .filter(|(_, window)| *window == prefix.as_bytes())
            .filter_map(|(offset, _)| bytes.get(offset + prefix.len()..offset + prefix.len() + 40))
            .collect();
        matches.len() == 1 && matches[0] == expected.as_bytes()
    };
    if bytes.get(..2) != Some(b"MZ")
        || machine != Some(expected_machine)
        || !marker_is_exact("TALKING_QUILL_SOURCE_COMMIT", source_commit)
        || !marker_is_exact("TALKING_QUILL_SOURCE_TREE", source_tree)
    {
        return Err(fail(
            EXIT_REJECTED,
            "Staged native role architecture or source identity is invalid.",
        ));
    }
    Ok(())
}

pub(super) struct ServiceHandle(pub(super) SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

pub(super) unsafe fn wide_ptr_string(pointer: *const u16) -> String {
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pointer, length) })
}

#[cfg(feature = "acceptance-faults")]
pub(super) fn crash_at(package: &ParsedPackage, phase: &str) {
    if package.manifest.fault_phase.as_deref() == Some(phase) {
        let namespace = std::env::var("TQ_MACHINE_LOCK_TEST_NAMESPACE_ID")
            .ok()
            .filter(|value| {
                value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        let nonce = std::env::var("TQ_FAULT_AUDIT_NONCE").ok().filter(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if let (Some(namespace), Some(nonce)) = (namespace, nonce) {
            let audit = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("tmp/machine-lock-tests/windows-setup")
                .join(namespace)
                .join("fault-audit-v1.json");
            let record = format!(
                "{{\"nonce\":\"{nonce}\",\"phase\":\"{phase}\",\"processId\":{}}}\n",
                std::process::id()
            );
            let _ = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(audit)
                .and_then(|mut file| {
                    file.write_all(record.as_bytes())?;
                    file.sync_all()
                });
        }
        std::process::exit(197);
    }
}

#[cfg(not(feature = "acceptance-faults"))]
pub(super) fn crash_at(package: &ParsedPackage, _phase: &str) {
    debug_assert!(package.manifest.fault_phase.is_none());
}
