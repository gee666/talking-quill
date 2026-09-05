//! Final launcher publication and machine relaunch owner retirement.
use super::*;

pub(in super::super) fn terminal_final_launcher(
    paths: &Paths,
    generation: &str,
) -> Result<PathBuf> {
    validate_machine_lock_suffix(generation)?;
    Ok(paths
        .program_data
        .join(format!("{TERMINAL_FINAL_LAUNCHER_PREFIX}{generation}.exe")))
}

pub(in super::super) fn publish_terminal_final_launcher(
    paths: &Paths,
    generation: &str,
) -> Result<PathBuf> {
    let source = paths.recovery_launcher.clone();
    let target = terminal_final_launcher(paths, generation)?;
    if !path_present(&target)? {
        let temporary = paths.program_data.join(format!(
            ".Talking Quill.terminal-relaunch-pending-{}.exe",
            random_machine_lock_suffix()?
        ));
        fs::copy(&source, &temporary).map_err(io_failure)?;
        apply_lock_dacl(&temporary, MEDIUM_LAUNCHER_FILE_SDDL)?;
        flush_file(&temporary)?;
        durable_replace(&temporary, &target)?;
    }
    if file_hash(&source)? != file_hash(&target)?
        || !marker_security_is_exact(&target, MEDIUM_LAUNCHER_FILE_SDDL)?
    {
        return Err(fail(EXIT_REJECTED, "Terminal final launcher is invalid."));
    }
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    let mut key = ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    } != 0
    {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
    }
    let command = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        target.display()
    );
    let value = wide(OsStr::new(&command));
    let status = unsafe {
        RegSetValueExW(
            key,
            wide(OsStr::new("Talking Quill Update Relaunch")).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    let flushed = status == 0 && unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if !flushed {
        return Err(fail(
            EXIT_FAILURE,
            "Cannot publish terminal final launcher.",
        ));
    }
    flush_setup_directory(&paths.program_data)?;
    Ok(target)
}

pub(in super::super) fn clear_machine_relaunch_owner(paths: &Paths) -> Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Talking Quill Update Relaunch";
    let mut key = ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(OsStr::new(RUN_KEY)).as_ptr(),
            0,
            KEY_READ | KEY_WRITE,
            &mut key,
        )
    };
    if opened == 2 {
        return Ok(());
    }
    if opened != 0 {
        return Err(fail(EXIT_FAILURE, "Cannot open machine relaunch owner."));
    }
    let launcher = paths.recovery_launcher.clone();
    let expected = format!(
        "\"{}\" --windows-update-relaunch-owner-v1",
        launcher.display()
    );
    let actual = read_registry_value(key, VALUE, 1024)?;
    let final_owner = actual.as_deref().is_some_and(|value| {
        let prefix = format!(
            "\"{}\\{TERMINAL_FINAL_LAUNCHER_PREFIX}",
            paths.program_data.display()
        );
        let suffix = ".exe\" --windows-update-relaunch-owner-v1";
        value.starts_with(&prefix)
            && value.ends_with(suffix)
            && value[prefix.len()..value.len() - suffix.len()].len() == 32
            && value[prefix.len()..value.len() - suffix.len()]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    });
    if actual
        .as_deref()
        .is_some_and(|value| value != expected && !final_owner)
    {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_REJECTED, "Machine relaunch owner was replaced."));
    }
    if actual.is_some() && unsafe { RegDeleteValueW(key, wide(OsStr::new(VALUE)).as_ptr()) } != 0 {
        unsafe { RegCloseKey(key) };
        return Err(fail(EXIT_FAILURE, "Cannot retire machine relaunch owner."));
    }
    let flushed = unsafe { RegFlushKey(key) } == 0;
    unsafe { RegCloseKey(key) };
    if flushed {
        Ok(())
    } else {
        Err(fail(
            EXIT_FAILURE,
            "Cannot flush machine relaunch retirement.",
        ))
    }
}
