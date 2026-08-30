//! Installed macOS owner bootstrap for the permanent no-suppression gateway.
//!
//! This module may authenticate and register the fixed LoginItem, but it has no
//! dependency on the keyboard-owner crate and contains no input APIs.

mod cms;
mod config;
mod connector;
mod identity;
pub(crate) use identity::validate_spawned_sec_code_identity;
mod keychain;
mod maintenance;
mod provisioning;
mod service_management;

pub use connector::MacosOwnerConnector;
pub use maintenance::{
    MacosOwnerExitVerifier, probe_installed_target, resume_committed_uninstall_cleanup,
    run_finalizer,
};
pub use service_management::{LoginItemStatus, MacosLoginItemService};

pub fn validate_installed_owner() -> bool {
    config::InstalledConfig::load().is_ok()
}

pub(crate) fn validate_bridge_parent(parent_pid: u32) -> bool {
    let Ok(config) = config::InstalledConfig::load_for_bridge() else {
        return false;
    };
    for role in [&config.gateway, &config.owner] {
        if validate_spawned_sec_code_identity(
            parent_pid,
            &role.canonical_executable_path,
            role.executable_sha256,
            role.code_directory_hash,
            &role.designated_requirement,
        )
        .is_ok()
        {
            return true;
        }
    }
    false
}
