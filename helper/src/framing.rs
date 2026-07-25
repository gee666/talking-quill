use std::io::{self, Read, Write};

use thiserror::Error;

pub const MAX_FRAME_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("I/O error while reading or writing a protocol frame")]
    Io(#[from] io::Error),
    #[error("truncated frame length prefix")]
    TruncatedPrefix,
    #[error("truncated frame payload")]
    TruncatedPayload,
    #[error("invalid frame length {0}")]
    InvalidLength(u32),
    #[error("outbound frame is too large: {0} bytes")]
    OutboundTooLarge(usize),
}

/// Reads one four-byte, big-endian length-prefixed frame.
///
/// EOF before any prefix byte is a clean stream close. Lengths must be within
/// `1..=MAX_FRAME_BYTES` (16 KiB); invalid or oversized lengths are rejected
/// before allocating a payload buffer.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0_u8; 4];
    let mut read = 0;
    while read < prefix.len() {
        match reader.read(&mut prefix[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::TruncatedPrefix),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error)),
        }
    }

    let declared = u32::from_be_bytes(prefix);
    if declared == 0 || declared as usize > MAX_FRAME_BYTES {
        return Err(FrameError::InvalidLength(declared));
    }

    let mut payload = vec![0_u8; declared as usize];
    let mut read = 0;
    while read < payload.len() {
        match reader.read(&mut payload[read..]) {
            Ok(0) => return Err(FrameError::TruncatedPayload),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    Ok(Some(payload))
}

pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), FrameError> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::OutboundTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| FrameError::OutboundTooLarge(payload.len()))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}
