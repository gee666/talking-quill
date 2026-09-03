fn main() {
    for name in ["TALKING_QUILL_SOURCE_COMMIT", "TALKING_QUILL_SOURCE_TREE"] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_else(|_| "0".repeat(40));
        assert!(value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()));
        println!("cargo:rustc-env={name}={value}");
    }
}
