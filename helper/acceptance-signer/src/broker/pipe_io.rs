use super::*;

pub(super) fn accept_authorized(
    pipe: HANDLE,
    child: &ProbeChild,
    input: &ProbeInput,
    control: &Control,
    armed: bool,
) -> Result<(serde_json::Value, u32), &'static str> {
    use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError};
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};
    let mut rejected = 0;
    loop {
        let event = Handle(unsafe {
            windows_sys::Win32::System::Threading::CreateEventW(null(), 1, 0, null())
        });
        if event.0.is_null() {
            return Err("pipe event");
        }
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.0;
        let timeout = remaining_ms(input.absolute_deadline_ms)?;
        let connected = unsafe { ConnectNamedPipe(pipe, &mut overlapped) };
        if connected == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_PENDING {
                match wait_overlapped_or_control(event.0, timeout, control) {
                    ControlAction::Continue => {}
                    ControlAction::Terminate => {
                        cancel_overlapped(pipe, &mut overlapped);
                        return Err("probe terminated");
                    }
                    ControlAction::Deadline => {
                        cancel_overlapped(pipe, &mut overlapped);
                        return Err("pipe deadline");
                    }
                }
                let mut transferred = 0;
                if unsafe { GetOverlappedResult(pipe, &overlapped, &mut transferred, 0) } == 0 {
                    return Err("pipe connect");
                }
            } else if error != ERROR_PIPE_CONNECTED {
                return Err("pipe connect");
            }
        }
        let pid = talking_quill_windows_owner_ipc::peer::named_pipe_client_pid(unsafe {
            std::os::windows::io::BorrowedHandle::borrow_raw(pipe)
        });
        let authorized = match pid {
            Ok(pid) => authorized_peer(pid, child, input).unwrap_or(false),
            Err(_) => false,
        };
        if !authorized {
            rejected += 1;
            unsafe { DisconnectNamedPipe(pipe) };
            if rejected >= 64 {
                return Err("forged clients");
            }
            continue;
        }
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0u8; 4096];
            let count =
                read_pipe_overlapped(pipe, &mut buffer, input.absolute_deadline_ms, control)?;
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > 64 * 1024 {
                unsafe { DisconnectNamedPipe(pipe) };
                return Err("probe frame bound");
            }
            if count == 0 || bytes.last() == Some(&b'\n') {
                break;
            }
        }
        unsafe { DisconnectNamedPipe(pipe) };
        if bytes.last() != Some(&b'\n')
            || bytes[..bytes.len() - 1].contains(&b'\n')
            || bytes.get(bytes.len().saturating_sub(2)) == Some(&b'\r')
        {
            rejected += 1;
            continue;
        }
        let value: serde_json::Value = match serde_json::from_slice(&bytes[..bytes.len() - 1]) {
            Ok(value) => value,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        let valid = value.get("version") == Some(&serde_json::json!(1))
            && value.get("correlation").and_then(|v| v.as_str()) == Some(&input.launch_correlation)
            && value.get("runtimeLifecycleAuthoritative") == Some(&serde_json::json!(false))
            && if armed {
                value.get("phase").and_then(|v| v.as_str()) == input.armed_expected_phase.as_deref()
            } else {
                value.get("result").and_then(|v| v.as_str()) == Some("passed")
            };
        if valid {
            return Ok((value, rejected));
        }
        rejected += 1;
    }
}

pub(super) fn read_pipe_overlapped(
    pipe: HANDLE,
    buffer: &mut [u8],
    deadline: u64,
    control: &Control,
) -> Result<usize, &'static str> {
    use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    let event = Handle(unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(null(), 1, 0, null())
    });
    if event.0.is_null() {
        return Err("pipe read event");
    }
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event.0;
    let timeout = remaining_ms(deadline)?;
    let mut count = 0;
    if unsafe {
        ReadFile(
            pipe,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            null_mut(),
            &mut overlapped,
        )
    } == 0
    {
        let error = unsafe { GetLastError() };
        if error == ERROR_BROKEN_PIPE {
            return Ok(0);
        }
        if error != ERROR_IO_PENDING {
            return Err("pipe read");
        }
        match wait_overlapped_or_control(event.0, timeout, control) {
            ControlAction::Continue => {}
            ControlAction::Terminate => {
                cancel_overlapped(pipe, &mut overlapped);
                return Err("probe terminated");
            }
            ControlAction::Deadline => {
                cancel_overlapped(pipe, &mut overlapped);
                return Err("pipe read deadline");
            }
        }
        if unsafe { GetOverlappedResult(pipe, &overlapped, &mut count, 0) } == 0 {
            return Err("pipe read");
        }
    } else if unsafe { GetOverlappedResult(pipe, &overlapped, &mut count, 0) } == 0 {
        return Err("pipe read result");
    }
    Ok(count as usize)
}

pub(super) fn wait_overlapped_or_control(
    event: HANDLE,
    timeout_ms: u32,
    control: &Control,
) -> ControlAction {
    let started = std::time::Instant::now();
    loop {
        let elapsed = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        if elapsed >= timeout_ms {
            return ControlAction::Deadline;
        }
        let slice = (timeout_ms - elapsed).min(50);
        if unsafe { WaitForSingleObject(event, slice) } == WAIT_OBJECT_0 {
            return ControlAction::Continue;
        }
        if control.poll().unwrap_or(Some(ControlAction::Terminate))
            == Some(ControlAction::Terminate)
        {
            return ControlAction::Terminate;
        }
    }
}

pub(super) fn cancel_overlapped(
    pipe: HANDLE,
    overlapped: &mut windows_sys::Win32::System::IO::OVERLAPPED,
) {
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult};
    unsafe { CancelIoEx(pipe, overlapped) };
    let mut transferred = 0;
    unsafe { GetOverlappedResult(pipe, overlapped, &mut transferred, 1) };
}
