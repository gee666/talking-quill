//! Bounded shutdown of processes mapped from the installation being replaced.

use super::*;
use std::os::windows::process::CommandExt;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, PROCESS_TERMINATE};

pub(super) fn request_runtime_exit(paths: &Paths, ignored_process: Option<u32>) -> Result<()> {
    if !runtime_process_active(paths, ignored_process)? {
        return Ok(());
    }
    let application = paths.install.join("Talking Quill.exe");
    // Old or partially removed applications may not start at all. The installer
    // must still be able to replace them using its own process cleanup below.
    if application.is_file() {
        assert_plain_file(&application)?;
        if let Ok(mut request) = Command::new(&application)
            .arg("--talking-quill-request-machine-quit")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if request.try_wait().map_err(io_failure)?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if request.try_wait().map_err(io_failure)?.is_none() {
                let _ = request.kill();
                let _ = request.wait();
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !runtime_process_active(paths, ignored_process)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Enumerate again after termination in case an old supervisor was still
    // launching children. Every termination uses a retained, path-checked handle.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        terminate_tree_processes(&paths.install, ignored_process)?;
        if !runtime_process_active(paths, ignored_process)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(fail(
                EXIT_FAILURE,
                "Windows has not released the Talking Quill files. Retry after the blocking process exits.",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn runtime_process_active(paths: &Paths, ignored: Option<u32>) -> Result<bool> {
    Ok(process_ids()?.into_iter().any(|pid| {
        pid != std::process::id()
            && Some(pid) != ignored
            && process_image(pid).is_ok_and(|image| image_is_within(&image, &paths.install))
    }))
}

pub(super) fn terminate_tree_processes(root: &Path, ignored: Option<u32>) -> Result<()> {
    for pid in process_ids()? {
        if pid == std::process::id() || Some(pid) == ignored {
            continue;
        }
        if !process_image(pid).is_ok_and(|image| image_is_within(&image, root)) {
            continue;
        }
        // SAFETY: OpenProcess returns an owned handle or null. Query and terminate
        // use that same handle, so PID reuse cannot redirect the termination.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            if process_image(pid).is_ok_and(|image| image_is_within(&image, root)) {
                return Err(fail(
                    EXIT_FAILURE,
                    "Cannot close a Talking Quill process. Administrative installation is required.",
                ));
            }
            continue;
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        if !process_image_from_handle(&process).is_ok_and(|image| image_is_within(&image, root)) {
            continue;
        }
        if unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
            continue;
        }
        if unsafe { TerminateProcess(process.as_raw_handle(), 0) } == 0
            && unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_OBJECT_0
        {
            return Err(fail(
                EXIT_FAILURE,
                "Windows could not stop the previous Talking Quill process.",
            ));
        }
        if unsafe { WaitForSingleObject(process.as_raw_handle(), 10_000) } != WAIT_OBJECT_0 {
            return Err(fail(
                EXIT_FAILURE,
                "Windows is still closing a Talking Quill process.",
            ));
        }
    }
    Ok(())
}

fn image_is_within(image: &Path, root: &Path) -> bool {
    let image = image.to_string_lossy().to_lowercase();
    let root = root.to_string_lossy().to_lowercase();
    image
        .strip_prefix(root.trim_end_matches('\\'))
        .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn process_ids() -> Result<Vec<u32>> {
    // SAFETY: the snapshot is retained until enumeration completes.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(EXIT_FAILURE, "Cannot inspect runtime processes."));
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ids = Vec::new();
    let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while available {
        ids.push(entry.th32ProcessID);
        available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    Ok(ids)
}

pub(super) fn process_image_from_handle(process: &OwnedHandle) -> Result<PathBuf> {
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as u32;
    // SAFETY: path has length writable UTF-16 elements; process is retained.
    if unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut length)
    } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read the setup peer image path.",
        ));
    }
    path.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_wide(&path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_stops_a_real_process_without_killing_a_sibling_installation() {
        struct Probe(std::process::Child);
        impl Drop for Probe {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = std::env::temp_dir().join(format!(
            "tq-process-cleanup-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let target = root.join("install");
        let sibling = root.join("install-other");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir(&sibling).unwrap();
        let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/ping.exe");
        for directory in [&target, &sibling] {
            fs::copy(&ping, directory.join("cleanup-probe.exe")).unwrap();
        }
        let launch = |directory: &Path| {
            Probe(
                Command::new(directory.join("cleanup-probe.exe"))
                    .args(["-t", "127.0.0.1"])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        };
        {
            let mut inside = launch(&target);
            let mut outside = launch(&sibling);
            std::thread::sleep(Duration::from_millis(150));
            terminate_tree_processes(&target, Some(inside.0.id())).unwrap();
            assert!(inside.0.try_wait().unwrap().is_none());
            terminate_tree_processes(&target, None).unwrap();
            assert!(inside.0.try_wait().unwrap().is_some());
            assert!(outside.0.try_wait().unwrap().is_none());
        }
        for directory in [&target, &sibling] {
            fs::remove_file(directory.join("cleanup-probe.exe")).unwrap();
            fs::remove_dir(directory).unwrap();
        }
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn process_cleanup_requires_a_complete_install_directory_component() {
        let root = Path::new(r"C:\Program Files\Talking Quill");
        assert!(image_is_within(
            Path::new(r"c:\program files\talking quill\resources\helper\owner.exe"),
            root
        ));
        assert!(!image_is_within(
            Path::new(r"C:\Program Files\Talking Quill Other\owner.exe"),
            root
        ));
        assert!(!image_is_within(
            Path::new(r"C:\Elsewhere\Talking Quill.exe"),
            root
        ));
    }
}
