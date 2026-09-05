//! Exact predecessor arguments and helper authorization.
use super::*;

pub(in super::super) fn validate_predecessor_arguments(
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

pub(in super::super) fn authenticate_predecessor_helper(
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
