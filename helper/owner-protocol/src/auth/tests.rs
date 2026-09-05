//! Authentication primitives, trust policy, and frozen cross-language vectors.
use super::*;

mod policy;
mod primitives;
mod vectors;

fn text<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field].as_str().expect("fixture string")
}

fn hex32(value: &str) -> Bytes32 {
    Bytes32::new(hex32_array(value))
}

fn hex32_array(value: &str) -> [u8; 32] {
    hex(value).try_into().expect("32-byte hex")
}

fn hex(value: &str) -> Vec<u8> {
    let compact: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex"))
        .collect()
}
