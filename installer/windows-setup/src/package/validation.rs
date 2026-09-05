//! Manifest identity, block layout, and Windows path admission.
use super::*;

pub(super) fn validate_manifest(
    manifest: &Manifest,
    package_size: u64,
    manifest_size: u64,
) -> Result<(), PackageError> {
    if manifest.schema_version != 2
        || !matches!(manifest.architecture.as_str(), "x64" | "arm64")
        || !valid_version(&manifest.version)
        || !git_object_id(&manifest.source_commit)
        || !git_object_id(&manifest.source_tree)
        || !(matches!(manifest.package_mode.as_str(), "fresh" | "update")
            || (cfg!(feature = "installed-acceptance-repair") && manifest.package_mode == "repair")
            || (cfg!(feature = "stale-schema2-cleanup")
                && manifest.package_mode == "stale-schema2-cleanup"))
        || !hex_digest(&manifest.target.release_build_digest)
        || !hex_digest(&manifest.target.gateway_sha256)
        || !hex_digest(&manifest.target.owner_sha256)
        || !hex_digest(&manifest.target.recovery_launcher_sha256)
        || (manifest.fault_phase.is_some()
            && (!cfg!(feature = "acceptance-faults") || manifest.package_mode != "repair"))
        || manifest.fault_phase.as_deref().is_some_and(|phase| {
            !matches!(
                phase,
                "staged"
                    | "prepared"
                    | "predecessorMoved"
                    | "publishing"
                    | "publishedBeforePersist"
                    | "published"
                    | "registered"
                    | "committed"
                    | "legacyRetiring"
                    | "legacyRetired"
                    | "terminalAcceptance"
            )
        })
        || !hex_digest(&manifest.tree_sha256)
        || manifest.files.is_empty()
        || manifest.files.len() > MAX_FILES
        || (manifest.package_mode == "update") != manifest.predecessor.is_some()
        || (manifest.package_mode == "stale-schema2-cleanup"
            && (manifest.predecessor.is_some() || manifest.fault_phase.is_some()))
    {
        return Err(PackageError::Identity);
    }
    if let Some(previous) = &manifest.predecessor
        && (!valid_version(&previous.version)
            || !hex_digest(&previous.release_build_digest)
            || !hex_digest(&previous.gateway_sha256)
            || !hex_digest(&previous.owner_sha256))
    {
        return Err(PackageError::Identity);
    }
    let mut names = BTreeSet::new();
    let mut expected_offset = manifest_size;
    let mut total = 0_u64;
    let mut tree = Sha256::new();
    for file in &manifest.files {
        validate_path(&file.path)?;
        let folded = file.path.to_lowercase();
        if !names.insert(folded) {
            return Err(PackageError::Collision);
        }
        if file.mode != 0
            || file.size > MAX_FILE_BYTES
            || file.block_size == 0
            || file.block_offset != expected_offset
            || !hex_digest(&file.sha256)
        {
            return Err(PackageError::Block);
        }
        total = total.checked_add(file.size).ok_or(PackageError::Limit)?;
        if total > MAX_TREE_BYTES {
            return Err(PackageError::Limit);
        }
        expected_offset = expected_offset
            .checked_add(file.block_size)
            .ok_or(PackageError::Range)?;
        frame(&mut tree, &file.path);
        frame(&mut tree, &file.mode.to_string());
        frame(&mut tree, &file.size.to_string());
        frame(&mut tree, &file.sha256);
    }
    let role_hash = |path: &str| {
        manifest
            .files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.sha256.as_str())
    };
    if role_hash("resources/helper/talking-quill-helper.exe")
        != Some(manifest.target.gateway_sha256.as_str())
        || role_hash("resources/helper/talking-quill-keyboard-owner.exe")
            != Some(manifest.target.owner_sha256.as_str())
        || role_hash("resources/helper/talking-quill-update-recovery-launcher.exe")
            != Some(manifest.target.recovery_launcher_sha256.as_str())
        || manifest
            .files
            .iter()
            .filter(|file| file.path == "resources/keyboard-owner-release-v1.json")
            .count()
            != 1
        || expected_offset != package_size
        || hex(&tree.finalize()) != manifest.tree_sha256
    {
        return Err(PackageError::Digest);
    }
    Ok(())
}

pub fn validate_path(path: &str) -> Result<(), PackageError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.len() > 1024
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
    {
        return Err(PackageError::Path);
    }
    for part in path.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || part.ends_with(' ')
            || part.len() > 255
        {
            return Err(PackageError::Path);
        }
        let stem = part
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(PackageError::Path);
        }
        if part.chars().any(|character| {
            character < ' ' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
        }) {
            return Err(PackageError::Path);
        }
    }
    Ok(())
}

fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn git_object_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
