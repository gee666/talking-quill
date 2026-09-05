use super::*;

pub(super) struct ProbeChild {
    pub(super) process: Handle,
    pub(super) _thread: Handle,
    pub(super) job: Handle,
    pub(super) pid: u32,
    pub(super) facts: talking_quill_windows_owner_ipc::peer::PeerFacts,
}

impl ProbeChild {
    pub(super) fn wait_until(
        &mut self,
        deadline: u64,
        control: &Control,
    ) -> Result<ControlAction, &'static str> {
        loop {
            let timeout = remaining_ms(deadline)?.min(50);
            if unsafe { WaitForSingleObject(self.process.0, timeout) } == WAIT_OBJECT_0 {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 || code != 0 {
                    return Err("probe exit");
                }
                return Ok(ControlAction::Continue);
            }
            if control.poll()? == Some(ControlAction::Terminate) {
                return Ok(ControlAction::Terminate);
            }
        }
    }
    pub(super) fn terminate_and_wait(&mut self) -> Result<(), &'static str> {
        unsafe { TerminateJobObject(self.job.0, 80) };
        if unsafe { WaitForSingleObject(self.process.0, 5000) } != WAIT_OBJECT_0 {
            Err("probe teardown")
        } else {
            Ok(())
        }
    }
}

pub(super) fn launch_probe_process(
    path: &Path,
    startup_pipe_name: &str,
    startup_pipe: HANDLE,
    startup_frame: &[u8],
    expected: &Snapshot,
    deadline: u64,
    control: &Control,
) -> Result<ProbeChild, &'static str> {
    let (nul_read, nul_write) = nul_handles()?;
    let inherited = [nul_read.0, nul_write.0];
    let attributes = attribute_list(&inherited)?;
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = nul_read.0;
    startup.StartupInfo.hStdOutput = nul_write.0;
    startup.StartupInfo.hStdError = nul_write.0;
    startup.lpAttributeList = attributes.list;
    let job = kill_job()?;
    let startup_argument =
        format!("--talking-quill-installed-acceptance-startup-pipe-v1={startup_pipe_name}");
    let mut command = command_line(path, &[&startup_argument])?;
    let mut application = wide(path.as_os_str())?;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_mut_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err("create probe");
    }
    let process = Handle(info.hProcess);
    let thread = Handle(info.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        unsafe { TerminateProcess(process.0, 79) };
        return Err("assign probe job");
    }
    drop(attributes);
    drop(nul_read);
    drop(nul_write);
    let actual = process_snapshot(process.0)?;
    if &actual != expected {
        return Err("probe process image");
    }
    verify_parent_and_creation(info.dwProcessId, process.0)?;
    let peer =
        talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(info.dwProcessId)
            .map_err(|_| "probe process facts")?;
    if peer.facts.image_sha256 != expected.sha256
        || peer.facts.file_identity.volume_serial != expected.volume
        || peer.facts.file_identity.file_index != expected.index
    {
        return Err("probe process facts");
    }
    let facts = peer.facts.clone();
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err("resume probe");
    }
    deliver_startup(
        startup_pipe,
        info.dwProcessId,
        &facts,
        startup_frame,
        deadline,
        control,
    )?;
    Ok(ProbeChild {
        process,
        _thread: thread,
        job,
        pid: info.dwProcessId,
        facts,
    })
}
