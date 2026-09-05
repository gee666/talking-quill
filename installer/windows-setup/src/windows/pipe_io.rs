//! Bounded message-pipe I/O. Pending buffers remain owned until completion.
use super::*;

pub(super) fn deadline_ms(deadline: Instant) -> Result<u32> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| fail(EXIT_REJECTED, "Setup pipe deadline expired."))?;
    Ok(u32::try_from(remaining.as_millis().max(1)).unwrap_or(u32::MAX - 1))
}

fn wait_overlapped(
    handle: std::os::windows::io::RawHandle,
    event: std::os::windows::io::RawHandle,
    monitor: Option<std::os::windows::io::RawHandle>,
    overlapped: &mut OVERLAPPED,
    deadline: Instant,
) -> Result<u32> {
    let handles = [event, monitor.unwrap_or(event)];
    let count = if monitor.is_some() { 2 } else { 1 };
    let result = unsafe {
        WaitForMultipleObjects(
            count,
            handles.as_ptr(),
            0,
            deadline_ms(deadline).unwrap_or(0),
        )
    };
    if result != WAIT_OBJECT_0 {
        unsafe { CancelIoEx(handle, overlapped) };
        // Cancellation only requests completion. Windows may still write into
        // OVERLAPPED and the caller's stack buffer until this wait finishes.
        let mut transferred = 0;
        unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 1) };
        return Err(fail(
            EXIT_REJECTED,
            if result == WAIT_OBJECT_0 + 1 {
                "Setup pipe peer exited during authentication."
            } else {
                "Setup pipe operation timed out."
            },
        ));
    }
    let mut transferred = 0;
    if unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, 0) } == 0 {
        let error = std::io::Error::last_os_error();
        return Err(fail(
            EXIT_REJECTED,
            format!("Setup connection closed before the operation completed: {error}"),
        ));
    }
    Ok(transferred)
}

fn new_overlapped() -> Result<(OwnedHandle, OVERLAPPED)> {
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(fail(EXIT_FAILURE, "Cannot create setup pipe event."));
    }
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
    overlapped.hEvent = event.as_raw_handle();
    Ok((event, overlapped))
}

pub(super) fn pipe_connect(
    handle: std::os::windows::io::RawHandle,
    monitor: std::os::windows::io::RawHandle,
    deadline: Instant,
) -> Result<()> {
    let (event, mut overlapped) = new_overlapped()?;
    if unsafe { ConnectNamedPipe(handle, &mut overlapped) } == 0 {
        match unsafe { GetLastError() } {
            ERROR_PIPE_CONNECTED => return Ok(()),
            ERROR_IO_PENDING => {
                wait_overlapped(
                    handle,
                    event.as_raw_handle(),
                    Some(monitor),
                    &mut overlapped,
                    deadline,
                )?;
            }
            _ => return Err(fail(EXIT_REJECTED, "The elevated worker did not connect.")),
        }
    }
    Ok(())
}

pub(super) fn pipe_write(
    handle: std::os::windows::io::RawHandle,
    bytes: &[u8],
    monitor: Option<std::os::windows::io::RawHandle>,
    deadline: Instant,
) -> Result<()> {
    let (event, mut overlapped) = new_overlapped()?;
    let mut immediate = 0;
    let transferred = if unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut immediate,
            &mut overlapped,
        )
    } != 0
    {
        immediate
    } else if unsafe { GetLastError() } == ERROR_IO_PENDING {
        wait_overlapped(
            handle,
            event.as_raw_handle(),
            monitor,
            &mut overlapped,
            deadline,
        )?
    } else {
        return Err(fail(EXIT_REJECTED, "Setup pipe write failed."));
    };
    if transferred as usize != bytes.len() {
        return Err(fail(EXIT_REJECTED, "Setup pipe write was truncated."));
    }
    Ok(())
}

pub(super) fn pipe_read<const N: usize>(
    handle: std::os::windows::io::RawHandle,
    monitor: Option<std::os::windows::io::RawHandle>,
    deadline: Instant,
) -> Result<[u8; N]> {
    let (event, mut overlapped) = new_overlapped()?;
    let mut bytes = [0_u8; N];
    let mut immediate = 0;
    let transferred = if unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr().cast(),
            N as u32,
            &mut immediate,
            &mut overlapped,
        )
    } != 0
    {
        immediate
    } else if unsafe { GetLastError() } == ERROR_IO_PENDING {
        wait_overlapped(
            handle,
            event.as_raw_handle(),
            monitor,
            &mut overlapped,
            deadline,
        )?
    } else {
        return Err(fail(EXIT_REJECTED, "Setup pipe read failed."));
    };
    if transferred as usize != N {
        return Err(fail(EXIT_REJECTED, "Setup pipe frame was truncated."));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_pipe() -> (OwnedHandle, OwnedHandle) {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let name = wide(OsStr::new(&format!(
            r"\\.\pipe\TalkingQuill.Setup.Test.{}",
            hex_bytes(&random)
        )));
        let server = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                256,
                256,
                100,
                ptr::null(),
            )
        };
        assert_ne!(server, INVALID_HANDLE_VALUE);
        let server = unsafe { OwnedHandle::from_raw_handle(server) };
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        assert_ne!(client, INVALID_HANDLE_VALUE);
        let client = unsafe { OwnedHandle::from_raw_handle(client) };
        pipe_connect(
            server.as_raw_handle(),
            unsafe { GetCurrentProcess() },
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        (server, client)
    }

    #[test]
    fn cancelled_read_is_drained_before_the_next_message() {
        let (server, client) = connected_pipe();
        for timeout in [Duration::ZERO, Duration::from_millis(10)] {
            assert!(
                pipe_read::<1>(server.as_raw_handle(), None, Instant::now() + timeout).is_err()
            );
            let deadline = Instant::now() + Duration::from_secs(1);
            pipe_write(client.as_raw_handle(), &[42], None, deadline).unwrap();
            assert_eq!(
                pipe_read::<1>(server.as_raw_handle(), None, deadline).unwrap(),
                [42]
            );
        }
        drop(client);
        assert!(
            pipe_read::<1>(
                server.as_raw_handle(),
                None,
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
    }
}
