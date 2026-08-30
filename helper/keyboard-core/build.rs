use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");
    if env::var_os("CARGO_FEATURE_NATIVE_TEST_INPUT").is_some()
        && env::var("OPT_LEVEL").is_ok_and(|level| level != "0")
    {
        panic!("native-test-input cannot be compiled into an optimized keyboard core");
    }
}
