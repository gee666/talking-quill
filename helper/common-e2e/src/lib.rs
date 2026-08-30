//! Test-only crate for cross-process gateway/owner integration suites.

#[cfg(not(debug_assertions))]
compile_error!("talking-quill-common-e2e is forbidden in optimized builds");
