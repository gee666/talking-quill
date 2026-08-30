#![cfg(target_os = "macos")]

use std::process::Command;

fn helper(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_talking-quill-helper"))
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn maintenance_cli_rejects_nonexact_arguments_without_starting_electron_protocol() {
    let cases = [
        vec!["--owner-maintenance".into()],
        vec!["--owner-maintenance".into(), "update".into()],
        vec![
            "--owner-maintenance".into(),
            "UNINSTALL".into(),
            "00".into(),
            "00".into(),
        ],
        vec![
            "--owner-maintenance".into(),
            "uninstall".into(),
            "A".repeat(64),
            "0".repeat(64),
        ],
    ];
    for arguments in cases {
        let output = helper(&arguments);
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn maintenance_cli_rejects_unicode_non_ascii_arguments() {
    let output = helper(&[
        "--owner-maintenance".into(),
        "uninstall".into(),
        "é".repeat(32),
        "0".repeat(64),
    ]);
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn maintenance_cli_rejects_non_unicode_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let output = Command::new(env!("CARGO_BIN_EXE_talking-quill-helper"))
        .arg("--owner-maintenance")
        .arg("uninstall")
        .arg(std::ffi::OsString::from_vec(vec![0xff; 64]))
        .arg("0".repeat(64))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
}

#[test]
fn valid_maintenance_cli_stops_at_platform_authority_bootstrap() {
    let output = helper(&[
        "--owner-maintenance".into(),
        "uninstall".into(),
        "0".repeat(64),
        "0".repeat(64),
    ]);
    assert_eq!(output.status.code(), Some(69));
    assert!(output.stdout.is_empty());
}
