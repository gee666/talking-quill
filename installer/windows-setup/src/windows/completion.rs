//! A worker reports its result before exiting; only the normal controller owns UI.
use super::*;

pub(super) const COMPLETION_BYTES: usize = 2054;

pub(super) fn encode_completion(result: &Result<i32>) -> [u8; COMPLETION_BYTES] {
    let (code, message) = match result {
        Ok(code) => (*code, ""),
        Err(error) => (error.code, error.message.as_str()),
    };
    let mut length = message.len().min(COMPLETION_BYTES - 6);
    while !message.is_char_boundary(length) {
        length -= 1;
    }
    let mut frame = [0; COMPLETION_BYTES];
    frame[..4].copy_from_slice(&code.to_le_bytes());
    frame[4..6].copy_from_slice(&(length as u16).to_le_bytes());
    frame[6..6 + length].copy_from_slice(&message.as_bytes()[..length]);
    frame
}

pub(super) fn decode_completion(frame: &[u8; COMPLETION_BYTES]) -> Result<i32> {
    let code = i32::from_le_bytes(frame[..4].try_into().unwrap());
    let length = usize::from(u16::from_le_bytes(frame[4..6].try_into().unwrap()));
    if length > COMPLETION_BYTES - 6 {
        return Err(fail(
            EXIT_REJECTED,
            "Setup worker returned an invalid completion message.",
        ));
    }
    let message = std::str::from_utf8(&frame[6..6 + length]).map_err(|_| {
        fail(
            EXIT_REJECTED,
            "Setup worker returned an invalid error message.",
        )
    })?;
    if code == 0 {
        Ok(0)
    } else {
        Err(fail(
            code,
            if message.is_empty() {
                "Setup could not complete the operation."
            } else {
                message
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_preserves_worker_errors_and_bounds_unicode_messages() {
        assert_eq!(decode_completion(&encode_completion(&Ok(0))).unwrap(), 0);
        let error = fail(EXIT_REJECTED, "Installer test failure");
        let received = decode_completion(&encode_completion(&Err(error))).unwrap_err();
        assert_eq!(received.code, EXIT_REJECTED);
        assert_eq!(received.message, "Installer test failure");
        let long = "失敗".repeat(COMPLETION_BYTES);
        let received =
            decode_completion(&encode_completion(&Err(fail(EXIT_FAILURE, &long)))).unwrap_err();
        assert!(long.starts_with(&received.message));
        assert!(received.message.len() <= COMPLETION_BYTES - 6);
        let mut invalid = [0; COMPLETION_BYTES];
        invalid[4..6].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decode_completion(&invalid).is_err());
    }
}
