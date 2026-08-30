use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_TEST_TRANSPORT");
    println!("cargo:rerun-if-env-changed=OPT_LEVEL");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    println!("cargo:rerun-if-env-changed=RUSTFLAGS");

    if env::var_os("CARGO_FEATURE_TEST_TRANSPORT").is_none() {
        return;
    }

    let profile_optimized = match env::var("OPT_LEVEL") {
        Ok(level) => level != "0",
        Err(env::VarError::NotPresent) => {
            panic!("Cargo did not provide OPT_LEVEL while test-transport was enabled")
        }
        Err(env::VarError::NotUnicode(_)) => {
            panic!("Cargo provided a non-Unicode OPT_LEVEL while test-transport was enabled")
        }
    };
    let rustflags_optimized =
        encoded_rustflags_request_optimization() || raw_rustflags_request_optimization();

    assert!(
        !(profile_optimized || rustflags_optimized),
        "owner-protocol test-transport bypasses platform trust and cannot compile with optimization"
    );
}

fn encoded_rustflags_request_optimization() -> bool {
    let Some(encoded) = env::var_os("CARGO_ENCODED_RUSTFLAGS") else {
        return false;
    };
    let encoded = encoded.into_string().unwrap_or_else(|_| {
        panic!(
            "Cargo provided non-Unicode CARGO_ENCODED_RUSTFLAGS while test-transport was enabled"
        )
    });
    rustc_args_request_optimization(
        encoded
            .split('\u{1f}')
            .flat_map(str::split_ascii_whitespace),
    )
}

// Cargo normally removes RUSTFLAGS and exposes CARGO_ENCODED_RUSTFLAGS to build
// scripts. Inspect the raw value too when a Cargo wrapper/configuration retains
// it. This is a normal Cargo packaging guard, not an adversarial rustc sandbox.
fn raw_rustflags_request_optimization() -> bool {
    let Some(flags) = env::var_os("RUSTFLAGS") else {
        return false;
    };
    let flags = flags.into_string().unwrap_or_else(|_| {
        panic!("Cargo provided non-Unicode RUSTFLAGS while test-transport was enabled")
    });
    rustc_args_request_optimization(flags.split_ascii_whitespace())
}

fn rustc_args_request_optimization<'a>(args: impl IntoIterator<Item = &'a str>) -> bool {
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let codegen_option = if argument == "-C" {
            args.next()
        } else {
            argument
                .strip_prefix("-C")
                .map(|option| option.trim_start_matches('='))
        };
        if codegen_option.is_some_and(optimization_option_is_nonzero) {
            return true;
        }
    }
    false
}

fn optimization_option_is_nonzero(option: &str) -> bool {
    option
        .strip_prefix("opt-level=")
        .is_some_and(|level| level != "0")
}
