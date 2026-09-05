//! Terminal cleanup namespace and security policy.
use super::*;

pub(super) const TERMINAL_UNINSTALL_RECORD_NAME: &str = "terminal-uninstall-record-v1.json";
pub(super) const TERMINAL_RECOVERY_TOMBSTONE_PREFIX: &str = ".Talking Quill.recovery-tombstone-";
pub(super) const TERMINAL_FINAL_LAUNCHER_PREFIX: &str = ".Talking Quill Terminal Relaunch-";
pub(super) const TERMINAL_SERVICE_PREFIX: &str = "TalkingQuillTerminalCleanup-";
pub(super) const TERMINAL_SERVICE_IMAGE_PREFIX: &str = ".Talking Quill Terminal Cleanup-";
pub(super) const TERMINAL_SERVICE_PENDING_PREFIX: &str = ".Talking Quill.terminal-cleanup-pending-";
pub(super) const TERMINAL_SERVICE_FILE_SDDL: &str = MACHINE_LOCK_FILE_SDDL;
pub(super) const TERMINAL_SERVICE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;LC;;;AU)";
pub(super) const UNINSTALL_FINALIZER_PENDING_PREFIX: &str =
    ".Talking Quill.uninstall-finalizer-pending-";
pub(super) const UNINSTALL_FINALIZER_PREFIX: &str = ".Talking Quill.uninstall-finalizer-";
pub(super) const UNINSTALL_FINALIZER_NAME: &str = "Talking Quill Uninstall Finalizer.exe";
pub(super) const MEDIUM_FINALIZER_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;AU)";
pub(super) const MEDIUM_FINALIZER_FILE_SDDL: &str =
    "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;AU)";
pub(super) const MEDIUM_LAUNCHER_DIRECTORY_SDDL: &str = MEDIUM_FINALIZER_DIRECTORY_SDDL;
pub(super) const MEDIUM_LAUNCHER_FILE_SDDL: &str = MEDIUM_FINALIZER_FILE_SDDL;
pub(super) const LEGACY_LOCK_RETIREMENT_EPOCH: u8 = 3;
