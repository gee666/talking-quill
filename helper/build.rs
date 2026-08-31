use std::{env, fs, path::PathBuf, process::Command};

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
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256");
    let update_key_path = PathBuf::from("../build/windows-update-public-key.sec1");
    println!("cargo:rerun-if-changed={}", update_key_path.display());
    let update_key = fs::read_to_string(&update_key_path)
        .expect("build/windows-update-public-key.sec1 is required for updater-compatible builds");
    let update_key = update_key.trim();
    assert!(
        update_key.len() == 130
            && update_key.starts_with("04")
            && update_key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "build/windows-update-public-key.sec1 must contain one lowercase uncompressed P-256 SEC1 key"
    );
    println!("cargo:rustc-env=TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1={update_key}");
    let publication_key_path = PathBuf::from("../build/release-manifest-public-key.sec1");
    println!("cargo:rerun-if-changed={}", publication_key_path.display());
    let publication_key = fs::read_to_string(&publication_key_path)
        .expect("build/release-manifest-public-key.sec1 is required for updater-compatible builds");
    let publication_key = publication_key.trim();
    assert!(
        publication_key.len() == 130
            && publication_key.starts_with("04")
            && publication_key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && publication_key != update_key,
        "the release-manifest key must be one lowercase uncompressed P-256 key distinct from the updater key"
    );
    println!("cargo:rustc-env=TALKING_QUILL_RELEASE_MANIFEST_PUBLIC_KEY_SEC1={publication_key}");
    println!("cargo:rerun-if-env-changed=TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE");
    println!(
        "cargo:rustc-env=TALKING_QUILL_MACOS_BUILD_VARIANT={}",
        env::var("TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE").unwrap_or_else(|_| "source".into())
    );
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Security");
    }
}
