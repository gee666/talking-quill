use talking_quill_keyboard_core::{ActivationBindings, transactional::TransactionEngine};

#[test]
fn owner_imports_shared_core_directly_without_compatibility_facade() {
    let _ = ActivationBindings::default();
    let _ = TransactionEngine::default();

    let owner_lib = include_str!("../src/lib.rs");
    assert!(!owner_lib.contains("pub mod keyboard"));
    assert!(!owner_lib.contains("talking_quill_keyboard_core::*"));

    let owner_sources = [
        include_str!("../src/platform/mod.rs"),
        include_str!("../src/platform/observability.rs"),
        include_str!("../src/platform/unsupported.rs"),
    ]
    .join("\n");
    assert!(!owner_sources.contains("crate::keyboard"));
    assert!(owner_sources.contains("talking_quill_keyboard_core"));
}

#[test]
fn owner_executable_defaults_safe_and_keeps_local_wiring_explicit() {
    let owner_manifest = include_str!("../Cargo.toml");
    let owner_lib = include_str!("../src/lib.rs");
    let adapter = include_str!("../src/adapter.rs");
    let platform_adapter = include_str!("../src/platform_adapter.rs");
    let runtime = include_str!("../src/runtime.rs");
    let main = include_str!("../src/main.rs");
    let platform = [
        include_str!("../src/platform/mod.rs"),
        include_str!("../src/platform/windows.rs"),
        include_str!("../src/platform/macos.rs"),
        include_str!("../src/platform/unsupported.rs"),
    ]
    .join("\n");

    assert!(!owner_manifest.contains("required-features = [\"local-unsigned-owner\"]"));
    assert!(owner_manifest.contains("default = []"));
    assert!(main.contains("#[cfg(not(feature = \"local-unsigned-owner\"))]"));
    assert!(owner_manifest.contains("executable = true"));
    assert!(owner_manifest.contains("owner-protocol-server = true"));
    assert!(owner_lib.contains("pub mod build_mode"));
    assert!(owner_lib.contains("feature = \"local-unsigned-owner\""));
    assert!(owner_lib.contains("pub mod platform"));
    assert!(!owner_lib.contains("#[cfg(not(debug_assertions))]\nmod platform"));
    assert!(adapter.contains("pub trait NativeAdapter"));
    assert!(adapter.contains("pub enum BrokerEvent"));
    assert!(owner_lib.contains("pub mod platform_adapter"));
    assert!(platform_adapter.contains("impl<P: Platform> NativeAdapter for PlatformAdapter<P>"));
    assert!(platform_adapter.contains("impl PlatformAdapter<NativePlatform>"));
    assert!(platform_adapter.contains("ActivationCaptureGate::for_process()"));
    for blocked in [
        "NamedPipe",
        "UnixListener",
        "SMAppService",
        "CreateNamedPipe",
    ] {
        assert!(!adapter.contains(blocked), "blocked C1 endpoint: {blocked}");
    }
    assert!(!platform.contains("impl NativeAdapter for NativePlatform"));
    assert!(!platform_adapter.contains("NamedPipe"));
    assert!(!platform_adapter.contains("UnixListener"));
    assert!(runtime.contains("pub trait AuthenticatedConnectionSource"));
    assert!(runtime.contains("pub trait SingletonCoordinator"));
    assert!(runtime.contains("pub trait RuntimeSignalSource"));
    assert!(runtime.contains("OwnerProtocolServer::start"));
    assert!(runtime.contains("NativeAdapterExecutor::new"));
    assert!(runtime.contains("PlatformAdapter::start"));
    assert!(main.contains("ProductionRuntime::production"));
    for forbidden in ["CreateNamedPipe", "UnixListener", "Keychain"] {
        assert!(
            !runtime.contains(forbidden),
            "R4 implemented R5 endpoint: {forbidden}"
        );
    }
}

#[test]
fn macos_runtime_security_anchors_are_not_sidecar_controlled() {
    let config = include_str!("../src/macos/config.rs");
    let endpoint = include_str!("../src/macos/endpoint.rs");
    let singleton = include_str!("../src/macos/singleton.rs");
    let keychain = include_str!("../src/macos/native_keychain.rs");
    let runtime = include_str!("../src/runtime.rs");
    assert!(config.contains("option_env!(\"TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256\")"));
    assert!(config.contains("Contents"));
    assert!(config.contains("keyboard-owner-r5m.json"));
    assert!(!config.contains("release_policy_signer_certificate_sha256: String"));
    assert!(endpoint.contains("unlink_socket_if_matches"));
    assert!(endpoint.contains("verify_listener_address"));
    assert!(!endpoint.contains("verify_listener_inode"));
    assert!(endpoint.contains("libc::posix_spawn"));
    assert!(endpoint.contains("POSIX_SPAWN_CLOEXEC_DEFAULT"));
    assert!(endpoint.contains("DARWIN_SOCK_CLOEXEC"));
    assert!(!endpoint.contains("libc::pipe("));
    assert!(endpoint.contains("libc::F_DUPFD_CLOEXEC"));
    assert!(endpoint.contains("BROKER_SAFE_SOURCE_FD_MIN"));
    assert!(endpoint.contains("socket_kernel_peer_identity"));
    assert!(endpoint.contains("broker_pid != pid as u32"));
    assert!(endpoint.contains("validated_owner_executable"));
    assert!(endpoint.contains("/dev/fd/"));
    let parent_broker_auth = endpoint
        .split("fn run_native_auth_broker(")
        .nth(1)
        .and_then(|body| body.split("fn encode_broker_request").next())
        .expect("parent broker authentication body");
    assert!(!parent_broker_auth.contains("PeerEvidence::new"));
    assert!(!parent_broker_auth.contains("SecCode"));
    assert!(endpoint.contains("BrokerWaitState::NoChild"));
    assert!(endpoint.contains("libc::kill(pid, libc::SIGKILL)"));
    assert!(endpoint.contains("BROKER_REAPER"));
    assert!(endpoint.contains("Zeroizing"));
    assert!(keychain.contains("kSecUseAuthenticationUIFail"));
    assert!(!keychain.contains("kSecUseAuthenticationUISkip"));
    assert!(keychain.contains("kSecAttrAccessGroup"));
    assert!(keychain.contains("kSecMatchLimitAll"));
    assert!(runtime.contains(".shutdown_endpoint()"));
    assert!(runtime.contains("self.singleton.release();"));
    assert!(runtime.contains("source Drop/quiescence before releasing singleton locks"));
    let release = singleton
        .split("fn release(&mut self)")
        .nth(1)
        .expect("macOS release implementation");
    assert!(
        release
            .find("self.maintenance_fd.take()")
            .expect("maintenance unlock")
            < release.find("self.owner_fd.take()").expect("owner unlock")
    );
}

#[test]
fn shared_core_manifest_has_no_reverse_workspace_dependency() {
    let core_manifest = include_str!("../../keyboard-core/Cargo.toml");
    for forbidden in [
        "talking-quill-keyboard-owner",
        "talking-quill-helper",
        "talking-quill-owner-protocol",
    ] {
        assert!(
            !core_manifest.contains(forbidden),
            "reverse dependency: {forbidden}"
        );
    }
}
