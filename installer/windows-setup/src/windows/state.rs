//! Persisted transaction records and resolved installer paths. Wire schemas stay unchanged.
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Transaction {
    pub(super) schema_version: u8,
    pub(super) phase: String,
    pub(super) action: String,
    pub(super) had_predecessor: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct TerminalUninstallRecord {
    pub(super) schema_version: u8,
    pub(super) generation: String,
    pub(super) phase: String,
    pub(super) maintenance_sha256: String,
    pub(super) uninstall_command: String,
    pub(super) quiet_uninstall_command: String,
    pub(super) service_name: String,
    pub(super) service_image: String,
    pub(super) service_sha256: String,
    pub(super) service_file_identity: String,
    pub(super) record_file_identity: String,
}

pub(super) struct Paths {
    pub(super) install: PathBuf,
    pub(super) staging: PathBuf,
    pub(super) backup: PathBuf,
    pub(super) transaction: PathBuf,
    pub(super) maintenance_generation_record: PathBuf,
    pub(super) maintenance_uninstaller: PathBuf,
    pub(super) recovery_launcher: PathBuf,
    pub(super) profile: PathBuf,
    pub(super) legacy_authority: PathBuf,
    pub(super) legacy_quarantine: PathBuf,
    pub(super) legacy_task_file: PathBuf,
    pub(super) program_data: PathBuf,
}
