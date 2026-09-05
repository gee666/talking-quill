use super::connection::EXPECTED_FAKE_OWNER_CRASH_EXIT;
use super::gateway::{read_v10, wait_gateway_owner_ready, write_v10};
use super::process::{connect_retry, free_address, spawn_gateway_role, spawn_role};
use std::time::{Duration, Instant};

pub(super) fn gateway_fake_owner_forwarding_suite() {
    let owner_address = free_address();
    let gateway_address = free_address();
    let crashing_fake_owner = spawn_role("crashing-fake-owner", owner_address);
    let gateway = spawn_gateway_role("gateway-fake-auth", gateway_address, owner_address);
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
    let first_owner_instance = initialized["result"]["keyboardOwner"]["instanceId"].clone();
    let first_lease_epoch = initialized["result"]["keyboardOwner"]["leaseEpoch"].clone();
    assert_eq!(initialized["result"]["protocolVersion"], 10);
    assert_eq!(initialized["result"]["hookStatus"], "installed_unobserved");
    assert_eq!(
        initialized["result"]["keyboardOwner"]["model"],
        "out_of_process"
    );
    assert_eq!(initialized["result"]["keyboardOwner"]["protocolVersion"], 1);
    assert_eq!(
        initialized["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );
    assert_eq!(
        initialized["result"]["keyboardOwner"]["authenticated"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardCapture"]["activationAvailable"],
        true
    );
    assert_eq!(
        initialized["result"]["keyboardCapture"]["sessionKeyCaptureAvailable"],
        true
    );
    // Establish an accepted session mode, then force an uncertain configuration
    // mutation. The uncertain configuration must not become reconnect intent.
    write_v10(
        &mut electron,
        id,
        "session.set_capture",
        serde_json::json!({"mode":"recording"}),
    );
    let _ = read_v10(&mut electron, id);
    id += 1;
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
    let crashed_configuration = read_v10(&mut electron, id);
    id += 1;
    assert!(matches!(
        crashed_configuration["error"]["code"].as_i64(),
        Some(-32_003) | Some(-32_011)
    ));
    crashing_fake_owner.wait_exit_code(EXPECTED_FAKE_OWNER_CRASH_EXIT);

    // The owner process is now gone and no replacement endpoint exists. A
    // fresh mutation must fail rather than being reported as applied.
    let unavailable_started = Instant::now();
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
    let unavailable_configuration = read_v10(&mut electron, id);
    assert!(
        unavailable_started.elapsed() < Duration::from_secs(2),
        "post-crash mutation exceeded the gateway RPC timeout"
    );
    assert!(matches!(
        unavailable_configuration["error"]["code"].as_i64(),
        Some(-32_003) | Some(-32_011)
    ));
    id += 1;
    write_v10(&mut electron, id, "ping", serde_json::json!({}));
    let unavailable = read_v10(&mut electron, id);
    id += 1;
    assert_eq!(unavailable["result"]["hookStatus"], "unavailable");
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["state"],
        "unavailable"
    );
    assert_eq!(
        unavailable["result"]["keyboardOwner"]["authenticated"],
        false
    );

    let replacement_fake_owner = spawn_role("replacement-fake-owner", owner_address);
    let (replacement_ready, next_id) = wait_gateway_owner_ready(&mut electron, unavailable, id);
    id = next_id;
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["state"],
        "leased_disabled"
    );
    assert_ne!(
        replacement_ready["result"]["keyboardOwner"]["instanceId"],
        first_owner_instance
    );
    assert_ne!(
        replacement_ready["result"]["keyboardOwner"]["leaseEpoch"],
        first_lease_epoch
    );
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["leaseEpoch"],
        8
    );
    assert_eq!(
        replacement_ready["result"]["keyboardOwner"]["buildId"],
        "common-e2e-build"
    );
    // The replacement starts disabled with no replay of the uncertain
    // configuration. Fresh accepted operations establish the new desired
    // configuration and session mode on that replacement connection.
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
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(
        &mut electron,
        id,
        "session.set_capture",
        serde_json::json!({"mode":"recording"}),
    );
    assert!(read_v10(&mut electron, id)["result"].is_object());
    id += 1;
    write_v10(&mut electron, id, "shutdown", serde_json::json!({}));
    let shutdown = read_v10(&mut electron, id);
    assert_eq!(shutdown["result"]["ownerDisposition"], "neutral");
    drop(electron);
    gateway.wait_success();
    replacement_fake_owner.wait_success();
}
