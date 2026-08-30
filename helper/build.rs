use std::{env, fs, path::PathBuf};

fn main() {
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
