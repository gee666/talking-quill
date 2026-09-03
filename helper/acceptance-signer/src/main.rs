use std::env;
#[cfg(not(windows))]
use std::fs;
use std::fs::File;
use std::io::{self, Read};
#[cfg(not(windows))]
use std::path::{Path, PathBuf};

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use zeroize::Zeroize;

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
const MAX_KEY_BYTES: u64 = 512;
const MAX_MESSAGE_BYTES: u64 = 64 * 1024;

fn main() {
    if let Err(error) = run() {
        eprintln!("acceptance signer failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    std::hint::black_box(SOURCE_COMMIT_MARKER);
    std::hint::black_box(SOURCE_TREE_MARKER);
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let mode = arguments.next().ok_or("signer mode is missing")?;
    let value = arguments
        .next()
        .ok_or("private key capability is missing")?;
    if arguments.next().is_some() {
        return Err("unexpected signer argument".into());
    }
    #[cfg(windows)]
    let mut key_file = {
        use std::os::windows::io::{FromRawHandle, RawHandle};
        use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
        use windows_sys::Win32::Foundation::SetHandleInformation;
        if mode != "--private-key-handle-v1" {
            return Err("the Windows signer requires a broker key handle".into());
        }
        let raw = value
            .to_str()
            .and_then(|text| text.parse::<usize>().ok())
            .filter(|handle| *handle != 0 && *handle != usize::MAX)
            .ok_or("private key handle is invalid")?;
        if unsafe { SetHandleInformation(raw as _, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err("private key handle could not be sealed".into());
        }
        unsafe { File::from_raw_handle(raw as RawHandle) }
    };
    #[cfg(not(windows))]
    let mut key_file = {
        if mode != "--private-key" {
            return Err("usage: talking-quill-acceptance-signer --private-key <pkcs8-der>".into());
        }
        let key_path = PathBuf::from(value);
        validate_regular_no_link_path(&key_path)?;
        open_key(&key_path)?
    };
    validate_open_key(&key_file)?;
    let key_size = key_file
        .metadata()
        .map_err(|_| "private key metadata is unavailable")?
        .len();
    if key_size == 0 || key_size > MAX_KEY_BYTES {
        return Err("private key size is invalid".into());
    }
    let mut key_bytes = Vec::with_capacity(key_size as usize);
    key_file
        .read_to_end(&mut key_bytes)
        .map_err(|_| "private key could not be read")?;
    if key_bytes.len() as u64 != key_size {
        key_bytes.zeroize();
        return Err("private key changed while it was read".into());
    }
    let parsed_key = SigningKey::from_pkcs8_der(&key_bytes);
    key_bytes.zeroize();
    let signing_key = parsed_key.map_err(|_| "private key is not canonical P-256 PKCS8")?;

    let message = read_bounded_stdin()?;
    let signature: Signature = signing_key.sign(&message);
    let verifying = signing_key.verifying_key().to_sec1_point(false);
    println!("{}", hex(signature.to_bytes().as_slice()));
    println!("{}", hex(verifying.as_bytes()));
    Ok(())
}

fn read_bounded_stdin() -> Result<Vec<u8>, String> {
    let mut input = io::stdin().take(MAX_MESSAGE_BYTES + 1);
    let mut message = Vec::new();
    input
        .read_to_end(&mut message)
        .map_err(|_| "signing message could not be read")?;
    if message.is_empty() || message.len() as u64 > MAX_MESSAGE_BYTES {
        return Err("signing message size is invalid".into());
    }
    Ok(message)
}

#[cfg(not(windows))]
fn validate_regular_no_link_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("private key path must be absolute".into());
    }
    let mut current = Some(path);
    while let Some(component) = current {
        let metadata = fs::symlink_metadata(component)
            .map_err(|_| "private key path metadata is unavailable")?;
        if metadata.file_type().is_symlink() {
            return Err("private key path contains a link".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err("private key path contains a reparse point".into());
            }
        }
        current = component.parent();
    }
    let metadata = fs::metadata(path).map_err(|_| "private key metadata is unavailable")?;
    if !metadata.is_file() {
        return Err("private key is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("private key must have one link".into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("private key is a reparse point".into());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_key(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|_| "private key could not be opened".into())
}

#[cfg(not(windows))]
fn validate_open_key(_file: &File) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn validate_open_key(file: &File) -> Result<(), String> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: the handle remains owned by `file`, and the output points to writable initialized
    // storage for the duration of this synchronous call.
    let result = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as HANDLE, information.as_mut_ptr())
    };
    if result == 0 {
        return Err("private key handle identity is unavailable".into());
    }
    // SAFETY: a successful GetFileInformationByHandle initialized the complete structure.
    let information = unsafe { information.assume_init() };
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if information.nNumberOfLinks != 1
        || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("private key handle is linked or a reparse point".into());
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6979_signatures_repeat_for_the_same_key_and_message() {
        let key = SigningKey::from_slice(&[7_u8; 32]).expect("test key");
        let first: Signature = key.sign(b"fixed acceptance request");
        let second: Signature = key.sign(b"fixed acceptance request");
        assert_eq!(first.to_bytes(), second.to_bytes());
        assert_ne!(
            first.to_bytes(),
            <SigningKey as Signer<Signature>>::sign(&key, b"changed acceptance request").to_bytes()
        );
    }
}
