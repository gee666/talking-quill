//! Validate release identity, predecessor relationships, and package authorization.
use super::*;

pub(super) fn known_folder(folder: &windows_sys::core::GUID) -> Result<PathBuf, i32> {
    let mut value = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut value) } < 0
        || value.is_null()
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut length = 0usize;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    let path = PathBuf::from(
        String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| EXIT_LAUNCH_FAILED)?,
    );
    unsafe { CoTaskMemFree(value.cast()) };
    Ok(path)
}

pub(super) fn trusted_installed_bootstrap() -> Result<(PathBuf, File, FileIdentity, [u8; 32]), i32>
{
    let current =
        std::fs::canonicalize(std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?)
            .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let expected = known_folder(&FOLDERID_ProgramFiles)?
        .join("Talking Quill")
        .join("resources")
        .join("helper")
        .join("talking-quill-helper.exe");
    if !paths_equal(&current, &expected) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut retained = open_locked(&current).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let identity = file_identity(&retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let hash = hash_file(&mut retained).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    Ok((current, retained, identity, hash))
}

pub(super) fn verify_update_relation(
    candidate: &UpdateCandidate,
    package_sha256: &str,
) -> Result<(), i32> {
    let current = std::env::current_exe().map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let staged = current
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("talking-quill-update-bootstrap.exe"));
    let resources = current
        .parent()
        .and_then(|parent| {
            if staged {
                Some(parent.to_owned())
            } else {
                parent.parent().map(Path::to_owned)
            }
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let manifest_name = if staged {
        "predecessor-release.json"
    } else {
        "keyboard-owner-release-v1.json"
    };
    let bytes = std::fs::read(resources.join(manifest_name)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if bytes.is_empty() || bytes.len() > 64 * 1024 {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let installed: InstalledManifest =
        serde_json::from_slice(&bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let digest = |value: &str| decode_hash(value).is_some();
    if candidate.platform != "win"
        || installed.platform != "win"
        || candidate.architecture != installed.architecture
        || !matches!(candidate.architecture.as_str(), "x64" | "arm64")
        || candidate.owner_mode != "local-unsigned-enabled"
        || candidate.package_mode != "update"
        || candidate.package_sha256.len() != 64
        || candidate.channel != format!("latest-{}", candidate.architecture)
        || candidate.transaction_binding != "source-target-package-sha256-v1"
        || candidate.version.is_empty()
        || !valid_source_identity(&candidate.source_commit)
        || !valid_source_identity(&candidate.source_tree)
        || !valid_source_identity(&installed.source_commit)
        || !valid_source_identity(&installed.source_tree)
        || candidate.release_build_digest == installed.release_build_digest
        || !digest(&candidate.release_build_digest)
        || !digest(&candidate.package_layout_digest)
        || !digest(&candidate.package_sha256)
        || candidate.roles.len() != 3
        || installed.roles.len() != 3
        || candidate.predecessor.platform != "win"
        || candidate.predecessor.architecture != installed.architecture
        || candidate.predecessor.version != installed.version
        || !digest(&installed.release_build_digest)
        || candidate.predecessor.release_build_digest != installed.release_build_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    if candidate.package_sha256 != package_sha256
        || candidate.release_build_digest != candidate.package_layout_digest
        || canonical_candidate_layout(candidate)? != candidate.package_layout_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let candidate_gateway = update_role(&candidate.roles, "gateway")?;
    let candidate_owner = update_role(&candidate.roles, "owner")?;
    let candidate_recovery_launcher = update_role(&candidate.roles, "recovery-launcher")?;
    let installed_gateway = update_role(&installed.roles, "gateway")?;
    let installed_owner = update_role(&installed.roles, "owner")?;
    let installed_recovery_launcher = update_role(&installed.roles, "recovery-launcher")?;
    if installed_gateway.path != "resources/helper/talking-quill-helper.exe"
        || installed_gateway.suppression_capable
        || installed_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !installed_owner.suppression_capable
        || installed_recovery_launcher.path
            != "resources/helper/talking-quill-update-recovery-launcher.exe"
        || installed_recovery_launcher.suppression_capable
        || !digest(&installed_gateway.sha256)
        || !digest(&installed_owner.sha256)
        || !digest(&installed_recovery_launcher.sha256)
        || candidate_gateway.path != "resources/helper/talking-quill-helper.exe"
        || candidate_gateway.suppression_capable
        || !digest(&candidate_gateway.sha256)
        || candidate_owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !candidate_owner.suppression_capable
        || !digest(&candidate_owner.sha256)
        || candidate_recovery_launcher.path
            != "resources/helper/talking-quill-update-recovery-launcher.exe"
        || candidate_recovery_launcher.suppression_capable
        || !digest(&candidate_recovery_launcher.sha256)
        || candidate.predecessor.gateway_sha256 != installed_gateway.sha256
        || candidate.predecessor.owner_sha256 != installed_owner.sha256
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut gateway_file = open_locked(&current).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let owner_name = if staged {
        "predecessor-owner.exe"
    } else {
        "talking-quill-keyboard-owner.exe"
    };
    let role_directory = current.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let mut owner_file =
        open_locked(&role_directory.join(owner_name)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let recovery_launcher_name = if staged {
        "predecessor-recovery-launcher.exe"
    } else {
        "talking-quill-update-recovery-launcher.exe"
    };
    let mut recovery_launcher_file = open_locked(&role_directory.join(recovery_launcher_name))
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut gateway_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
        != decode_hash(&installed_gateway.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        || hash_file(&mut owner_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&installed_owner.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        || hash_file(&mut recovery_launcher_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&installed_recovery_launcher.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(())
}

pub(super) fn verify_update_relation_against_snapshot(
    candidate: &UpdateCandidate,
    package_sha256: &str,
    predecessor: &InstalledManifest,
) -> Result<(), i32> {
    let gateway = update_role(&predecessor.roles, "gateway")?;
    let owner = update_role(&predecessor.roles, "owner")?;
    if candidate.package_sha256 != package_sha256
        || candidate.predecessor.version != predecessor.version
        || candidate.predecessor.platform != predecessor.platform
        || candidate.predecessor.architecture != predecessor.architecture
        || candidate.predecessor.release_build_digest != predecessor.release_build_digest
        || candidate.predecessor.gateway_sha256 != gateway.sha256
        || candidate.predecessor.owner_sha256 != owner.sha256
        || canonical_candidate_layout(candidate)? != candidate.package_layout_digest
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_installed_snapshot(predecessor)
}

pub(super) fn verify_installed_snapshot(snapshot: &InstalledManifest) -> Result<(), i32> {
    let installed = installed_manifest()?;
    if installed.version != snapshot.version
        || installed.platform != snapshot.platform
        || installed.architecture != snapshot.architecture
        || installed.source_commit != snapshot.source_commit
        || installed.source_tree != snapshot.source_tree
        || installed.release_build_digest != snapshot.release_build_digest
        || installed.roles.len() != snapshot.roles.len()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let root = known_folder(&FOLDERID_ProgramFiles)?.join("Talking Quill");
    for role in &snapshot.roles {
        let current = update_role(&installed.roles, &role.role)?;
        if current.path != role.path
            || current.sha256 != role.sha256
            || current.suppression_capable != role.suppression_capable
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        let mut file = open_locked(&root.join(&role.path)).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut file).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != decode_hash(&role.sha256).ok_or(EXIT_IDENTITY_MISMATCH)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    }
    Ok(())
}

pub(super) fn verify_update_authorization(candidate: &UpdateCandidate) -> Result<(), i32> {
    let primary = env!("TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1");
    let primary_digest = hex_digest(&Sha256::digest(decode_hex_bytes(primary, 65)?));
    if candidate.authorization.verification_key_sha256.as_deref() != Some(primary_digest.as_str()) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    verify_update_authorization_with_key(candidate, primary)
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn verify_update_authorization_with_key(
    candidate: &UpdateCandidate,
    public_hex: &str,
) -> Result<(), i32> {
    if candidate.authorization.scheme != "p256-sha256-v1" {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let public = decode_hex_bytes(public_hex, 65)?;
    let verifying_key =
        VerifyingKey::from_sec1_bytes(&public).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let signature_bytes = decode_base64(&candidate.authorization.signature)?;
    let signature = Signature::from_der(&signature_bytes).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    verifying_key
        .verify(&authorization_transcript(candidate)?, &signature)
        .map_err(|_| EXIT_IDENTITY_MISMATCH)
}

pub(super) fn authorization_transcript(candidate: &UpdateCandidate) -> Result<Vec<u8>, i32> {
    let package = decode_hash(&candidate.package_sha256).ok_or(EXIT_IDENTITY_MISMATCH)?;
    let layout = decode_hash(&candidate.package_layout_digest).ok_or(EXIT_IDENTITY_MISMATCH)?;
    let mut transcript = Vec::with_capacity(110);
    transcript.extend_from_slice(b"talking-quill/windows-update-authorization/v1\0");
    transcript.extend_from_slice(&package);
    transcript.extend_from_slice(&layout);
    Ok(transcript)
}

pub(super) fn decode_hex_bytes(value: &str, expected: usize) -> Result<Vec<u8>, i32> {
    if value.len() != expected.saturating_mul(2) {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    (0..expected)
        .map(|index| {
            u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|_| EXIT_IDENTITY_MISMATCH)
        })
        .collect()
}

pub(super) fn valid_source_identity(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn canonical_candidate_layout(candidate: &UpdateCandidate) -> Result<String, i32> {
    let mut hash = Sha256::new();
    hash.update(b"talking-quill/package-layout/v1\0");
    for (name, value) in [
        ("version", candidate.version.as_str()),
        ("platform", candidate.platform.as_str()),
        ("architecture", candidate.architecture.as_str()),
        ("ownerMode", candidate.owner_mode.as_str()),
        ("packageMode", candidate.package_mode.as_str()),
        ("sourceCommit", candidate.source_commit.as_str()),
        ("sourceTree", candidate.source_tree.as_str()),
    ] {
        hash_identity_field(&mut hash, name, value)?;
    }
    for role in &candidate.roles {
        hash_identity_field(
            &mut hash,
            "role",
            &format!(
                "{}\0{}\0{}\0{}",
                role.role, role.path, role.sha256, role.suppression_capable
            ),
        )?;
    }
    hash_identity_field(&mut hash, "predecessorPresent", "true")?;
    for (name, value) in [
        (
            "predecessorPlatform",
            candidate.predecessor.platform.as_str(),
        ),
        (
            "predecessorArchitecture",
            candidate.predecessor.architecture.as_str(),
        ),
        ("predecessorVersion", candidate.predecessor.version.as_str()),
        (
            "predecessorReleaseBuildDigest",
            candidate.predecessor.release_build_digest.as_str(),
        ),
        (
            "predecessorGatewaySha256",
            candidate.predecessor.gateway_sha256.as_str(),
        ),
        (
            "predecessorOwnerSha256",
            candidate.predecessor.owner_sha256.as_str(),
        ),
    ] {
        hash_identity_field(&mut hash, name, value)?;
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(super) fn hash_identity_field(hash: &mut Sha256, name: &str, value: &str) -> Result<(), i32> {
    let name_length = u16::try_from(name.len()).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let value_length = u32::try_from(value.len()).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    hash.update(name_length.to_be_bytes());
    hash.update(value_length.to_be_bytes());
    hash.update(name.as_bytes());
    hash.update(value.as_bytes());
    Ok(())
}

pub(super) fn update_role<'a>(roles: &'a [UpdateRole], name: &str) -> Result<&'a UpdateRole, i32> {
    roles
        .iter()
        .find(|value| value.role == name)
        .ok_or(EXIT_IDENTITY_MISMATCH)
}
