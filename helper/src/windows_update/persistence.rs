//! Bound retries and persist restart recovery records.
use super::*;

pub(super) fn protected_visible_attempt(directory: &Path, generation: &str) -> Option<u8> {
    let path = visible_retry_path(directory, generation).ok()?;
    if !has_exact_security(&path, RETRY_COUNTER_SDDL).ok()? {
        return None;
    }
    let length = std::fs::metadata(path).ok()?.len();
    u8::try_from(length).ok()
}

pub(super) fn begin_protected_visible_retry(directory: &Path, generation: &str) -> Result<u8, i32> {
    let path = visible_retry_path(directory, generation)?;
    if !has_exact_security(&path, RETRY_COUNTER_SDDL)? {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .share_mode(0)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    let attempt = u8::try_from(file.metadata().map_err(|_| EXIT_LAUNCH_FAILED)?.len())
        .map_err(|_| EXIT_IDENTITY_MISMATCH)?;
    if attempt >= MAX_VISIBLE_RECOVERY_ATTEMPTS {
        return Ok(attempt.saturating_add(1));
    }
    file.write_all(&[1])
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    Ok(attempt + 1)
}

pub(super) fn show_visible_retry_paused() {
    let text = wide_nul(Path::new(
        "Talking Quill could not obtain administrator approval after three attempts. Automatic update prompts are paused and the recovery generation is retained. Open Apps > Installed apps and choose Uninstall for Talking Quill to run maintenance. Maintenance recovers the installed state before continuing.",
    ));
    let title = wide_nul(Path::new("Talking Quill recovery paused"));
    if let (Ok(text), Ok(title)) = (text, title) {
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONWARNING,
            )
        };
    }
}

pub(super) fn persist_request(encoded: &str) -> Result<(), i32> {
    let path = recovery_request_path()?;
    if path.exists() {
        return if std::fs::read_to_string(path).map_err(|_| EXIT_LAUNCH_FAILED)? == encoded {
            Ok(())
        } else {
            Err(EXIT_IDENTITY_MISMATCH)
        };
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&path, RESTRICTED_FILE_SDDL)?;
    file.write_all(encoded.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn acknowledge_recovery_ownership() -> Result<(), i32> {
    let path = recovery_directory()?.join("recovery-owned-v2");
    if path.exists() {
        return if std::fs::read(&path).map_err(|_| EXIT_LAUNCH_FAILED)? == b"owned-v2" {
            Ok(())
        } else {
            Err(EXIT_IDENTITY_MISMATCH)
        };
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&path, RESTRICTED_FILE_SDDL)?;
    file.write_all(b"owned-v2")
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)
}

pub(super) fn read_persisted_request() -> Result<String, i32> {
    let value =
        std::fs::read_to_string(recovery_request_path()?).map_err(|_| EXIT_INVALID_REQUEST)?;
    if value.is_empty() || value.len() > 64 * 1024 {
        return Err(EXIT_INVALID_REQUEST);
    }
    Ok(value)
}

pub(super) fn validate_generation(generation: &str) -> Result<(), i32> {
    if generation.len() == 32
        && generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(EXIT_INVALID_REQUEST)
    }
}

pub(super) fn new_recovery_generation() -> Result<String, i32> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| EXIT_LAUNCH_FAILED)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn recovery_value_name(generation: &str) -> Result<String, i32> {
    validate_generation(generation)?;
    Ok(format!("{RUN_ONCE_VALUE_PREFIX}{generation}"))
}

pub(super) fn active_generation_path(directory: &Path) -> PathBuf {
    directory.join("active-recovery-generation-v1")
}

pub(super) fn persist_active_generation(directory: &Path, generation: &str) -> Result<(), i32> {
    validate_generation(generation)?;
    let counter = visible_retry_path(directory, generation)?;
    if counter.exists() {
        if !has_exact_security(&counter, RETRY_COUNTER_SDDL)? {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&counter)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&counter, RETRY_COUNTER_SDDL)?;
        file.sync_all().map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let suffix = published_recovery_suffix(directory)?;
    let binding = recovery_binding_path(generation)?;
    if binding.exists() {
        if std::fs::read_to_string(&binding).map_err(|_| EXIT_IDENTITY_MISMATCH)? != suffix
            || !has_exact_security(&binding, MEDIUM_LAUNCHER_FILE_SDDL)?
        {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&binding)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&binding, MEDIUM_LAUNCHER_FILE_SDDL)?;
        file.write_all(suffix.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    let target = active_generation_path(directory);
    let temporary = directory.join(format!(
        ".active-recovery-generation-v1.tmp-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .open(&temporary)
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    apply_restricted_dacl(&temporary, RESTRICTED_FILE_SDDL)?;
    file.write_all(generation.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    drop(file);
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
    Ok(())
}

pub(super) fn published_recovery_suffix(directory: &Path) -> Result<&str, i32> {
    directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .filter(|value| {
            value.len() == 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)
}

pub(super) fn read_active_generation(directory: &Path) -> Result<String, i32> {
    let generation = std::fs::read_to_string(active_generation_path(directory))
        .map_err(|_| EXIT_LAUNCH_FAILED)?;
    validate_generation(&generation)?;
    Ok(generation)
}

pub(super) fn persist_run_once(command: &str, generation: &str) -> Result<(), i32> {
    if command.encode_utf16().count() + 1 > 260 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let command = wide_nul(Path::new(command))?;
    let status = unsafe {
        RegSetValueExW(
            key,
            wide_nul(Path::new(&recovery_value_name(generation)?))?.as_ptr(),
            0,
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

pub(super) fn persist_restart_recovery(directory: &Path, generation: &str) -> Result<(), i32> {
    if read_active_generation(directory)? != generation
        || protected_visible_attempt(directory, generation).is_none()
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let launcher = medium_launcher_path()?;
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-resume-v2={generation}",
            launcher.display()
        ),
        generation,
    )
}

pub(super) fn persist_prelaunch_cleanup(
    installed_helper: &Path,
    directory: &Path,
    identity: &str,
    generation: &str,
) -> Result<(), i32> {
    let suffix = directory
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".Talking Quill.update-bootstrap-"))
        .filter(|value| {
            value.len() == 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or(EXIT_IDENTITY_MISMATCH)?;
    let identity_path = directory.join("cleanup-tree-identity-v1");
    if identity_path.exists() {
        if std::fs::read_to_string(&identity_path).map_err(|_| EXIT_LAUNCH_FAILED)? != identity {
            return Err(EXIT_IDENTITY_MISMATCH);
        }
    } else {
        let mut identity_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&identity_path)
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
        apply_restricted_dacl(&identity_path, RESTRICTED_FILE_SDDL)?;
        identity_file
            .write_all(identity.as_bytes())
            .and_then(|_| identity_file.sync_all())
            .map_err(|_| EXIT_LAUNCH_FAILED)?;
    }
    persist_active_generation(directory, generation)?;
    let binding = format!("{suffix}:{generation}");
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-cleanup-v1={binding}",
            installed_helper.display()
        ),
        generation,
    )
}

pub(super) fn clear_restart_recovery(generation: &str) -> Result<(), i32> {
    let mut key = std::ptr::null_mut();
    if unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide_nul(Path::new(RUN_ONCE_KEY))?.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return Err(EXIT_LAUNCH_FAILED);
    }
    let status = unsafe {
        RegDeleteValueW(
            key,
            wide_nul(Path::new(&recovery_value_name(generation)?))?.as_ptr(),
        )
    };
    let flushed = (status == 0 || status == 2) && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        let _ = std::fs::remove_file(recovery_binding_path(generation)?);
        Ok(())
    } else {
        Err(EXIT_LAUNCH_FAILED)
    }
}

pub(super) fn schedule_staged_cleanup(previous_generation: Option<&str>) -> Result<(), i32> {
    let _state = RecoveryStateLock::acquire()?;
    let current = std::env::current_exe().map_err(|_| EXIT_LAUNCH_FAILED)?;
    let directory = current
        .parent()
        .map(Path::to_owned)
        .ok_or(EXIT_LAUNCH_FAILED)?;
    if directory
        .file_name()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.starts_with(".Talking Quill.update-bootstrap-"))
    {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let generation = previous_generation.ok_or(EXIT_INVALID_REQUEST)?;
    if read_active_generation(&directory)? != generation {
        return Err(EXIT_IDENTITY_MISMATCH);
    }
    let launcher = medium_launcher_path()?;
    let suffix = published_recovery_suffix(&directory)?;
    persist_run_once(
        &format!(
            "\"{}\" --windows-update-cleanup-v1={suffix}:{generation}",
            launcher.display()
        ),
        generation,
    )?;
    spawn_staged_cleanup(&directory, generation)
}
