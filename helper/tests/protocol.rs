use std::{
    io::{Cursor, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, bounded};
use proptest::{
    prelude::*,
    test_runner::{Config as ProptestConfig, RngSeed},
};
use serde_json::{Value, json};
use talking_quill_helper::{
    CriticalDelivery, RunError,
    framing::{MAX_FRAME_BYTES, read_frame, write_frame},
    keyboard::{
        ActivationBinding, ActivationBindings, ActivationKey, EventPhase, HelperEvent, ProfileId,
        SessionKey, Shortcut, ShortcutModifiers,
    },
    platform::{
        CallbackGate, FrontApp, HookStatus, PasteFailure, PasteResult, PermissionState,
        Permissions, Platform, PlatformError, TerminalReason, TerminalSignal,
    },
    protocol::{INBOUND_METHODS, Outbound, Server, encode_outbound, parse_request},
    run_framed_stream,
};

struct FakeState {
    activation: Mutex<ActivationKey>,
    activation_bindings: Mutex<ActivationBindings>,
    activation_enabled: Mutex<bool>,
    capture: Mutex<bool>,
    calls: Mutex<Vec<&'static str>>,
    emit_shutdown_event: AtomicBool,
    gate_was_closed_on_shutdown: AtomicBool,
    oversized_front_app: AtomicBool,
    fail_activation_config: AtomicBool,
    terminal_on_shutdown: AtomicBool,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            activation: Mutex::new(ActivationKey::DEFAULT),
            activation_bindings: Mutex::new(ActivationBindings::default()),
            activation_enabled: Mutex::new(false),
            capture: Mutex::new(false),
            calls: Mutex::new(Vec::new()),
            emit_shutdown_event: AtomicBool::new(false),
            gate_was_closed_on_shutdown: AtomicBool::new(false),
            oversized_front_app: AtomicBool::new(false),
            fail_activation_config: AtomicBool::new(false),
            terminal_on_shutdown: AtomicBool::new(false),
        }
    }
}

impl FakeState {
    fn record(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

struct BlockingAfterData {
    data: Cursor<Vec<u8>>,
    release: Receiver<()>,
}

impl Read for BlockingAfterData {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.data.read(buffer)?;
        if read > 0 {
            return Ok(read);
        }
        let _ = self.release.recv();
        Ok(0)
    }
}

struct FakePlatform {
    state: Arc<FakeState>,
    outbound: Option<crossbeam_channel::Sender<Outbound>>,
    gate: Option<Arc<CallbackGate>>,
}

impl Platform for FakePlatform {
    fn start(
        _outbound: crossbeam_channel::Sender<Outbound>,
        _gate: Arc<CallbackGate>,
        _terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        Ok(Self {
            state: Arc::new(FakeState::default()),
            outbound: None,
            gate: None,
        })
    }

    fn hook_status(&self) -> HookStatus {
        self.state.record("hook_status");
        HookStatus::Ready
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        self.state.record("configure_activation");
        if self.state.fail_activation_config.load(Ordering::Acquire) {
            return Err(PlatformError::NativeFailure);
        }
        *self.state.activation.lock().unwrap() = bindings
            .iter()
            .next()
            .map_or(ActivationKey::DEFAULT, |binding| {
                binding.shortcut().trigger()
            });
        *self.state.activation_bindings.lock().unwrap() = bindings;
        *self.state.activation_enabled.lock().unwrap() = enabled;
        Ok(())
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        self.state.record("set_session_capture");
        *self.state.capture.lock().unwrap() = active;
        Ok(())
    }

    fn inject_paste(&self) -> PasteResult {
        self.state.record("inject_paste");
        PasteResult {
            submitted: true,
            reason: None,
        }
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        self.state.record("front_app");
        if self.state.oversized_front_app.load(Ordering::Acquire) {
            Ok(FrontApp {
                process_name: "\u{0001}".repeat(10_000),
                window_title: "\u{0001}".repeat(10_000),
                window_bounds: None,
            })
        } else {
            Ok(FrontApp {
                process_name: "target.exe".into(),
                window_title: "Document".into(),
                window_bounds: None,
            })
        }
    }

    fn permissions(&self) -> Permissions {
        self.state.record("permissions");
        Permissions {
            accessibility: PermissionState::NotApplicable,
            input_monitoring: PermissionState::NotApplicable,
            event_post: PermissionState::NotApplicable,
        }
    }

    fn shutdown(&mut self) -> Option<TerminalReason> {
        self.state.record("shutdown");
        if let Some(gate) = &self.gate {
            self.state
                .gate_was_closed_on_shutdown
                .store(!gate.is_open(), Ordering::Release);
        }
        if self.state.emit_shutdown_event.load(Ordering::Acquire)
            && let Some(outbound) = &self.outbound
        {
            let _ = outbound.try_send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    Shortcut::new(
                        ShortcutModifiers {
                            ctrl: false,
                            alt: true,
                            shift: false,
                            meta: false,
                        },
                        &[ActivationKey::Z],
                    )
                    .unwrap(),
                ),
                phase: EventPhase::Up,
            }));
        }
        self.state
            .terminal_on_shutdown
            .load(Ordering::Acquire)
            .then_some(TerminalReason::OwnerThreadUnresponsive)
    }
}

fn setup_observable() -> (
    Server<FakePlatform>,
    Receiver<Outbound>,
    Arc<FakeState>,
    Arc<CallbackGate>,
) {
    let gate = Arc::new(CallbackGate::new());
    let state = Arc::new(FakeState::default());
    let (terminal_tx, _terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (sender, receiver) = bounded(32);
    let (critical_sender, critical_receiver) = bounded::<CriticalDelivery>(1);
    let relay = sender.clone();
    thread::spawn(move || {
        while let Ok(delivery) = critical_receiver.recv() {
            let Some(batch) = delivery.accept() else {
                return;
            };
            for message in batch {
                if relay.send(message).is_err() {
                    return;
                }
            }
        }
    });
    (
        Server::new(
            FakePlatform {
                state: Arc::clone(&state),
                outbound: Some(sender.clone()),
                gate: Some(Arc::clone(&gate)),
            },
            sender,
            critical_sender,
            Arc::clone(&gate),
            terminal,
        ),
        receiver,
        state,
        gate,
    )
}

fn fake_platform() -> FakePlatform {
    FakePlatform {
        state: Arc::new(FakeState::default()),
        outbound: None,
        gate: None,
    }
}

fn setup() -> (Server<FakePlatform>, Receiver<Outbound>) {
    let (server, receiver, _state, _gate) = setup_observable();
    (server, receiver)
}

fn request(id: u64, method: &str, params: Value) -> Vec<u8> {
    request_with_id(json!(id), method, params)
}

fn request_with_id(id: Value, method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .unwrap()
}

fn notification(method: &str, params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .unwrap()
}

fn raw_request(id: &str, method: &str, params: &str) -> Vec<u8> {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":{},"params":{params}}}"#,
        serde_json::to_string(method).unwrap()
    )
    .into_bytes()
}

fn receive(receiver: &Receiver<Outbound>) -> Value {
    let outbound = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("response");
    serde_json::from_slice(&encode_outbound(&outbound).unwrap()).unwrap()
}

fn shortcut_value(keys: &[&str], ctrl: bool, alt: bool, shift: bool, meta: bool) -> Value {
    json!({
        "modifiers": {"ctrl": ctrl, "alt": alt, "shift": shift, "meta": meta},
        "keys": keys,
    })
}

fn alt_shortcut(key: &str, shift: bool) -> Value {
    shortcut_value(&[key], false, true, shift, false)
}

fn profile_id(index: usize) -> String {
    match index {
        0 => "general".to_owned(),
        1 => "prompt".to_owned(),
        _ => format!("00000000-0000-4000-8000-{index:012x}"),
    }
}

fn binding_value(profile_id: &str, shortcut: Value) -> Value {
    json!({"profileId": profile_id, "shortcut": shortcut})
}

fn alt_binding(profile_id: &str, key: &str, shift: bool) -> Value {
    binding_value(profile_id, alt_shortcut(key, shift))
}

fn alt_shortcut_model(key: ActivationKey, shift: bool) -> Shortcut {
    Shortcut::new(
        ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift,
            meta: false,
        },
        &[key],
    )
    .unwrap()
}

fn initialize(server: &mut Server<FakePlatform>, receiver: &Receiver<Outbound>) {
    assert!(server.handle_payload(&request(1, "initialize", json!({"protocolVersion": 5}),)));
    let response = receive(receiver);
    assert_eq!(response["result"]["protocolVersion"], 5);
    assert!(response["result"].get("defaultActivationKey").is_none());
}

fn assert_error(receiver: &Receiver<Outbound>, code: i64, id: Value) {
    let response = receive(receiver);
    assert_eq!(response["error"]["code"], code, "{response}");
    assert_eq!(response["id"], id, "{response}");
    assert!(response.get("result").is_none(), "{response}");
}

fn setup_for_method(method: &str) -> (Server<FakePlatform>, Receiver<Outbound>) {
    let (mut server, receiver) = setup();
    if method != "initialize" {
        initialize(&mut server, &receiver);
    }
    (server, receiver)
}

#[test]
fn inbound_allowlist_is_exact() {
    assert_eq!(
        INBOUND_METHODS,
        [
            "initialize",
            "activation.configure",
            "session.set_capture",
            "paste.inject",
            "front_app.get",
            "permissions.get",
            "ping",
            "shutdown",
        ]
    );
}

#[test]
fn initialization_must_be_first_exactly_once_and_exactly_version_five() {
    let (mut server, receiver, state, gate) = setup_observable();
    assert!(!gate.is_open());

    assert!(server.handle_payload(&request(0, "unknown", json!({}))));
    assert_error(&receiver, -32_601, json!(0));
    assert!(!gate.is_open());

    assert!(server.handle_payload(&request(1, "ping", json!({}))));
    assert_error(&receiver, -32_002, json!(1));
    assert!(!gate.is_open());

    assert!(server.handle_payload(&request(
        2,
        "initialize",
        json!({"protocolVersion": 1, "extra": true}),
    )));
    assert_error(&receiver, -32_602, json!(2));
    assert!(!gate.is_open());

    assert!(server.handle_payload(&request(3, "initialize", json!({"protocolVersion": 1}),)));
    assert_error(&receiver, -32_001, json!(3));
    assert!(!gate.is_open());

    initialize(&mut server, &receiver);
    assert!(gate.is_open());

    assert!(server.handle_payload(&request(4, "initialize", json!({}))));
    assert_error(&receiver, -32_002, json!(4));
    assert!(gate.is_open());

    assert!(server.handle_payload(&request(5, "session.set_capture", json!({"active": true}))));
    assert_eq!(receive(&receiver)["result"], json!({"active": true}));
    assert!(*state.capture.lock().unwrap());

    assert!(!server.handle_payload(&request(6, "shutdown", json!({}))));
    assert_eq!(
        receive(&receiver),
        json!({"jsonrpc": "2.0", "id": 6, "result": {}})
    );
    assert!(!*state.capture.lock().unwrap());
    assert!(!gate.is_open());
}

#[test]
fn successful_paste_commit_disables_session_capture_before_responding() {
    let (mut server, receiver, state, _gate) = setup_observable();
    initialize(&mut server, &receiver);
    assert!(server.handle_payload(&request(2, "session.set_capture", json!({"active": true}),)));
    let _ = receive(&receiver);
    assert!(*state.capture.lock().unwrap());

    assert!(server.handle_payload(&request(3, "paste.inject", json!({}))));
    let committed = receive(&receiver);
    assert_eq!(committed["method"], "paste.committed");
    assert_eq!(committed["params"]["requestId"], 3);
    let response = receive(&receiver);
    assert_eq!(response["result"]["submitted"], true);
    assert!(!*state.capture.lock().unwrap());
}

#[test]
fn string_request_ids_are_echoed_by_responses_and_paste_commit_notifications() {
    let (mut server, receiver, _state, _gate) = setup_observable();
    assert!(server.handle_payload(&request_with_id(
        json!("initialize-id"),
        "initialize",
        json!({"protocolVersion": 5}),
    )));
    assert_eq!(receive(&receiver)["id"], "initialize-id");

    assert!(server.handle_payload(&request_with_id(
        json!("paste-id"),
        "paste.inject",
        json!({}),
    )));
    let committed = receive(&receiver);
    assert_eq!(committed["params"]["requestId"], "paste-id");
    assert_eq!(receive(&receiver)["id"], "paste-id");
}

#[test]
fn activation_stays_disabled_until_exact_configuration_enables_it() {
    let (mut server, receiver, state, _gate) = setup_observable();
    initialize(&mut server, &receiver);
    assert!(!*state.activation_enabled.lock().unwrap());
    assert_eq!(*state.activation.lock().unwrap(), ActivationKey::DEFAULT);
    assert!(
        !state
            .calls
            .lock()
            .unwrap()
            .contains(&"configure_activation")
    );

    assert!(server.handle_payload(&request(
        2,
        "activation.configure",
        json!({"enabled": true, "bindings": [alt_binding("general", "B", false)]}),
    )));
    assert_eq!(
        receive(&receiver),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {"enabled": true, "bindings": [alt_binding("general", "B", false)]},
        })
    );
    assert!(*state.activation_enabled.lock().unwrap());
    assert_eq!(*state.activation.lock().unwrap(), ActivationKey::B);

    assert!(server.handle_payload(&request(
        3,
        "activation.configure",
        json!({"enabled": false, "bindings": [alt_binding("general", "C", false)]}),
    )));
    assert_eq!(
        receive(&receiver)["result"],
        json!({"enabled": false, "bindings": [alt_binding("general", "C", false)]})
    );
    assert!(!*state.activation_enabled.lock().unwrap());
    assert_eq!(*state.activation.lock().unwrap(), ActivationKey::C);
}

#[test]
fn activation_configuration_failure_is_native_error_and_retains_previous_state() {
    let (mut server, receiver, state, _gate) = setup_observable();
    initialize(&mut server, &receiver);
    assert!(server.handle_payload(&request(
        2,
        "activation.configure",
        json!({"enabled": true, "bindings": [alt_binding("general", "A", false)]}),
    )));
    let _ = receive(&receiver);

    state.fail_activation_config.store(true, Ordering::Release);
    assert!(server.handle_payload(&request(
        3,
        "activation.configure",
        json!({"enabled": true, "bindings": [alt_binding("general", "B", false)]}),
    )));
    assert_error(&receiver, -32_003, json!(3));
    assert!(*state.activation_enabled.lock().unwrap());
    assert_eq!(*state.activation.lock().unwrap(), ActivationKey::A);
}

#[test]
fn worst_case_native_front_app_result_is_sanitized_below_frame_limit() {
    let (mut server, receiver, state, _gate) = setup_observable();
    initialize(&mut server, &receiver);
    state.oversized_front_app.store(true, Ordering::Release);

    let id = 9_007_199_254_740_991_u64;
    assert!(server.handle_payload(&request_with_id(json!(id), "front_app.get", json!({}),)));
    let response = receive(&receiver);
    assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_FRAME_BYTES);
    assert!(response["result"]["processName"].as_str().unwrap().len() < 10_000);
    assert!(response["result"]["windowTitle"].as_str().unwrap().len() < 10_000);
}

#[test]
fn shutdown_response_is_enqueued_after_gate_close_and_hook_quiescence() {
    let (mut server, receiver, state, gate) = setup_observable();
    initialize(&mut server, &receiver);
    state.emit_shutdown_event.store(true, Ordering::Release);

    assert!(!server.handle_payload(&request(2, "shutdown", json!({}))));
    assert!(!gate.is_open());
    assert!(state.gate_was_closed_on_shutdown.load(Ordering::Acquire));

    let final_callback_event = receive(&receiver);
    assert_eq!(final_callback_event["method"], "activation.event");
    let shutdown_response = receive(&receiver);
    assert_eq!(
        shutdown_response,
        json!({"jsonrpc": "2.0", "id": 2, "result": {}})
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn platform_shutdown_terminal_failure_suppresses_success_and_survives_clean_eof() {
    let (mut server, receiver, state, gate) = setup_observable();
    initialize(&mut server, &receiver);
    state.terminal_on_shutdown.store(true, Ordering::Release);

    assert!(!server.handle_payload(&request(2, "shutdown", json!({}))));
    assert!(!gate.is_open());
    assert!(receiver.try_recv().is_err());

    let eof_state = Arc::new(FakeState::default());
    eof_state
        .terminal_on_shutdown
        .store(true, Ordering::Release);
    let result = run_framed_stream(
        FakePlatform {
            state: eof_state,
            outbound: None,
            gate: None,
        },
        Cursor::new(Vec::new()),
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(RunError::Terminal(TerminalReason::OwnerThreadUnresponsive))
    ));
}

#[test]
fn in_memory_runner_exercises_multiple_framed_requests_shutdown_eof_and_truncation() {
    let mut input = Vec::new();
    for payload in [
        request(1, "initialize", json!({"protocolVersion": 5})),
        request(2, "ping", json!({})),
        request(3, "shutdown", json!({})),
    ] {
        write_frame(&mut input, &payload).unwrap();
    }
    let mut output = Vec::new();
    run_framed_stream(fake_platform(), Cursor::new(input), &mut output).unwrap();
    let mut output = Cursor::new(output);
    let responses: Vec<Value> = std::iter::from_fn(|| read_frame(&mut output).transpose())
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .map(|payload| serde_json::from_slice(&payload).unwrap())
        .collect();
    assert_eq!(
        responses
            .iter()
            .map(|value| value["id"].as_u64())
            .collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)]
    );
    assert!(read_frame(&mut output).unwrap().is_none());

    let mut eof_input = Vec::new();
    write_frame(
        &mut eof_input,
        &request(1, "initialize", json!({"protocolVersion": 5})),
    )
    .unwrap();
    let mut eof_output = Vec::new();
    run_framed_stream(fake_platform(), Cursor::new(eof_input), &mut eof_output).unwrap();
    assert!(read_frame(&mut Cursor::new(eof_output)).unwrap().is_some());

    let truncated = vec![0, 0, 0, 8, b'{', b'}'];
    assert!(matches!(
        run_framed_stream(fake_platform(), Cursor::new(truncated), Vec::new()),
        Err(RunError::Framing(_))
    ));
}

#[test]
fn framed_runner_does_not_wait_for_eof_after_shutdown() {
    let mut input = Vec::new();
    for payload in [
        request(1, "initialize", json!({"protocolVersion": 5})),
        request(2, "shutdown", json!({})),
    ] {
        write_frame(&mut input, &payload).unwrap();
    }
    let (release_tx, release_rx) = bounded(1);
    let started = std::time::Instant::now();
    let result = run_framed_stream(
        fake_platform(),
        BlockingAfterData {
            data: Cursor::new(input),
            release: release_rx,
        },
        Vec::new(),
    );
    release_tx.send(()).unwrap();

    assert!(result.is_ok());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn full_ordinary_queue_cannot_drop_a_reserved_paste_delivery() {
    let gate = Arc::new(CallbackGate::new());
    let state = Arc::new(FakeState::default());
    let (terminal_tx, _terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(256);
    let (critical_tx, critical_rx) = bounded::<CriticalDelivery>(1);
    let (delivered_tx, delivered_rx) = bounded(1);
    thread::spawn(move || {
        while let Ok(delivery) = critical_rx.recv() {
            let Some(batch) = delivery.accept() else {
                return;
            };
            if delivered_tx.send(batch).is_err() {
                return;
            }
        }
    });
    let mut server = Server::new(
        FakePlatform {
            state: Arc::clone(&state),
            outbound: None,
            gate: None,
        },
        outbound_tx.clone(),
        critical_tx,
        gate,
        terminal,
    );
    initialize(&mut server, &outbound_rx);
    for _ in 0..256 {
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    alt_shortcut_model(ActivationKey::A, false),
                ),
                phase: EventPhase::Down,
            }))
            .unwrap();
    }

    assert!(server.handle_payload(&request(257, "paste.inject", json!({}))));
    assert_eq!(outbound_rx.len(), 256);
    assert!(state.calls.lock().unwrap().contains(&"inject_paste"));
    let delivered = delivered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(delivered.len(), 2);
    let committed: Value =
        serde_json::from_slice(&encode_outbound(&delivered[0]).unwrap()).unwrap();
    let response: Value = serde_json::from_slice(&encode_outbound(&delivered[1]).unwrap()).unwrap();
    assert_eq!(committed["method"], "paste.committed");
    assert_eq!(committed["params"]["requestId"], 257);
    assert_eq!(response["id"], 257);
    assert_eq!(response["result"]["submitted"], true);
}

#[test]
fn unavailable_writer_acquisition_rejects_before_native_paste_dispatch() {
    let gate = Arc::new(CallbackGate::new());
    let state = Arc::new(FakeState::default());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(2);
    let (critical_tx, _critical_rx) = bounded(1);
    let mut server = Server::new(
        FakePlatform {
            state: Arc::clone(&state),
            outbound: None,
            gate: None,
        },
        outbound_tx,
        critical_tx,
        gate,
        terminal,
    );
    initialize(&mut server, &outbound_rx);

    assert!(!server.handle_payload(&request(2, "paste.inject", json!({}))));
    assert!(!state.calls.lock().unwrap().contains(&"inject_paste"));
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
}

#[test]
fn full_framed_coordinator_dispatches_every_registered_method_in_sequence() {
    let calls = [
        (1, "initialize", json!({"protocolVersion": 5})),
        (
            2,
            "activation.configure",
            json!({"enabled": true, "bindings": [alt_binding("general", "Z", false)]}),
        ),
        (3, "session.set_capture", json!({"active": true})),
        (4, "paste.inject", json!({})),
        (5, "front_app.get", json!({})),
        (6, "permissions.get", json!({})),
        (7, "ping", json!({})),
        (8, "shutdown", json!({})),
    ];
    assert_eq!(
        calls.each_ref().map(|(_, method, _)| *method),
        INBOUND_METHODS
    );
    let mut framed = Vec::new();
    for (id, method, params) in &calls {
        write_frame(&mut framed, &request(*id, method, params.clone())).unwrap();
    }

    let (mut server, receiver) = setup();
    let mut input = Cursor::new(framed);
    let mut handled = 0;
    while let Some(payload) = read_frame(&mut input).unwrap() {
        let keep_running = server.handle_payload(&payload);
        let method = calls[handled].1;
        if method == "paste.inject" {
            assert_eq!(receive(&receiver)["method"], "paste.committed");
        }
        let response = receive(&receiver);
        assert_eq!(response["id"], (handled + 1) as u64);
        handled += 1;
        if method == "shutdown" {
            assert!(!keep_running);
            break;
        }
        assert!(keep_running);
    }
    assert_eq!(handled, INBOUND_METHODS.len());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn every_allowed_method_dispatches_after_initialization() {
    let (mut server, receiver) = setup();
    initialize(&mut server, &receiver);

    for (id, method, params) in [
        (
            2,
            "activation.configure",
            json!({"enabled": true, "bindings": [alt_binding("general", "Z", false)]}),
        ),
        (3, "session.set_capture", json!({"active": true})),
        (4, "paste.inject", json!({})),
        (5, "front_app.get", json!({})),
        (6, "permissions.get", json!({})),
        (7, "ping", json!({})),
    ] {
        assert!(server.handle_payload(&request(id, method, params)));
        if method == "paste.inject" {
            let committed = receive(&receiver);
            assert_eq!(committed["method"], "paste.committed");
            assert_eq!(committed["params"]["requestId"], id);
        }
        let response = receive(&receiver);
        assert_eq!(response["id"], id);
        assert!(response.get("result").is_some(), "{method}: {response}");
    }

    assert!(!server.handle_payload(&request(8, "shutdown", json!({}))));
    assert_eq!(receive(&receiver)["id"], 8);
}

#[test]
fn unknown_methods_and_unknown_params_are_rejected() {
    let (mut server, receiver) = setup();
    initialize(&mut server, &receiver);

    for (id, method) in [
        (2, "keyboard.type"),
        (3, "Initialize"),
        (4, "ping "),
        (5, "activation.event"),
        (6, "session.key"),
        (7, "shutdown.now"),
    ] {
        assert!(server.handle_payload(&request(id, method, json!({}))));
        assert_error(&receiver, -32_601, json!(id));
    }

    assert!(server.handle_payload(&request(8, "ping", json!({"extra": true}))));
    assert_eq!(receive(&receiver)["error"]["code"], -32_602);

    assert!(server.handle_payload(&request(
        9,
        "activation.configure",
        json!({"enabled": true, "key": "Z", "extra": true}),
    )));
    assert_eq!(receive(&receiver)["error"]["code"], -32_602);

    assert!(server.handle_payload(&request(
        10,
        "activation.configure",
        json!({"enabled": true, "key": "Escape"}),
    )));
    assert_eq!(receive(&receiver)["error"]["code"], -32_602);
}

#[test]
fn malformed_json_and_batches_are_rejected_with_null_ids() {
    for (payload, code) in [
        (&br#"{"#[..], -32_700),
        (&br#"{"jsonrpc":"2.0"} trailing"#[..], -32_700),
        (&br#"[]"#[..], -32_600),
        (
            &br#"[{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}]"#[..],
            -32_600,
        ),
        (
            &br#"[{"jsonrpc":"2.0","id":1,"method":"ping","params":{}},{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}]"#[..],
            -32_600,
        ),
    ] {
        let (mut server, receiver) = setup();
        assert!(server.handle_payload(payload));
        assert_error(&receiver, code, Value::Null);
    }
}

#[test]
fn invalid_version_without_id_is_not_treated_as_a_notification() {
    let (mut server, receiver) = setup();
    assert!(server.handle_payload(
        br#"{"jsonrpc":"1.0","method":"initialize","params":{"protocolVersion":1}}"#
    ));
    assert_error(&receiver, -32_600, Value::Null);
}

#[test]
fn envelope_types_fields_and_duplicates_are_strictly_validated() {
    let mut cases = vec![
        br#"{"id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#.to_vec(),
        br#"{"jsonrpc":2,"id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":null,"id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":true,"id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":[],"id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":1,"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":null,"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":true,"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":[],"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":{},"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"","params":{}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":null}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":true}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":1}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":"object"}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":[]}"#.to_vec(),
        br#"{"jsonrpc":"2.0","jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"id":2,"method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","method":"initialize","params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1},"params":{"protocolVersion":1}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1},"extra":true}"#.to_vec(),
        br#"{"jsonrpc":"2.0","method":"ping","method":"ping","params":{}}"#.to_vec(),
        br#"{"jsonrpc":"2.0","method":"ping","params":{},"extra":true}"#.to_vec(),
        br#"{"jsonrpc":"2.0","method":"ping","params":null}"#.to_vec(),
        br#"{"jsonrpc":"2.0","method":"","params":{}}"#.to_vec(),
    ];
    cases.push(request(1, &"m".repeat(65), json!({})));

    for payload in cases {
        let (mut server, receiver) = setup();
        assert!(server.handle_payload(&payload));
        assert_error(&receiver, -32_600, Value::Null);
    }
}

#[test]
fn request_id_boundaries_and_types_are_enforced() {
    let (mut server, receiver) = setup();
    initialize(&mut server, &receiver);

    let valid_ids = [
        json!(0),
        json!(9_007_199_254_740_991_u64),
        json!("request"),
        json!("é".repeat(32)),
    ];
    for id in valid_ids {
        assert!(server.handle_payload(&request_with_id(id.clone(), "ping", json!({}))));
        let response = receive(&receiver);
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["ok"], true);
    }

    let invalid_ids = [
        "-1".to_owned(),
        "1.5".to_owned(),
        "1e0".to_owned(),
        "true".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        "{}".to_owned(),
        "9007199254740992".to_owned(),
        serde_json::to_string("").unwrap(),
        serde_json::to_string(&"a".repeat(65)).unwrap(),
        serde_json::to_string(&"é".repeat(33)).unwrap(),
    ];
    for id in invalid_ids {
        assert!(server.handle_payload(&raw_request(&id, "ping", "{}")));
        assert_error(&receiver, -32_600, Value::Null);
    }
}

#[test]
fn every_method_requires_object_params_and_rejects_unknown_fields() {
    for method in INBOUND_METHODS {
        let missing_params = format!(
            r#"{{"jsonrpc":"2.0","id":20,"method":{}}}"#,
            serde_json::to_string(method).unwrap()
        );
        let (mut server, receiver) = setup_for_method(method);
        assert!(server.handle_payload(missing_params.as_bytes()));
        assert_error(&receiver, -32_600, Value::Null);

        for params in ["null", "true", "1", r#""text""#, "[]"] {
            let (mut server, receiver) = setup_for_method(method);
            assert!(server.handle_payload(&raw_request("21", method, params)));
            assert_error(&receiver, -32_600, Value::Null);
        }

        let (mut server, receiver) = setup_for_method(method);
        assert!(server.handle_payload(&request(22, method, json!({"unknown": true}))));
        assert_error(&receiver, -32_602, json!(22));
    }
}

#[test]
fn typed_params_reject_missing_wrong_and_duplicate_fields() {
    let cases = [
        ("initialize", "{}"),
        ("initialize", r#"{"protocolVersion":true}"#),
        ("initialize", r#"{"protocolVersion":"1"}"#),
        ("initialize", r#"{"protocolVersion":-1}"#),
        ("initialize", r#"{"protocolVersion":1.0}"#),
        ("initialize", r#"{"protocolVersion":65536}"#),
        ("initialize", r#"{"protocolVersion":1,"protocolVersion":1}"#),
        ("activation.configure", "{}"),
        ("activation.configure", r#"{"key":"A"}"#),
        ("activation.configure", r#"{"enabled":true}"#),
        ("activation.configure", r#"{"enabled":1,"key":"A"}"#),
        ("activation.configure", r#"{"enabled":null,"key":"A"}"#),
        ("activation.configure", r#"{"enabled":true,"key":"a"}"#),
        ("activation.configure", r#"{"enabled":true,"key":"Escape"}"#),
        ("activation.configure", r#"{"enabled":true,"key":1}"#),
        (
            "activation.configure",
            r#"{"enabled":true,"enabled":false,"key":"A"}"#,
        ),
        (
            "activation.configure",
            r#"{"enabled":true,"key":"A","key":"B"}"#,
        ),
        ("session.set_capture", "{}"),
        ("session.set_capture", r#"{"active":1}"#),
        ("session.set_capture", r#"{"active":null}"#),
        ("session.set_capture", r#"{"active":true,"active":false}"#),
    ];

    for (method, params) in cases {
        let (mut server, receiver) = setup_for_method(method);
        assert!(server.handle_payload(&raw_request("30", method, params)));
        assert_error(&receiver, -32_602, json!(30));
    }
}

#[test]
fn secure_input_paste_failure_has_stable_wire_value() {
    assert_eq!(
        serde_json::to_value(PasteResult {
            submitted: false,
            reason: Some(PasteFailure::SecureInput),
        })
        .unwrap(),
        json!({"submitted": false, "reason": "secure_input"})
    );
}

#[test]
fn outbound_keyboard_notifications_have_fixed_methods_and_params() {
    let activation = Outbound::Event(HelperEvent::Activation {
        binding: ActivationBinding::new(
            ProfileId::PROMPT,
            alt_shortcut_model(ActivationKey::Z, true),
        ),
        phase: EventPhase::Down,
    });
    let activation: Value = serde_json::from_slice(&encode_outbound(&activation).unwrap()).unwrap();
    assert_eq!(
        activation,
        json!({
            "jsonrpc": "2.0",
            "method": "activation.event",
            "params": {
                "phase": "down",
                "profileId": "prompt",
                "shortcut": alt_shortcut("Z", true),
            },
        })
    );

    let complete = Outbound::Event(HelperEvent::ActivationComplete {
        binding: ActivationBinding::new(
            ProfileId::GENERAL,
            alt_shortcut_model(ActivationKey::X, false),
        ),
        held_ms: 599,
    });
    let complete: Value = serde_json::from_slice(&encode_outbound(&complete).unwrap()).unwrap();
    assert_eq!(
        complete,
        json!({
            "jsonrpc": "2.0",
            "method": "activation.event",
            "params": {
                "phase": "complete",
                "profileId": "general",
                "shortcut": alt_shortcut("X", false),
                "heldMs": 599,
            },
        })
    );

    let session = Outbound::Event(HelperEvent::SessionKey {
        key: SessionKey::Escape,
        phase: EventPhase::Up,
    });
    let session: Value = serde_json::from_slice(&encode_outbound(&session).unwrap()).unwrap();
    assert_eq!(
        session,
        json!({
            "jsonrpc": "2.0",
            "method": "session.key",
            "params": {"key": "escape", "phase": "up"},
        })
    );
}

#[test]
fn valid_notifications_are_never_executed_or_answered() {
    let (mut server, receiver, state, gate) = setup_observable();
    assert!(server.handle_payload(&notification("initialize", json!({"protocolVersion": 5}),)));
    assert!(receiver.try_recv().is_err());
    assert!(!gate.is_open());

    initialize(&mut server, &receiver);
    state.calls.lock().unwrap().clear();

    for (method, params) in [
        ("initialize", json!({"protocolVersion": 5})),
        (
            "activation.configure",
            json!({"enabled": true, "bindings": [alt_binding("general", "Z", false)]}),
        ),
        ("session.set_capture", json!({"active": true})),
        ("paste.inject", json!({})),
        ("front_app.get", json!({})),
        ("permissions.get", json!({})),
        ("ping", json!({})),
        ("shutdown", json!({})),
        ("unknown.notification", json!({})),
    ] {
        assert!(server.handle_payload(&notification(method, params)));
        assert!(receiver.try_recv().is_err(), "{method}");
    }

    assert!(state.calls.lock().unwrap().is_empty());
    assert_eq!(*state.activation.lock().unwrap(), ActivationKey::DEFAULT);
    assert!(!*state.activation_enabled.lock().unwrap());
    assert!(!*state.capture.lock().unwrap());
    assert!(gate.is_open());

    assert!(server.handle_payload(&request(2, "ping", json!({}))));
    assert_eq!(receive(&receiver)["result"]["ok"], true);
}

#[test]
fn initialization_response_disconnect_is_terminal_and_gate_stays_closed() {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(1);
    let (critical_tx, _critical_rx) = bounded(1);
    drop(outbound_rx);
    let mut server = Server::new(
        FakePlatform {
            state: Arc::new(FakeState::default()),
            outbound: Some(outbound_tx.clone()),
            gate: Some(Arc::clone(&gate)),
        },
        outbound_tx,
        critical_tx,
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );

    assert!(!server.handle_payload(&request(1, "initialize", json!({"protocolVersion": 5}),)));
    assert!(!gate.is_open());
    assert_eq!(
        terminal.reason(),
        Some(TerminalReason::OutboundQueueUnavailable)
    );
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
}

#[test]
fn invalid_params_error_disconnect_propagates_terminal_failure() {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(2);
    let (critical_tx, _critical_rx) = bounded(1);
    let mut server = Server::new(
        FakePlatform {
            state: Arc::new(FakeState::default()),
            outbound: Some(outbound_tx.clone()),
            gate: Some(Arc::clone(&gate)),
        },
        outbound_tx,
        critical_tx,
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );
    assert!(server.handle_payload(&request(1, "initialize", json!({"protocolVersion": 5}),)));
    let _ = outbound_rx.recv().unwrap();
    assert!(gate.is_open());
    drop(outbound_rx);

    assert!(!server.handle_payload(&request(
        2,
        "activation.configure",
        json!({"enabled": true, "key": "Escape"}),
    )));
    assert!(!gate.is_open());
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
}

#[test]
fn full_response_queue_is_terminal_instead_of_blocking_server() {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, _outbound_rx) = bounded(1);
    let (critical_tx, _critical_rx) = bounded(1);
    let mut server = Server::new(
        FakePlatform {
            state: Arc::new(FakeState::default()),
            outbound: Some(outbound_tx.clone()),
            gate: Some(Arc::clone(&gate)),
        },
        outbound_tx,
        critical_tx,
        Arc::clone(&gate),
        terminal,
    );
    assert!(server.handle_payload(&request(1, "initialize", json!({"protocolVersion": 5}),)));
    assert!(gate.is_open());

    assert!(!server.handle_payload(&request(2, "ping", json!({}))));
    assert!(!gate.is_open());
    assert_eq!(
        terminal_rx.try_recv(),
        Ok(TerminalReason::OutboundQueueUnavailable)
    );
}

#[test]
fn committed_malformed_protocol_corpus_is_rejected_with_bounded_responses() {
    const CORPUS: &str = include_str!("fixtures/malformed-protocol.jsonl");
    let mut cases = 0;
    for payload in CORPUS.lines().filter(|line| !line.is_empty()) {
        let (mut server, receiver) = setup();
        assert!(
            server.handle_payload(payload.as_bytes()),
            "corpus case {cases}"
        );
        let outbound = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("malformed corpus response");
        let encoded = encode_outbound(&outbound).expect("bounded corpus response");
        assert!(encoded.len() <= MAX_FRAME_BYTES, "corpus case {cases}");
        let response: Value = serde_json::from_slice(&encoded).expect("JSON response");
        assert!(
            response.get("error").is_some(),
            "corpus case {cases}: {response}"
        );
        cases += 1;
    }
    assert_eq!(cases, 23);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1_024,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5eed_1300),
        ..ProptestConfig::default()
    })]

    #[test]
    fn parser_never_panics_for_arbitrary_bounded_payloads(
        payload in proptest::collection::vec(any::<u8>(), 0..=(MAX_FRAME_BYTES * 2)),
    ) {
        let _ = parse_request(&payload);
    }

    #[test]
    fn initialized_dispatcher_handles_arbitrary_object_params_with_bounded_output(
        method_index in 0usize..INBOUND_METHODS.len(),
        entries in proptest::collection::vec(("[a-z]{0,8}", any::<i64>()), 0..12),
    ) {
        let (mut server, receiver) = setup();
        initialize(&mut server, &receiver);
        let params: serde_json::Map<String, Value> = entries
            .into_iter()
            .map(|(key, value)| (key, json!(value)))
            .collect();
        let payload = request(2, INBOUND_METHODS[method_index], Value::Object(params));
        let _keep_running = server.handle_payload(&payload);
        let first = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("dispatcher response");
        prop_assert!(encode_outbound(&first).expect("bounded response").len() <= MAX_FRAME_BYTES);
        while let Ok(outbound) = receiver.try_recv() {
            prop_assert!(encode_outbound(&outbound).expect("bounded output").len() <= MAX_FRAME_BYTES);
        }
    }
}

#[test]
fn protocol_v5_configures_ordered_chords_and_preserves_every_wire_field() {
    let (mut server, receiver, state, _gate) = setup_observable();
    initialize(&mut server, &receiver);
    let values = json!({
        "enabled": true,
        "bindings": [
            binding_value("general", shortcut_value(&["X"], false, true, false, false)),
            binding_value("prompt", shortcut_value(&["X", "P"], false, true, false, false)),
            binding_value("prompt-to-english", shortcut_value(&["X", "Q"], false, true, false, false)),
            binding_value("markdown", shortcut_value(&["X", "M"], false, true, false, false)),
            binding_value("translate-to-english", shortcut_value(&["X", "T"], false, true, false, false)),
            binding_value(&profile_id(2), shortcut_value(&["Q"], true, true, false, false))
        ]
    });
    assert!(server.handle_payload(&request(90, "activation.configure", values.clone())));
    assert_eq!(receive(&receiver)["result"], values);

    let configured = *state.activation_bindings.lock().unwrap();
    assert_eq!(
        serde_json::to_value(configured).unwrap(),
        values["bindings"]
    );
    assert_eq!(
        configured.iter().next().unwrap().shortcut().keys(),
        &[ActivationKey::X]
    );
    assert_eq!(
        configured.iter().nth(1).unwrap().shortcut().trigger(),
        ActivationKey::P
    );
}

#[test]
fn protocol_v5_enforces_binding_count_enablement_key_and_conflict_rules() {
    let (mut server, receiver) = setup();
    initialize(&mut server, &receiver);

    let thirteen: Vec<_> = (0..13)
        .map(|index| {
            let key = char::from(b'A' + index as u8).to_string();
            alt_binding(&profile_id(index), &key, false)
        })
        .collect();
    assert!(server.handle_payload(&request(
        90,
        "activation.configure",
        json!({"enabled": true, "bindings": thirteen}),
    )));
    assert!(receive(&receiver).get("result").is_some());

    assert!(server.handle_payload(&request(
        91,
        "activation.configure",
        json!({"enabled": false, "bindings": []}),
    )));
    assert_eq!(
        receive(&receiver)["result"],
        json!({"enabled": false, "bindings": []})
    );

    let fourteen: Vec<_> = (0..14)
        .map(|index| {
            let key = char::from(b'A' + index as u8).to_string();
            alt_binding(&profile_id(index), &key, false)
        })
        .collect();
    let mut twenty_seven: Vec<_> = ('A'..='Z').map(|key| key.to_string()).collect();
    twenty_seven.push("A".to_owned());
    let alt_x = shortcut_value(&["X"], false, true, false, false);
    let alt_x_p = shortcut_value(&["X", "P"], false, true, false, false);
    let alt_x_q = shortcut_value(&["X", "Q"], false, true, false, false);
    let invalid_profile = "00000000-0000-0000-8000-000000000001";
    let invalid = [
        json!({"enabled": true, "bindings": []}),
        json!({"enabled": true, "bindings": fourteen}),
        json!({"enabled": true, "bindings": [binding_value("general", shortcut_value(&[], false, true, false, false))]}),
        json!({"enabled": true, "bindings": [binding_value("general", shortcut_value(&["A"], false, false, false, false))]}),
        json!({"enabled": true, "bindings": [binding_value("general", shortcut_value(&["A", "A"], false, true, false, false))]}),
        json!({"enabled": true, "bindings": [{"profileId": "general", "shortcut": {"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": ["a"]}}]}),
        json!({"enabled": true, "bindings": [{"profileId": "general", "shortcut": {"modifiers": {"alt": true, "shift": false, "meta": false}, "keys": ["A"]}}]}),
        json!({"enabled": true, "bindings": [{"profileId": "general", "shortcut": {"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false, "capsLock": false}, "keys": ["A"]}}]}),
        json!({"enabled": true, "bindings": [{"profileId": "general", "shortcut": {"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": ["A"], "trigger": "A"}}]}),
        json!({"enabled": true, "bindings": [binding_value("general", shortcut_value(&twenty_seven.iter().map(String::as_str).collect::<Vec<_>>(), false, true, false, false))]}),
        json!({"enabled": true, "bindings": [binding_value("general", alt_x.clone()), binding_value("prompt", alt_x.clone())]}),
        json!({"enabled": true, "bindings": [binding_value(&profile_id(2), alt_x.clone()), binding_value(&profile_id(3), alt_x_p.clone())]}),
        json!({"enabled": true, "bindings": [binding_value("general", alt_x_p), binding_value("prompt", alt_x.clone())]}),
        json!({"enabled": true, "bindings": [binding_value("general", alt_x_q)]}),
        json!({"enabled": true, "bindings": [binding_value("general", alt_x.clone()), binding_value("general", shortcut_value(&["Q"], false, true, false, false))]}),
        json!({"enabled": true, "bindings": [binding_value(invalid_profile, alt_x.clone())]}),
        json!({"enabled": true, "bindings": [{"profileId": "general", "shortcut": alt_x.clone(), "extra": true}]}),
        json!({"enabled": true, "bindings": [alt_x]}),
    ];

    for (offset, params) in invalid.into_iter().enumerate() {
        let id = 100 + offset as u64;
        assert!(server.handle_payload(&request(id, "activation.configure", params)));
        assert_error(&receiver, -32_602, json!(id));
    }
}

#[test]
fn protocol_v5_allows_prefixes_when_modifier_masks_differ_and_rejects_legacy_shape() {
    let (mut server, receiver) = setup();
    initialize(&mut server, &receiver);
    let values = json!({
        "enabled": true,
        "bindings": [
            binding_value("general", shortcut_value(&["X"], false, true, false, false)),
            binding_value("prompt", shortcut_value(&["X", "P"], true, false, true, false))
        ]
    });
    assert!(server.handle_payload(&request(200, "activation.configure", values.clone())));
    assert_eq!(receive(&receiver)["result"], values);

    let shift_only = json!({
        "enabled": true,
        "bindings": [binding_value("general", shortcut_value(&["P"], false, false, true, false))]
    });
    assert!(server.handle_payload(&request(201, "activation.configure", shift_only.clone(),)));
    assert_eq!(receive(&receiver)["result"], shift_only);

    assert!(server.handle_payload(&request(
        202,
        "activation.configure",
        json!({"enabled": true, "bindings": [{"key": "Z", "shift": false}]}),
    )));
    assert_error(&receiver, -32_602, json!(202));
}
