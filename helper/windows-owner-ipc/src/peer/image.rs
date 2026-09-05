//! Streaming image digest and source marker inspection.

use super::{PeerError, SourceIdentity};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek};

const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const SOURCE_MARKER_PREFIX: &[u8] = b"TALKING_QUILL_SOURCE_";

pub(super) fn inspect_image(
    file: &mut std::fs::File,
) -> Result<([u8; 32], Option<SourceIdentity>), PeerError> {
    let length = file.metadata().map_err(|_| PeerError::Image)?.len();
    if !(1..=MAX_IMAGE_BYTES).contains(&length) {
        return Err(PeerError::Image);
    }
    file.rewind().map_err(|_| PeerError::Image)?;
    let mut hash = Sha256::new();
    let mut commit = MarkerScan::new(b"COMMIT=");
    let mut tree = MarkerScan::new(b"TREE=");
    let mut tail = Vec::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| PeerError::Image)?;
        if read == 0 {
            break;
        }
        let before = copied;
        copied += read as u64;
        if copied > MAX_IMAGE_BYTES {
            return Err(PeerError::Image);
        }
        hash.update(&buffer[..read]);
        let origin = before.saturating_sub(tail.len() as u64);
        tail.extend_from_slice(&buffer[..read]);
        commit.scan(&tail, origin)?;
        tree.scan(&tail, origin)?;
        const RETAINED_MARKER_BYTES: usize = 96;
        if tail.len() > RETAINED_MARKER_BYTES {
            tail.drain(..tail.len() - RETAINED_MARKER_BYTES);
        }
    }
    if copied != length {
        return Err(PeerError::Image);
    }
    file.rewind().map_err(|_| PeerError::Image)?;
    let source_identity = match (commit.finish(), tree.finish()) {
        (Ok(commit), Ok(tree)) => Some(SourceIdentity { commit, tree }),
        (Err(_), Err(_)) => None,
        _ => return Err(PeerError::Image),
    };
    Ok((hash.finalize().into(), source_identity))
}

struct MarkerScan {
    marker: Vec<u8>,
    offset: Option<u64>,
    value: Option<String>,
}

impl MarkerScan {
    fn new(suffix: &[u8]) -> Self {
        let mut marker = Vec::with_capacity(SOURCE_MARKER_PREFIX.len() + suffix.len());
        marker.extend_from_slice(SOURCE_MARKER_PREFIX);
        marker.extend_from_slice(suffix);
        Self {
            marker,
            offset: None,
            value: None,
        }
    }

    fn scan(&mut self, bytes: &[u8], origin: u64) -> Result<(), PeerError> {
        for (local, window) in bytes.windows(self.marker.len()).enumerate() {
            if window != self.marker.as_slice() {
                continue;
            }
            let offset = origin
                .checked_add(u64::try_from(local).map_err(|_| PeerError::Image)?)
                .ok_or(PeerError::Image)?;
            if self.offset == Some(offset) {
                continue;
            }
            if self.offset.is_some() {
                return Err(PeerError::Image);
            }
            let start = local
                .checked_add(self.marker.len())
                .ok_or(PeerError::Image)?;
            let Some(value) = bytes.get(start..start + 40) else {
                continue;
            };
            if !value
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
            {
                return Err(PeerError::Image);
            }
            self.offset = Some(offset);
            self.value = Some(String::from_utf8(value.to_vec()).map_err(|_| PeerError::Image)?);
        }
        Ok(())
    }

    fn finish(self) -> Result<String, PeerError> {
        self.value.ok_or(PeerError::Image)
    }
}
