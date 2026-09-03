#[cfg(not(windows))]
fn main() {
    eprintln!("Windows acceptance broker requires Windows");
    std::process::exit(79);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("Windows acceptance broker failed: {error}");
        std::process::exit(79);
    }
}

#[cfg(windows)]
mod windows {
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::fs::{File, OpenOptions};
    use std::io::{BufRead, Read, Seek, Write};
    use std::mem::{size_of, size_of_val};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};
    use std::time::Duration;
    use talking_quill_acceptance_signer::windows_key_security::open_validated_private_key;
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetFileInformationByHandle,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, QueryFullProcessImageNameW,
        ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };

    #[used]
    static SOURCE_COMMIT_MARKER: &str = concat!(
        "TALKING_QUILL_SOURCE_COMMIT=",
        env!("TALKING_QUILL_SOURCE_COMMIT")
    );
    #[used]
    static SOURCE_TREE_MARKER: &str = concat!(
        "TALKING_QUILL_SOURCE_TREE=",
        env!("TALKING_QUILL_SOURCE_TREE")
    );

    const MAX_REQUEST: u64 = 160 * 1024;
    const MAX_PAYLOAD: usize = 64 * 1024;
    const MAX_IMAGE: u64 = 256 * 1024 * 1024;
    const TIMEOUT_MS: u32 = 10_000;

    #[derive(Deserialize)]
    #[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
    enum Request {
        Sign {
            version: u8,
            correlation: String,
            #[serde(rename = "brokerSha256")]
            broker_sha256: String,
            #[serde(rename = "brokerBytes")]
            broker_bytes: u64,
            #[serde(rename = "signerPath")]
            signer_path: String,
            #[serde(rename = "signerSha256")]
            signer_sha256: String,
            #[serde(rename = "signerBytes")]
            signer_bytes: u64,
            #[serde(rename = "privateKeyPath")]
            private_key_path: String,
            #[serde(rename = "payloadHex")]
            payload_hex: String,
            #[serde(rename = "sourceCommit")]
            source_commit: Option<String>,
            #[serde(rename = "sourceTree")]
            source_tree: Option<String>,
        },
        Probe {
            version: u8,
            correlation: String,
            #[serde(rename = "brokerSha256")]
            broker_sha256: String,
            #[serde(rename = "brokerBytes")]
            broker_bytes: u64,
            #[serde(rename = "executablePath")]
            executable_path: String,
            #[serde(rename = "executableSha256")]
            executable_sha256: String,
            #[serde(rename = "executableBytes")]
            executable_bytes: u64,
            #[serde(rename = "sourceCommit")]
            source_commit: String,
            #[serde(rename = "sourceTree")]
            source_tree: String,
            #[serde(rename = "startupFrameHex")]
            startup_frame_hex: String,
            #[serde(rename = "readinessPipe")]
            readiness_pipe: String,
            #[serde(rename = "armedPipe")]
            armed_pipe: Option<String>,
            #[serde(rename = "armedExpectedPhase")]
            armed_expected_phase: Option<String>,
            #[serde(rename = "launchCorrelation")]
            launch_correlation: String,
            #[serde(rename = "absoluteDeadlineMs")]
            absolute_deadline_ms: u64,
        },
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        version: u8,
        correlation: String,
        result: &'static str,
        signer_sha256: String,
        signer_bytes: u64,
        retained_identity_matches: bool,
        process_identity_matches: bool,
        process_hash_matches: bool,
        parent_identity_matches: bool,
        creation_identity_matches: bool,
        signature_hex: String,
        public_key_sec1_hex: String,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Snapshot {
        volume: u32,
        index: u64,
        bytes: u64,
        sha256: [u8; 32],
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for Attributes {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
    }

    struct SnapshotDir(PathBuf);
    impl Drop for SnapshotDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn run() -> Result<(), &'static str> {
        std::hint::black_box(SOURCE_COMMIT_MARKER);
        std::hint::black_box(SOURCE_TREE_MARKER);
        let mut frame = Vec::new();
        std::io::stdin()
            .lock()
            .take(MAX_REQUEST + 1)
            .read_until(b'\n', &mut frame)
            .map_err(|_| "request read")?;
        if frame.is_empty()
            || frame.len() as u64 > MAX_REQUEST
            || frame.last() != Some(&b'\n')
            || frame.get(frame.len().saturating_sub(2)) == Some(&b'\r')
        {
            return Err("request frame");
        }
        frame.pop();
        let request: Request = serde_json::from_slice(&frame).map_err(|_| "request schema")?;
        match request {
            Request::Sign {
                version,
                correlation,
                broker_sha256,
                broker_bytes,
                signer_path,
                signer_sha256,
                signer_bytes,
                private_key_path,
                payload_hex,
                source_commit,
                source_tree,
            } => {
                validate_header(version, &correlation)?;
                let mut broker = retain_self(decode_hash(&broker_sha256)?, broker_bytes)?;
                if !source_matches(
                    &mut broker,
                    source_commit.as_deref(),
                    source_tree.as_deref(),
                )? {
                    return Err("broker provenance");
                }
                let response = sign(SignInput {
                    correlation,
                    signer_path: PathBuf::from(signer_path),
                    signer_sha256: decode_hash(&signer_sha256)?,
                    signer_bytes,
                    private_key_path: PathBuf::from(private_key_path),
                    payload: decode_hex(&payload_hex, MAX_PAYLOAD)?,
                    source_commit,
                    source_tree,
                })?;
                emit_response(&response)
            }
            Request::Probe {
                version,
                correlation,
                broker_sha256,
                broker_bytes,
                executable_path,
                executable_sha256,
                executable_bytes,
                source_commit,
                source_tree,
                startup_frame_hex,
                readiness_pipe,
                armed_pipe,
                armed_expected_phase,
                launch_correlation,
                absolute_deadline_ms,
            } => {
                validate_header(version, &correlation)?;
                let mut broker = retain_self(decode_hash(&broker_sha256)?, broker_bytes)?;
                if !source_matches(&mut broker, Some(&source_commit), Some(&source_tree))? {
                    return Err("broker provenance");
                }
                run_probe(ProbeInput {
                    correlation,
                    executable_path: PathBuf::from(executable_path),
                    executable_sha256: decode_hash(&executable_sha256)?,
                    executable_bytes,
                    source_commit,
                    source_tree,
                    startup_frame: decode_hex(&startup_frame_hex, 20 * 1024)?,
                    readiness_pipe,
                    armed_pipe,
                    armed_expected_phase,
                    launch_correlation,
                    absolute_deadline_ms,
                })
            }
        }
    }

    fn emit_response<T: Serialize>(response: &T) -> Result<(), &'static str> {
        let encoded = serde_json::to_vec(response).map_err(|_| "response encode")?;
        if encoded.len() > 64 * 1024 {
            return Err("response bound");
        }
        std::io::stdout()
            .write_all(&encoded)
            .map_err(|_| "response write")?;
        std::io::stdout()
            .write_all(b"\n")
            .map_err(|_| "response write")?;
        std::io::stdout().flush().map_err(|_| "response flush")
    }

    struct ProbeInput {
        correlation: String,
        executable_path: PathBuf,
        executable_sha256: [u8; 32],
        executable_bytes: u64,
        source_commit: String,
        source_tree: String,
        startup_frame: Vec<u8>,
        readiness_pipe: String,
        armed_pipe: Option<String>,
        armed_expected_phase: Option<String>,
        launch_correlation: String,
        absolute_deadline_ms: u64,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ProbeEvent<'a> {
        version: u8,
        correlation: &'a str,
        event: &'a str,
        process_id: u32,
        rejected_clients: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<serde_json::Value>,
    }

    fn run_probe(input: ProbeInput) -> Result<(), &'static str> {
        if !is_lower_hex(&input.launch_correlation, 32)
            || !is_lower_hex(&input.source_commit, 40)
            || !is_lower_hex(&input.source_tree, 40)
            || !valid_pipe_name(&input.readiness_pipe)
            || input
                .armed_pipe
                .as_deref()
                .is_some_and(|name| !valid_pipe_name(name))
            || input.armed_pipe.is_none() != input.armed_expected_phase.is_none()
            || input.absolute_deadline_ms <= unix_ms()?
        {
            return Err("probe request");
        }
        require_absolute_no_reparse(&input.executable_path)?;
        let mut expected_file = open_locked(&input.executable_path)?;
        let expected = snapshot(&mut expected_file)?;
        if expected.sha256 != input.executable_sha256 || expected.bytes != input.executable_bytes {
            return Err("probe image identity");
        }
        let security =
            talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon()
                .map_err(|_| "pipe security")?;
        let readiness = talking_quill_windows_owner_ipc::endpoint::create_server_instance(
            &input.readiness_pipe,
            &security,
            true,
        )
        .map_err(|_| "readiness pipe")?;
        let armed = input
            .armed_pipe
            .as_deref()
            .map(|name| {
                talking_quill_windows_owner_ipc::endpoint::create_server_instance(
                    name, &security, true,
                )
                .map_err(|_| "armed pipe")
            })
            .transpose()?;
        let control = Control::new(input.correlation.clone());
        let mut startup_nonce = [0u8; 16];
        getrandom::fill(&mut startup_nonce).map_err(|_| "startup pipe random")?;
        let startup_pipe_name = format!(
            r"\\.\pipe\TalkingQuill.AcceptanceStartup.{}",
            startup_nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let startup_pipe =
            talking_quill_windows_owner_ipc::endpoint::create_outbound_server_instance(
                &startup_pipe_name,
                &security,
                true,
            )
            .map_err(|_| "startup pipe")?;
        let mut child = launch_probe_process(
            &input.executable_path,
            &startup_pipe_name,
            startup_pipe.as_raw_handle(),
            &input.startup_frame,
            &expected,
            input.absolute_deadline_ms,
            &control,
        )?;
        emit_response(&ProbeEvent {
            version: 1,
            correlation: &input.correlation,
            event: "listening",
            process_id: child.pid,
            rejected_clients: 0,
            value: None,
        })?;
        let terminate = |child: &mut ProbeChild, rejected_clients| {
            child.terminate_and_wait()?;
            emit_response(&ProbeEvent {
                version: 1,
                correlation: &input.correlation,
                event: "terminated",
                process_id: child.pid,
                rejected_clients,
                value: None,
            })
        };
        if let Some(pipe) = armed.as_ref() {
            let (value, rejected) =
                match accept_authorized(pipe.as_raw_handle(), &child, &input, &control, true) {
                    Ok(value) => value,
                    Err("probe terminated") => return terminate(&mut child, 0),
                    Err(error) => return Err(error),
                };
            emit_response(&ProbeEvent {
                version: 1,
                correlation: &input.correlation,
                event: "armed",
                process_id: child.pid,
                rejected_clients: rejected,
                value: Some(value),
            })?;
            if control.wait(input.absolute_deadline_ms)? == ControlAction::Terminate {
                return terminate(&mut child, rejected);
            }
        }
        let (value, rejected) =
            match accept_authorized(readiness.as_raw_handle(), &child, &input, &control, false) {
                Ok(value) => value,
                Err("probe terminated") => return terminate(&mut child, 0),
                Err(error) => return Err(error),
            };
        if child.wait_until(input.absolute_deadline_ms, &control)? == ControlAction::Terminate {
            return terminate(&mut child, rejected);
        }
        emit_response(&ProbeEvent {
            version: 1,
            correlation: &input.correlation,
            event: "complete",
            process_id: child.pid,
            rejected_clients: rejected,
            value: Some(value),
        })
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum ControlAction {
        Continue,
        Terminate,
        Deadline,
    }

    struct Control {
        receiver: std::sync::mpsc::Receiver<Result<ControlAction, &'static str>>,
    }

    impl Control {
        fn new(correlation: String) -> Self {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                loop {
                    let result = read_action(&correlation).map(|action| {
                        if action == "continue" {
                            ControlAction::Continue
                        } else {
                            ControlAction::Terminate
                        }
                    });
                    let terminal = !matches!(result, Ok(ControlAction::Continue));
                    if sender.send(result).is_err() || terminal {
                        break;
                    }
                }
            });
            Self { receiver }
        }

        fn poll(&self) -> Result<Option<ControlAction>, &'static str> {
            match self.receiver.try_recv() {
                Ok(action) => action.map(Some),
                Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Ok(Some(ControlAction::Terminate))
                }
            }
        }

        fn wait(&self, deadline: u64) -> Result<ControlAction, &'static str> {
            let duration = Duration::from_millis(u64::from(remaining_ms(deadline)?));
            match self.receiver.recv_timeout(duration) {
                Ok(action) => action,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err("probe action deadline"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    Ok(ControlAction::Terminate)
                }
            }
        }
    }

    struct ProbeChild {
        process: Handle,
        _thread: Handle,
        job: Handle,
        pid: u32,
        facts: talking_quill_windows_owner_ipc::peer::PeerFacts,
    }

    impl ProbeChild {
        fn wait_until(
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
        fn terminate_and_wait(&mut self) -> Result<(), &'static str> {
            unsafe { TerminateJobObject(self.job.0, 80) };
            if unsafe { WaitForSingleObject(self.process.0, 5000) } != WAIT_OBJECT_0 {
                Err("probe teardown")
            } else {
                Ok(())
            }
        }
    }

    fn launch_probe_process(
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
        let (_storage, attributes) = attribute_list(&inherited)?;
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = nul_read.0;
        startup.StartupInfo.hStdOutput = nul_write.0;
        startup.StartupInfo.hStdError = nul_write.0;
        startup.lpAttributeList = attributes.0;
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

    fn deliver_startup(
        pipe: HANDLE,
        expected_pid: u32,
        expected: &talking_quill_windows_owner_ipc::peer::PeerFacts,
        frame: &[u8],
        deadline: u64,
        control: &Control,
    ) -> Result<(), &'static str> {
        use windows_sys::Win32::Foundation::{
            ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError,
        };
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
                    match wait_overlapped_or_control(event.0, remaining_ms(deadline)?, control) {
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
                match wait_overlapped_or_control(event.0, remaining_ms(deadline)?, control) {
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

    fn nul_handles() -> Result<(Handle, Handle), &'static str> {
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        let name: Vec<u16> = "NUL".encode_utf16().chain([0]).collect();
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let read = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &security,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        let write = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &security,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if read == INVALID_HANDLE_VALUE || write == INVALID_HANDLE_VALUE {
            return Err("nul handles");
        }
        Ok((Handle(read), Handle(write)))
    }

    fn accept_authorized(
        pipe: HANDLE,
        child: &ProbeChild,
        input: &ProbeInput,
        control: &Control,
        armed: bool,
    ) -> Result<(serde_json::Value, u32), &'static str> {
        use windows_sys::Win32::Foundation::{
            ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError,
        };
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
                && value.get("correlation").and_then(|v| v.as_str())
                    == Some(&input.launch_correlation)
                && value.get("runtimeLifecycleAuthoritative") == Some(&serde_json::json!(false))
                && if armed {
                    value.get("phase").and_then(|v| v.as_str())
                        == input.armed_expected_phase.as_deref()
                } else {
                    value.get("result").and_then(|v| v.as_str()) == Some("passed")
                };
            if valid {
                return Ok((value, rejected));
            }
            rejected += 1;
        }
    }

    fn read_pipe_overlapped(
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

    fn wait_overlapped_or_control(
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

    fn cancel_overlapped(
        pipe: HANDLE,
        overlapped: &mut windows_sys::Win32::System::IO::OVERLAPPED,
    ) {
        use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult};
        unsafe { CancelIoEx(pipe, overlapped) };
        let mut transferred = 0;
        unsafe { GetOverlappedResult(pipe, overlapped, &mut transferred, 1) };
    }

    fn authorized_peer(
        pid: u32,
        child: &ProbeChild,
        input: &ProbeInput,
    ) -> Result<bool, &'static str> {
        let peer = match talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(pid) {
            Ok(peer) => peer,
            Err(_) => return Ok(false),
        };
        let facts = &peer.facts;
        let expected = &child.facts;
        if !peer_identity_matches(
            facts,
            expected,
            input.executable_sha256,
            &input.source_commit,
            &input.source_tree,
        ) {
            return Ok(false);
        }
        if pid == child.pid {
            return Ok(facts.creation_marker == expected.creation_marker);
        }
        is_creation_bound_descendant(
            pid,
            facts.creation_marker,
            child.pid,
            expected.creation_marker,
        )
    }

    fn peer_identity_matches(
        facts: &talking_quill_windows_owner_ipc::peer::PeerFacts,
        expected: &talking_quill_windows_owner_ipc::peer::PeerFacts,
        image_sha256: [u8; 32],
        source_commit: &str,
        source_tree: &str,
    ) -> bool {
        facts.user_sid == expected.user_sid
            && facts.logon_sid == expected.logon_sid
            && facts.wts_session_id == expected.wts_session_id
            && facts.integrity_rid == expected.integrity_rid
            && facts.architecture == expected.architecture
            && facts.canonical_image == expected.canonical_image
            && facts.file_identity == expected.file_identity
            && facts.image_sha256 == image_sha256
            && (facts.source_identity.is_none()
                || facts.source_identity.as_ref().is_some_and(|source| {
                    source.commit == source_commit && source.tree == source_tree
                }))
    }

    fn creation_chain_reaches_root(chain: &[(u32, u64)], root: u32, root_creation: u64) -> bool {
        let mut later = u64::MAX;
        for &(pid, creation) in chain {
            if creation > later {
                return false;
            }
            if pid == root {
                return creation == root_creation;
            }
            later = creation;
        }
        false
    }

    fn is_creation_bound_descendant(
        mut pid: u32,
        mut creation: u64,
        root: u32,
        root_creation: u64,
    ) -> Result<bool, &'static str> {
        let mut chain = vec![(pid, creation)];
        for _ in 0..32 {
            let parent = process_parent(pid)?;
            if parent == 0 || parent == pid {
                return Ok(false);
            }
            let peer = match talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(
                parent,
            ) {
                Ok(peer) => peer,
                Err(_) => return Ok(false),
            };
            pid = parent;
            creation = peer.facts.creation_marker;
            chain.push((pid, creation));
            if parent == root {
                return Ok(creation_chain_reaches_root(&chain, root, root_creation));
            }
        }
        Ok(false)
    }

    fn process_parent(pid: u32) -> Result<u32, &'static str> {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
        if snapshot.0 == INVALID_HANDLE_VALUE {
            return Err("ancestry snapshot");
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0 {
            loop {
                if entry.th32ProcessID == pid {
                    return Ok(entry.th32ParentProcessID);
                }
                if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                    break;
                }
            }
        }
        Err("ancestry pid")
    }

    fn read_action(correlation: &str) -> Result<String, &'static str> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Action {
            version: u8,
            correlation: String,
            action: String,
        }
        let mut frame = String::new();
        std::io::stdin()
            .read_line(&mut frame)
            .map_err(|_| "probe action")?;
        if frame.len() > 1024 || !frame.ends_with('\n') {
            return Err("probe action");
        }
        let action: Action = serde_json::from_str(&frame).map_err(|_| "probe action")?;
        if action.version != 1
            || action.correlation != correlation
            || !matches!(action.action.as_str(), "continue" | "terminate")
        {
            return Err("probe action");
        }
        Ok(action.action)
    }

    fn valid_pipe_name(value: &str) -> bool {
        value.starts_with(r"\\.\pipe\TalkingQuill.")
            && value.len() <= 240
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '\\' | '.' | '-'))
    }
    fn unix_ms() -> Result<u64, &'static str> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "clock")?
            .as_millis()
            .try_into()
            .map_err(|_| "clock")
    }
    fn remaining_ms(deadline: u64) -> Result<u32, &'static str> {
        let remaining = deadline.checked_sub(unix_ms()?).ok_or("deadline")?;
        Ok(remaining.clamp(1, u64::from(u32::MAX)) as u32)
    }

    struct SignInput {
        correlation: String,
        signer_path: PathBuf,
        signer_sha256: [u8; 32],
        signer_bytes: u64,
        private_key_path: PathBuf,
        payload: Vec<u8>,
        source_commit: Option<String>,
        source_tree: Option<String>,
    }

    fn sign(input: SignInput) -> Result<Response, &'static str> {
        require_absolute_no_reparse(&input.signer_path)?;
        let mut signer = open_locked(&input.signer_path)?;
        let retained = snapshot(&mut signer)?;
        if retained.sha256 != input.signer_sha256
            || retained.bytes != input.signer_bytes
            || !source_matches(
                &mut signer,
                input.source_commit.as_deref(),
                input.source_tree.as_deref(),
            )?
        {
            return Err("signer identity");
        }
        let mut key = open_validated_private_key(&input.private_key_path)?;
        let directory = protected_snapshot_directory()?;
        let snapshot_path = directory.0.join("acceptance-signer.exe");
        let mut snapshot_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .open(&snapshot_path)
            .map_err(|_| "snapshot create")?;
        signer.rewind().map_err(|_| "signer rewind")?;
        std::io::copy(&mut signer, &mut snapshot_file).map_err(|_| "snapshot copy")?;
        snapshot_file.flush().map_err(|_| "snapshot flush")?;
        drop(snapshot_file);
        let mut snapshot_image = open_locked(&snapshot_path)?;
        let frozen = snapshot(&mut snapshot_image)?;
        if frozen.sha256 != retained.sha256 || frozen.bytes != retained.bytes {
            return Err("snapshot identity");
        }
        let child = launch_signer(&snapshot_path, &mut key.file, &input.payload, &frozen)?;
        let output = child.wait_and_read()?;
        let mut lines = output.stdout.split('\n');
        let signature = lines.next().ok_or("signer output")?;
        let public_key = lines.next().ok_or("signer output")?;
        if lines.next() != Some("")
            || lines.next().is_some()
            || !is_lower_hex(signature, 128)
            || !public_key.starts_with("04")
            || !is_lower_hex(public_key, 130)
            || !output.stderr.is_empty()
            || output.exit_code != 0
        {
            return Err("signer output");
        }
        Ok(Response {
            version: 1,
            correlation: input.correlation,
            result: "passed",
            signer_sha256: hex(&retained.sha256),
            signer_bytes: retained.bytes,
            retained_identity_matches: true,
            process_identity_matches: true,
            process_hash_matches: true,
            parent_identity_matches: true,
            creation_identity_matches: true,
            signature_hex: signature.to_owned(),
            public_key_sec1_hex: public_key.to_owned(),
        })
    }

    struct Child {
        process: Handle,
        _thread: Handle,
        job: Handle,
        stdout: File,
        stderr: File,
        payload_delivery: std::sync::mpsc::Receiver<Result<(), &'static str>>,
    }
    struct ChildOutput {
        stdout: String,
        stderr: String,
        exit_code: u32,
    }

    impl Child {
        fn wait_and_read(mut self) -> Result<ChildOutput, &'static str> {
            if unsafe { WaitForSingleObject(self.process.0, TIMEOUT_MS) } != WAIT_OBJECT_0 {
                unsafe { TerminateJobObject(self.job.0, 80) };
                let _ = unsafe { WaitForSingleObject(self.process.0, 5000) };
                return Err("signer timeout");
            }
            self.payload_delivery
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| "payload delivery")??;
            let mut exit_code = 0;
            if unsafe { GetExitCodeProcess(self.process.0, &mut exit_code) } == 0 {
                return Err("signer exit");
            }
            let mut stdout = String::new();
            let mut stderr = String::new();
            self.stdout
                .read_to_string(&mut stdout)
                .map_err(|_| "stdout read")?;
            self.stderr
                .read_to_string(&mut stderr)
                .map_err(|_| "stderr read")?;
            if stdout.len() > 512 || stderr.len() > 512 {
                return Err("signer output bound");
            }
            Ok(ChildOutput {
                stdout,
                stderr,
                exit_code,
            })
        }
    }

    fn launch_signer(
        path: &Path,
        key: &mut File,
        payload: &[u8],
        expected: &Snapshot,
    ) -> Result<Child, &'static str> {
        let (stdin_read, stdin_write) = input_pipe()?;
        let (stdout_read, stdout_write) = output_pipe()?;
        let (stderr_read, stderr_write) = output_pipe()?;
        let inherited = [
            stdin_read.0,
            stdout_write.0,
            stderr_write.0,
            key.as_raw_handle(),
        ];
        let (mut storage, attributes) = attribute_list(&inherited)?;
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin_read.0;
        startup.StartupInfo.hStdOutput = stdout_write.0;
        startup.StartupInfo.hStdError = stderr_write.0;
        startup.lpAttributeList = attributes.0;
        let job = kill_job()?;
        let argument = format!("{}", key.as_raw_handle() as usize);
        let mut command = command_line(path, &["--private-key-handle-v1", &argument])?;
        let mut application = wide(path.as_os_str())?;
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        make_inheritable(key.as_raw_handle())?;
        let created = unsafe {
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
        };
        let sealed = make_non_inheritable(key.as_raw_handle());
        if created == 0 {
            return sealed.and(Err("create signer"));
        }
        let process = Handle(info.hProcess);
        let thread = Handle(info.hThread);
        if let Err(error) = sealed {
            unsafe { TerminateProcess(process.0, 79) };
            return Err(error);
        }
        if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
            unsafe { TerminateProcess(process.0, 79) };
            return Err("assign signer job");
        }
        drop(attributes);
        storage.clear();
        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);
        let actual = process_snapshot(process.0)?;
        if &actual != expected {
            return Err("process image identity");
        }
        verify_parent_and_creation(info.dwProcessId, process.0)?;
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            return Err("resume signer");
        }
        let payload_delivery = spawn_writer(stdin_write, payload.to_vec(), "payload write");
        Ok(Child {
            process,
            _thread: thread,
            job,
            stdout: stdout_read,
            stderr: stderr_read,
            payload_delivery,
        })
    }

    fn verify_parent_and_creation(pid: u32, process: HANDLE) -> Result<(), &'static str> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
        if snapshot.0 == INVALID_HANDLE_VALUE {
            return Err("process ancestry");
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0 {
            loop {
                if entry.th32ProcessID == pid {
                    found = entry.th32ParentProcessID == unsafe { GetCurrentProcessId() };
                    break;
                }
                if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                    break;
                }
            }
        }
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if !found
            || unsafe {
                windows_sys::Win32::System::Threading::GetProcessTimes(
                    process,
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                )
            } == 0
        {
            return Err("process parent identity");
        }
        let mut parent_creation = FILETIME::default();
        if unsafe {
            windows_sys::Win32::System::Threading::GetProcessTimes(
                GetCurrentProcess(),
                &mut parent_creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
            || filetime_value(creation) < filetime_value(parent_creation)
        {
            return Err("process creation identity");
        }
        Ok(())
    }

    fn filetime_value(value: windows_sys::Win32::Foundation::FILETIME) -> u64 {
        (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
    }

    fn spawn_writer(
        mut output: File,
        bytes: Vec<u8>,
        error: &'static str,
    ) -> std::sync::mpsc::Receiver<Result<(), &'static str>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = output.write_all(&bytes).map_err(|_| error);
            drop(output);
            let _ = sender.send(result);
        });
        receiver
    }

    fn raw_pipe() -> Result<(HANDLE, HANDLE), &'static str> {
        let mut read = null_mut();
        let mut write = null_mut();
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
            return Err("pipe create");
        }
        Ok((read, write))
    }

    fn input_pipe() -> Result<(Handle, File), &'static str> {
        let (read, write) = raw_pipe()?;
        make_non_inheritable(write)?;
        Ok((Handle(read), unsafe { File::from_raw_handle(write) }))
    }

    fn output_pipe() -> Result<(File, Handle), &'static str> {
        let (read, write) = raw_pipe()?;
        make_non_inheritable(read)?;
        Ok((unsafe { File::from_raw_handle(read) }, Handle(write)))
    }

    fn attribute_list(handles: &[HANDLE]) -> Result<(Vec<usize>, Attributes), &'static str> {
        let mut bytes = 0usize;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err("attribute size");
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0
            || unsafe {
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr().cast(),
                    size_of_val(handles),
                    null_mut(),
                    null_mut(),
                )
            } == 0
        {
            return Err("attribute list");
        }
        Ok((storage, Attributes(list)))
    }

    fn kill_job() -> Result<Handle, &'static str> {
        let job = Handle(unsafe { CreateJobObjectW(null(), null()) });
        if job.0.is_null() {
            return Err("job create");
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err("job policy");
        }
        Ok(job)
    }

    fn process_snapshot(process: HANDLE) -> Result<Snapshot, &'static str> {
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

    fn retain_self(expected_sha256: [u8; 32], expected_bytes: u64) -> Result<File, &'static str> {
        let path = std::env::current_exe().map_err(|_| "broker path")?;
        require_absolute_no_reparse(&path)?;
        let mut file = open_locked(&path)?;
        let observed = snapshot(&mut file)?;
        if observed.sha256 != expected_sha256 || observed.bytes != expected_bytes {
            return Err("broker identity");
        }
        Ok(file)
    }

    fn open_locked(path: &Path) -> Result<File, &'static str> {
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| "file open")
    }

    fn snapshot(file: &mut File) -> Result<Snapshot, &'static str> {
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

    fn source_matches(
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

    fn protected_snapshot_directory() -> Result<SnapshotDir, &'static str> {
        let parent = std::env::temp_dir();
        require_absolute_no_reparse(&parent)?;
        let security =
            talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon()
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

    fn require_absolute_no_reparse(path: &Path) -> Result<(), &'static str> {
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

    fn make_inheritable(handle: HANDLE) -> Result<(), &'static str> {
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
            Err("handle inherit")
        } else {
            Ok(())
        }
    }
    fn make_non_inheritable(handle: HANDLE) -> Result<(), &'static str> {
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            Err("handle seal")
        } else {
            Ok(())
        }
    }
    fn validate_header(version: u8, correlation: &str) -> Result<(), &'static str> {
        if version == 1 && is_lower_hex(correlation, 32) {
            Ok(())
        } else {
            Err("request header")
        }
    }
    fn decode_hash(value: &str) -> Result<[u8; 32], &'static str> {
        let bytes = decode_hex(value, 32)?;
        if bytes.len() != 32 {
            return Err("hash");
        }
        bytes.try_into().map_err(|_| "hash")
    }
    fn decode_hex(value: &str, max: usize) -> Result<Vec<u8>, &'static str> {
        if value.is_empty()
            || !value.len().is_multiple_of(2)
            || value.len() / 2 > max
            || !is_lower_hex(value, value.len())
        {
            return Err("hex");
        }
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| "hex")?, 16)
                    .map_err(|_| "hex")
            })
            .collect()
    }
    fn is_lower_hex(value: &str, length: usize) -> bool {
        value.len() == length
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    fn hex(value: &[u8; 32]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>, &'static str> {
        let mut out: Vec<u16> = value.encode_wide().collect();
        if out.is_empty() || out.contains(&0) {
            return Err("wide path");
        }
        out.push(0);
        Ok(out)
    }
    fn quote(value: &str) -> String {
        let mut out = String::from("\"");
        let mut slashes = 0;
        for ch in value.chars() {
            if ch == '\\' {
                slashes += 1;
            } else {
                if ch == '"' {
                    out.push_str(&"\\".repeat(slashes * 2 + 1));
                } else {
                    out.push_str(&"\\".repeat(slashes));
                }
                slashes = 0;
                out.push(ch);
            }
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        out
    }
    fn command_line(path: &Path, arguments: &[&str]) -> Result<Vec<u16>, &'static str> {
        let path = path.to_str().ok_or("path unicode")?;
        let text = std::iter::once(path)
            .chain(arguments.iter().copied())
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        wide(std::ffi::OsStr::new(&text))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use talking_quill_windows_owner_ipc::image_policy::WindowsArchitecture;
        use talking_quill_windows_owner_ipc::peer::{FileIdentity, PeerFacts, SourceIdentity};

        fn facts() -> PeerFacts {
            PeerFacts {
                process_id: 40,
                creation_marker: 400,
                wts_session_id: 3,
                user_sid: vec![1, 2],
                logon_sid: vec![3, 4],
                integrity_rid: 0x2000,
                architecture: WindowsArchitecture::X64,
                canonical_image: PathBuf::from(r"C:\Program Files\Talking Quill\Talking Quill.exe"),
                file_identity: FileIdentity {
                    volume_serial: 9,
                    file_index: 12,
                },
                image_sha256: [7; 32],
                source_identity: Some(SourceIdentity {
                    commit: "1".repeat(40),
                    tree: "2".repeat(40),
                }),
            }
        }

        #[test]
        fn forged_first_client_does_not_consume_the_later_authorized_identity() {
            let expected = facts();
            let mut forged = expected.clone();
            forged.process_id = 41;
            forged.user_sid = vec![9];
            let admitted = [forged, expected.clone()].into_iter().find(|candidate| {
                peer_identity_matches(
                    candidate,
                    &expected,
                    [7; 32],
                    &"1".repeat(40),
                    &"2".repeat(40),
                )
            });
            assert_eq!(admitted.map(|facts| facts.process_id), Some(40));
        }

        #[test]
        fn terminate_action_remains_visible_after_continue() {
            let (sender, receiver) = std::sync::mpsc::channel();
            sender.send(Ok(ControlAction::Continue)).unwrap();
            sender.send(Ok(ControlAction::Terminate)).unwrap();
            drop(sender);
            let control = Control { receiver };
            assert!(control.wait(unix_ms().unwrap() + 1_000).is_ok());
            assert!(matches!(
                control.poll().unwrap(),
                Some(ControlAction::Terminate)
            ));
        }

        #[test]
        fn pid_reuse_and_time_reversing_ancestry_are_rejected() {
            assert!(creation_chain_reaches_root(
                &[(90, 500), (40, 400)],
                40,
                400
            ));
            assert!(!creation_chain_reaches_root(
                &[(90, 500), (40, 401)],
                40,
                400
            ));
            assert!(!creation_chain_reaches_root(
                &[(90, 300), (40, 400)],
                40,
                400
            ));
        }

        #[test]
        fn sid_logon_session_integrity_and_image_mismatches_are_rejected() {
            let expected = facts();
            for mutate in [
                |facts: &mut PeerFacts| facts.user_sid.push(9),
                |facts: &mut PeerFacts| facts.logon_sid.push(9),
                |facts: &mut PeerFacts| facts.wts_session_id += 1,
                |facts: &mut PeerFacts| facts.integrity_rid += 1,
                |facts: &mut PeerFacts| facts.canonical_image.push("forged.exe"),
                |facts: &mut PeerFacts| facts.file_identity.file_index += 1,
                |facts: &mut PeerFacts| facts.image_sha256[0] ^= 1,
                |facts: &mut PeerFacts| {
                    facts.source_identity.as_mut().unwrap().commit = "9".repeat(40)
                },
            ] {
                let mut candidate = expected.clone();
                mutate(&mut candidate);
                assert!(!peer_identity_matches(
                    &candidate,
                    &expected,
                    [7; 32],
                    &"1".repeat(40),
                    &"2".repeat(40)
                ));
            }
        }

        #[test]
        fn signer_rename_hardlink_reparse_and_replacement_are_blocked() {
            let mut nonce = [0u8; 8];
            getrandom::fill(&mut nonce).unwrap();
            let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("tmp")
                .join(format!("acceptance-broker-race-{suffix}"));
            std::fs::create_dir(&root).unwrap();
            let source = root.join("signer.exe");
            let replacement = root.join("replacement.exe");
            let linked = root.join("linked.exe");
            std::fs::write(&source, b"trusted").unwrap();
            std::fs::write(&replacement, b"forged").unwrap();
            std::fs::hard_link(&source, &linked).unwrap();
            let mut linked_file = open_locked(&source).unwrap();
            assert!(snapshot(&mut linked_file).is_err());
            drop(linked_file);
            std::fs::remove_file(&linked).unwrap();
            let retained = open_locked(&source).unwrap();
            assert!(std::fs::rename(&replacement, &source).is_err());
            assert!(std::fs::rename(&source, root.join("renamed.exe")).is_err());
            #[cfg(windows)]
            {
                use std::os::windows::fs::symlink_file;
                let reparse = root.join("reparse.exe");
                if symlink_file(&source, &reparse).is_ok() {
                    assert!(require_absolute_no_reparse(&reparse).is_err());
                }
            }
            drop(retained);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
