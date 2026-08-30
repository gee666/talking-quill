const SERVICE_BRIDGE_ROLE_MARKER: &str =
    "TALKING_QUILL_MACOS_SERVICE_BRIDGE=LOCAL_MAINTENANCE_AUTHORITY_CANNOT_SUPPRESS";

#[cfg(target_os = "macos")]
fn main() {
    std::hint::black_box(SERVICE_BRIDGE_ROLE_MARKER);
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    use std::io::{BufRead, Write};

    let mut arguments = std::env::args();
    let _executable = arguments.next();
    let operation = arguments.next();
    if arguments.next().is_some() {
        std::process::exit(64);
    }
    let Some(operation) = operation else {
        std::process::exit(64);
    };
    if operation == "serve-authenticated" {
        let input = std::io::stdin();
        let mut lines = input.lock().lines();
        let key = lines
            .next()
            .and_then(Result::ok)
            .and_then(|value| decode_hex::<32>(&value));
        let Some(key) = key else {
            std::process::exit(64);
        };
        if talking_quill_helper::macos_service_bridge::validate_parent_process(unsafe {
            libc::getppid()
        } as u32)
        .is_err()
        {
            std::process::exit(70);
        }
        let mut output = std::io::stdout().lock();
        for (index, line) in lines.enumerate() {
            let Ok(operation) = line else {
                std::process::exit(70)
            };
            let result = talking_quill_helper::macos_service_bridge::run(&operation);
            let status = result.map_or(usize::MAX, |value| value);
            let sequence = index as u64 + 1;
            let pid = std::process::id();
            let audit_session = unsafe { audit_session_self() };
            let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).unwrap();
            mac.update(b"talking-quill/macos-service-bridge-response/v1\0");
            mac.update(&pid.to_be_bytes());
            mac.update(&audit_session.to_be_bytes());
            mac.update(&sequence.to_be_bytes());
            mac.update(operation.as_bytes());
            mac.update(&(status as u64).to_be_bytes());
            let tag: String = mac
                .finalize()
                .into_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if writeln!(
                output,
                "{pid} {audit_session} {sequence} {operation} {status} {tag}"
            )
            .and_then(|_| output.flush())
            .is_err()
            {
                std::process::exit(70);
            }
        }
        return;
    }
    // Status/ACL diagnostics are non-mutating. Mutations require the retained,
    // authenticated serve path above.
    if !matches!(operation.as_str(), "status" | "acl-denial") {
        std::process::exit(64);
    }
    match talking_quill_helper::macos_service_bridge::run(&operation) {
        Ok(status) => println!("{status}"),
        Err(_) => std::process::exit(70),
    }
}

#[cfg(target_os = "macos")]
fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = [0_u8; N];
    for (target, pair) in output.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *target = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    (!output.iter().all(|byte| *byte == 0)).then_some(output)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn audit_session_self() -> u32;
}

#[cfg(not(target_os = "macos"))]
fn main() {
    std::hint::black_box(SERVICE_BRIDGE_ROLE_MARKER);
    std::process::exit(69);
}
