use std::io::{self, Read};

use thiserror::Error;

pub const MAX_BODY_LENGTH: usize = 16_384;
pub const LENGTH_PREFIX_BYTES: usize = 4;

#[derive(Debug, Error)]
pub enum FramingError {
    #[error("owner-protocol frame body length is outside 1..=16384")]
    InvalidLength,
    #[error("owner-protocol frame is truncated")]
    Truncated,
    #[error("owner-protocol frame has trailing bytes")]
    TrailingBytes,
    #[error("owner-protocol transport read failed")]
    Io(#[source] io::Error),
}

pub fn encode_outer_frame(body: &[u8]) -> Result<Vec<u8>, FramingError> {
    validate_body_length(body.len())?;
    let length = u32::try_from(body.len()).map_err(|_| FramingError::InvalidLength)?;
    let mut frame = Vec::with_capacity(LENGTH_PREFIX_BYTES + body.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

/// Decodes exactly one complete frame. Concatenated frames are rejected here;
/// streaming transports should call [`read_outer_frame`] repeatedly.
pub fn decode_outer_frame(frame: &[u8]) -> Result<&[u8], FramingError> {
    if frame.len() < LENGTH_PREFIX_BYTES {
        return Err(FramingError::Truncated);
    }
    let body_length = u32::from_be_bytes(frame[..4].try_into().expect("four-byte prefix")) as usize;
    validate_body_length(body_length)?;
    let expected = LENGTH_PREFIX_BYTES + body_length;
    match frame.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(FramingError::Truncated),
        std::cmp::Ordering::Greater => Err(FramingError::TrailingBytes),
        std::cmp::Ordering::Equal => Ok(&frame[LENGTH_PREFIX_BYTES..]),
    }
}

/// Reads one frame. A clean EOF before any prefix byte returns `Ok(None)`;
/// partial prefixes and partial bodies are fatal truncations.
pub fn read_outer_frame<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, FramingError> {
    let mut prefix = [0_u8; LENGTH_PREFIX_BYTES];
    let mut read = 0;
    while read < prefix.len() {
        match reader.read(&mut prefix[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(FramingError::Truncated),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FramingError::Io(error)),
        }
    }
    let body_length = u32::from_be_bytes(prefix) as usize;
    validate_body_length(body_length)?;
    let mut body = vec![0_u8; body_length];
    let mut read = 0;
    while read < body.len() {
        match reader.read(&mut body[read..]) {
            Ok(0) => return Err(FramingError::Truncated),
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FramingError::Io(error)),
        }
    }
    Ok(Some(body))
}

fn validate_body_length(length: usize) -> Result<(), FramingError> {
    if (1..=MAX_BODY_LENGTH).contains(&length) {
        Ok(())
    } else {
        Err(FramingError::InvalidLength)
    }
}
