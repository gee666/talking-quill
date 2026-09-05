#![cfg(debug_assertions)]

#[path = "common_cross_process/connection.rs"]
mod connection;
#[path = "common_cross_process/fake_owner.rs"]
mod fake_owner;
#[path = "common_cross_process/forwarding_suite.rs"]
mod forwarding_suite;
#[path = "common_cross_process/gateway.rs"]
mod gateway;
#[path = "common_cross_process/gateway_suites.rs"]
mod gateway_suites;
#[path = "common_cross_process/native.rs"]
mod native;
#[path = "common_cross_process/owner_runtime.rs"]
mod owner_runtime;
#[path = "common_cross_process/owner_suite.rs"]
mod owner_suite;
#[path = "common_cross_process/process.rs"]
mod process;

use fake_owner::{
    run_crashing_fake_owner_process, run_deadline_fake_owner_process,
    run_replacement_fake_owner_process,
};
use forwarding_suite::gateway_fake_owner_forwarding_suite;
use gateway::run_gateway_with_fake_auth_process;
use gateway_suites::{
    fake_authentication_cannot_cross_production_transport_brand,
    gateway_deadline_cross_process_suite, gateway_owner_runtime_composition_suite,
};
use owner_runtime::run_owner_runtime_with_fake_auth_process;
use owner_suite::owner_runtime_fake_gateway_suite;
use process::ROLE_ENV;

#[test]
fn common_cross_process_foundation() {
    match std::env::var(ROLE_ENV).ok().as_deref() {
        Some("crashing-fake-owner") => run_crashing_fake_owner_process(),
        Some("replacement-fake-owner") => run_replacement_fake_owner_process(),
        Some("deadline-fake-owner") => run_deadline_fake_owner_process(),
        Some("gateway-fake-auth") => run_gateway_with_fake_auth_process(true),
        Some("gateway-owner-runtime-fake-auth") => run_gateway_with_fake_auth_process(false),
        Some("owner-runtime-fake-auth") => run_owner_runtime_with_fake_auth_process(),
        None => {
            fake_authentication_cannot_cross_production_transport_brand();
            gateway_fake_owner_forwarding_suite();
            gateway_deadline_cross_process_suite();
            gateway_owner_runtime_composition_suite();
            owner_runtime_fake_gateway_suite();
        }
        Some(role) => panic!("unknown cross-process role: {role}"),
    }
}
