use sha2::{Digest, Sha256};

const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_REQUIREMENT_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OuterIdentity {
    pub(crate) leaf_certificate_sha256: String,
    pub(crate) identifier: String,
    pub(crate) team_identifier: Option<String>,
    pub(crate) designated_requirement: String,
    pub(crate) designated_requirement_sha256: String,
}

pub(crate) fn matches_trusted(observed: &OuterIdentity, trusted: &OuterIdentity) -> bool {
    observed == trusted
}

pub(crate) fn parse_codesign_identity(
    output: &str,
    leaf_certificate_sha256: &str,
) -> Result<OuterIdentity, ()> {
    if output.as_bytes().contains(&0) || output.len() > MAX_OUTPUT_BYTES {
        return Err(());
    }
    let identifier = one(output, "Identifier=")?;
    let team = one(output, "TeamIdentifier=")?;
    let requirement = one(output, "designated => ")?;
    let signature = one_signature(output)?;
    if signature == "adhoc"
        || !valid_identifier(identifier)
        || requirement.is_empty()
        || requirement.len() > MAX_REQUIREMENT_BYTES
        || !valid_sha256(leaf_certificate_sha256)
    {
        return Err(());
    }
    let team_identifier = if team == "not set" {
        None
    } else if valid_team_identifier(team) {
        Some(team.to_owned())
    } else {
        return Err(());
    };
    Ok(OuterIdentity {
        leaf_certificate_sha256: leaf_certificate_sha256.to_owned(),
        identifier: identifier.to_owned(),
        team_identifier,
        designated_requirement: requirement.to_owned(),
        designated_requirement_sha256: hex(&Sha256::digest(requirement.as_bytes())),
    })
}

fn one<'a>(output: &'a str, prefix: &str) -> Result<&'a str, ()> {
    let mut values = output
        .lines()
        .filter_map(|line| line.strip_suffix('\r').unwrap_or(line).strip_prefix(prefix));
    let value = values.next().filter(|value| !value.is_empty()).ok_or(())?;
    values.next().is_none().then_some(value).ok_or(())
}

fn one_signature(output: &str) -> Result<&str, ()> {
    let mut values = output.lines().filter_map(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        line.strip_prefix("Signature=")
            .or_else(|| line.strip_prefix("Signature size="))
    });
    let value = values.next().filter(|value| !value.is_empty()).ok_or(())?;
    values.next().is_none().then_some(value).ok_or(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_team_identifier(value: &str) -> bool {
    value.len() == 10
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(target_os = "macos")]
pub(crate) fn inspect(
    path: &std::path::Path,
    scratch: &std::path::Path,
) -> Result<OuterIdentity, ()> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut child = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-", "--verbose=4"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    let stdout = child.stdout.take().ok_or(())?;
    let stderr = child.stderr.take().ok_or(())?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|_| ())
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|_| ())
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| ())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(());
    }
    let mut bytes = stdout_reader.join().map_err(|_| ())??;
    bytes.extend(stderr_reader.join().map_err(|_| ())??);
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(());
    }
    let output = std::str::from_utf8(&bytes).map_err(|_| ())?;
    if output
        .lines()
        .any(|line| line.trim_end_matches('\r') == "Signature=adhoc")
    {
        return Err(());
    }
    let certificate_directory = scratch.join(format!(
        ".outer-certificate-{}-{}",
        std::process::id(),
        CERTIFICATE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&certificate_directory).map_err(|_| ())?;
    let prefix = certificate_directory.join("leaf");
    let mut extraction = Command::new("/usr/bin/codesign");
    extraction
        .args(["-d", "--extract-certificates"])
        .arg(&prefix)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let extracted = run_status_bounded(&mut extraction, Duration::from_secs(5));
    let leaf = std::fs::read(prefix.with_file_name("leaf0"));
    let _ = std::fs::remove_dir_all(&certificate_directory);
    extracted?;
    let leaf = leaf.map_err(|_| ())?;
    let leaf_sha256 = hex(&Sha256::digest(leaf));
    parse_codesign_identity(output, &leaf_sha256)
}

#[cfg(target_os = "macos")]
static CERTIFICATE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(target_os = "macos")]
fn run_status_bounded(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Result<(), ()> {
    let mut child = command.spawn().map_err(|_| ())?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|_| ())? {
            return status.success().then_some(()).ok_or(());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(requirement: &str, team: &str, signature: &str) -> String {
        format!(
            "Executable=/Applications/Talking Quill.app/Contents/MacOS/Talking Quill\nIdentifier=com.talkingquill.app\nSignature={signature}\nTeamIdentifier={team}\ndesignated => {requirement}\n"
        )
    }

    #[test]
    fn certificate_identity_retains_exact_designated_requirement() {
        let requirement = "identifier \"com.talkingquill.app\" and anchor trusted and certificate leaf = H\"0123\"";
        let identity =
            parse_codesign_identity(&signed(requirement, "not set", "4096"), &"11".repeat(32))
                .unwrap();
        assert_eq!(identity.identifier, "com.talkingquill.app");
        assert_eq!(identity.team_identifier, None);
        assert_eq!(identity.designated_requirement, requirement);
        assert_eq!(identity.designated_requirement_sha256.len(), 64);
    }

    #[test]
    fn same_signer_identity_matches_for_update_and_rollback() {
        let output = signed(
            "identifier \"com.talkingquill.app\" and anchor apple generic and certificate leaf[subject.OU] = TEAMID1234",
            "TEAMID1234",
            "4789",
        );
        let predecessor = parse_codesign_identity(&output, &"11".repeat(32)).unwrap();
        let update = parse_codesign_identity(&output, &"11".repeat(32)).unwrap();
        let rollback = parse_codesign_identity(&output, &"11".repeat(32)).unwrap();
        assert!(matches_trusted(&update, &predecessor));
        assert!(matches_trusted(&rollback, &predecessor));
    }

    #[test]
    fn rejects_ad_hoc_and_altered_resources_resigned_by_wrong_identity() {
        assert!(
            parse_codesign_identity(
                &signed(
                    "identifier \"com.talkingquill.app\" and cdhash H\"00\"",
                    "not set",
                    "adhoc"
                ),
                &"11".repeat(32),
            )
            .is_err()
        );
        let predecessor = parse_codesign_identity(
            &signed(
                "identifier \"com.talkingquill.app\" and anchor trusted and certificate leaf = H\"1111\"",
                "not set",
                "4096",
            ),
            &"11".repeat(32),
        )
        .unwrap();
        let attacker = parse_codesign_identity(
            &signed(
                "identifier \"com.talkingquill.app\" and anchor trusted and certificate leaf = H\"2222\"",
                "not set",
                "4096",
            ),
            &"22".repeat(32),
        )
        .unwrap();
        assert!(!matches_trusted(&attacker, &predecessor));
        assert!(
            parse_codesign_identity(
                &(signed("req", "not set", "4096") + "Identifier=evil.example\n"),
                &"11".repeat(32),
            )
            .is_err()
        );
    }
}
