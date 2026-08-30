#[cfg(target_os = "macos")]
fn main() {
    use std::fs;
    use std::io::{BufRead, Write};
    use std::path::PathBuf;
    use std::process::Command;

    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    const SERVICE: &str = "com.talkingquill.app.keyboard-owner";
    const ACCOUNT: &str = "owner-ipc-v1";
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "serve-authenticated" {
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        let key = lines
            .next()
            .and_then(Result::ok)
            .and_then(|line| decode_key(&line))
            .unwrap_or_else(|| std::process::exit(70));
        let pid = std::process::id();
        let audit_session = talking_quill_keyboard_owner::macos::current_audit_token()
            .map(|token| token.audit_session_id())
            .unwrap_or_else(|_| std::process::exit(70));
        let mut sequence = 0_u64;
        for command in lines.map_while(Result::ok) {
            if command != "handshake-present" && command != "fixed-absent" {
                std::process::exit(70);
            }
            sequence = sequence
                .checked_add(1)
                .unwrap_or_else(|| std::process::exit(70));
            let statuses =
                talking_quill_keyboard_owner::macos::fixed_item_query_statuses_without_ui()
                    .unwrap_or_else(|_| std::process::exit(70));
            let valid = if command == "handshake-present" {
                statuses[0] == security_framework_sys::base::errSecSuccess
            } else {
                statuses == [security_framework_sys::base::errSecItemNotFound; 2]
            };
            if !valid {
                std::process::exit(77);
            }
            let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(&key)
                .unwrap_or_else(|_| std::process::exit(70));
            mac.update(b"talking-quill/macos-keychain-fixture/v1\0");
            mac.update(&pid.to_be_bytes());
            mac.update(&audit_session.to_be_bytes());
            mac.update(&sequence.to_be_bytes());
            mac.update(command.as_bytes());
            mac.update(&statuses[0].to_be_bytes());
            mac.update(&statuses[1].to_be_bytes());
            println!(
                "{pid} {audit_session} {sequence} {command} {} {} {}",
                statuses[0],
                statuses[1],
                hex(&mac.finalize().into_bytes())
            );
            std::io::stdout()
                .flush()
                .unwrap_or_else(|_| std::process::exit(70));
        }
        return;
    }
    let root = PathBuf::from("tmp/native-keychain-fixture");
    let search = root.join("search-list.txt");
    let run = |arguments: &[&str]| {
        Command::new("/usr/bin/security")
            .args(arguments)
            .status()
            .is_ok_and(|status| status.success())
    };
    let cleanup = || {
        let _ = run(&["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT]);
        if let Ok(value) = fs::read_to_string(&search) {
            let keychains: Vec<_> = value.lines().filter(|line| !line.is_empty()).collect();
            if !keychains.is_empty() {
                let mut arguments = vec!["list-keychains", "-d", "user", "-s"];
                arguments.extend(keychains);
                let _ = run(&arguments);
            }
        }
        for name in ["duplicate-a.keychain-db", "duplicate-b.keychain-db"] {
            let path = root.join(name);
            let _ = Command::new("/usr/bin/security")
                .arg("delete-keychain")
                .arg(path)
                .status();
        }
        let _ = fs::remove_dir_all(&root);
    };
    fn decode_key(value: &str) -> Option<[u8; 32]> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut key = [0_u8; 32];
        for (target, pair) in key.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
            *target = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(key)
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    if mode == "cleanup" || mode == "missing" {
        cleanup();
        return;
    }
    cleanup();
    fs::create_dir_all(&root).expect("create fixture state");
    match mode.as_str() {
        "one-valid" => assert!(run(&[
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-X",
            &"07".repeat(32)
        ])),
        "one-all-zero" => assert!(run(&[
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-X",
            &"00".repeat(32)
        ])),
        "malformed" => assert!(run(&[
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-X",
            &"07".repeat(31)
        ])),
        "ui-required" => assert!(run(&[
            "add-generic-password",
            "-s",
            SERVICE,
            "-a",
            ACCOUNT,
            "-X",
            &"07".repeat(32),
            "-T",
            "/usr/bin/false"
        ])),
        "duplicate" => {
            let output = Command::new("/usr/bin/security")
                .args(["list-keychains", "-d", "user"])
                .output()
                .expect("read search list");
            let original: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|line| line.trim().trim_matches('"').to_owned())
                .filter(|line| !line.is_empty())
                .collect();
            fs::write(&search, original.join("\n") + "\n").expect("save search list");
            let a = root.join("duplicate-a.keychain-db");
            let b = root.join("duplicate-b.keychain-db");
            for path in [&a, &b] {
                assert!(
                    Command::new("/usr/bin/security")
                        .args(["create-keychain", "-p", "fixture-password"])
                        .arg(path)
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(
                    Command::new("/usr/bin/security")
                        .args(["unlock-keychain", "-p", "fixture-password"])
                        .arg(path)
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(
                    Command::new("/usr/bin/security")
                        .args([
                            "add-generic-password",
                            "-s",
                            SERVICE,
                            "-a",
                            ACCOUNT,
                            "-X",
                            &"07".repeat(32)
                        ])
                        .arg(path)
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let mut command = Command::new("/usr/bin/security");
            command.args(["list-keychains", "-d", "user", "-s"]);
            for path in original {
                command.arg(path);
            }
            command.arg(a).arg(b);
            assert!(command.status().unwrap().success());
        }
        _ => std::process::exit(64),
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    std::process::exit(69);
}
