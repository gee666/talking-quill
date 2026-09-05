//! Package, predecessor, and installed-image authorization.
use super::*;

mod identity;
pub(super) use identity::*;

mod protection;
pub(super) use protection::*;

mod predecessor;
pub(super) use predecessor::*;

mod uninstall;
pub(super) use uninstall::*;

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
