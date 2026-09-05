//! Fixed-width policy and proof encodings. Field order is part of the protocol.

use crate::channel::StablePipeBinding;
use crate::image_policy::WindowsArchitecture;
use sha2::{Digest, Sha256};

pub fn protected_policy_proof(binding: &StablePipeBinding) -> Vec<u8> {
    let mut proof = Vec::with_capacity(136);
    proof.extend_from_slice(b"TQKOWPR1");
    proof.extend_from_slice(binding.manifest_sha256.as_bytes());
    proof.extend_from_slice(binding.release_policy_digest.as_bytes());
    proof.extend_from_slice(binding.release_build_digest.as_bytes());
    proof.extend_from_slice(&binding.credential_binding_digest);
    proof
}

pub(super) fn role_digest(role: u8, image: [u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"TQKO-WINDOWS-LOCAL-ROLE-V1\0");
    hash.update([role]);
    hash.update(image);
    hash.finalize().into()
}

pub(super) fn encode_policy(
    architecture: WindowsArchitecture,
    release: [u8; 32],
    gateway: [u8; 32],
    owner: [u8; 32],
    gateway_role: [u8; 32],
    owner_role: [u8; 32],
    predecessor: Option<([u8; 32], [u8; 32], [u8; 32])>,
) -> [u8; 328] {
    let mut bytes = [0_u8; 328];
    bytes[..8].copy_from_slice(b"TQKOPOL1");
    bytes[8..10].copy_from_slice(&1_u16.to_be_bytes());
    bytes[10] = 1;
    bytes[11] = match architecture {
        WindowsArchitecture::X64 => 1,
        WindowsArchitecture::Arm64 => 2,
    };
    bytes[12] = 2;
    bytes[13] = u8::from(predecessor.is_some());
    bytes[16..48].copy_from_slice(&release);
    bytes[48..80].copy_from_slice(&gateway);
    bytes[80..112].copy_from_slice(&owner);
    bytes[112..144].copy_from_slice(&gateway_role);
    bytes[144..176].copy_from_slice(&owner_role);
    for offset in [176, 200] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_be_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&1_u32.to_be_bytes());
        bytes[offset + 8..offset + 16].copy_from_slice(&7_u64.to_be_bytes());
        bytes[offset + 16..offset + 24].copy_from_slice(&1_u64.to_be_bytes());
    }
    if let Some((release, gateway, owner)) = predecessor {
        bytes[224..256].copy_from_slice(&release);
        bytes[256..288].copy_from_slice(&gateway);
        bytes[288..320].copy_from_slice(&owner);
        bytes[320] = 1;
        bytes[321] = match architecture {
            WindowsArchitecture::X64 => 1,
            WindowsArchitecture::Arm64 => 2,
        };
    }
    bytes
}
