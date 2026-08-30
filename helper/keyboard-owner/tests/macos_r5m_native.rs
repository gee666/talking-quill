#![cfg(target_os = "macos")]

use std::ffi::{CString, c_char};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use talking_quill_keyboard_owner::macos::{
    GATEWAY_SIGNING_IDENTIFIER, IdentityError, KeychainStore, LocalSigningIdentity,
    MacosEndpointConfig, NativeKeychainStore, NativePeerEvidence, OWNER_SIGNING_IDENTIFIER,
    PeerEvidenceProvider, RequirementHash, audit_token_connection_binding, current_audit_token,
    requirement_digest,
};
use talking_quill_owner_protocol::auth::{
    AuthenticationKeys, HandshakeTrustVerifier, KeyAgreementMaterial, Transcript, TranscriptInput,
};
use talking_quill_owner_protocol::framing::{encode_outer_frame, read_outer_frame};
use talking_quill_owner_protocol::release_policy::ReleasePolicy;
use talking_quill_owner_protocol::schema::{
    Authenticate, Empty, HandshakeMessage, Hello, Platform, ProtocolHeader, Purpose,
    parse_handshake_json,
};
use talking_quill_owner_protocol::{
    Bytes32, FeatureBits, GatewayMessage, GatewaySessionCodec, Request,
};

#[test]
fn retained_vnode_execution_child_marker() {
    if let Some(marker) = std::env::var_os("TALKING_QUILL_RETAINED_VNODE_MARKER") {
        fs::write(marker, b"TQ_RETAINED_VNODE_ORIGINAL").expect("write retained marker");
    }
}

#[test]
fn dev_fd_spawn_executes_retained_signed_vnode_after_path_replacement() {
    let suffix: String = Bytes32::random().expect("test suffix").as_bytes()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp")
        .join(format!("r5m-retained-vnode-{suffix}"));
    fs::create_dir_all(&root).expect("create retained-vnode fixture");
    let path = root.join("owner-fixture");
    fs::copy(std::env::current_exe().unwrap(), &path).expect("copy test executable");
    let sign = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&path)
        .status()
        .expect("codesign retained executable");
    assert!(sign.success(), "ad-hoc fixture signing failed");
    assert!(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&path)
            .status()
            .expect("verify signed retained executable")
            .success()
    );
    let retained = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .expect("open retained executable vnode");
    let replacement = root.join("replacement");
    fs::copy("/usr/bin/false", &replacement).expect("copy replacement executable");
    fs::rename(&replacement, &path).expect("replace executable pathname");
    let marker = root.join("marker");
    let executable = CString::new(format!("/dev/fd/{}", retained.as_raw_fd())).unwrap();
    let exact = CString::new("--exact").unwrap();
    let child_test = CString::new("retained_vnode_execution_child_marker").unwrap();
    let marker_env = CString::new(format!(
        "TALKING_QUILL_RETAINED_VNODE_MARKER={}",
        marker.display()
    ))
    .unwrap();
    let lang = CString::new("LANG=C").unwrap();
    let mut argv = [
        executable.as_ptr().cast_mut(),
        exact.as_ptr().cast_mut(),
        child_test.as_ptr().cast_mut(),
        std::ptr::null_mut::<c_char>(),
    ];
    let mut environment = [
        marker_env.as_ptr().cast_mut(),
        lang.as_ptr().cast_mut(),
        std::ptr::null_mut::<c_char>(),
    ];
    let mut attributes: libc::posix_spawnattr_t = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::posix_spawnattr_init(&raw mut attributes) },
        0
    );
    assert_eq!(
        unsafe {
            libc::posix_spawnattr_setflags(
                &raw mut attributes,
                libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short,
            )
        },
        0
    );
    let mut pid = 0;
    let spawn = unsafe {
        libc::posix_spawn(
            &raw mut pid,
            executable.as_ptr(),
            std::ptr::null(),
            &attributes,
            argv.as_mut_ptr(),
            environment.as_mut_ptr(),
        )
    };
    unsafe { libc::posix_spawnattr_destroy(&raw mut attributes) };
    assert_eq!(spawn, 0, "spawn retained executable vnode");
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    assert_eq!(
        fs::read(&marker).expect("original-vnode marker"),
        b"TQ_RETAINED_VNODE_ORIGINAL"
    );
    drop(retained);
    fs::remove_dir_all(root).expect("remove retained-vnode fixture");
}

fn executable_digest(path: &std::path::Path) -> Bytes32 {
    let mut file = File::open(path).expect("open native test executable");
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).expect("hash native test executable");
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Bytes32::new(hash.finalize().into())
}

fn codesign_output_for(path: &std::path::Path) -> String {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dvvv"])
        .arg(path)
        .output()
        .expect("inspect native test signing information");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn codesign_output() -> String {
    codesign_output_for(&std::env::current_exe().expect("current executable"))
}

fn signing_identifier_for(path: &std::path::Path) -> String {
    codesign_output_for(path)
        .lines()
        .find_map(|line| line.strip_prefix("Identifier=").map(str::to_owned))
        .expect("codesign identifier")
}

fn signing_identifier_and_cdhash() -> (String, RequirementHash) {
    let output = codesign_output();
    let identifier = output
        .lines()
        .find_map(|line| line.strip_prefix("Identifier=").map(str::to_owned))
        .expect("codesign identifier");
    let encoded = output
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .expect("codesign CDHash");
    assert_eq!(encoded.len(), 40);
    let mut bytes = [0_u8; 20];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] =
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("CDHash hex");
    }
    (identifier, RequirementHash::new(bytes))
}

fn current_designated_requirement() -> String {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--requirements", "-"])
        .arg(std::env::current_exe().expect("current executable"))
        .output()
        .expect("inspect native test requirement");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| {
            line.split_once("designated =>")
                .map(|(_, value)| value.trim().to_owned())
        })
        .filter(|value| !value.is_empty())
        .expect("native designated requirement")
}

#[test]
fn accepted_socket_binds_kernel_peer_to_strict_dynamic_and_static_seccode() {
    let (owner, peer) = UnixStream::pair().expect("connected local socket");
    let executable = std::env::current_exe()
        .expect("test executable")
        .canonicalize()
        .expect("canonical executable");
    let digest = executable_digest(&executable);
    let (identifier, code_directory_hash) = signing_identifier_and_cdhash();
    let evidence = NativePeerEvidence::acquire(
        &owner,
        identifier.clone(),
        executable.clone(),
        digest,
        Bytes32::new([7; 32]),
        LocalSigningIdentity::AdHoc {
            code_directory_hash,
        },
    )
    .expect("kernel peer evidence");
    assert_eq!(
        evidence.peer_credentials().expect("getpeereid").uid,
        unsafe { libc::geteuid() }
    );
    assert_eq!(
        evidence.peer_pid().expect("LOCAL_PEERPID"),
        std::process::id()
    );
    let token = evidence.peer_audit_token().expect("LOCAL_PEERTOKEN");
    assert_eq!(
        evidence.connection_binding().expect("client binding"),
        audit_token_connection_binding(token, unsafe { libc::geteuid() })
    );
    assert_eq!(token.pid(), std::process::id());
    assert_eq!(
        token.audit_session_id(),
        current_audit_token()
            .expect("current token")
            .audit_session_id()
    );
    let identity = evidence
        .sec_code_identity(token)
        .expect("strict Security.framework code identity");
    assert!(identity.statically_valid);
    assert_eq!(identity.signing_identifier, identifier);
    assert_eq!(identity.executable_sha256, digest);
    assert_eq!(
        identity.signing_identity,
        LocalSigningIdentity::AdHoc {
            code_directory_hash,
        }
    );
    let requirement = current_designated_requirement();
    assert!(
        evidence
            .evaluate_designated_requirement(token, &requirement)
            .expect("Security.framework requirement evaluation")
    );
    assert!(matches!(
        evidence.evaluate_designated_requirement(token, "identifier \"definitely.wrong\""),
        Ok(false) | Err(IdentityError::InvalidCode)
    ));
    drop(peer);
}

#[test]
fn native_keychain_query_is_bounded_and_never_requests_authentication_ui() {
    let started = Instant::now();
    let result = NativeKeychainStore.read_owner_handshake_secret_without_ui();
    assert!(started.elapsed() < Duration::from_secs(2));
    if let Ok(secret) = result {
        assert_ne!(secret.expose_to_authenticated_handshake(), &[0; 32]);
    }
}

#[test]
#[ignore = "requires disposable native Keychain fixture"]
fn native_keychain_zero_one_duplicate_malformed_and_ui_fail_semantics() {
    use talking_quill_keyboard_owner::macos::KeychainError;

    let fixture = std::env::var_os("TALKING_QUILL_R5M_NATIVE_KEYCHAIN_FIXTURE")
        .map(std::path::PathBuf::from)
        .expect("Keychain fixture executable");
    let run = |mode: &str| {
        assert!(Command::new(&fixture).arg(mode).status().unwrap().success());
        NativeKeychainStore.read_owner_handshake_secret_without_ui()
    };
    assert!(matches!(run("missing"), Err(KeychainError::Missing)));
    assert!(run("one-valid").is_ok());
    assert!(matches!(
        run("one-all-zero"),
        Err(KeychainError::InvalidItem)
    ));
    assert!(matches!(run("duplicate"), Err(KeychainError::InvalidItem)));
    assert!(matches!(run("malformed"), Err(KeychainError::InvalidItem)));
    let started = Instant::now();
    assert!(matches!(
        run("ui-required"),
        Err(KeychainError::Unavailable)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    let _ = Command::new(fixture).arg("cleanup").status();
}

struct FixtureCleanup {
    owner: Option<Child>,
    cleanup: std::path::PathBuf,
}

impl Drop for FixtureCleanup {
    fn drop(&mut self) {
        if let Some(owner) = &mut self.owner {
            let _ = owner.kill();
            let _ = owner.wait();
        }
        let _ = Command::new(&self.cleanup).arg("cleanup").status();
    }
}

struct FixtureTrust;

impl HandshakeTrustVerifier for FixtureTrust {
    fn verify(
        &self,
        _hello: &Hello,
        _challenge: &talking_quill_owner_protocol::schema::Challenge,
        _client_policy: &ReleasePolicy,
        _owner_policy: &ReleasePolicy,
    ) -> Result<(), talking_quill_owner_protocol::AuthenticationError> {
        Ok(())
    }
}

fn fixture_protocol_header() -> ProtocolHeader {
    ProtocolHeader {
        major: talking_quill_owner_protocol::PROTOCOL_MAJOR,
        minor: 0,
        compatibility_epoch: talking_quill_owner_protocol::COMPATIBILITY_EPOCH,
        supported_feature_bits: FeatureBits::new(talking_quill_owner_protocol::BASE_V1),
        required_feature_bits: FeatureBits::new(talking_quill_owner_protocol::BASE_V1),
    }
}

fn fixture_session_digest(uid: u32, audit_session_id: u32) -> Bytes32 {
    let mut digest = Sha256::new();
    digest.update(b"talking-quill/macos-audit-session/v1\0");
    digest.update(uid.to_be_bytes());
    digest.update(audit_session_id.to_be_bytes());
    Bytes32::new(digest.finalize().into())
}

fn run_repository_gateway_fixture(socket: &std::path::Path) {
    let config = MacosEndpointConfig::load_for_current_gateway_install()
        .expect("gateway fixture config and exact gateway code identity");
    let token = current_audit_token().expect("gateway audit token");
    let uid = unsafe { libc::geteuid() };
    let binding = audit_token_connection_binding(token, uid);
    let hello = Hello::new(
        Purpose::Observe,
        fixture_protocol_header(),
        Bytes32::random().expect("gateway nonce"),
        Platform::Macos,
        config.architecture,
        config.peer_policy.release_build_digest,
        config.peer_policy.gateway.executable_sha256,
        config.installation_identity_digest,
        requirement_digest(&config.peer_policy.gateway.designated_requirement),
        fixture_session_digest(uid, token.audit_session_id()),
        config.gateway_release_policy.clone(),
        config.gateway_release_policy_signature.clone(),
        binding,
        None,
    )
    .expect("gateway hello");
    let mut stream = UnixStream::connect(socket).expect("connect production owner");
    stream
        .write_all(&encode_outer_frame(&hello.to_json().unwrap()).unwrap())
        .unwrap();
    let challenge_body = read_outer_frame(&mut stream).unwrap().expect("challenge");
    let HandshakeMessage::Challenge(challenge) = parse_handshake_json(&challenge_body).unwrap()
    else {
        panic!("owner challenge");
    };
    assert_eq!(challenge.platform_credential_binding_digest, binding);
    assert_eq!(
        challenge.executable_sha256,
        config.peer_policy.owner.executable_sha256
    );
    assert_eq!(
        challenge.release_build_digest,
        config.peer_policy.release_build_digest
    );
    assert_eq!(
        challenge.installation_identity_digest,
        config.installation_identity_digest
    );
    assert_eq!(challenge.owner_release_policy, config.owner_release_policy);
    let transcript = Transcript::build(
        &TranscriptInput::from_verified_handshake(&hello, &challenge, &FixtureTrust).unwrap(),
    )
    .unwrap();
    let mut secret = NativeKeychainStore
        .read_owner_handshake_secret_without_ui()
        .expect("fixture Keychain secret")
        .into_authenticated_handshake_secret();
    let material = KeyAgreementMaterial::macos(&mut secret);
    let keys = AuthenticationKeys::derive(&transcript, &material).unwrap();
    let proof = keys.proofs(&transcript).client_proof;
    let authenticate = Authenticate::new(proof).to_json().unwrap();
    stream
        .write_all(&encode_outer_frame(&authenticate).unwrap())
        .unwrap();
    let finish = read_outer_frame(&mut stream)
        .unwrap()
        .expect("authenticated finish");
    let session = keys
        .pending_gateway_session(&transcript)
        .unwrap()
        .accept_authenticated_finish(&finish, &proof)
        .unwrap();
    let mut codec = GatewaySessionCodec::new(session, keys.gateway_frame_key()).unwrap();
    let request = codec
        .encode_request(&Request::HealthGet(Empty {}))
        .unwrap()
        .into_frame();
    stream.write_all(&request).unwrap();
    let response = read_outer_frame(&mut stream)
        .unwrap()
        .expect("health response");
    assert!(matches!(
        codec
            .receive_owner_frame(&encode_outer_frame(&response).unwrap())
            .unwrap(),
        GatewayMessage::Response { .. }
    ));
    let nonce = std::env::var("TALKING_QUILL_R5M_FIXTURE_NONCE").expect("fixture nonce");
    println!("TQ_R5M_NATIVE_V1 {nonce} authenticated owner_attached request_response_ok");
}

#[test]
#[ignore = "launched only as the repository-controlled signed gateway fixture"]
fn repository_signed_gateway_fixture_child() {
    let socket = std::env::var_os("TALKING_QUILL_R5M_FIXTURE_SOCKET")
        .map(std::path::PathBuf::from)
        .expect("fixture socket");
    run_repository_gateway_fixture(&socket);
}

/// Explicit native fixture contract:
/// - owner is the real signed production owner bundle executable;
/// - gateway is a separately signed gateway fixture which computes
///   platformCredentialBindingDigest from its own audit token (not input);
/// - setup provisions either the compiled exact-access-group item or, when no
///   group is compiled, a disposable per-item trusted-application/designated-
///   requirement ACL; duplicate/malformed/UI-denial cases remain required.
///
/// Native ACL acceptance/denial certification is an R8-M fixture obligation,
/// not something the query can infer.
#[test]
#[ignore = "requires the exact packaged outer application fixture"]
fn exact_packaged_artifact_identity_and_bridge_policy_validate() {
    let app = std::env::var_os("TALKING_QUILL_R5M_NATIVE_OUTER_APP")
        .map(std::path::PathBuf::from)
        .expect("exact outer application fixture");
    let helper = app.join("Contents/Resources/helper/talking-quill-helper");
    let owner = app.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner");
    let bridge = app.join("Contents/MacOS/talking-quill-macos-service-bridge");
    for role in [&helper, &owner, &bridge] {
        assert!(role.is_file());
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict"])
                .arg(role)
                .status()
                .unwrap()
                .success()
        );
    }
    assert!(
        Command::new(&helper)
            .arg("--macos-owner-validate-install")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new(&bridge)
            .arg("status")
            .status()
            .unwrap()
            .success()
    );
}

#[test]
#[ignore = "requires signed/provisioned R5-M native fixture environment"]
fn signed_gateway_subprocess_reaches_real_published_owner_endpoint() {
    let owner = std::env::var_os("TALKING_QUILL_R5M_NATIVE_OWNER_FIXTURE")
        .map(std::path::PathBuf::from)
        .expect("signed owner fixture path");
    let gateway = std::env::var_os("TALKING_QUILL_R5M_NATIVE_GATEWAY_FIXTURE")
        .map(std::path::PathBuf::from)
        .expect("signed gateway fixture path");
    let keychain = std::env::var_os("TALKING_QUILL_R5M_NATIVE_KEYCHAIN_FIXTURE")
        .map(std::path::PathBuf::from)
        .expect("Keychain fixture setup/cleanup executable");
    let owner = owner.canonicalize().expect("canonical owner fixture");
    let gateway = gateway.canonicalize().expect("canonical gateway fixture");
    assert_eq!(signing_identifier_for(&owner), OWNER_SIGNING_IDENTIFIER);
    assert_eq!(signing_identifier_for(&gateway), GATEWAY_SIGNING_IDENTIFIER);
    assert!(
        Command::new(&keychain)
            .arg("setup")
            .status()
            .unwrap()
            .success()
    );
    let child = Command::new(&owner)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("launch real owner");
    let mut cleanup = FixtureCleanup {
        owner: Some(child),
        cleanup: keychain,
    };
    let socket =
        talking_quill_keyboard_owner::macos::MacosRuntimeDirectory::for_current_audit_session()
            .expect("audit-session runtime")
            .socket_path();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "owner did not publish endpoint");
        std::thread::sleep(Duration::from_millis(10));
    }
    let nonce: String = Bytes32::random()
        .expect("fixture request nonce")
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let output = Command::new(gateway)
        .args([
            "--ignored",
            "--exact",
            "repository_signed_gateway_fixture_child",
            "--nocapture",
        ])
        .env("TALKING_QUILL_R5M_FIXTURE_SOCKET", &socket)
        .env("TALKING_QUILL_R5M_FIXTURE_NONCE", &nonce)
        .output()
        .expect("launch repository gateway fixture");
    assert!(output.status.success(), "gateway fixture process failed");
    let marker =
        format!("TQ_R5M_NATIVE_V1 {nonce} authenticated owner_attached request_response_ok");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&marker),
        "missing nonce-bound authenticated request/response marker"
    );
    if let Some(owner) = &mut cleanup.owner {
        let _ = owner.kill();
        let _ = owner.wait();
    }
    cleanup.owner = None;
}
