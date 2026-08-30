//! Stable Windows named-pipe connection identity shared by gateway and owner.
//!
//! Values in this module are derived independently from kernel peer facts and
//! retained installed images. They are comparison inputs for protocol-v1 and
//! never credentials delivered by a launcher or accepted from a handshake.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::image_policy::{Sha256Digest, WindowsArchitecture};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPurpose {
    Observe,
    Capture,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionBinding {
    pub user_sid_digest: [u8; 32],
    pub logon_sid_digest: [u8; 32],
    pub wts_session_id: u32,
    pub integrity_rid: u32,
}

impl fmt::Debug for SessionBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionBinding(<redacted>)")
    }
}

/// Exact stable-pipe peer relation derived independently by both processes.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StablePipeBinding {
    pub peer_binding_id: [u8; 32],
    pub endpoint_binding_id: [u8; 32],
    pub purpose: ChannelPurpose,
    pub architecture: WindowsArchitecture,
    pub gateway_sha256: Sha256Digest,
    pub owner_sha256: Sha256Digest,
    pub gateway_role_digest: Sha256Digest,
    pub owner_role_digest: Sha256Digest,
    pub release_policy_digest: Sha256Digest,
    pub release_build_digest: Sha256Digest,
    pub manifest_sha256: Sha256Digest,
    pub installation_id: String,
    pub build_id: String,
    pub gateway_process_id: u32,
    pub owner_process_id: u32,
    pub gateway_creation_marker: u64,
    pub owner_creation_marker: u64,
    pub owner_integrity_rid: u32,
    pub session: SessionBinding,
    pub credential_binding_digest: [u8; 32],
}

impl fmt::Debug for StablePipeBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StablePipeBinding(<redacted>)")
    }
}
