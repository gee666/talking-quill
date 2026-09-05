use super::connection::{OWNER_INSTANCE, material};
use super::gateway::{read_v10, wait_gateway_owner_ready, write_v10};
use super::process::{
    connect_retry, free_address, release_authoritative_neutral, spawn_gateway_role, spawn_role,
};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use talking_quill_owner_protocol::StreamOrderedTransport;
use talking_quill_owner_protocol::client::{ClientError, OwnerProtocolClient};
use talking_quill_owner_protocol::schema::Purpose;

pub(super) fn fake_authentication_cannot_cross_production_transport_brand() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client_stream = TcpStream::connect(address).unwrap();
    let (_peer_stream, _) = listener.accept().unwrap();
    client_stream.set_nonblocking(true).unwrap();
    let (fake_gateway_codec, _) = material(Purpose::Capture, OWNER_INSTANCE[0])
        .codecs()
        .unwrap();

    // `StreamOrderedTransport` retains the production/default brand because it
    // does not override `OrderedTransport::is_test_only`. Fake session material
    // is rejected before any frame can cross that boundary.
    let result = OwnerProtocolClient::new(
        StreamOrderedTransport::new(client_stream).unwrap(),
        fake_gateway_codec,
    );
    assert!(matches!(
        result,
        Err(ClientError::TransportAuthenticationBoundary)
    ));
}

pub(super) fn gateway_deadline_cross_process_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let owner = spawn_role("deadline-fake-owner", owner_address);
    let gateway = spawn_gateway_role(
        "gateway-owner-runtime-fake-auth",
        gateway_address,
        owner_address,
    );
    let mut electron = connect_retry(gateway_address);
    electron
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();

    write_v10(
        &mut electron,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": 10}),
    );
    let initialized = read_v10(&mut electron, 1);
    let (_, mut id) = wait_gateway_owner_ready(&mut electron, initialized, 2);
    let configuration = serde_json::json!({
        "enabled": true,
        "bindings": [{"profileId":"general","shortcut":{
            "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
            "keys":["X"]
        }}]
    });
    write_v10(
        &mut electron,
        id,
        "activation.configure",
        configuration.clone(),
    );
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(&mut electron, id, "activation.configure", configuration);
    let expired = read_v10(&mut electron, id);
    assert!(expired["error"].is_object(), "{expired}");
    id += 1;
    owner.wait_success();

    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let unavailable = read_v10(&mut electron, id);
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["authenticated"],
        false
    );
    id += 1;
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let _ = read_v10(&mut electron, id);
    drop(electron);
    gateway.wait_success();
}

pub(super) fn gateway_owner_runtime_composition_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let owner = spawn_role("owner-runtime-fake-auth", owner_address);
    let gateway = spawn_gateway_role(
        "gateway-owner-runtime-fake-auth",
        gateway_address,
        owner_address,
    );
    let mut electron = connect_retry(gateway_address);
    electron
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    write_v10(
        &mut electron,
        1,
        "initialize",
        serde_json::json!({"protocolVersion": 10}),
    );
    let initialized = read_v10(&mut electron, 1);
    let (initialized, mut id) = wait_gateway_owner_ready(&mut electron, initialized, 2);
    assert_eq!(initialized["result"]["protocolVersion"], 10);
    assert_eq!(initialized["result"]["hookStatus"], "installed_unobserved");
    assert_eq!(
        initialized["result"]["keyboardOwner"]["authenticated"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );

    write_v10(
        &mut electron,
        id,
        "activation.configure",
        serde_json::json!({
            "enabled": true,
            "bindings": [{"profileId":"general","shortcut":{
                "modifiers":{"ctrl":false,"alt":true,"shift":false,"meta":false},
                "keys":["X"]
            }}]
        }),
    );
    let configured = read_v10(&mut electron, id);
    assert!(configured.get("result").is_some(), "{configured}");
    id += 1;

    // This following request serializes behind owner configuration and the
    // native-adapter active-candidate observation.
    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let enabled = read_v10(&mut electron, id);
    assert_eq!(
        enabled["result"]["keyboardOwner"]["state"],
        "leased_enabled"
    );
    id += 1;
    release_authoritative_neutral(owner_address);
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let shutdown = read_v10(&mut electron, id);
    assert_eq!(shutdown["result"]["ownerDisposition"], "neutral");

    // Planned quit closes the stable endpoint after the correlated release and
    // exits only after authoritative neutrality.
    drop(electron);
    gateway.wait_success();
    owner.wait_success();
}
