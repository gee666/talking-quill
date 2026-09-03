#[used]
static SOURCE_COMMIT_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_COMMIT=",
    env!("TALKING_QUILL_SOURCE_COMMIT")
);
#[used]
static SOURCE_TREE_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_TREE=",
    env!("TALKING_QUILL_SOURCE_TREE")
);

#[cfg(not(windows))]
fn main() {
    std::process::exit(79);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("update key tool failed: {error}");
        std::process::exit(79);
    }
}

#[cfg(windows)]
fn run() -> Result<(), &'static str> {
    std::hint::black_box(SOURCE_COMMIT_MARKER);
    std::hint::black_box(SOURCE_TREE_MARKER);
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
    use std::io::{Read, Seek};
    use std::path::PathBuf;
    use talking_quill_acceptance_signer::windows_key_security::{
        create_protected_private_key, delete_validated_private_key, open_validated_private_key,
    };
    use zeroize::Zeroize;

    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let mode = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or("mode")?;
    let path = PathBuf::from(arguments.next().ok_or("key path")?);
    if arguments.next().is_some() || !path.is_absolute() {
        return Err("arguments");
    }
    if mode == "--delete-protected-key-v1" {
        delete_validated_private_key(&path)?;
        println!("{{\"result\":\"deleted\"}}");
        return Ok(());
    }

    let signing_key = if mode == "--generate-protected-key-v1" {
        let mut seed = [0u8; 32];
        let key = loop {
            getrandom::fill(&mut seed).map_err(|_| "key randomness")?;
            if let Ok(key) = SigningKey::from_slice(&seed) {
                break key;
            }
        };
        seed.zeroize();
        let document = key.to_pkcs8_der().map_err(|_| "key encode")?;
        let mut bytes = document.as_bytes().to_vec();
        let created = create_protected_private_key(&path, &bytes);
        bytes.zeroize();
        created?;
        key
    } else if mode == "--import-protected-key-v1" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(513)
            .read_to_end(&mut bytes)
            .map_err(|_| "key import read")?;
        if bytes.is_empty() || bytes.len() > 512 {
            bytes.zeroize();
            return Err("key import size");
        }
        let parsed = SigningKey::from_pkcs8_der(&bytes);
        if parsed
            .as_ref()
            .ok()
            .and_then(|key| key.to_pkcs8_der().ok())
            .is_none_or(|canonical| canonical.as_bytes() != bytes)
        {
            bytes.zeroize();
            return Err("key import encoding");
        }
        let key = parsed.map_err(|_| "key import parse")?;
        let created = create_protected_private_key(&path, &bytes);
        bytes.zeroize();
        created?;
        key
    } else if mode == "--validate-protected-key-v1" {
        let mut validated = open_validated_private_key(&path)?;
        let mut bytes = Vec::new();
        validated
            .file
            .read_to_end(&mut bytes)
            .map_err(|_| "key read")?;
        validated.file.rewind().map_err(|_| "key rewind")?;
        let parsed = SigningKey::from_pkcs8_der(&bytes);
        bytes.zeroize();
        parsed.map_err(|_| "key parse")?
    } else {
        return Err("mode");
    };
    let public = signing_key.verifying_key().to_sec1_point(false);
    println!(
        "{{\"publicKeySec1Hex\":\"{}\",\"result\":\"passed\"}}",
        hex(public.as_bytes())
    );
    Ok(())
}

#[cfg(windows)]
fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(TABLE[(byte >> 4) as usize] as char);
        output.push(TABLE[(byte & 15) as usize] as char);
    }
    output
}
