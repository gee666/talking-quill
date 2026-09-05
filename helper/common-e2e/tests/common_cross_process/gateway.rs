use super::connection::{OWNER_INSTANCE, TcpConnector};
use super::process::address_from_env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use talking_quill_helper::gateway::ActivationCaptureGate;
use talking_quill_helper::owner::platform_client::OwnerGatewayBackend;

pub(super) fn run_gateway_with_fake_auth_process(rotating_fake_owner_identity: bool) {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let input = stream.try_clone().unwrap();
    let output = stream;
    talking_quill_helper::run_framed_stream_with_factory_and_gate(
        input,
        output,
        ActivationCaptureGate::open_for_test_harness(),
        |outbound, _, _, capture_gate| {
            let owner_address = std::env::var("TALKING_QUILL_COMMON_E2E_OWNER_ADDRESS")
                .unwrap()
                .parse()
                .unwrap();
            let connector = if rotating_fake_owner_identity {
                TcpConnector::rotating_fake_owners(owner_address)
            } else {
                TcpConnector::fixed(owner_address, OWNER_INSTANCE[0])
            };
            OwnerGatewayBackend::connect_with(Box::new(connector), outbound, capture_gate)
        },
    )
    .unwrap();
}

pub(super) fn write_v10(stream: &mut TcpStream, id: u64, method: &str, params: serde_json::Value) {
    let body = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": method, "params": params
    }))
    .unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

pub(super) fn read_v10(stream: &mut TcpStream, expected_id: u64) -> serde_json::Value {
    loop {
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length).unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if value.get("id").and_then(serde_json::Value::as_u64) == Some(expected_id) {
            return value;
        }
    }
}

pub(super) fn wait_gateway_owner_ready(
    stream: &mut TcpStream,
    mut response: serde_json::Value,
    mut next_id: u64,
) -> (serde_json::Value, u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if response["result"]["keyboardOwner"]["authenticated"] == true
            && response["result"]["hookStatus"] == "installed_unobserved"
        {
            return (response, next_id);
        }
        assert!(
            Instant::now() < deadline,
            "gateway owner did not become protocol-v10 ready: {response}"
        );
        write_v10(stream, next_id, "ping", serde_json::json!({}));
        response = read_v10(stream, next_id);
        next_id += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
}
