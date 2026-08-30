#![cfg(feature = "local-unsigned-owner")]

use serde_json::Value;
use std::process::Command;

#[test]
fn executable_reports_an_enabled_unsigned_profile_without_test_seams() {
    let output = Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .arg("--build-info")
        .env_remove("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
        .output()
        .expect("run keyboard owner build-info");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).expect("strict JSON build info");
    assert_eq!(body["captureEnabled"], true);
    assert_eq!(body["runtimeRollbackActive"], false);
    assert_eq!(body["testSeams"], false);
    assert_eq!(
        body["modeMarker"],
        "TALKING_QUILL_KEYBOARD_OWNER=DEFAULT_ENABLED_OUT_OF_PROCESS_LOCAL_UNSIGNED"
    );
    let profile = body["profile"].as_str().expect("profile string");
    assert!(
        matches!(
            profile,
            "LOCAL_UNSIGNED_OWNER_WIN_X64"
                | "LOCAL_UNSIGNED_OWNER_WIN_ARM64"
                | "LOCAL_OWNER_MAC_X64"
                | "LOCAL_OWNER_MAC_ARM64"
        ),
        "unexpected profile: {profile}"
    );
}

#[test]
fn runtime_rollback_closes_the_local_gate_without_changing_artifact_identity() {
    let output = Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .arg("--build-info")
        .env("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE", "1")
        .output()
        .expect("run rolled-back keyboard owner build-info");

    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).expect("strict JSON build info");
    assert_eq!(body["captureEnabled"], false);
    assert_eq!(body["runtimeRollbackActive"], true);
    assert_eq!(body["testSeams"], false);
}

#[test]
fn explicit_local_runtime_starts_native_owner_and_neutral_drains_before_exit() {
    let output = Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .arg("--runtime-smoke")
        .env_remove("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
        .output()
        .expect("run keyboard owner runtime smoke");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn runtime_smoke_honors_process_rollback_and_still_neutral_drains() {
    let output = Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .arg("--runtime-smoke")
        .env("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE", "1")
        .output()
        .expect("run rolled-back owner runtime smoke");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
