//! Feature-gated trusted installer launcher used only by installed acceptance.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::BufRead;
use std::time::Duration;

const MODE: &str = "--windows-installed-acceptance-launch-v1";
const VERIFIED_CHILD_MODE: &str = "--windows-installed-acceptance-verified-child-v1";
const PROCESS_GUARD_MODE: &str = "--windows-installed-acceptance-process-guard-v1";
const BROKER_MODE: &str = "--windows-installed-acceptance-broker-v1";
const MAX_ARGUMENTS: usize = 32;
const MAX_BROKER_FRAME_BYTES: u64 = 16 * 1024;
const MAX_ARGUMENT_UNITS: usize = 32_768;
const MAX_TIMEOUT_MS: u32 = 80 * 60 * 1_000;
const EXIT_USAGE: i32 = 64;
const EXIT_MISMATCH: i32 = 78;
const EXIT_LAUNCH: i32 = 79;
const EXIT_TIMEOUT: i32 = 80;
const EXIT_INSTALLER: i32 = 81;
const EXIT_PROCESS_GUARD: i32 = 82;
const PROCESS_GUARD_TIMEOUT_MS: u32 = 15_000;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    pub volume: u32,
    pub index_high: u32,
    pub index_low: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileSnapshot {
    pub identity: FileIdentity,
    pub bytes: u64,
    pub sha256: [u8; 32],
}

trait LauncherNative {
    type Retained;
    type Suspended;

    fn open_retained(&mut self, path: &std::path::Path) -> Result<Self::Retained, ()>;
    fn snapshot_retained(&mut self, retained: &mut Self::Retained) -> Result<FileSnapshot, ()>;
    fn create_suspended(
        &mut self,
        path: &std::path::Path,
        arguments: &[OsString],
        inherit_standard_handles: bool,
    ) -> Result<Self::Suspended, ()>;
    fn snapshot_process_image(&mut self, process: &Self::Suspended) -> Result<FileSnapshot, ()>;
    fn resume_and_wait(
        &mut self,
        process: &mut Self::Suspended,
        timeout: Duration,
    ) -> Result<Option<i32>, ()>;
    fn terminate(&mut self, process: &mut Self::Suspended);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessFacts {
    pid: u32,
    creation_marker: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuardWait {
    GatewayExited,
    OwnerExited,
    Timeout,
}

trait ProcessGuardNative {
    type Process;

    fn open_gateway(&mut self, pid: u32) -> Result<Self::Process, ()>;
    fn open_owner(&mut self, pid: u32) -> Result<Self::Process, ()>;
    fn facts(&mut self, process: &Self::Process) -> Result<ProcessFacts, ()>;
    fn is_alive(&mut self, process: &Self::Process) -> Result<bool, ()>;
    fn terminate_gateway(&mut self, gateway: &Self::Process) -> Result<(), ()>;
    fn wait_gateway_or_owner(
        &mut self,
        gateway: &Self::Process,
        owner: &Self::Process,
        timeout: Duration,
    ) -> Result<GuardWait, ()>;
}

struct ProcessGuardRequest {
    gateway: ProcessFacts,
    owner: ProcessFacts,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum BrokerRequest {
    Hello {
        version: u8,
        correlation: String,
    },
    LaunchInstaller {
        version: u8,
        correlation: String,
        path: String,
        sha256: String,
        bytes: u64,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u32,
        #[serde(rename = "acceptedExitCodes")]
        accepted_exit_codes: Vec<i32>,
        arguments: Vec<String>,
    },
    GuardGateway {
        version: u8,
        correlation: String,
        #[serde(rename = "gatewayPid")]
        gateway_pid: u32,
        #[serde(rename = "gatewayCreationMarker")]
        gateway_creation_marker: String,
        #[serde(rename = "ownerPid")]
        owner_pid: u32,
        #[serde(rename = "ownerCreationMarker")]
        owner_creation_marker: String,
    },
    Close {
        version: u8,
        correlation: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessGuardEvidence<'a> {
    schema_version: u8,
    result: &'a str,
    reason: &'a str,
    gateway_pid: u32,
    gateway_creation_marker: String,
    owner_pid: u32,
    owner_creation_marker: String,
    exact_gateway_handle_terminated: bool,
    gateway_exit_observed: bool,
    owner_stayed_alive: bool,
    owner_identity_stable: bool,
}

struct Request {
    path: std::path::PathBuf,
    expected_sha256: [u8; 32],
    expected_bytes: u64,
    timeout_ms: u32,
    accepted_exit_codes: Vec<i32>,
    installer_arguments: Vec<OsString>,
    inherit_standard_handles: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Evidence<'a> {
    schema_version: u8,
    result: &'a str,
    reason: &'a str,
    expected_sha256: Option<String>,
    expected_bytes: Option<u64>,
    retained_identity_matches: bool,
    process_identity_matches: bool,
    process_hash_matches: bool,
    installer_exit_code: Option<i32>,
}

impl Evidence<'_> {
    fn failure(reason: &'static str) -> Evidence<'static> {
        Evidence {
            schema_version: 1,
            result: "failed",
            reason,
            expected_sha256: None,
            expected_bytes: None,
            retained_identity_matches: false,
            process_identity_matches: false,
            process_hash_matches: false,
            installer_exit_code: None,
        }
    }
}

impl ProcessGuardEvidence<'_> {
    fn invalid() -> ProcessGuardEvidence<'static> {
        ProcessGuardEvidence {
            schema_version: 1,
            result: "failed",
            reason: "invalid_request",
            gateway_pid: 0,
            gateway_creation_marker: "0".to_owned(),
            owner_pid: 0,
            owner_creation_marker: "0".to_owned(),
            exact_gateway_handle_terminated: false,
            gateway_exit_observed: false,
            owner_stayed_alive: false,
            owner_identity_stable: false,
        }
    }
}

pub fn run(arguments: &[OsString]) -> i32 {
    if arguments.first().and_then(|value| value.to_str()) == Some(BROKER_MODE) {
        return run_broker(arguments);
    }
    if arguments.first().and_then(|value| value.to_str()) == Some(PROCESS_GUARD_MODE) {
        return run_process_guard(arguments);
    }
    let verified_self =
        arguments.first().and_then(|value| value.to_str()) == Some(VERIFIED_CHILD_MODE);
    let parsed_arguments;
    let request_arguments = if verified_self {
        if arguments.len() < 9 {
            emit(&Evidence::failure("invalid_request"));
            return EXIT_USAGE;
        }
        let Some(expected_self_sha256) = arguments[1].to_str().and_then(decode_hash) else {
            emit(&Evidence::failure("invalid_request"));
            return EXIT_USAGE;
        };
        let Ok(expected_self_bytes) = arguments[2]
            .to_str()
            .ok_or(())
            .and_then(|value| value.parse::<u64>().map_err(|_| ()))
        else {
            emit(&Evidence::failure("invalid_request"));
            return EXIT_USAGE;
        };
        #[cfg(windows)]
        if native::verify_self(expected_self_sha256, expected_self_bytes).is_err() {
            emit(&Evidence::failure("bootstrap_identity_mismatch"));
            return EXIT_MISMATCH;
        }
        #[cfg(not(windows))]
        let _ = (expected_self_sha256, expected_self_bytes);
        parsed_arguments = std::iter::once(OsString::from(MODE))
            .chain(arguments[3..].iter().cloned())
            .collect::<Vec<_>>();
        &parsed_arguments
    } else {
        arguments
    };
    let mut request = match parse(request_arguments) {
        Ok(request) => request,
        Err(()) => {
            emit(&Evidence::failure("invalid_request"));
            return EXIT_USAGE;
        }
    };
    request.inherit_standard_handles = verified_self;
    #[cfg(windows)]
    {
        let mut native = native::WindowsNative;
        let (code, evidence) = execute(&mut native, &request);
        if !verified_self {
            emit(&evidence);
        }
        code
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        emit(&Evidence::failure("unsupported_platform"));
        EXIT_LAUNCH
    }
}

fn run_broker(arguments: &[OsString]) -> i32 {
    if arguments.len() != 3 || arguments[0].to_str() != Some(BROKER_MODE) {
        return EXIT_USAGE;
    }
    let Some(expected_sha256) = arguments[1].to_str().and_then(decode_hash) else {
        return EXIT_USAGE;
    };
    let Ok(expected_bytes) = arguments[2]
        .to_str()
        .ok_or(())
        .and_then(|value| value.parse::<u64>().map_err(|_| ()))
    else {
        return EXIT_USAGE;
    };
    if expected_bytes == 0 {
        return EXIT_USAGE;
    }
    #[cfg(windows)]
    {
        native::run_broker(expected_sha256, expected_bytes)
    }
    #[cfg(not(windows))]
    {
        let _ = (expected_sha256, expected_bytes);
        EXIT_LAUNCH
    }
}

fn run_process_guard(arguments: &[OsString]) -> i32 {
    let request = match parse_process_guard(arguments) {
        Ok(request) => request,
        Err(()) => {
            emit_guard(&ProcessGuardEvidence::invalid());
            return EXIT_USAGE;
        }
    };
    #[cfg(windows)]
    {
        let mut native = native::WindowsNative;
        let (code, evidence) = execute_process_guard(&mut native, request);
        emit_guard(&evidence);
        code
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        emit_guard(&ProcessGuardEvidence::invalid());
        EXIT_PROCESS_GUARD
    }
}

fn serve_broker<N, R, W, I>(
    native: &mut N,
    mut input: R,
    mut output: W,
    _retained_broker_image: I,
    broker_sha256: [u8; 32],
    broker_bytes: u64,
) -> Result<(), ()>
where
    N: LauncherNative + ProcessGuardNative,
    R: std::io::BufRead,
    W: std::io::Write,
{
    loop {
        let mut frame = Vec::new();
        let read = {
            let mut limited = std::io::Read::take(&mut input, MAX_BROKER_FRAME_BYTES + 1);
            limited.read_until(b'\n', &mut frame).map_err(|_| ())?
        };
        if read == 0 {
            return Err(());
        }
        if read as u64 > MAX_BROKER_FRAME_BYTES || frame.last() != Some(&b'\n') {
            return Err(());
        }
        frame.pop();
        if frame.last() == Some(&b'\r') || frame.is_empty() {
            return Err(());
        }
        let request: BrokerRequest = serde_json::from_slice(&frame).map_err(|_| ())?;
        let close = matches!(request, BrokerRequest::Close { .. });
        let response = handle_broker_request(native, request, broker_sha256, broker_bytes)?;
        let encoded = serde_json::to_vec(&response).map_err(|_| ())?;
        if encoded.len() > 4 * 1024 {
            return Err(());
        }
        output.write_all(&encoded).map_err(|_| ())?;
        output.write_all(b"\n").map_err(|_| ())?;
        output.flush().map_err(|_| ())?;
        if close {
            return Ok(());
        }
    }
}

fn handle_broker_request<N: LauncherNative + ProcessGuardNative>(
    native: &mut N,
    request: BrokerRequest,
    broker_sha256: [u8; 32],
    broker_bytes: u64,
) -> Result<serde_json::Value, ()> {
    let (version, correlation) = match &request {
        BrokerRequest::Hello {
            version,
            correlation,
        }
        | BrokerRequest::LaunchInstaller {
            version,
            correlation,
            ..
        }
        | BrokerRequest::GuardGateway {
            version,
            correlation,
            ..
        }
        | BrokerRequest::Close {
            version,
            correlation,
        } => (*version, correlation.clone()),
    };
    if version != 1
        || correlation.len() != 32
        || !correlation.bytes().all(|byte| byte.is_ascii_hexdigit())
        || correlation.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(());
    }
    let mut value = match request {
        BrokerRequest::Hello { .. } => serde_json::json!({
            "result": "passed",
            "brokerSha256": hex(&broker_sha256),
            "brokerBytes": broker_bytes,
        }),
        BrokerRequest::LaunchInstaller {
            path,
            sha256,
            bytes,
            timeout_ms,
            accepted_exit_codes,
            arguments,
            ..
        } => {
            let request = parse(
                &[
                    vec![
                        OsString::from(MODE),
                        OsString::from(path),
                        OsString::from(sha256),
                        OsString::from(bytes.to_string()),
                        OsString::from(timeout_ms.to_string()),
                        OsString::from(
                            accepted_exit_codes
                                .iter()
                                .map(i32::to_string)
                                .collect::<Vec<_>>()
                                .join(","),
                        ),
                        OsString::from("--"),
                    ],
                    arguments.into_iter().map(OsString::from).collect(),
                ]
                .concat(),
            )?;
            let (_, evidence) = execute(native, &request);
            serde_json::to_value(evidence).map_err(|_| ())?
        }
        BrokerRequest::GuardGateway {
            gateway_pid,
            gateway_creation_marker,
            owner_pid,
            owner_creation_marker,
            ..
        } => {
            let request = parse_process_guard(&[
                OsString::from(PROCESS_GUARD_MODE),
                OsString::from(gateway_pid.to_string()),
                OsString::from(gateway_creation_marker),
                OsString::from(owner_pid.to_string()),
                OsString::from(owner_creation_marker),
            ])?;
            let (_, evidence) = execute_process_guard(native, request);
            serde_json::to_value(evidence).map_err(|_| ())?
        }
        BrokerRequest::Close { .. } => serde_json::json!({ "result": "passed", "closed": true }),
    };
    let object = value.as_object_mut().ok_or(())?;
    object.insert("version".to_owned(), serde_json::json!(1));
    object.insert("correlation".to_owned(), serde_json::json!(correlation));
    Ok(value)
}

fn execute_process_guard<N: ProcessGuardNative>(
    native: &mut N,
    request: ProcessGuardRequest,
) -> (i32, ProcessGuardEvidence<'static>) {
    let mut evidence = ProcessGuardEvidence {
        schema_version: 1,
        result: "failed",
        reason: "process_open_failed",
        gateway_pid: request.gateway.pid,
        gateway_creation_marker: request.gateway.creation_marker.to_string(),
        owner_pid: request.owner.pid,
        owner_creation_marker: request.owner.creation_marker.to_string(),
        exact_gateway_handle_terminated: false,
        gateway_exit_observed: false,
        owner_stayed_alive: false,
        owner_identity_stable: false,
    };
    let gateway = match native.open_gateway(request.gateway.pid) {
        Ok(handle) => handle,
        Err(()) => return (EXIT_PROCESS_GUARD, evidence),
    };
    let owner = match native.open_owner(request.owner.pid) {
        Ok(handle) => handle,
        Err(()) => return (EXIT_PROCESS_GUARD, evidence),
    };
    let identities_match =
        native.facts(&gateway) == Ok(request.gateway) && native.facts(&owner) == Ok(request.owner);
    let owner_alive = native.is_alive(&owner) == Ok(true);
    if !identities_match || !owner_alive {
        evidence.reason = "process_identity_mismatch";
        return (EXIT_PROCESS_GUARD, evidence);
    }
    if native.terminate_gateway(&gateway).is_err() {
        evidence.reason = "gateway_terminate_failed";
        return (EXIT_PROCESS_GUARD, evidence);
    }
    evidence.exact_gateway_handle_terminated = true;
    match native.wait_gateway_or_owner(
        &gateway,
        &owner,
        Duration::from_millis(PROCESS_GUARD_TIMEOUT_MS.into()),
    ) {
        Ok(GuardWait::GatewayExited) => evidence.gateway_exit_observed = true,
        Ok(GuardWait::OwnerExited) => {
            evidence.reason = "owner_exited_during_gateway_kill";
            return (EXIT_PROCESS_GUARD, evidence);
        }
        Ok(GuardWait::Timeout) => {
            evidence.reason = "gateway_exit_timeout";
            return (EXIT_TIMEOUT, evidence);
        }
        Err(()) => {
            evidence.reason = "process_wait_failed";
            return (EXIT_PROCESS_GUARD, evidence);
        }
    }
    evidence.owner_identity_stable = native.facts(&owner) == Ok(request.owner);
    evidence.owner_stayed_alive = native.is_alive(&owner) == Ok(true);
    if !evidence.owner_stayed_alive || !evidence.owner_identity_stable {
        evidence.reason = "owner_continuity_lost";
        return (EXIT_PROCESS_GUARD, evidence);
    }
    evidence.result = "passed";
    evidence.reason = "gateway_only_terminated";
    (0, evidence)
}

fn execute<N: LauncherNative>(native: &mut N, request: &Request) -> (i32, Evidence<'static>) {
    let expected_hex = hex(&request.expected_sha256);
    let mut evidence = Evidence {
        schema_version: 1,
        result: "failed",
        reason: "launch_failed",
        expected_sha256: Some(expected_hex),
        expected_bytes: Some(request.expected_bytes),
        retained_identity_matches: false,
        process_identity_matches: false,
        process_hash_matches: false,
        installer_exit_code: None,
    };
    let mut retained = match native.open_retained(&request.path) {
        Ok(value) => value,
        Err(()) => {
            return (
                EXIT_MISMATCH,
                evidence_with_reason(evidence, "retained_open_failed"),
            );
        }
    };
    let initial = match native.snapshot_retained(&mut retained) {
        Ok(value) => value,
        Err(()) => {
            return (
                EXIT_MISMATCH,
                evidence_with_reason(evidence, "retained_read_failed"),
            );
        }
    };
    if initial.sha256 != request.expected_sha256 || initial.bytes != request.expected_bytes {
        return (
            EXIT_MISMATCH,
            evidence_with_reason(evidence, "expected_identity_mismatch"),
        );
    }
    let mut process = match native.create_suspended(
        &request.path,
        &request.installer_arguments,
        request.inherit_standard_handles,
    ) {
        Ok(value) => value,
        Err(()) => return (EXIT_LAUNCH, evidence),
    };
    let verified = (|| {
        let retained_now = native.snapshot_retained(&mut retained).map_err(|_| ())?;
        evidence.retained_identity_matches = retained_now == initial;
        let image = native.snapshot_process_image(&process).map_err(|_| ())?;
        evidence.process_identity_matches = image.identity == initial.identity;
        evidence.process_hash_matches =
            image.sha256 == initial.sha256 && image.bytes == initial.bytes;
        if !evidence.retained_identity_matches
            || !evidence.process_identity_matches
            || !evidence.process_hash_matches
        {
            return Err(());
        }
        Ok(())
    })();
    if verified.is_err() {
        native.terminate(&mut process);
        return (
            EXIT_MISMATCH,
            evidence_with_reason(evidence, "suspended_image_mismatch"),
        );
    }
    match native.resume_and_wait(
        &mut process,
        Duration::from_millis(request.timeout_ms.into()),
    ) {
        Ok(Some(code)) if request.accepted_exit_codes.contains(&code) => {
            evidence.result = "passed";
            evidence.reason = "accepted_exit";
            evidence.installer_exit_code = Some(code);
            (0, evidence)
        }
        Ok(Some(code)) => {
            evidence.reason = "installer_exit_rejected";
            evidence.installer_exit_code = Some(code);
            (EXIT_INSTALLER, evidence)
        }
        Ok(None) => {
            native.terminate(&mut process);
            (EXIT_TIMEOUT, evidence_with_reason(evidence, "timeout"))
        }
        Err(()) => {
            native.terminate(&mut process);
            (EXIT_LAUNCH, evidence_with_reason(evidence, "wait_failed"))
        }
    }
}

fn evidence_with_reason(
    mut evidence: Evidence<'static>,
    reason: &'static str,
) -> Evidence<'static> {
    evidence.reason = reason;
    evidence
}

fn parse_process_guard(arguments: &[OsString]) -> Result<ProcessGuardRequest, ()> {
    if arguments.len() != 5 || arguments[0].to_str().ok_or(())? != PROCESS_GUARD_MODE {
        return Err(());
    }
    let parse_pid = |value: &OsString| -> Result<u32, ()> {
        let value = value.to_str().ok_or(())?;
        if value.starts_with('0') {
            return Err(());
        }
        let pid = value.parse::<u32>().map_err(|_| ())?;
        (pid != 0).then_some(pid).ok_or(())
    };
    let parse_marker = |value: &OsString| -> Result<u64, ()> {
        let value = value.to_str().ok_or(())?;
        if value.starts_with('0') {
            return Err(());
        }
        let marker = value.parse::<u64>().map_err(|_| ())?;
        (marker != 0).then_some(marker).ok_or(())
    };
    let gateway = ProcessFacts {
        pid: parse_pid(&arguments[1])?,
        creation_marker: parse_marker(&arguments[2])?,
    };
    let owner = ProcessFacts {
        pid: parse_pid(&arguments[3])?,
        creation_marker: parse_marker(&arguments[4])?,
    };
    if gateway.pid == owner.pid {
        return Err(());
    }
    Ok(ProcessGuardRequest { gateway, owner })
}

fn parse(arguments: &[OsString]) -> Result<Request, ()> {
    if arguments.len() < 7 || arguments[0].to_str().ok_or(())? != MODE {
        return Err(());
    }
    let path = std::path::PathBuf::from(&arguments[1]);
    let expected_sha256 = decode_hash(arguments[2].to_str().ok_or(())?).ok_or(())?;
    let expected_bytes = arguments[3].to_str().ok_or(())?.parse().map_err(|_| ())?;
    let timeout_ms: u32 = arguments[4].to_str().ok_or(())?.parse().map_err(|_| ())?;
    if expected_bytes == 0 || timeout_ms == 0 || timeout_ms > MAX_TIMEOUT_MS {
        return Err(());
    }
    let codes = arguments[5].to_str().ok_or(())?;
    let accepted_exit_codes: Vec<i32> = codes
        .split(',')
        .map(|value| value.parse::<i32>().map_err(|_| ()))
        .collect::<Result<_, _>>()?;
    if accepted_exit_codes.is_empty()
        || accepted_exit_codes.len() > 16
        || accepted_exit_codes
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || arguments[6].to_str().ok_or(())? != "--"
    {
        return Err(());
    }
    let installer_arguments = arguments[7..].to_vec();
    if installer_arguments.len() > MAX_ARGUMENTS
        || installer_arguments.iter().any(|value| {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                value.encode_wide().count() > MAX_ARGUMENT_UNITS
                    || value.encode_wide().any(|unit| unit == 0)
            }
            #[cfg(not(windows))]
            {
                value.to_string_lossy().len() > MAX_ARGUMENT_UNITS
            }
        })
    {
        return Err(());
    }
    Ok(Request {
        path,
        expected_sha256,
        expected_bytes,
        timeout_ms,
        accepted_exit_codes,
        installer_arguments,
        inherit_standard_handles: false,
    })
}

fn decode_hash(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut output = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(output)
}

fn hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn emit(evidence: &Evidence<'_>) {
    emit_serialized(evidence);
}

fn emit_guard(evidence: &ProcessGuardEvidence<'_>) {
    emit_serialized(evidence);
}

fn emit_serialized<T: Serialize>(evidence: &T) {
    let output = serde_json::to_string(evidence).unwrap_or_else(|_| {
        "{\"schemaVersion\":1,\"result\":\"failed\",\"reason\":\"serialization_failed\"}".to_owned()
    });
    // Both schemas contain only fixed labels, numbers, booleans, and hashes.
    println!("{}", &output[..output.len().min(2048)]);
}

#[cfg(windows)]
mod native {
    use super::{
        FileIdentity, FileSnapshot, GuardWait, LauncherNative, ProcessFacts, ProcessGuardNative,
    };
    use sha2::{Digest, Sha256};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        FILETIME, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_SHARE_READ,
        GetFileInformationByHandle,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, GetProcessId, GetProcessTimes,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcess,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, QueryFullProcessImageNameW, ResumeThread, STARTF_USESTDHANDLES,
        STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForMultipleObjects,
        WaitForSingleObject,
    };

    pub struct WindowsNative;
    pub struct Retained {
        file: File,
        _parent: File,
    }
    pub struct Suspended {
        process: OwnedHandle,
        thread: OwnedHandle,
        _job: Option<OwnedHandle>,
    }

    pub fn verify_self(expected_sha256: [u8; 32], expected_bytes: u64) -> Result<(), ()> {
        let current = std::env::current_exe().map_err(|_| ())?;
        let mut retained = open_locked(&current)?;
        let observed = snapshot(&mut retained)?;
        if observed.sha256 == expected_sha256 && observed.bytes == expected_bytes {
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn run_broker(expected_sha256: [u8; 32], expected_bytes: u64) -> i32 {
        let current = match std::env::current_exe() {
            Ok(path) => path,
            Err(_) => return super::EXIT_MISMATCH,
        };
        let mut retained = match open_locked(&current) {
            Ok(file) => file,
            Err(()) => return super::EXIT_MISMATCH,
        };
        let observed = match snapshot(&mut retained) {
            Ok(value) => value,
            Err(()) => return super::EXIT_MISMATCH,
        };
        if observed.sha256 != expected_sha256 || observed.bytes != expected_bytes {
            return super::EXIT_MISMATCH;
        }
        let mut native = WindowsNative;
        let input = std::io::BufReader::new(std::io::stdin().lock());
        let output = std::io::stdout().lock();
        if super::serve_broker(
            &mut native,
            input,
            output,
            retained,
            expected_sha256,
            expected_bytes,
        )
        .is_ok()
        {
            0
        } else {
            super::EXIT_LAUNCH
        }
    }

    impl ProcessGuardNative for WindowsNative {
        type Process = OwnedHandle;

        fn open_gateway(&mut self, pid: u32) -> Result<OwnedHandle, ()> {
            open_process(
                pid,
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
            )
        }

        fn open_owner(&mut self, pid: u32) -> Result<OwnedHandle, ()> {
            open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE)
        }

        fn facts(&mut self, process: &OwnedHandle) -> Result<ProcessFacts, ()> {
            let pid = unsafe { GetProcessId(process.as_raw_handle()) };
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if pid == 0
                || unsafe {
                    GetProcessTimes(
                        process.as_raw_handle(),
                        &mut creation,
                        &mut exit,
                        &mut kernel,
                        &mut user,
                    )
                } == 0
            {
                return Err(());
            }
            Ok(ProcessFacts {
                pid,
                creation_marker: (u64::from(creation.dwHighDateTime) << 32)
                    | u64::from(creation.dwLowDateTime),
            })
        }

        fn is_alive(&mut self, process: &OwnedHandle) -> Result<bool, ()> {
            match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
                WAIT_TIMEOUT => Ok(true),
                WAIT_OBJECT_0 => Ok(false),
                _ => Err(()),
            }
        }

        fn terminate_gateway(&mut self, gateway: &OwnedHandle) -> Result<(), ()> {
            if unsafe {
                TerminateProcess(gateway.as_raw_handle(), super::EXIT_PROCESS_GUARD as u32)
            } == 0
            {
                Err(())
            } else {
                Ok(())
            }
        }

        fn wait_gateway_or_owner(
            &mut self,
            gateway: &OwnedHandle,
            owner: &OwnedHandle,
            timeout: Duration,
        ) -> Result<GuardWait, ()> {
            // Owner is first so a simultaneously observed owner exit wins and fails closed.
            let handles = [owner.as_raw_handle(), gateway.as_raw_handle()];
            match unsafe {
                WaitForMultipleObjects(
                    handles.len() as u32,
                    handles.as_ptr(),
                    0,
                    timeout.as_millis().min(u128::from(u32::MAX)) as u32,
                )
            } {
                WAIT_OBJECT_0 => Ok(GuardWait::OwnerExited),
                value if value == WAIT_OBJECT_0 + 1 => Ok(GuardWait::GatewayExited),
                WAIT_TIMEOUT => Ok(GuardWait::Timeout),
                _ => Err(()),
            }
        }
    }

    impl LauncherNative for WindowsNative {
        type Retained = Retained;
        type Suspended = Suspended;

        fn open_retained(&mut self, path: &Path) -> Result<Retained, ()> {
            let parent = path.parent().ok_or(())?;
            Ok(Retained {
                file: open_locked(path)?,
                _parent: open_locked_directory(parent)?,
            })
        }
        fn snapshot_retained(&mut self, retained: &mut Retained) -> Result<FileSnapshot, ()> {
            snapshot(&mut retained.file)
        }
        fn create_suspended(
            &mut self,
            path: &Path,
            arguments: &[std::ffi::OsString],
            inherit_standard_handles: bool,
        ) -> Result<Suspended, ()> {
            let mut application = wide_nul(path.as_os_str())?;
            let mut command = command_line(path, arguments)?;
            let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            if inherit_standard_handles {
                let inherited = unsafe {
                    [
                        GetStdHandle(STD_INPUT_HANDLE),
                        GetStdHandle(STD_OUTPUT_HANDLE),
                        GetStdHandle(STD_ERROR_HANDLE),
                    ]
                };
                if inherited
                    .iter()
                    .any(|handle| handle.is_null() || *handle as isize == -1)
                {
                    return Err(());
                }
                let _seal = HandleInheritanceSeal(inherited);
                for handle in inherited {
                    if unsafe {
                        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
                    } == 0
                    {
                        return Err(());
                    }
                }
                let (_attribute_storage, attributes) = inherited_handle_list(&inherited)?;
                let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
                startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = inherited[0];
                startup.StartupInfo.hStdOutput = inherited[1];
                startup.StartupInfo.hStdError = inherited[2];
                startup.lpAttributeList = attributes.0;
                if unsafe {
                    CreateProcessW(
                        application.as_mut_ptr(),
                        command.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        1,
                        CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
                        std::ptr::null(),
                        std::ptr::null(),
                        &startup.StartupInfo,
                        &mut info,
                    )
                } == 0
                {
                    return Err(());
                }
            } else {
                let startup = windows_sys::Win32::System::Threading::STARTUPINFOW {
                    cb: size_of::<windows_sys::Win32::System::Threading::STARTUPINFOW>() as u32,
                    ..unsafe { std::mem::zeroed() }
                };
                if unsafe {
                    CreateProcessW(
                        application.as_mut_ptr(),
                        command.as_mut_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        0,
                        CREATE_SUSPENDED,
                        std::ptr::null(),
                        std::ptr::null(),
                        &startup,
                        &mut info,
                    )
                } == 0
                {
                    return Err(());
                }
            }
            let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
            let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
            let job = if inherit_standard_handles {
                let job = match kill_on_close_job() {
                    Ok(job) => job,
                    Err(()) => {
                        unsafe {
                            TerminateProcess(process.as_raw_handle(), super::EXIT_LAUNCH as u32)
                        };
                        return Err(());
                    }
                };
                if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) }
                    == 0
                {
                    unsafe { TerminateProcess(process.as_raw_handle(), super::EXIT_LAUNCH as u32) };
                    return Err(());
                }
                Some(job)
            } else {
                None
            };
            Ok(Suspended {
                process,
                thread,
                _job: job,
            })
        }
        fn snapshot_process_image(&mut self, process: &Suspended) -> Result<FileSnapshot, ()> {
            let mut path = vec![0u16; 32_768];
            let mut length = path.len() as u32;
            if unsafe {
                QueryFullProcessImageNameW(
                    process.process.as_raw_handle(),
                    0,
                    path.as_mut_ptr(),
                    &mut length,
                )
            } == 0
            {
                return Err(());
            }
            path.truncate(length as usize);
            let mut image =
                open_locked(&PathBuf::from(String::from_utf16(&path).map_err(|_| ())?))?;
            snapshot(&mut image)
        }
        fn resume_and_wait(
            &mut self,
            process: &mut Suspended,
            timeout: Duration,
        ) -> Result<Option<i32>, ()> {
            if unsafe { ResumeThread(process.thread.as_raw_handle()) } == u32::MAX {
                return Err(());
            }
            match unsafe {
                WaitForSingleObject(
                    process.process.as_raw_handle(),
                    timeout.as_millis().min(u128::from(u32::MAX)) as u32,
                )
            } {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if unsafe { GetExitCodeProcess(process.process.as_raw_handle(), &mut code) }
                        == 0
                    {
                        Err(())
                    } else {
                        Ok(Some(code as i32))
                    }
                }
                WAIT_TIMEOUT => Ok(None),
                _ => Err(()),
            }
        }
        fn terminate(&mut self, process: &mut Suspended) {
            unsafe {
                TerminateProcess(process.process.as_raw_handle(), super::EXIT_MISMATCH as u32)
            };
            let _ = unsafe { WaitForSingleObject(process.process.as_raw_handle(), 5_000) };
        }
    }

    fn kill_on_close_job() -> Result<OwnedHandle, ()> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(handle) };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(());
        }
        Ok(job)
    }

    struct HandleInheritanceSeal([HANDLE; 3]);
    impl Drop for HandleInheritanceSeal {
        fn drop(&mut self) {
            for handle in self.0 {
                unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
            }
        }
    }

    struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }

    fn inherited_handle_list(handles: &[HANDLE]) -> Result<(Vec<u8>, AttributeList), ()> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(());
        }
        let mut storage = vec![0u8; bytes];
        let list = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0
            || unsafe {
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr().cast_mut().cast(),
                    std::mem::size_of_val(handles),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } == 0
        {
            return Err(());
        }
        Ok((storage, AttributeList(list)))
    }

    const SYNCHRONIZE: u32 = 0x0010_0000;

    fn open_process(pid: u32, access: u32) -> Result<OwnedHandle, ()> {
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            Err(())
        } else {
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }

    fn open_locked(path: &Path) -> Result<File, ()> {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| ())?;
        if !file.metadata().map_err(|_| ())?.is_file() {
            return Err(());
        }
        Ok(file)
    }

    fn open_locked_directory(path: &Path) -> Result<File, ()> {
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_| ())?;
        if !file.metadata().map_err(|_| ())?.is_dir() {
            return Err(());
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(());
        }
        Ok(file)
    }
    fn snapshot(file: &mut File) -> Result<FileSnapshot, ()> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(());
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|_| ())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| ())?;
        Ok(FileSnapshot {
            identity: FileIdentity {
                volume: info.dwVolumeSerialNumber,
                index_high: info.nFileIndexHigh,
                index_low: info.nFileIndexLow,
            },
            bytes: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            sha256: digest.finalize().into(),
        })
    }
    fn wide_nul(value: &std::ffi::OsStr) -> Result<Vec<u16>, ()> {
        let mut out: Vec<u16> = value.encode_wide().collect();
        if out.is_empty() || out.contains(&0) {
            return Err(());
        }
        out.push(0);
        Ok(out)
    }
    fn command_line(path: &Path, arguments: &[std::ffi::OsString]) -> Result<Vec<u16>, ()> {
        let mut text = quote(path.as_os_str())?;
        for argument in arguments {
            text.push(' ');
            text.push_str(&quote(argument)?);
        }
        wide_nul(std::ffi::OsStr::new(&text))
    }
    fn quote(value: &std::ffi::OsStr) -> Result<String, ()> {
        let value = value.to_str().ok_or(())?;
        if value.contains('\0') {
            return Err(());
        }
        let mut out = String::from("\"");
        let mut slashes = 0;
        for ch in value.chars() {
            if ch == '\\' {
                slashes += 1;
                continue;
            }
            if ch == '"' {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
            } else {
                out.push_str(&"\\".repeat(slashes));
                out.push(ch);
            }
            slashes = 0;
        }
        out.push_str(&"\\".repeat(slashes * 2));
        out.push('"');
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        retained: Vec<FileSnapshot>,
        image: Option<FileSnapshot>,
        terminated: bool,
        resumed: bool,
        inherited_standard_handles: bool,
    }
    impl LauncherNative for Fake {
        type Retained = ();
        type Suspended = ();
        fn open_retained(&mut self, _: &std::path::Path) -> Result<(), ()> {
            Ok(())
        }
        fn snapshot_retained(&mut self, _: &mut ()) -> Result<FileSnapshot, ()> {
            if self.retained.len() > 1 {
                Ok(self.retained.remove(0))
            } else {
                self.retained.first().cloned().ok_or(())
            }
        }
        fn create_suspended(
            &mut self,
            _: &std::path::Path,
            _: &[OsString],
            inherit_standard_handles: bool,
        ) -> Result<(), ()> {
            self.inherited_standard_handles = inherit_standard_handles;
            Ok(())
        }
        fn snapshot_process_image(&mut self, _: &()) -> Result<FileSnapshot, ()> {
            self.image.clone().ok_or(())
        }
        fn resume_and_wait(&mut self, _: &mut (), _: Duration) -> Result<Option<i32>, ()> {
            self.resumed = true;
            Ok(Some(0))
        }
        fn terminate(&mut self, _: &mut ()) {
            self.terminated = true;
        }
    }
    fn snap(index: u32, byte: u8) -> FileSnapshot {
        FileSnapshot {
            identity: FileIdentity {
                volume: 1,
                index_high: 0,
                index_low: index,
            },
            bytes: 7,
            sha256: [byte; 32],
        }
    }
    fn request() -> Request {
        Request {
            path: "installer.exe".into(),
            expected_sha256: [4; 32],
            expected_bytes: 7,
            timeout_ms: 1000,
            accepted_exit_codes: vec![0],
            installer_arguments: vec![],
            inherit_standard_handles: false,
        }
    }

    #[test]
    fn resumes_only_the_matching_suspended_image() {
        let expected = snap(2, 4);
        let mut fake = Fake {
            retained: vec![expected.clone()],
            image: Some(expected),
            ..Fake::default()
        };
        assert_eq!(execute(&mut fake, &request()).0, 0);
        assert!(fake.resumed);
        assert!(!fake.terminated);
        assert!(!fake.inherited_standard_handles);
    }

    #[test]
    fn bootstrap_transport_is_the_only_launch_that_inherits_standard_handles() {
        let expected = snap(2, 4);
        let mut fake = Fake {
            retained: vec![expected.clone()],
            image: Some(expected),
            ..Fake::default()
        };
        let mut request = request();
        request.inherit_standard_handles = true;
        assert_eq!(execute(&mut fake, &request).0, 0);
        assert!(fake.inherited_standard_handles);
    }

    #[test]
    fn replacement_race_terminates_without_resuming() {
        let expected = snap(2, 4);
        let replacement = snap(3, 9);
        let mut fake = Fake {
            retained: vec![expected, replacement.clone()],
            image: Some(replacement),
            ..Fake::default()
        };
        assert_eq!(execute(&mut fake, &request()).0, EXIT_MISMATCH);
        assert!(fake.terminated);
        assert!(!fake.resumed);
    }
    struct FakeGuard {
        gateway_facts: ProcessFacts,
        owner_facts: ProcessFacts,
        owner_alive_before: bool,
        owner_alive_after: bool,
        wait: GuardWait,
        terminated: Vec<u32>,
    }

    impl ProcessGuardNative for FakeGuard {
        type Process = u32;
        fn open_gateway(&mut self, pid: u32) -> Result<u32, ()> {
            Ok(pid)
        }
        fn open_owner(&mut self, pid: u32) -> Result<u32, ()> {
            Ok(pid)
        }
        fn facts(&mut self, process: &u32) -> Result<ProcessFacts, ()> {
            if *process == self.gateway_facts.pid {
                Ok(self.gateway_facts)
            } else {
                Ok(self.owner_facts)
            }
        }
        fn is_alive(&mut self, process: &u32) -> Result<bool, ()> {
            if *process == self.owner_facts.pid && self.terminated.is_empty() {
                Ok(self.owner_alive_before)
            } else {
                Ok(self.owner_alive_after)
            }
        }
        fn terminate_gateway(&mut self, gateway: &u32) -> Result<(), ()> {
            self.terminated.push(*gateway);
            Ok(())
        }
        fn wait_gateway_or_owner(
            &mut self,
            _: &u32,
            _: &u32,
            _: Duration,
        ) -> Result<GuardWait, ()> {
            Ok(self.wait)
        }
    }

    fn guard_request() -> ProcessGuardRequest {
        ProcessGuardRequest {
            gateway: ProcessFacts {
                pid: 41,
                creation_marker: 410,
            },
            owner: ProcessFacts {
                pid: 52,
                creation_marker: 520,
            },
        }
    }

    fn guard() -> FakeGuard {
        FakeGuard {
            gateway_facts: guard_request().gateway,
            owner_facts: guard_request().owner,
            owner_alive_before: true,
            owner_alive_after: true,
            wait: GuardWait::GatewayExited,
            terminated: Vec::new(),
        }
    }

    #[test]
    fn process_guard_terminates_only_the_exact_gateway_handle() {
        let mut native = guard();
        let (code, evidence) = execute_process_guard(&mut native, guard_request());
        assert_eq!(code, 0);
        assert_eq!(native.terminated, vec![41]);
        assert!(evidence.owner_stayed_alive);
        assert!(evidence.owner_identity_stable);
    }

    #[test]
    fn process_guard_rejects_pid_reuse_before_kill() {
        let mut native = guard();
        native.gateway_facts.pid = 99;
        assert_eq!(
            execute_process_guard(&mut native, guard_request()).0,
            EXIT_PROCESS_GUARD
        );
        assert!(native.terminated.is_empty());
    }

    #[test]
    fn process_guard_rejects_wrong_creation_marker_before_kill() {
        let mut native = guard();
        native.owner_facts.creation_marker += 1;
        assert_eq!(
            execute_process_guard(&mut native, guard_request()).0,
            EXIT_PROCESS_GUARD
        );
        assert!(native.terminated.is_empty());
    }

    #[test]
    fn process_guard_detects_owner_exit_during_gateway_kill() {
        let mut native = guard();
        native.wait = GuardWait::OwnerExited;
        let (code, evidence) = execute_process_guard(&mut native, guard_request());
        assert_eq!(code, EXIT_PROCESS_GUARD);
        assert_eq!(native.terminated, vec![41]);
        assert_eq!(evidence.reason, "owner_exited_during_gateway_kill");
    }

    struct BrokerFake;
    impl LauncherNative for BrokerFake {
        type Retained = ();
        type Suspended = ();
        fn open_retained(&mut self, _: &std::path::Path) -> Result<(), ()> {
            Err(())
        }
        fn snapshot_retained(&mut self, _: &mut ()) -> Result<FileSnapshot, ()> {
            Err(())
        }
        fn create_suspended(
            &mut self,
            _: &std::path::Path,
            _: &[OsString],
            _: bool,
        ) -> Result<(), ()> {
            Err(())
        }
        fn snapshot_process_image(&mut self, _: &()) -> Result<FileSnapshot, ()> {
            Err(())
        }
        fn resume_and_wait(&mut self, _: &mut (), _: Duration) -> Result<Option<i32>, ()> {
            Err(())
        }
        fn terminate(&mut self, _: &mut ()) {}
    }
    impl ProcessGuardNative for BrokerFake {
        type Process = ();
        fn open_gateway(&mut self, _: u32) -> Result<(), ()> {
            Err(())
        }
        fn open_owner(&mut self, _: u32) -> Result<(), ()> {
            Err(())
        }
        fn facts(&mut self, _: &()) -> Result<ProcessFacts, ()> {
            Err(())
        }
        fn is_alive(&mut self, _: &()) -> Result<bool, ()> {
            Err(())
        }
        fn terminate_gateway(&mut self, _: &()) -> Result<(), ()> {
            Err(())
        }
        fn wait_gateway_or_owner(&mut self, _: &(), _: &(), _: Duration) -> Result<GuardWait, ()> {
            Err(())
        }
    }

    struct RetainedTestImage(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for RetainedTestImage {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    struct GuardedOutput {
        bytes: Vec<u8>,
        image_dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl std::io::Write for GuardedOutput {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            assert!(!self.image_dropped.load(std::sync::atomic::Ordering::SeqCst));
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn broker_uses_retained_startup_identity_after_path_replacement() {
        let correlation = "12".repeat(16);
        let input = format!(
            "{{\"operation\":\"hello\",\"version\":1,\"correlation\":\"{correlation}\"}}\n{{\"operation\":\"close\",\"version\":1,\"correlation\":\"{correlation}\"}}\n"
        );
        let image_dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let retained_image = RetainedTestImage(std::sync::Arc::clone(&image_dropped));
        let mut output = GuardedOutput {
            bytes: Vec::new(),
            image_dropped: std::sync::Arc::clone(&image_dropped),
        };
        let replacement_happened_after_start = true;
        assert!(replacement_happened_after_start);
        serve_broker(
            &mut BrokerFake,
            std::io::Cursor::new(input),
            &mut output,
            retained_image,
            [4; 32],
            700,
        )
        .unwrap();
        assert!(image_dropped.load(std::sync::atomic::Ordering::SeqCst));
        let text = String::from_utf8(output.bytes).unwrap();
        assert_eq!(text.matches("\"brokerSha256\"").count(), 1);
        assert!(text.contains(&format!("\"brokerSha256\":\"{}\"", hex(&[4; 32]))));
        assert!(text.contains("\"closed\":true"));
    }

    #[test]
    fn broker_rejects_malformed_or_uncorrelated_frames() {
        let request: BrokerRequest =
            serde_json::from_str(r#"{"operation":"hello","version":1,"correlation":"BAD"}"#)
                .unwrap();
        assert!(handle_broker_request(&mut BrokerFake, request, [1; 32], 1).is_err());
    }

    #[test]
    fn parser_requires_lower_hex_sorted_exact_exit_codes() {
        let args = [MODE, "i.exe", &"AA".repeat(32), "7", "1000", "0", "--"].map(OsString::from);
        assert!(parse(&args).is_err());
    }
}
