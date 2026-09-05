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
    #[serde(deserialize_with = "required_option")]
    pub predecessor: Option<Predecessor>,
    pub target: TargetIdentity,
    #[serde(deserialize_with = "required_option")]
    pub fault_phase: Option<String>,
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
pub struct TargetIdentity {
    pub release_build_digest: String,
    pub gateway_sha256: String,
    pub owner_sha256: String,
    pub recovery_launcher_sha256: String,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
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
        || footer[104..].iter().any(|byte| *byte != 0)
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
    let mut decoder = zstd::stream::read::Decoder::new(limited)
        .map_err(|_| PackageError::Block)?
        .single_frame();
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
    let compressed = decoder.finish();
    let buffered = compressed.buffer().len() as u64;
    let remaining = compressed.into_inner().limit();
    if buffered.checked_add(remaining) != Some(0) {
        return Err(PackageError::Block);
    }
    if written != file.size || hex(&hash.finalize()) != file.sha256 {
        return Err(PackageError::Digest);
    }
    Ok(())
}

mod validation;
use validation::validate_manifest;
#[cfg(test)]
use validation::validate_path;

fn frame(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
