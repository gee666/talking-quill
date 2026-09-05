use super::*;

pub(super) struct SignInput {
    pub(super) correlation: String,
    pub(super) signer_path: PathBuf,
    pub(super) signer_sha256: [u8; 32],
    pub(super) signer_bytes: u64,
    pub(super) private_key_path: PathBuf,
    pub(super) payload: Vec<u8>,
    pub(super) source_commit: Option<String>,
    pub(super) source_tree: Option<String>,
}

pub(super) fn sign(input: SignInput) -> Result<Response, &'static str> {
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
    let attributes = attribute_list(&inherited)?;
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin_read.0;
    startup.StartupInfo.hStdOutput = stdout_write.0;
    startup.StartupInfo.hStdError = stderr_write.0;
    startup.lpAttributeList = attributes.list;
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
