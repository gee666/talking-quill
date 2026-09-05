use super::*;

pub(super) fn deliver_startup(
    pipe: HANDLE,
    expected_pid: u32,
    expected: &talking_quill_windows_owner_ipc::peer::PeerFacts,
    frame: &[u8],
    deadline: u64,
    control: &Control,
) -> Result<(), &'static str> {
    use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};
    let mut rejected = 0;
    loop {
        let event = Handle(unsafe {
            windows_sys::Win32::System::Threading::CreateEventW(null(), 1, 0, null())
        });
        if event.0.is_null() {
            return Err("startup event");
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        let connected = unsafe { ConnectNamedPipe(pipe, &mut overlapped) };
        if connected == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_PENDING {
                let timeout = match remaining_ms(deadline) {
                    Ok(timeout) => timeout,
                    Err(error) => {
                        // Drain the kernel borrow before the OVERLAPPED and event drop.
                        cancel_overlapped(pipe, &mut overlapped);
                        return Err(error);
                    }
                };
                match wait_overlapped_or_control(event.0, timeout, control) {
                    ControlAction::Continue => {}
                    ControlAction::Terminate => {
                        cancel_overlapped(pipe, &mut overlapped);
                        return Err("probe terminated");
                    }
                    ControlAction::Deadline => {
                        cancel_overlapped(pipe, &mut overlapped);
                        return Err("startup deadline");
                    }
                }
                let mut transferred = 0;
                if unsafe { GetOverlappedResult(pipe, &overlapped, &mut transferred, 0) } == 0 {
                    return Err("startup connect");
                }
            } else if error != ERROR_PIPE_CONNECTED {
                return Err("startup connect");
            }
        }
        let pid = talking_quill_windows_owner_ipc::peer::named_pipe_client_pid(unsafe {
            std::os::windows::io::BorrowedHandle::borrow_raw(pipe)
        });
        let authorized = pid
            .ok()
            .filter(|value| *value == expected_pid)
            .and_then(|value| {
                talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(value).ok()
            })
            .is_some_and(|peer| peer.facts == *expected);
        if !authorized {
            rejected += 1;
            unsafe { DisconnectNamedPipe(pipe) };
            if rejected >= 64 {
                return Err("startup forged clients");
            }
            continue;
        }
        let event = Handle(unsafe {
            windows_sys::Win32::System::Threading::CreateEventW(null(), 1, 0, null())
        });
        if event.0.is_null() {
            return Err("startup write event");
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        let mut count = 0;
        if unsafe {
            WriteFile(
                pipe,
                frame.as_ptr(),
                frame.len() as u32,
                null_mut(),
                &mut overlapped,
            )
        } == 0
        {
            if unsafe { GetLastError() } != ERROR_IO_PENDING {
                return Err("startup write");
            }
            let timeout = match remaining_ms(deadline) {
                Ok(timeout) => timeout,
                Err(error) => {
                    // Drain the kernel borrow before the frame, OVERLAPPED and event drop.
                    cancel_overlapped(pipe, &mut overlapped);
                    return Err(error);
                }
            };
            match wait_overlapped_or_control(event.0, timeout, control) {
                ControlAction::Continue => {}
                ControlAction::Terminate => {
                    cancel_overlapped(pipe, &mut overlapped);
                    return Err("probe terminated");
                }
                ControlAction::Deadline => {
                    cancel_overlapped(pipe, &mut overlapped);
                    return Err("startup write deadline");
                }
            }
        }
        if unsafe { GetOverlappedResult(pipe, &overlapped, &mut count, 0) } == 0
            || count as usize != frame.len()
        {
            return Err("startup write");
        }
        unsafe { DisconnectNamedPipe(pipe) };
        return Ok(());
    }
}
