use super::*;

pub(super) fn validate_header(version: u8, correlation: &str) -> Result<(), &'static str> {
    if version == 1 && is_lower_hex(correlation, 32) {
        Ok(())
    } else {
        Err("request header")
    }
}
pub(super) fn decode_hash(value: &str) -> Result<[u8; 32], &'static str> {
    let bytes = decode_hex(value, 32)?;
    if bytes.len() != 32 {
        return Err("hash");
    }
    bytes.try_into().map_err(|_| "hash")
}
pub(super) fn decode_hex(value: &str, max: usize) -> Result<Vec<u8>, &'static str> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || value.len() / 2 > max
        || !is_lower_hex(value, value.len())
    {
        return Err("hex");
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| "hex")?, 16).map_err(|_| "hex")
        })
        .collect()
}
pub(super) fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(super) fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>, &'static str> {
    let mut out: Vec<u16> = value.encode_wide().collect();
    if out.is_empty() || out.contains(&0) {
        return Err("wide path");
    }
    out.push(0);
    Ok(out)
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
        } else {
            if ch == '"' {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
            } else {
                out.push_str(&"\\".repeat(slashes));
            }
            slashes = 0;
            out.push(ch);
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}
pub(super) fn command_line(path: &Path, arguments: &[&str]) -> Result<Vec<u16>, &'static str> {
    let path = path.to_str().ok_or("path unicode")?;
    let text = std::iter::once(path)
        .chain(arguments.iter().copied())
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ");
    wide(std::ffi::OsStr::new(&text))
}
