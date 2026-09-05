//! Locked file identity, path comparison, hashes, and bounded decoding.
use super::*;

pub(super) fn verify_suspended_process(
    process: HANDLE,
    expected_path: &Path,
    expected_identity: FileIdentity,
    expected_hash: [u8; 32],
) -> Result<(), ()> {
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err(());
    }
    path.truncate(length as usize);
    let launched = PathBuf::from(String::from_utf16(&path).map_err(|_| ())?);
    if !paths_equal(&launched, expected_path) {
        return Err(());
    }
    let mut reopened = open_locked(&launched).map_err(|_| ())?;
    if file_identity(&reopened).map_err(|_| ())? != expected_identity
        || hash_file(&mut reopened).map_err(|_| ())? != expected_hash
    {
        return Err(());
    }
    Ok(())
}

pub(super) fn open_locked(path: &Path) -> std::io::Result<File> {
    let raw = unsafe {
        CreateFileW(
            wide_nul(path)
                .map_err(|_| std::io::Error::other("invalid installer path"))?
                .as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::other(
            "installer is not a plain regular file",
        ));
    }
    Ok(file)
}

pub(super) fn file_identity(file: &File) -> std::io::Result<FileIdentity> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        index_high: info.nFileIndexHigh,
        index_low: info.nFileIndexLow,
    })
}

pub(super) fn hash_file(file: &mut File) -> std::io::Result<[u8; 32]> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(digest.finalize().into())
}

pub(super) fn paths_equal(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let text = path.as_os_str().to_string_lossy();
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    }
    normalized(left).eq_ignore_ascii_case(&normalized(right))
}

pub(super) fn wide_nul(path: &Path) -> Result<Vec<u16>, i32> {
    use std::os::windows::ffi::OsStrExt;
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.is_empty() || value.contains(&0) {
        return Err(EXIT_INVALID_REQUEST);
    }
    value.push(0);
    Ok(value)
}

pub(super) fn decode_hash(value: &str) -> Option<[u8; 32]> {
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(output)
}

pub(super) fn decode_base64(value: &str) -> Result<Vec<u8>, i32> {
    if value.is_empty() || !value.len().is_multiple_of(4) || value.len() > 64 * 1024 {
        return Err(EXIT_INVALID_REQUEST);
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    for chunk in value.as_bytes().chunks_exact(4) {
        let a = base64_value(chunk[0]).ok_or(EXIT_INVALID_REQUEST)?;
        let b = base64_value(chunk[1]).ok_or(EXIT_INVALID_REQUEST)?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2]).ok_or(EXIT_INVALID_REQUEST)?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3]).ok_or(EXIT_INVALID_REQUEST)?
        };
        if chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(EXIT_INVALID_REQUEST);
        }
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

pub(super) fn base64_value(value: u8) -> Option<u8> {
    match value {
        b'A'..=b'Z' => Some(value - b'A'),
        b'a'..=b'z' => Some(value - b'a' + 26),
        b'0'..=b'9' => Some(value - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
