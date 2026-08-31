use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FOOTER_MAGIC: &[u8; 8] = b"TQPKG2\0\0";
pub const FOOTER_SIZE: usize = 128;
pub const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_FILES: usize = 200_000;
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_TREE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageError {
    Footer,
    Range,
    Digest,
    Manifest,
    NonCanonicalManifest,
    Identity,
    Path,
    Collision,
    Limit,
    Block,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u16,
    pub architecture: String,
    pub version: String,
    pub source_commit: String,
    pub source_tree: String,
    pub package_mode: String,
    pub predecessor: Option<Predecessor>,
    pub tree_sha256: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Predecessor {
    pub version: String,
    pub release_build_digest: String,
    pub gateway_sha256: String,
    pub owner_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestFile {
    pub path: String,
    pub mode: u32,
    pub size: u64,
    pub sha256: String,
    pub block_offset: u64,
    pub block_size: u64,
}

#[derive(Debug, Clone)]
pub struct ParsedPackage {
    pub package_offset: u64,
    pub manifest: Manifest,
}

pub fn parse<R: Read + Seek>(
    reader: &mut R,
    image_size: u64,
) -> Result<ParsedPackage, PackageError> {
    if image_size < FOOTER_SIZE as u64 {
        return Err(PackageError::Footer);
    }
    let footer_position = footer_position(reader, image_size)?;
    reader
        .seek(SeekFrom::Start(footer_position))
        .map_err(|_| PackageError::Footer)?;
    let mut footer = [0_u8; FOOTER_SIZE];
    reader
        .read_exact(&mut footer)
        .map_err(|_| PackageError::Footer)?;
    if &footer[..8] != FOOTER_MAGIC
        || u32::from_le_bytes(footer[8..12].try_into().unwrap()) != 2
        || footer[12..16] != [0; 4]
        || footer[112..].iter().any(|byte| *byte != 0)
    {
        return Err(PackageError::Footer);
    }
    let package_offset = u64::from_le_bytes(footer[16..24].try_into().unwrap());
    let package_size = u64::from_le_bytes(footer[24..32].try_into().unwrap());
    let manifest_size = u64::from_le_bytes(footer[32..40].try_into().unwrap());
    let package_end = package_offset
        .checked_add(package_size)
        .ok_or(PackageError::Range)?;
    if package_size == 0
        || manifest_size == 0
        || manifest_size > MAX_MANIFEST_BYTES
        || manifest_size > package_size
        || package_end != footer_position
    {
        return Err(PackageError::Range);
    }
    reader
        .seek(SeekFrom::Start(package_offset))
        .map_err(|_| PackageError::Range)?;
    let mut package_hash = Sha256::new();
    let mut remaining = package_size;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        let take = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        reader
            .read_exact(&mut buffer[..take])
            .map_err(|_| PackageError::Range)?;
        package_hash.update(&buffer[..take]);
        remaining -= take as u64;
    }
    if package_hash.finalize().as_slice() != &footer[40..72] {
        return Err(PackageError::Digest);
    }
    reader
        .seek(SeekFrom::Start(package_offset))
        .map_err(|_| PackageError::Range)?;
    let mut manifest_bytes = vec![0_u8; manifest_size as usize];
    reader
        .read_exact(&mut manifest_bytes)
        .map_err(|_| PackageError::Range)?;
    if Sha256::digest(&manifest_bytes).as_slice() != &footer[72..104] {
        return Err(PackageError::Digest);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).map_err(|_| PackageError::Manifest)?;
    let canonical = serde_json::to_vec(&value).map_err(|_| PackageError::Manifest)?;
    if canonical != manifest_bytes {
        return Err(PackageError::NonCanonicalManifest);
    }
    let manifest: Manifest = serde_json::from_value(value).map_err(|_| PackageError::Manifest)?;
    validate_manifest(&manifest, package_size, manifest_size)?;
    Ok(ParsedPackage {
        package_offset,
        manifest,
    })
}

fn footer_position<R: Read + Seek>(reader: &mut R, image_size: u64) -> Result<u64, PackageError> {
    let mut dos = [0_u8; 64];
    reader
        .seek(SeekFrom::Start(0))
        .and_then(|_| reader.read_exact(&mut dos))
        .map_err(|_| PackageError::Footer)?;
    if &dos[..2] != b"MZ" {
        return Err(PackageError::Footer);
    }
    let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    let mut optional = [0_u8; 160];
    reader
        .seek(SeekFrom::Start(pe + 24))
        .and_then(|_| reader.read_exact(&mut optional))
        .map_err(|_| PackageError::Footer)?;
    let directory = match u16::from_le_bytes(optional[..2].try_into().unwrap()) {
        0x20b => 112,
        0x10b => 96,
        _ => return Err(PackageError::Footer),
    };
    let certificate_offset =
        u32::from_le_bytes(optional[directory + 32..directory + 36].try_into().unwrap()) as u64;
    let certificate_size =
        u32::from_le_bytes(optional[directory + 36..directory + 40].try_into().unwrap()) as u64;
    let package_end = if certificate_offset == 0 && certificate_size == 0 {
        image_size
    } else if certificate_offset.checked_add(certificate_size) == Some(image_size)
        && certificate_size >= 8
    {
        certificate_offset
    } else {
        return Err(PackageError::Footer);
    };
    package_end
        .checked_sub(FOOTER_SIZE as u64)
        .ok_or(PackageError::Footer)
}

pub fn extract_file<R: Read + Seek, W: std::io::Write>(
    reader: &mut R,
    package: &ParsedPackage,
    file: &ManifestFile,
    output: &mut W,
) -> Result<(), PackageError> {
    reader
        .seek(SeekFrom::Start(package.package_offset + file.block_offset))
        .map_err(|_| PackageError::Block)?;
    let limited = reader.take(file.block_size);
    let mut decoder = zstd::stream::read::Decoder::new(limited).map_err(|_| PackageError::Block)?;
    let mut hash = Sha256::new();
    let mut written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = decoder.read(&mut buffer).map_err(|_| PackageError::Block)?;
        if count == 0 {
            break;
        }
        written = written
            .checked_add(count as u64)
            .ok_or(PackageError::Limit)?;
        if written > file.size || written > MAX_FILE_BYTES {
            return Err(PackageError::Limit);
        }
        hash.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|_| PackageError::Block)?;
    }
    if written != file.size || hex(&hash.finalize()) != file.sha256 {
        return Err(PackageError::Digest);
    }
    Ok(())
}

fn validate_manifest(
    manifest: &Manifest,
    package_size: u64,
    manifest_size: u64,
) -> Result<(), PackageError> {
    if manifest.schema_version != 2
        || !matches!(manifest.architecture.as_str(), "x64" | "arm64")
        || !valid_version(&manifest.version)
        || !git_object_id(&manifest.source_commit)
        || !git_object_id(&manifest.source_tree)
        || !matches!(
            manifest.package_mode.as_str(),
            "fresh" | "update" | "repair" | "release"
        )
        || !hex_digest(&manifest.tree_sha256)
        || manifest.files.is_empty()
        || manifest.files.len() > MAX_FILES
        || matches!(manifest.package_mode.as_str(), "update" | "release")
            != manifest.predecessor.is_some()
    {
        return Err(PackageError::Identity);
    }
    if let Some(previous) = &manifest.predecessor
        && (!valid_version(&previous.version)
            || !hex_digest(&previous.release_build_digest)
            || !hex_digest(&previous.gateway_sha256)
            || !hex_digest(&previous.owner_sha256))
    {
        return Err(PackageError::Identity);
    }
    let mut names = BTreeSet::new();
    let mut expected_offset = manifest_size;
    let mut total = 0_u64;
    let mut tree = Sha256::new();
    for file in &manifest.files {
        validate_path(&file.path)?;
        let folded = file.path.to_lowercase();
        if !names.insert(folded) {
            return Err(PackageError::Collision);
        }
        if file.mode != 0
            || file.size > MAX_FILE_BYTES
            || file.block_size == 0
            || file.block_offset != expected_offset
            || !hex_digest(&file.sha256)
        {
            return Err(PackageError::Block);
        }
        total = total.checked_add(file.size).ok_or(PackageError::Limit)?;
        if total > MAX_TREE_BYTES {
            return Err(PackageError::Limit);
        }
        expected_offset = expected_offset
            .checked_add(file.block_size)
            .ok_or(PackageError::Range)?;
        frame(&mut tree, &file.path);
        frame(&mut tree, &file.mode.to_string());
        frame(&mut tree, &file.size.to_string());
        frame(&mut tree, &file.sha256);
    }
    if expected_offset != package_size || hex(&tree.finalize()) != manifest.tree_sha256 {
        return Err(PackageError::Digest);
    }
    Ok(())
}

pub fn validate_path(path: &str) -> Result<(), PackageError> {
    if path.is_empty()
        || !path.is_ascii()
        || path.len() > 1024
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.contains('\0')
    {
        return Err(PackageError::Path);
    }
    for part in path.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || part.ends_with(' ')
            || part.len() > 255
        {
            return Err(PackageError::Path);
        }
        let stem = part
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(PackageError::Path);
        }
        if part.chars().any(|character| {
            character < ' ' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
        }) {
            return Err(PackageError::Path);
        }
    }
    Ok(())
}

fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn git_object_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn frame(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let content = b"native package payload";
        let block = zstd::stream::encode_all(&content[..], 3).unwrap();
        let path = "resources/app.asar";
        let content_hash = hex(&Sha256::digest(content));
        let mut tree = Sha256::new();
        for value in [path, "0", &content.len().to_string(), &content_hash] {
            frame(&mut tree, value);
        }
        let mut value = serde_json::json!({
            "architecture":"x64", "files":[{"blockOffset":0,"blockSize":block.len(),"mode":0,"path":path,"sha256":content_hash,"size":content.len()}],
            "packageMode":"fresh", "predecessor":null, "schemaVersion":2, "sourceCommit":"ab".repeat(20),
            "sourceTree":"cd".repeat(20), "treeSha256":hex(&tree.finalize()), "version":"0.0.69"
        });
        let mut manifest = serde_json::to_vec(&value).unwrap();
        value["files"][0]["blockOffset"] = serde_json::json!(manifest.len());
        manifest = serde_json::to_vec(&value).unwrap();
        value["files"][0]["blockOffset"] = serde_json::json!(manifest.len());
        manifest = serde_json::to_vec(&value).unwrap();
        let mut image = vec![0_u8; 512];
        image[..2].copy_from_slice(b"MZ");
        image[60..64].copy_from_slice(&64_u32.to_le_bytes());
        image[64..68].copy_from_slice(b"PE\0\0");
        image[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
        let package_offset = image.len() as u64;
        let mut package = manifest.clone();
        package.extend_from_slice(&block);
        image.extend_from_slice(&package);
        let mut footer = [0_u8; FOOTER_SIZE];
        footer[..8].copy_from_slice(FOOTER_MAGIC);
        footer[8..12].copy_from_slice(&2_u32.to_le_bytes());
        footer[16..24].copy_from_slice(&package_offset.to_le_bytes());
        footer[24..32].copy_from_slice(&(package.len() as u64).to_le_bytes());
        footer[32..40].copy_from_slice(&(manifest.len() as u64).to_le_bytes());
        footer[40..72].copy_from_slice(&Sha256::digest(&package));
        footer[72..104].copy_from_slice(&Sha256::digest(&manifest));
        image.extend_from_slice(&footer);
        image
    }

    #[test]
    fn parses_and_extracts_a_canonical_full_tree() {
        let bytes = fixture();
        let mut reader = std::io::Cursor::new(&bytes);
        let parsed = parse(&mut reader, bytes.len() as u64).unwrap();
        let mut output = Vec::new();
        extract_file(&mut reader, &parsed, &parsed.manifest.files[0], &mut output).unwrap();
        assert_eq!(output, b"native package payload");
    }

    #[test]
    fn rejects_package_and_manifest_mutation() {
        let mut package_mutation = fixture();
        package_mutation[512] ^= 1;
        assert!(matches!(
            parse(
                &mut std::io::Cursor::new(&package_mutation),
                package_mutation.len() as u64
            ),
            Err(PackageError::Digest)
        ));
        let mut footer_mutation = fixture();
        let position = footer_mutation.len() - FOOTER_SIZE + 8;
        footer_mutation[position] = 3;
        assert!(matches!(
            parse(
                &mut std::io::Cursor::new(&footer_mutation),
                footer_mutation.len() as u64
            ),
            Err(PackageError::Footer)
        ));
    }

    #[test]
    fn rejects_windows_path_aliases() {
        for path in [
            "../app.exe",
            "a\\b",
            "a:b",
            "CON",
            "dir/nul.txt",
            "a./b",
            "AUX.txt",
            "/root",
        ] {
            assert_eq!(validate_path(path), Err(PackageError::Path), "{path}");
        }
        assert_eq!(validate_path("resources/app.asar"), Ok(()));
    }

    #[test]
    fn rejects_case_collisions_and_noncanonical_json() {
        let files = vec![
            ManifestFile {
                path: "A.txt".into(),
                mode: 0,
                size: 1,
                sha256: "00".repeat(32),
                block_offset: 1,
                block_size: 1,
            },
            ManifestFile {
                path: "a.TXT".into(),
                mode: 0,
                size: 1,
                sha256: "00".repeat(32),
                block_offset: 2,
                block_size: 1,
            },
        ];
        let manifest = Manifest {
            schema_version: 2,
            architecture: "x64".into(),
            version: "0.0.69".into(),
            source_commit: "00".repeat(20),
            source_tree: "11".repeat(20),
            package_mode: "fresh".into(),
            predecessor: None,
            tree_sha256: "22".repeat(32),
            files,
        };
        assert_eq!(
            validate_manifest(&manifest, 3, 1),
            Err(PackageError::Collision)
        );
    }
}
