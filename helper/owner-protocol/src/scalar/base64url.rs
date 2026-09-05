//! Canonical unpadded base64url shared by opaque values and policy blobs.
use super::ScalarError;

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(crate) fn encode_base64url(input: &[u8]) -> String {
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let value = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        output.push(char::from(BASE64URL[((value >> 18) & 0x3f) as usize]));
        output.push(char::from(BASE64URL[((value >> 12) & 0x3f) as usize]));
        output.push(char::from(BASE64URL[((value >> 6) & 0x3f) as usize]));
        output.push(char::from(BASE64URL[(value & 0x3f) as usize]));
    }
    match chunks.remainder() {
        [one] => {
            output.push(char::from(BASE64URL[(one >> 2) as usize]));
            output.push(char::from(BASE64URL[((one & 0x03) << 4) as usize]));
        }
        [one, two] => {
            output.push(char::from(BASE64URL[(one >> 2) as usize]));
            output.push(char::from(
                BASE64URL[(((one & 0x03) << 4) | (two >> 4)) as usize],
            ));
            output.push(char::from(BASE64URL[((two & 0x0f) << 2) as usize]));
        }
        [] => {}
        _ => unreachable!("chunks_exact remainder is shorter than three"),
    }
    output
}

pub(crate) fn decode_base64url(input: &str) -> Result<Vec<u8>, ScalarError> {
    if input.contains('=') || input.len() % 4 == 1 || !input.is_ascii() {
        return Err(ScalarError::Base64Url);
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3 + 2);
    let bytes = input.as_bytes();
    let complete = bytes.len() / 4 * 4;
    for chunk in bytes[..complete].chunks_exact(4) {
        let a = decode_digit(chunk[0])?;
        let b = decode_digit(chunk[1])?;
        let c = decode_digit(chunk[2])?;
        let d = decode_digit(chunk[3])?;
        output.push((a << 2) | (b >> 4));
        output.push((b << 4) | (c >> 2));
        output.push((c << 6) | d);
    }
    match &bytes[complete..] {
        [a, b] => {
            let a = decode_digit(*a)?;
            let b = decode_digit(*b)?;
            if b & 0x0f != 0 {
                return Err(ScalarError::Base64Url);
            }
            output.push((a << 2) | (b >> 4));
        }
        [a, b, c] => {
            let a = decode_digit(*a)?;
            let b = decode_digit(*b)?;
            let c = decode_digit(*c)?;
            if c & 0x03 != 0 {
                return Err(ScalarError::Base64Url);
            }
            output.push((a << 2) | (b >> 4));
            output.push((b << 4) | (c >> 2));
        }
        [] => {}
        _ => return Err(ScalarError::Base64Url),
    }
    if encode_base64url(&output) != input {
        return Err(ScalarError::Base64Url);
    }
    Ok(output)
}

fn decode_digit(byte: u8) -> Result<u8, ScalarError> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'-' => Ok(62),
        b'_' => Ok(63),
        _ => Err(ScalarError::Base64Url),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_base64url, encode_base64url};

    #[test]
    fn base64url_rfc_examples_are_canonical() {
        for (plain, encoded) in [
            (b"".as_slice(), ""),
            (b"f".as_slice(), "Zg"),
            (b"fo".as_slice(), "Zm8"),
            (b"foo".as_slice(), "Zm9v"),
            (b"foobar".as_slice(), "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64url(plain), encoded);
            assert_eq!(decode_base64url(encoded).expect("valid"), plain);
        }
    }
}
