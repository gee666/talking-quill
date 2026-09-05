use super::*;

pub(super) struct ProbeInput {
    pub(super) correlation: String,
    pub(super) executable_path: PathBuf,
    pub(super) executable_sha256: [u8; 32],
    pub(super) executable_bytes: u64,
    pub(super) source_commit: String,
    pub(super) source_tree: String,
    pub(super) startup_frame: Vec<u8>,
    pub(super) readiness_pipe: String,
    pub(super) armed_pipe: Option<String>,
    pub(super) armed_expected_phase: Option<String>,
    pub(super) launch_correlation: String,
    pub(super) absolute_deadline_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProbeEvent<'a> {
    pub(super) version: u8,
    pub(super) correlation: &'a str,
    pub(super) event: &'a str,
    pub(super) process_id: u32,
    pub(super) rejected_clients: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) value: Option<serde_json::Value>,
}

pub(super) fn run_probe(input: ProbeInput) -> Result<(), &'static str> {
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
    let security = talking_quill_windows_owner_ipc::endpoint::EndpointSecurity::for_current_logon()
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
            talking_quill_windows_owner_ipc::endpoint::create_server_instance(name, &security, true)
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
    let startup_pipe = talking_quill_windows_owner_ipc::endpoint::create_outbound_server_instance(
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
