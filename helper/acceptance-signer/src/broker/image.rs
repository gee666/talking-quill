use super::*;

pub(super) fn process_snapshot(process: HANDLE) -> Result<Snapshot, &'static str> {
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err("process image path");
    }
    path.truncate(length as usize);
    let path = PathBuf::from(String::from_utf16(&path).map_err(|_| "process image path")?);
    let mut image = open_locked(&path)?;
    snapshot(&mut image)
}

pub(super) fn retain_self(
    expected_sha256: [u8; 32],
    expected_bytes: u64,
) -> Result<File, &'static str> {
    let path = std::env::current_exe().map_err(|_| "broker path")?;
    require_absolute_no_reparse(&path)?;
    let mut file = open_locked(&path)?;
    let observed = snapshot(&mut file)?;
    if observed.sha256 != expected_sha256 || observed.bytes != expected_bytes {
        return Err("broker identity");
    }
    Ok(file)
}

pub(super) fn open_locked(path: &Path) -> Result<File, &'static str> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| "file open")
}

pub(super) fn snapshot(file: &mut File) -> Result<Snapshot, &'static str> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || info.nNumberOfLinks != 1
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("image identity");
    }
    let bytes = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    if !(1..=MAX_IMAGE).contains(&bytes) {
        return Err("image size");
    }
    file.rewind().map_err(|_| "image rewind")?;
    let mut hash = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| "image read")?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > MAX_IMAGE {
            return Err("image size");
        }
        hash.update(&buffer[..count]);
    }
    file.rewind().map_err(|_| "image rewind")?;
    if copied != bytes {
        return Err("image changed");
    }
    Ok(Snapshot {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        bytes,
        sha256: hash.finalize().into(),
    })
}

pub(super) fn source_matches(
    file: &mut File,
    commit: Option<&str>,
    tree: Option<&str>,
) -> Result<bool, &'static str> {
    if commit.is_none() && tree.is_none() {
        return Ok(true);
    }
    let (Some(commit), Some(tree)) = (commit, tree) else {
        return Ok(false);
    };
    if !is_lower_hex(commit, 40) || !is_lower_hex(tree, 40) {
        return Ok(false);
    }
    file.rewind().map_err(|_| "source read")?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| "source read")?;
    file.rewind().map_err(|_| "source read")?;
    Ok(contains_once(
        &bytes,
        format!("TALKING_QUILL_SOURCE_COMMIT={commit}").as_bytes(),
    ) && contains_once(
        &bytes,
        format!("TALKING_QUILL_SOURCE_TREE={tree}").as_bytes(),
    ))
}

fn contains_once(haystack: &[u8], needle: &[u8]) -> bool {
    let mut matches = haystack
        .windows(needle.len())
        .filter(|window| *window == needle);
    matches.next().is_some() && matches.next().is_none()
}

pub(super) fn protected_snapshot_directory() -> Result<SnapshotDir, &'static str> {
    let parent = std::env::temp_dir();
    require_absolute_no_reparse(&parent)?;
    let security = talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon()
        .map_err(|_| "snapshot security")?;
    for _ in 0..32 {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| "snapshot random")?;
        let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = parent.join(format!("talking-quill-acceptance-signer-{suffix}"));
        let wide_path = wide(path.as_os_str())?;
        if unsafe { CreateDirectoryW(wide_path.as_ptr(), security.attributes()) } != 0 {
            require_absolute_no_reparse(&path)?;
            return Ok(SnapshotDir(path));
        }
    }
    Err("snapshot directory collision")
}

pub(super) fn require_absolute_no_reparse(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute() {
        return Err("path absolute");
    }
    let mut current = Some(path);
    while let Some(component) = current {
        let metadata = std::fs::symlink_metadata(component).map_err(|_| "path metadata")?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("path reparse");
        }
        current = component.parent();
    }
    Ok(())
}
