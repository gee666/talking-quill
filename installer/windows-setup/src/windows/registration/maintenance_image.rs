//! Maintenance image copying, repair restoration, and residue cleanup.
use super::*;

pub(in super::super) fn restore_repair_controller(paths: &Paths) -> Result<()> {
    let source = paths.backup.join("Uninstall Talking Quill.exe");
    let target = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    assert_plain_file(&target)?;
    let temporary = paths.install.join(".repair-controller-recovery.tmp");
    if path_present(&temporary)? {
        assert_plain_file(&temporary)?;
        if file_hash(&temporary)? == file_hash(&source)? {
            return durable_replace(&temporary, &target);
        }
        // The fixed plain file is installer-owned inside the protected target tree;
        // a mismatched value is an interrupted copy and is safe to recreate.
        fs::remove_file(&temporary).map_err(io_failure)?;
    }
    let mut input = File::open(source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    durable_replace(&temporary, &target)
}

pub(in super::super) fn remove_maintenance_temporary_files(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let owned = name
            .to_str()
            .and_then(|value| value.strip_prefix("Talking Quill Maintenance."))
            .and_then(|value| value.strip_suffix(".tmp"))
            .is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if owned {
            assert_plain_file(&entry.path())?;
            fs::remove_file(entry.path()).map_err(io_failure)?;
        }
    }
    Ok(())
}

pub(in super::super) fn remove_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    if path_present(&paths.maintenance_uninstaller)? {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        fs::remove_file(&paths.maintenance_uninstaller).map_err(io_failure)?;
    }
    Ok(())
}

pub(in super::super) fn ensure_maintenance_uninstaller(paths: &Paths) -> Result<()> {
    remove_maintenance_temporary_files(paths)?;
    let source = paths.install.join("Uninstall Talking Quill.exe");
    assert_plain_file(&source)?;
    let replacing = path_present(&paths.maintenance_uninstaller)?;
    if replacing {
        assert_plain_file(&paths.maintenance_uninstaller)?;
        if file_hash(&source)? == file_hash(&paths.maintenance_uninstaller)? {
            return Ok(());
        }
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = paths
        .maintenance_uninstaller
        .with_extension(format!("{suffix}.tmp"));
    let mut input = File::open(&source).map_err(io_failure)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    std::io::copy(&mut input, &mut output).map_err(io_failure)?;
    output.sync_all().map_err(io_failure)?;
    drop(output);
    if file_hash(&source)? != file_hash(&temporary)? {
        return Err(fail(
            EXIT_REJECTED,
            "Maintenance uninstaller verification failed.",
        ));
    }
    if replacing {
        durable_replace(&temporary, &paths.maintenance_uninstaller)
    } else {
        durable_rename(&temporary, &paths.maintenance_uninstaller)
    }
}

pub(in super::super) fn reclaim_stale_maintenance_uninstallers(paths: &Paths) -> Result<()> {
    let parent = paths
        .maintenance_uninstaller
        .parent()
        .ok_or_else(|| fail(EXIT_REJECTED, "Maintenance path has no parent."))?;
    for entry in fs::read_dir(parent).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let path = entry.path();
        if path == paths.maintenance_uninstaller {
            continue;
        }
        let stale = entry
            .file_name()
            .to_str()
            .and_then(maintenance_generation_from_name)
            .is_some();
        if stale {
            assert_plain_file(&path)?;
            fs::remove_file(path).map_err(io_failure)?;
        }
    }
    flush_setup_directory(parent)
}
