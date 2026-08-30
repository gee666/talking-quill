#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
use talking_quill_keyboard_core::SessionCaptureMode;
use talking_quill_keyboard_owner::ActivationCaptureGate;
#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
use talking_quill_keyboard_owner::OWNER_MODE_MARKER;

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
#[test]
fn feature_free_owner_is_positively_safe_disabled() {
    let gate = ActivationCaptureGate::default();
    assert!(!gate.is_open());
    assert!(gate.development_disabled());
    assert!(!gate.runtime_rollback_active());
    assert!(!gate.filter_enabled(true));
    assert_eq!(
        gate.filter_session_mode(SessionCaptureMode::Recording),
        SessionCaptureMode::Off
    );
    assert_eq!(
        OWNER_MODE_MARKER,
        "TALKING_QUILL_KEYBOARD_OWNER=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS"
    );
}

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
#[test]
fn feature_free_build_info_reports_an_inert_safe_disabled_artifact() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .arg("--build-info")
        .env_remove("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["profile"], "SAFE_DISABLED_OWNER");
    assert_eq!(value["captureEnabled"], false);
    assert_eq!(value["runtimeRollbackActive"], false);
    assert_eq!(value["testSeams"], false);
    assert_eq!(
        value["modeMarker"],
        "TALKING_QUILL_KEYBOARD_OWNER=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS"
    );
}

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
#[test]
fn feature_free_entrypoint_refuses_to_start_owner_runtime() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_talking-quill-keyboard-owner"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "keyboard-owner runtime unavailable in feature-free inert build\n"
    );
}

#[cfg(any(
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
))]
#[test]
fn owner_test_features_are_explicitly_non_promotable() {
    assert_eq!(
        ActivationCaptureGate::for_process().is_open(),
        !runtime_rollback_requested()
    );
}

#[cfg(feature = "local-unsigned-owner")]
#[test]
fn local_unsigned_owner_is_enabled_without_a_test_seam() {
    use talking_quill_keyboard_owner::{OWNER_BUILD_MODE, OWNER_MODE_MARKER};

    assert!(OWNER_BUILD_MODE.is_local_unsigned_owner());
    assert_eq!(
        ActivationCaptureGate::for_process().is_open(),
        !runtime_rollback_requested()
    );
    assert!(!ActivationCaptureGate::default().is_open());
    assert_eq!(
        OWNER_MODE_MARKER,
        "TALKING_QUILL_KEYBOARD_OWNER=DEFAULT_ENABLED_OUT_OF_PROCESS_LOCAL_UNSIGNED"
    );
}

#[cfg(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
))]
fn runtime_rollback_requested() -> bool {
    std::env::var_os("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE").is_some_and(|value| value == "1")
}
