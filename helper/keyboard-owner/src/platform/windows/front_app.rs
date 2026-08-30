use std::path::Path;

use windows_sys::Win32::{
    Foundation::{CloseHandle, RECT},
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId,
    },
};

use crate::platform::{FrontApp, PlatformError, WindowBounds};

const MAX_WINDOW_TITLE_UNITS: usize = 4096;
const MAX_PROCESS_PATH_UNITS: usize = 32_768;

pub(super) fn front_app() -> Result<FrontApp, PlatformError> {
    // SAFETY: GetForegroundWindow takes no pointers.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return Err(PlatformError::NativeFailure);
    }

    // SAFETY: `window` is a live HWND returned by the OS.
    let title_length = unsafe { GetWindowTextLengthW(window) };
    let title_capacity = usize::try_from(title_length.max(0))
        .unwrap_or(0)
        .saturating_add(1)
        .min(MAX_WINDOW_TITLE_UNITS);
    let mut title = vec![0_u16; title_capacity.max(1)];
    // SAFETY: the title buffer is writable for its declared length.
    let copied = unsafe {
        GetWindowTextW(
            window,
            title.as_mut_ptr(),
            i32::try_from(title.len()).map_err(|_| PlatformError::NativeFailure)?,
        )
    };
    let copied = usize::try_from(copied.max(0)).unwrap_or(0);
    let window_title = String::from_utf16_lossy(&title[..copied]);

    let mut process_id = 0_u32;
    // SAFETY: `process_id` is valid writable storage.
    unsafe { GetWindowThreadProcessId(window, &raw mut process_id) };
    if process_id == 0 {
        return Err(PlatformError::NativeFailure);
    }
    // SAFETY: requested rights are read-only and process_id came from Windows.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(PlatformError::NativeFailure);
    }

    let mut path = vec![0_u16; MAX_PROCESS_PATH_UNITS];
    let mut path_length = u32::try_from(path.len()).expect("bounded path length fits u32");
    // SAFETY: `process` is valid and the path buffer length matches the supplied
    // mutable size value.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &raw mut path_length,
        )
    };
    // SAFETY: process is an owned handle opened above.
    unsafe { CloseHandle(process) };
    if queried == 0 {
        return Err(PlatformError::NativeFailure);
    }
    let path = String::from_utf16_lossy(
        &path[..usize::try_from(path_length).map_err(|_| PlatformError::NativeFailure)?],
    );
    let process_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path)
        .to_owned();

    let mut rectangle = RECT::default();
    // SAFETY: rectangle is writable and window is the foreground HWND retained for this query.
    let window_bounds = if unsafe { GetWindowRect(window, &raw mut rectangle) } != 0 {
        let width = rectangle.right.saturating_sub(rectangle.left);
        let height = rectangle.bottom.saturating_sub(rectangle.top);
        match (u32::try_from(width), u32::try_from(height)) {
            (Ok(width), Ok(height)) if width > 0 && height > 0 => Some(WindowBounds {
                x: rectangle.left,
                y: rectangle.top,
                width,
                height,
            }),
            _ => None,
        }
    } else {
        None
    };

    Ok(FrontApp {
        process_name,
        window_title,
        window_bounds,
    })
}
