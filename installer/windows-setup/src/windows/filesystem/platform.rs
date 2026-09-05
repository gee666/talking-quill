//! Windows known folders, token inspection, and setup dialogs.
use super::*;

pub(in super::super) fn known_folder(
    identifier: *const windows_sys::core::GUID,
) -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(identifier, 0, ptr::null_mut(), &mut raw) } != 0
        || raw.is_null()
    {
        return Err(fail(EXIT_FAILURE, "Windows known-folder lookup failed."));
    }
    let length = unsafe { (0..).position(|index| *raw.add(index) == 0).unwrap_or(0) };
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(PathBuf::from(value))
}

pub(in super::super) fn token_is_elevated() -> Result<bool> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(fail(EXIT_REJECTED, "Cannot inspect the setup token."));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(fail(EXIT_REJECTED, "Cannot read setup elevation."));
    }
    Ok(elevation.TokenIsElevated != 0)
}

pub(in super::super) fn message_box(text: &str, flags: u32) -> i32 {
    let caption = wide(OsStr::new(concat!(
        "Talking Quill ",
        env!("CARGO_PKG_VERSION"),
        " setup"
    )));
    let text = wide(OsStr::new(text));
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            flags | MB_SETFOREGROUND,
        )
    }
}
pub(in super::super) fn report(message: &str) {
    message_box(message, 0x10);
}
pub(in super::super) fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}
