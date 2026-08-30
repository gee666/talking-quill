use std::{env, process::Command};

fn main() {
    for name in ["TALKING_QUILL_SOURCE_COMMIT", "TALKING_QUILL_SOURCE_TREE"] {
        println!("cargo:rerun-if-env-changed={name}");
        let revision = if name.ends_with("COMMIT") {
            "HEAD^{commit}"
        } else {
            "HEAD^{tree}"
        };
        let value = env::var(name).unwrap_or_else(|_| {
            String::from_utf8(
                Command::new("git")
                    .args(["rev-parse", revision])
                    .output()
                    .expect("git source identity is required")
                    .stdout,
            )
            .expect("git output must be UTF-8")
            .trim()
            .to_owned()
        });
        assert!(
            value.len() == 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "source identity must be a lowercase full Git object ID"
        );
        println!("cargo:rustc-env={name}={value}");
    }
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_LOCAL_UNSIGNED_OWNER");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_TRANSACTIONAL_SHORTCUTS_DEV");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_WINDOWS_NATIVE_TEST_INPUT");
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256");
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE");
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE");
    println!(
        "cargo:rustc-env=TALKING_QUILL_MACOS_BUILD_VARIANT={}",
        env::var("TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE").unwrap_or_else(|_| "source".into())
    );
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_KEYCHAIN_ACCESS_GROUP");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }

    let local_owner = env::var_os("CARGO_FEATURE_LOCAL_UNSIGNED_OWNER").is_some();
    let native_test_seam = env::var_os("CARGO_FEATURE_TRANSACTIONAL_SHORTCUTS_DEV").is_some()
        || env::var_os("CARGO_FEATURE_WINDOWS_NATIVE_TEST_INPUT").is_some();
    let macos_lifecycle_fixture =
        env::var_os("CARGO_FEATURE_MACOS_NATIVE_LIFECYCLE_FIXTURE").is_some();
    if macos_lifecycle_fixture {
        assert_eq!(
            env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("macos"),
            "the lifecycle fixture is macOS-only"
        );
        assert_eq!(
            env::var("TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE").as_deref(),
            Ok("permissioned-ci-v1"),
            "the lifecycle fixture requires its explicit permissioned CI gate"
        );
        assert!(
            env::var("TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE")
                .is_ok_and(|value| value.starts_with("drag-to-trash-fixture-")),
            "the lifecycle fixture requires a distinct non-production build nonce"
        );
    }
    assert!(
        !(local_owner && native_test_seam),
        "the local unsigned owner mode cannot contain native test seams"
    );
    let optimized = env::var("OPT_LEVEL").is_ok_and(|level| level != "0");
    println!("cargo:rustc-check-cfg=cfg(talking_quill_unoptimized_test_support)");
    if !optimized && !local_owner {
        println!("cargo:rustc-cfg=talking_quill_unoptimized_test_support");
    }
    assert!(
        !(native_test_seam && optimized),
        "native keyboard test seams cannot be included in any optimized owner profile"
    );
    // The macOS lifecycle fixture may be optimized to exercise the real owner
    // lifecycle, but only behind the three independent gates above.
    assert!(!macos_lifecycle_fixture || local_owner);
}
