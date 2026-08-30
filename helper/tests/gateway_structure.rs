use std::{
    io::{BufReader, BufWriter, Write},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use talking_quill_helper::framing::{read_frame, write_frame};

const GATEWAY_MARKER: &[u8] = b"TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS";
const FORBIDDEN_MARKERS: [&[u8]; 4] = [
    b"TALKING_QUILL_KEYBOARD_OWNER=",
    b"TALKING_QUILL_NATIVE_KEYBOARD_SUPPRESSION=",
    b"TALKING_QUILL_I1_TRANSACTIONAL_CAPTURE=",
    b"TALKING_QUILL_KEYBOARD_OWNER_TEST_SEAMS=",
];

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn request(writer: &mut impl Write, id: u64, method: &str, params: Value) {
    write_frame(
        writer,
        &serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .unwrap(),
    )
    .unwrap();
    writer.flush().unwrap();
}

fn response(reader: &mut impl std::io::Read) -> Value {
    serde_json::from_slice(&read_frame(reader).unwrap().expect("gateway response frame")).unwrap()
}

#[test]
fn workspace_defaults_build_gateway_and_inert_feature_free_owner() {
    let workspace =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert!(workspace.contains("default-members = [\".\", \"keyboard-owner\"]"));
    let owner = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/keyboard-owner/Cargo.toml"
    ))
    .unwrap();
    assert!(owner.contains("default = []"));
    assert!(!owner.contains("required-features = [\"local-unsigned-owner\"]"));
    assert!(workspace.contains("role = \"gateway\""));
}

#[cfg(windows)]
#[test]
fn retired_authority_arguments_cannot_select_an_alternate_runtime() {
    let executable = env!("CARGO_BIN_EXE_talking-quill-helper");
    for argument in [
        "--authority-role-gateway",
        "--test-windows-relay-collision-boundary",
        "--readiness-correlation=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let output = Command::new(executable)
            .arg(argument)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("unknown talking-quill-helper arguments")
        );
    }
}

#[test]
fn gateway_without_an_adjacent_eligible_owner_stays_unready_and_never_suppresses() {
    let executable = env!("CARGO_BIN_EXE_talking-quill-helper");
    let bytes = std::fs::read(executable).unwrap();
    assert!(contains(&bytes, GATEWAY_MARKER));
    for forbidden in FORBIDDEN_MARKERS {
        assert!(!contains(&bytes, forbidden));
    }

    let mut child = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut writer = BufWriter::new(child.stdin.take().unwrap());
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    request(&mut writer, 1, "initialize", json!({"protocolVersion": 10}));
    let initialized = response(&mut reader);
    assert_eq!(initialized["result"]["hookStatus"], "unavailable");
    assert_eq!(
        initialized["result"]["permissions"]["accessibility"],
        "unknown"
    );
    assert_eq!(
        initialized["result"]["keyboardCapture"],
        json!({
            "activationAvailable": false,
            "sessionKeyCaptureAvailable": false,
            "runtimeRollbackActive": false,
            "buildDisabled": false,
        })
    );

    request(
        &mut writer,
        2,
        "activation.configure",
        json!({
            "enabled": true,
            "bindings": [{
                "profileId": "general",
                "shortcut": {
                    "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                    "keys": ["X"]
                }
            }]
        }),
    );
    assert!(matches!(
        response(&mut reader)["error"]["code"].as_i64(),
        Some(-32003 | -32005 | -32007)
    ));

    request(
        &mut writer,
        3,
        "session.set_capture",
        json!({"mode": "recording"}),
    );
    assert!(matches!(
        response(&mut reader)["error"]["code"].as_i64(),
        Some(-32003 | -32005 | -32007)
    ));

    request(&mut writer, 4, "front_app.get", json!({}));
    assert_eq!(response(&mut reader)["error"]["code"], -32003);

    request(&mut writer, 5, "shutdown", json!({}));
    assert_eq!(
        response(&mut reader)["result"]["ownerDisposition"],
        "draining",
        "a never-acquired owner is conservatively reported as draining"
    );
    drop(writer);
    assert!(child.wait().unwrap().success());
}
