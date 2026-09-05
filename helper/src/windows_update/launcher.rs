//! Publish recovery launchers and retain generation-specific directories.
use super::*;

pub(super) fn published_recovery_launcher_name(
    source: &File,
    source_hash: [u8; 32],
) -> Result<String, i32> {
    let mut digest = Sha256::new();
    digest.update(source_hash);
    digest.update(file_identity_text(source)?.as_bytes());
    let hash = digest.finalize();
    let generation = hash[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{RECOVERY_LAUNCHER_PUBLISHED_PREFIX}{generation}.exe"
    ))
}

pub(super) fn ensure_medium_launcher(installed_helper: &Path) -> Result<PathBuf, i32> {
    let source = installed_helper
        .parent()
        .ok_or(EXIT_IDENTITY_MISMATCH)?
        .join(RECOVERY_LAUNCHER_NAME);
    let mut source_file = open_locked(&source).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let source_hash = hash_file(&mut source_file).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let published_name = published_recovery_launcher_name(&source_file, source_hash)?;
    let directory = medium_launcher_directory()?;
    reclaim_incomplete_launcher_directories(directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?)?;
    if !directory.exists() {
        publish_medium_launcher_directory(
            &directory,
            &mut source_file,
            source_hash,
            &published_name,
        )?;
        return Ok(directory.join(&published_name));
    }
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !has_exact_security(&directory, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let directory_identity = owned_tree_identity(&directory).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let identity_path = directory.join(RECOVERY_LAUNCHER_IDENTITY_NAME);
    if identity_path.exists() {
        if std::fs::read_to_string(&identity_path).map_err(|_| EXIT_IDENTITY_MISMATCH)?
            != directory_identity
            || !has_exact_security(&identity_path, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut identity_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&identity_path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&identity_path, MEDIUM_LAUNCHER_FILE_SDDL)?;
        identity_file
            .write_all(directory_identity.as_bytes())
            .and_then(|_| identity_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let target = directory.join(&published_name);
    if target.exists() {
        let mut existing = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut existing).map_err(|_| EXIT_IDENTITY_MISMATCH)? == source_hash
            && has_exact_security(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Ok(target);
        }
    }
    let temporary = directory.join(format!(".{published_name}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    std::io::copy(&mut source_file, &mut output).map_err(|_| EXIT_LAUNCH_FAILED)?;
    output.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(output);
    let mut copied = open_locked(&temporary).map_err(|_| EXIT_LAUNCH_FAILED)?;
    if hash_file(&mut copied).map_err(|_| EXIT_LAUNCH_FAILED)? != source_hash {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    drop(copied);
    if unsafe {
        MoveFileExW(
            wide_nul(&temporary)?.as_ptr(),
            wide_nul(&target)?.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let mut published = open_locked(&target).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if hash_file(&mut published).map_err(|_| EXIT_IDENTITY_MISMATCH)? != source_hash
        || !has_exact_security(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(target)
}

pub(super) fn publish_medium_launcher_directory(
    directory: &Path,
    source_file: &mut File,
    source_hash: [u8; 32],
    published_name: &str,
) -> Result<(), i32> {
    let parent = directory.parent().ok_or(EXIT_IDENTITY_MISMATCH)?;
    let token = new_recovery_generation()?;
    let pending = parent.join(format!("{RECOVERY_LAUNCHER_PENDING_PREFIX}{token}"));
    create_directory_with_sddl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    apply_restricted_dacl(&pending, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?;
    let identity = owned_tree_identity(&pending).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let result = (|| {
        let marker = pending.join(RECOVERY_LAUNCHER_IDENTITY_NAME);
        let mut marker_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&marker)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&marker, MEDIUM_LAUNCHER_FILE_SDDL)?;
        marker_file
            .write_all(identity.as_bytes())
            .and_then(|_| marker_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        let launcher = pending.join(published_name);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&launcher)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&launcher, MEDIUM_LAUNCHER_FILE_SDDL)?;
        source_file
            .seek(SeekFrom::Start(0))
            .and_then(|_| std::io::copy(source_file, &mut output).map(|_| ()))
            .and_then(|_| output.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        drop(output);
        let mut copied = open_locked(&launcher).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if hash_file(&mut copied).map_err(|_| EXIT_IDENTITY_MISMATCH)? != source_hash {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        drop(copied);
        if unsafe {
            MoveFileExW(
                wide_nul(&pending)?.as_ptr(),
                wide_nul(directory)?.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(EXIT_LAUNCH_FAILED);
        }
        if owned_tree_identity(directory).map_err(|_| EXIT_IDENTITY_MISMATCH)? != identity {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
        Ok(())
    })();
    if result.is_err() && pending.exists() {
        let _ = remove_owned_tree(&pending, &identity);
    }
    result
}

pub(super) fn reclaim_incomplete_launcher_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let pending = name
            .to_str()
            .and_then(|value| value.strip_prefix(RECOVERY_LAUNCHER_PENDING_PREFIX))
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !pending {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, MEDIUM_LAUNCHER_DIRECTORY_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    Ok(())
}

// A normal Run value is the durable retry record. Windows must not consume recovery ownership
// before the elevated child reports success, as RunOnce does even when UAC is cancelled.
pub(super) const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub(super) const RUN_ONCE_VALUE_PREFIX: &str = "Talking Quill Update Recovery ";
pub(super) const MAX_VISIBLE_RECOVERY_ATTEMPTS: u8 = 3;
pub(super) const RECOVERY_REQUEST_FILE: &str = "update-recovery-request-v2.txt";

pub(super) fn recovery_directory() -> Result<PathBuf, i32> {
    std::env::current_exe()
        .map_err(|_| EXIT_LAUNCH_FAILED)?
        .parent()
        .map(Path::to_owned)
        .ok_or(EXIT_LAUNCH_FAILED)
}

pub(super) fn recovery_request_path() -> Result<PathBuf, i32> {
    Ok(recovery_directory()?.join(RECOVERY_REQUEST_FILE))
}

pub(super) fn medium_launcher_directory() -> Result<PathBuf, i32> {
    Ok(known_folder(&FOLDERID_ProgramData)?.join("Talking Quill Update Recovery"))
}

pub(super) fn valid_published_recovery_launcher(path: &Path, directory: &Path) -> bool {
    path.parent() == Some(directory)
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix(RECOVERY_LAUNCHER_PUBLISHED_PREFIX))
            .and_then(|value| value.strip_suffix(".exe"))
            .is_some_and(|generation| {
                generation.len() == 32
                    && generation
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

pub(super) fn medium_launcher_path() -> Result<PathBuf, i32> {
    let directory = medium_launcher_directory()?;
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    if valid_published_recovery_launcher(&current, &directory) {
        return Ok(current);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    } != 0
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let command = read_registry_string(key, RELAUNCH_RUN_VALUE)?;
    unsafe { RegCloseKey(key) };
    let Some(path) = command
        .as_deref()
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix("\" --windows-update-relaunch-owner-v1"))
        .map(PathBuf::from)
    else {
        return Err(EXIT_IDENTITY_MISMATCH);
    };
    if valid_published_recovery_launcher(&path, &directory) {
        Ok(path)
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn recovery_binding_path(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    Ok(medium_launcher_directory()?.join(format!("recovery-binding-v1-{generation}")))
}

pub(super) fn medium_recovery_directory(generation: &str) -> Result<PathBuf, i32> {
    let launcher_directory = medium_launcher_directory()?;
    if !has_exact_security(&launcher_directory, MEDIUM_LAUNCHER_DIRECTORY_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let binding = recovery_binding_path(generation)?;
    if !has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let suffix = std::fs::read_to_string(binding).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    Ok(known_folder(&FOLDERID_ProgramData)?
        .join(format!(".Talking Quill.update-bootstrap-{suffix}")))
}

pub(super) fn recovery_command_present(generation: &str) -> Result<bool, i32> {
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(false);
    }
    if opened != 0 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let name = wide_nul(Path::new(&recovery_value_name(generation)?))?;
    let mut kind = 0_u32;
    let mut bytes = 0_u32;
    let first = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if first == 2 {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    if first != 0 || kind != REG_SZ || !(2..=2048).contains(&bytes) || !bytes.is_multiple_of(2) {
        unsafe { RegCloseKey(key) };
        return Ok(false);
    }
    let mut value = vec![0_u16; bytes as usize / 2];
    let second = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    unsafe { RegCloseKey(key) };
    if second != 0 || kind != REG_SZ {
        return Ok(false);
    }
    if value.last() == Some(&0) {
        value.pop();
    }
    let command = String::from_utf16(&value).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    let launcher = medium_launcher_path()?;
    let resume = format!(
        "\"{}\" --windows-update-resume-v2={generation}",
        launcher.display()
    );
    let cleanup = std::fs::read_to_string(recovery_binding_path(generation)?)
        .ok()
        .map(|suffix| {
            format!(
                "\"{}\" --windows-update-cleanup-v1={suffix}:{generation}",
                launcher.display()
            )
        });
    Ok(command == resume || cleanup.as_ref().is_some_and(|cleanup| cleanup == &command))
}

pub(super) fn owned_recovery_generations() -> Result<std::collections::BTreeSet<String>, i32> {
    let mut generations = std::collections::BTreeSet::new();
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    if opened == 0 {
        let mut index = 0_u32;
        loop {
            let mut name = [0_u16; 128];
            let mut length = name.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == 259 {
                break;
            }
            if status != 0 {
                unsafe { RegCloseKey(key) };
                return Err(EXIT_LAUNCH_FAILED);
            }
            let value = String::from_utf16_lossy(&name[..length as usize]);
            if let Some(generation) = value.strip_prefix(RUN_ONCE_VALUE_PREFIX)
                && validate_generation(generation).is_ok()
            {
                generations.insert(generation.to_owned());
            }
            index += 1;
        }
        unsafe { RegCloseKey(key) };
    } else if opened != 2 {
        return Err(EXIT_LAUNCH_FAILED);
    }
    if let Ok(entries) = std::fs::read_dir(medium_launcher_directory()?) {
        for entry in entries.flatten() {
            if let Some(generation) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("recovery-binding-v1-"))
                && validate_generation(generation).is_ok()
            {
                generations.insert(generation.to_owned());
            }
        }
    }
    Ok(generations)
}

pub(super) fn reclaim_incomplete_recovery_directories(root: &Path) -> Result<(), i32> {
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let pending = name
            .strip_prefix(".Talking Quill.update-bootstrap-pending-")
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        let published = name
            .strip_prefix(".Talking Quill.update-bootstrap-")
            .is_some_and(|suffix| {
                suffix.len() == 16
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !pending && !published {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        if !metadata.is_dir()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !has_exact_security(&path, RESTRICTED_STAGING_SDDL)?
        {
            continue;
        }
        let identity = owned_tree_identity(&path).map_err(|_| EXIT_IDENTITY_MISMATCH)?;
        let marker = std::fs::read_to_string(path.join("cleanup-tree-identity-v1"));
        if marker.as_ref().is_ok_and(|value| value != &identity) {
            continue;
        }
        if published && marker.is_err() {
            continue;
        }
        let generation = read_active_generation(&path).ok();
        let complete = generation.as_deref().is_some_and(|generation| {
            protected_visible_attempt(&path, generation).is_some()
                && recovery_binding_path(generation).is_ok_and(|binding| {
                    has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL).unwrap_or(false)
                        && std::fs::read_to_string(binding)
                            .is_ok_and(|suffix| Some(suffix.as_str()) == name.rsplit('-').next())
                })
                && recovery_command_present(generation).unwrap_or(false)
        });
        if pending || !complete {
            if let Some(generation) = generation.as_deref() {
                let _ = clear_restart_recovery(generation);
            }
            remove_owned_tree(&path, &identity).map_err(|_| EXIT_LAUNCH_FAILED)?;
        }
    }
    for generation in owned_recovery_generations()? {
        let complete = medium_recovery_directory(&generation).is_ok_and(|directory| {
            directory.exists()
                && read_active_generation(&directory).is_ok_and(|active| active == generation)
                && protected_visible_attempt(&directory, &generation).is_some()
                && recovery_command_present(&generation).unwrap_or(false)
        });
        if !complete {
            clear_restart_recovery(&generation)?;
        }
    }
    Ok(())
}

pub(super) fn find_recovery_directory(generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    let root = known_folder(&FOLDERID_ProgramData)?;
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|_| EXIT_LAUNCH_FAILED)? {
        let entry = entry.map_err(|_| EXIT_LAUNCH_FAILED)?;
        let name = entry.file_name();
        let Some(suffix) = name
            .to_str()
            .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        else {
            continue;
        };
        if suffix.len() != 16
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            continue;
        }
        let path = entry.path();
        let valid = std::fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
        }) && has_exact_security(&path, RESTRICTED_STAGING_SDDL).unwrap_or(false)
            && std::fs::read_to_string(path.join("cleanup-tree-identity-v1"))
                .ok()
                .is_some_and(|identity| {
                    !identity.is_empty()
                        && identity.len() <= 256
                        && owned_tree_identity(&path).is_ok_and(|actual| actual == identity)
                });
        if valid && read_active_generation(&path).is_ok_and(|active| active == generation) {
            matches.push(path);
        }
    }
    if matches.len() == 1 {
        Ok(matches.remove(0))
    } else {
        Err(EXIT_IDENTITY_MISMATCH)
    }
}

pub(super) fn visible_retry_path(directory: &Path, generation: &str) -> Result<PathBuf, i32> {
    validate_generation(generation)?;
    Ok(directory.join(format!("visible-attempt-v1-{generation}")))
}
