#![cfg(target_os = "macos")]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::macos_maintenance::{
    MacosMaintenanceOperation, MacosMaintenancePhase, MacosMaintenanceRecord,
};
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};

use crate::owner::client::OwnerConnector;
use crate::owner::maintenance_cli::{OwnerExitVerificationError, OwnerExitVerifier};

use super::config::InstalledConfig;
use super::service_management::MacosLoginItemService;

#[derive(Debug, Default)]
pub struct MacosOwnerExitVerifier {
    guard: Mutex<Option<File>>,
}

impl OwnerExitVerifier for MacosOwnerExitVerifier {
    fn wait_for_owner_exit(
        &self,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<bool, OwnerExitVerificationError> {
        let config =
            InstalledConfig::load().map_err(|_| OwnerExitVerificationError::Unavailable)?;
        let file =
            acquire_lock_after_owner_exit(&config, || cancelled.load(Ordering::Acquire), deadline)?;
        *self
            .guard
            .lock()
            .map_err(|_| OwnerExitVerificationError::Failed)? = Some(file);
        Ok(true)
    }
}

fn acquire_lock_after_owner_exit(
    config: &InstalledConfig,
    cancelled: impl Fn() -> bool,
    deadline: Instant,
) -> Result<File, OwnerExitVerificationError> {
    let lock = config.socket_path.with_file_name("maintenance.lock");
    while Instant::now() < deadline {
        if cancelled() {
            return Err(OwnerExitVerificationError::Cancelled);
        }
        if !config.socket_path.exists() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&lock)
                .map_err(|_| OwnerExitVerificationError::Failed)?;
            let metadata = file
                .metadata()
                .map_err(|_| OwnerExitVerificationError::Failed)?;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
                return Err(OwnerExitVerificationError::Failed);
            }
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(file);
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(OwnerExitVerificationError::Timeout)
}

/// Detached transaction owner. It validates the staged successor before
/// unregistering, retains maintenance exclusion across replacement, and writes
/// a durable exact completion record only after the new owner endpoint exists.
pub fn run_finalizer(arguments: &[String]) -> Result<(), FinalizeError> {
    let request = FinalizeRequest::parse(arguments)?;
    let owner_handoff = read_owner_handoff_pipe()?;
    let mut reporter = FinalizerReporter::new(owner_handoff, request.transaction.clone())?;
    let config = InstalledConfig::load().map_err(|_| FinalizeError)?;
    let record = authenticate_request(&request, &config, owner_handoff)?;
    let trusted_outer = trusted_predecessor_outer_identity(&request, record.phase)?;
    if reporter.cancelled()? {
        return Err(FinalizeError);
    }
    match record.phase {
        MacosMaintenancePhase::RolledBack => {
            verify_outer_signer(
                &request.installed_app,
                &trusted_outer,
                identity_scratch(&request)?,
            )?;
            recover_after_unregistration_unlocked(&request)?;
            if request.operation == Operation::Uninstall {
                reporter.error()?;
                return Err(FinalizeError);
            }
            reporter.ready()?;
            return Ok(());
        }
        MacosMaintenancePhase::InstallationComplete => {
            if request.operation == Operation::Uninstall {
                reporter.error()?;
                return Err(FinalizeError);
            }
            verify_outer_bundle(
                &request.installed_app,
                request.architecture.as_deref().ok_or(FinalizeError)?,
                &trusted_outer,
                identity_scratch(&request)?,
            )?;
            MacosLoginItemService
                .ensure_registered(&config)
                .map_err(|_| FinalizeError)?;
            wait_for_authenticated_target(&request)?;
            if super::keychain::read_maintenance_record()
                .map_err(|_| FinalizeError)?
                .is_some()
            {
                return Err(FinalizeError);
            }
            write_durable(&request, "complete")?;
            write_marker_if_missing(&request.complete, "complete\n")?;
            let backup = request
                .ready
                .parent()
                .ok_or(FinalizeError)?
                .join("predecessor.app");
            if backup.exists() {
                fs::remove_dir_all(backup).map_err(|_| FinalizeError)?;
            }
            reporter.ready()?;
            return Ok(());
        }
        MacosMaintenancePhase::InProgress => {}
    }
    let candidate = match request.operation {
        Operation::Update | Operation::Rollback => {
            Some(validate_candidate(&request, &config, &trusted_outer)?)
        }
        Operation::Uninstall => None,
    };
    let lock = acquire_lock_after_owner_exit(
        &config,
        || reporter.cancelled_now(),
        Instant::now() + Duration::from_secs(30),
    )
    .map_err(|_| FinalizeError)?;
    let proof = MacosLoginItemService
        .unregister(&config)
        .map_err(|_| FinalizeError)?;
    let mut recovery_backup = None;
    let mut uninstall_committed = false;
    let prepared = (|| -> Result<PreparedOutcome, FinalizeError> {
        if matches!(request.operation, Operation::Update | Operation::Uninstall) {
            recovery_backup = Some(copy_predecessor(
                &request,
                &config,
                &trusted_outer,
                &mut reporter,
            )?);
        }
        if reporter.cancelled()? {
            return Err(FinalizeError);
        }
        write_durable(&request, "prepared")?;
        write_marker(&request.ready, "ready\n")?;
        if request.operation == Operation::Uninstall {
            reporter.uninstall_ready()?;
            // This is the only uninstall cleanup budget. It covers observing the
            // outer-app removal, committing credential absence, and deleting the
            // exact tombstone and journal. No caller may create another deadline.
            let cleanup_deadline = Instant::now() + Duration::from_secs(120);
            wait_for_path_until(
                &request.installed_app,
                false,
                cleanup_deadline,
                reporter.cancellation_state(),
            )?;
            ensure_not_cancelled(reporter.cancellation_state())?;
            let retained_backup = prepare_uninstall_staging_for_last_backup(&request)?;
            ensure_not_cancelled(reporter.cancellation_state())?;
            recovery_backup = Some(retained_backup.clone());
            write_uninstall_cleanup_journal(
                &request,
                &config,
                "uninstall_commit_intent",
                &retained_backup,
            )?;
            // The synchronous final fd check and CAS form one linearizable race:
            // either cancellation wins, or deletion exclusively enters Committing.
            reporter.begin_credential_commit()?;
            if super::keychain::delete_after_unregistration(proof).is_err() {
                return Err(FinalizeError);
            }
            reporter.finish_credential_commit()?;
            uninstall_committed = true;
            write_uninstall_cleanup_journal(
                &request,
                &config,
                "uninstall_cleanup_pending",
                &retained_backup,
            )?;
            match finish_committed_uninstall(&request, &retained_backup, cleanup_deadline) {
                Ok(()) => reporter.complete()?,
                Err(_) => reporter.cleanup_pending()?,
            }
            return Ok(PreparedOutcome::Uninstalled);
        }
        reporter.ready()?;
        let candidate = candidate.ok_or(FinalizeError)?;
        let backup = if request.operation == Operation::Rollback {
            wait_for_process_exit(request.parent_pid, Duration::from_secs(30))?;
            let backup = install_rollback_candidate(&request)?;
            recovery_backup = Some(backup.clone());
            backup
        } else {
            recovery_backup.clone().ok_or(FinalizeError)?
        };
        wait_for_replacement(&request, &candidate, &trusted_outer)?;
        super::provisioning::provision_if_missing(&config).map_err(|_| FinalizeError)?;
        let complete_record = record.with_phase(MacosMaintenancePhase::InstallationComplete);
        super::keychain::write_maintenance_latch(
            &complete_record.encode().map_err(|_| FinalizeError)?,
        )
        .map_err(|_| FinalizeError)?;
        write_durable(&request, "installation_complete")?;
        Ok(PreparedOutcome::TargetReady(backup))
    })();
    let outcome = match prepared {
        Ok(value) => value,
        Err(_error) if uninstall_committed => {
            // Credential deletion is the commit boundary. Never start a second
            // cleanup attempt or reset its absolute budget here; startup owns
            // later idempotent resumption from the retained durable journal.
            drop(lock);
            if !reporter.is_terminal() {
                reporter.cleanup_pending()?;
            }
            return Ok(());
        }
        Err(error) => {
            let restored = recover_after_unregistration_under_lock(
                &request,
                recovery_backup.as_deref(),
                &config,
                record,
                &trusted_outer,
            );
            drop(lock);
            if restored.is_err() {
                return Err(FinalizeError);
            }
            recover_after_unregistration_unlocked(&request)?;
            return Err(error);
        }
    };
    drop(lock);
    let PreparedOutcome::TargetReady(backup) = outcome else {
        return Ok(());
    };

    if MacosLoginItemService.ensure_registered(&config).is_err()
        || wait_for_authenticated_target(&request).is_err()
        || super::keychain::read_maintenance_record()
            .map_err(|_| FinalizeError)?
            .is_some()
    {
        let _ = drain_target_for_rollback(&request, &config);
        let lock = acquire_lock_after_owner_exit(
            &config,
            || reporter.cancelled_now(),
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|_| FinalizeError)?;
        let _ = MacosLoginItemService.unregister(&config);
        let restored = recover_after_unregistration_under_lock(
            &request,
            Some(&backup),
            &config,
            record,
            &trusted_outer,
        );
        drop(lock);
        if restored.is_err() {
            return Err(FinalizeError);
        }
        recover_after_unregistration_unlocked(&request)?;
        return Err(FinalizeError);
    }
    if write_durable(&request, "complete").is_err()
        || write_marker_if_missing(&request.complete, "complete\n").is_err()
    {
        let _ = drain_target_for_rollback(&request, &config);
        let lock = acquire_lock_after_owner_exit(
            &config,
            || reporter.cancelled_now(),
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|_| FinalizeError)?;
        let _ = MacosLoginItemService.unregister(&config);
        let restored = recover_after_unregistration_under_lock(
            &request,
            Some(&backup),
            &config,
            record,
            &trusted_outer,
        );
        drop(lock);
        if restored.is_err() {
            return Err(FinalizeError);
        }
        recover_after_unregistration_unlocked(&request)?;
        return Err(FinalizeError);
    }
    fs::remove_dir_all(&backup).map_err(|_| FinalizeError)?;
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Operation {
    Update,
    Rollback,
    Uninstall,
}

impl Operation {
    const fn wire(self) -> MacosMaintenanceOperation {
        match self {
            Self::Update => MacosMaintenanceOperation::Update,
            Self::Rollback => MacosMaintenanceOperation::Rollback,
            Self::Uninstall => MacosMaintenanceOperation::Uninstall,
        }
    }
}

enum PreparedOutcome {
    Uninstalled,
    TargetReady(PathBuf),
}

struct FinalizeRequest {
    operation: Operation,
    transaction: String,
    source: String,
    target: Option<String>,
    target_owner: Option<String>,
    architecture: Option<String>,
    candidate_app: Option<PathBuf>,
    installed_app: PathBuf,
    ready: PathBuf,
    complete: PathBuf,
    parent_pid: libc::pid_t,
}

impl FinalizeRequest {
    fn parse(values: &[String]) -> Result<Self, FinalizeError> {
        if values.len() != 11 || !digest(&values[1]) || !digest(&values[2]) {
            return Err(FinalizeError);
        }
        let operation = match values[0].as_str() {
            "update" => Operation::Update,
            "rollback" => Operation::Rollback,
            "uninstall" => Operation::Uninstall,
            _ => return Err(FinalizeError),
        };
        let has_target = matches!(operation, Operation::Update | Operation::Rollback);
        if has_target != (digest(&values[3]) && digest(&values[4])) {
            return Err(FinalizeError);
        }
        let architecture = has_target.then(|| values[5].clone());
        if architecture
            .as_deref()
            .is_some_and(|value| !matches!(value, "x64" | "arm64"))
            || (!has_target && values[5] != "-")
        {
            return Err(FinalizeError);
        }
        let installed_app = normal_path(&values[7])?;
        let ready = normal_path(&values[8])?;
        let complete = normal_path(&values[9])?;
        let candidate_app = has_target.then(|| normal_path(&values[6])).transpose()?;
        let parent_pid = values[10]
            .parse::<libc::pid_t>()
            .map_err(|_| FinalizeError)?;
        if parent_pid <= 1 {
            return Err(FinalizeError);
        }
        let marker_parent = ready.parent().ok_or(FinalizeError)?;
        if complete.parent() != Some(marker_parent)
            || candidate_app
                .as_ref()
                .is_some_and(|path| path.parent() != Some(marker_parent))
            || installed_app.extension().and_then(|value| value.to_str()) != Some("app")
        {
            return Err(FinalizeError);
        }
        Ok(Self {
            operation,
            transaction: values[1].clone(),
            source: values[2].clone(),
            target: has_target.then(|| values[3].clone()),
            target_owner: has_target.then(|| values[4].clone()),
            architecture,
            candidate_app,
            installed_app,
            ready,
            complete,
            parent_pid,
        })
    }
}

struct Candidate {
    gateway_sha: String,
    owner_sha: String,
    bridge_sha: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CandidateWire {
    release_build_digest: String,
    installation_identity_digest: String,
    gateway: CandidateRole,
    owner: CandidateRole,
    bridge: CandidateRole,
    gateway_release_policy: PolicyBlob,
    gateway_release_policy_signature: PolicySignature,
    owner_release_policy: PolicyBlob,
    owner_release_policy_signature: PolicySignature,
    bridge_release_policy: PolicyBlob,
    bridge_release_policy_signature: PolicySignature,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CandidateRole {
    canonical_executable_path: PathBuf,
    executable_sha256: String,
    capture_authorized: bool,
    signing: CandidateSigning,
}
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum CandidateSigning {
    AdHoc {
        cdhash: String,
    },
    LocallyTrustedSelfSigned {
        #[serde(rename = "certificateSha256")]
        certificate_sha256: String,
        #[serde(rename = "certificateRequirementHash")]
        certificate_requirement_hash: String,
        cdhash: String,
    },
}

fn validate_candidate(
    request: &FinalizeRequest,
    config: &InstalledConfig,
    trusted_outer: &crate::macos_outer_identity::OuterIdentity,
) -> Result<Candidate, FinalizeError> {
    let app = request.candidate_app.as_ref().ok_or(FinalizeError)?;
    verify_outer_bundle(
        app,
        request.architecture.as_deref().ok_or(FinalizeError)?,
        trusted_outer,
        identity_scratch(request)?,
    )?;
    let wire: CandidateWire = serde_json::from_slice(&read_regular(
        &app.join("Contents/Resources/keyboard-owner-r5m.json"),
        64 * 1024,
    )?)
    .map_err(|_| FinalizeError)?;
    if wire.release_build_digest != request.target.as_deref().ok_or(FinalizeError)?
        || wire.owner.executable_sha256 != request.target_owner.as_deref().ok_or(FinalizeError)?
        || wire.installation_identity_digest != hex(config.installation_identity_digest.as_bytes())
        || !wire.gateway.capture_authorized
        || !wire.owner.capture_authorized
        || wire.bridge.capture_authorized
    {
        return Err(FinalizeError);
    }
    super::cms::verify(
        &wire.gateway_release_policy,
        &wire.gateway_release_policy_signature,
    )
    .map_err(|_| FinalizeError)?;
    super::cms::verify(
        &wire.owner_release_policy,
        &wire.owner_release_policy_signature,
    )
    .map_err(|_| FinalizeError)?;
    super::cms::verify(
        &wire.bridge_release_policy,
        &wire.bridge_release_policy_signature,
    )
    .map_err(|_| FinalizeError)?;
    let policy = wire
        .gateway_release_policy
        .decode()
        .map_err(|_| FinalizeError)?;
    let owner_policy = wire
        .owner_release_policy
        .decode()
        .map_err(|_| FinalizeError)?;
    if policy != owner_policy || wire.gateway_release_policy != wire.owner_release_policy {
        return Err(FinalizeError);
    }
    let bridge_policy = wire
        .bridge_release_policy
        .decode()
        .map_err(|_| FinalizeError)?;
    let predecessor = policy.predecessor.clone().ok_or(FinalizeError)?;
    if hex(policy.release_build_digest.as_bytes()) != wire.release_build_digest
        || hex(policy.gateway_sha256.as_bytes()) != wire.gateway.executable_sha256
        || hex(policy.owner_sha256.as_bytes()) != wire.owner.executable_sha256
        || hex(predecessor.release_build_digest.as_bytes()) != request.source
        || predecessor.gateway_sha256 != config.gateway.executable_sha256
        || predecessor.owner_sha256 != config.owner.executable_sha256
        || wire.gateway.canonical_executable_path
            != request.installed_app.join("Contents/Resources/helper/talking-quill-helper")
        || wire.owner.canonical_executable_path
            != request.installed_app.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner")
        || wire.bridge.canonical_executable_path
            != request.installed_app.join("Contents/MacOS/talking-quill-macos-service-bridge")
        || bridge_policy.release_build_digest != policy.release_build_digest
    {
        return Err(FinalizeError);
    }
    let gateway = app.join("Contents/Resources/helper/talking-quill-helper");
    let owner = app.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner");
    let bridge = app.join("Contents/MacOS/talking-quill-macos-service-bridge");
    if hash_file(&gateway)? != wire.gateway.executable_sha256
        || hash_file(&owner)? != wire.owner.executable_sha256
        || hash_file(&bridge)? != wire.bridge.executable_sha256
    {
        return Err(FinalizeError);
    }
    let gateway_requirement = wire
        .gateway
        .signing
        .requirement("com.talkingquill.app.helper")?;
    let owner_requirement = wire
        .owner
        .signing
        .requirement("com.talkingquill.app.keyboard-owner")?;
    let bridge_requirement = wire
        .bridge
        .signing
        .requirement("com.talkingquill.app.service-management")?;
    if requirement_digest(&gateway_requirement) != policy.gateway_signer_policy_digest
        || requirement_digest(&owner_requirement) != policy.owner_signer_policy_digest
        || bridge_policy.gateway_sha256 != decode_digest(&wire.bridge.executable_sha256)?
        || bridge_policy.owner_sha256 != decode_digest(&wire.bridge.executable_sha256)?
        || requirement_digest(&bridge_requirement) != bridge_policy.gateway_signer_policy_digest
        || requirement_digest(&bridge_requirement) != bridge_policy.owner_signer_policy_digest
    {
        return Err(FinalizeError);
    }
    verify_code(&gateway, &gateway_requirement)?;
    verify_code(&owner, &owner_requirement)?;
    verify_code(&bridge, &bridge_requirement)?;
    Ok(Candidate {
        gateway_sha: wire.gateway.executable_sha256,
        owner_sha: wire.owner.executable_sha256,
        bridge_sha: wire.bridge.executable_sha256,
    })
}

impl CandidateSigning {
    fn requirement(&self, identifier: &str) -> Result<String, FinalizeError> {
        let requirement = match self {
            CandidateSigning::AdHoc { cdhash } if hex_len(cdhash, 20) => {
                format!("identifier \"{identifier}\" and cdhash H\"{cdhash}\" and not anchor apple")
            }
            CandidateSigning::LocallyTrustedSelfSigned {
                certificate_sha256,
                certificate_requirement_hash,
                cdhash,
            } if hex_len(certificate_sha256, 32)
                && hex_len(certificate_requirement_hash, 20)
                && hex_len(cdhash, 20) =>
            {
                format!(
                    "identifier \"{identifier}\" and anchor trusted and certificate leaf = H\"{certificate_requirement_hash}\" and certificate root = H\"{certificate_requirement_hash}\" and not anchor apple"
                )
            }
            _ => return Err(FinalizeError),
        };
        Ok(requirement)
    }
}

fn requirement_digest(requirement: &str) -> talking_quill_owner_protocol::Bytes32 {
    talking_quill_owner_protocol::Bytes32::new(Sha256::digest(requirement.as_bytes()).into())
}

fn terminate_child_bounded(child: &mut Child) -> Result<(), FinalizeError> {
    let _ = child.kill();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if child.try_wait().map_err(|_| FinalizeError)?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(FinalizeError)
}

fn command_succeeds_bounded(
    command: &mut Command,
    timeout: Duration,
) -> Result<bool, FinalizeError> {
    let mut child = command.spawn().map_err(|_| FinalizeError)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|_| FinalizeError)? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            let _ = terminate_child_bounded(&mut child);
            return Err(FinalizeError);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_command_bounded(command: &mut Command, timeout: Duration) -> Result<(), FinalizeError> {
    command_succeeds_bounded(command, timeout)?
        .then_some(())
        .ok_or(FinalizeError)
}

fn verify_outer_bundle(
    app: &Path,
    architecture: &str,
    trusted: &crate::macos_outer_identity::OuterIdentity,
    scratch: &Path,
) -> Result<(), FinalizeError> {
    let native_architecture = if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return Err(FinalizeError);
    };
    if architecture != native_architecture {
        return Err(FinalizeError);
    }
    verify_outer_signer(app, trusted, scratch)?;
    verify_bundle_architectures(app, architecture)
}

fn verify_outer_signer(
    app: &Path,
    trusted: &crate::macos_outer_identity::OuterIdentity,
    scratch: &Path,
) -> Result<(), FinalizeError> {
    let observed = crate::macos_outer_identity::inspect(app, scratch).map_err(|_| FinalizeError)?;
    if !crate::macos_outer_identity::matches_trusted(&observed, trusted) {
        return Err(FinalizeError);
    }
    let mut codesign = Command::new("/usr/bin/codesign");
    codesign
        .args([
            "--verify",
            "--deep",
            "--strict",
            &format!("-R={}", trusted.designated_requirement),
        ])
        .arg(app);
    run_command_bounded(&mut codesign, Duration::from_secs(15))
}

fn trusted_predecessor_outer_identity(
    request: &FinalizeRequest,
    phase: MacosMaintenancePhase,
) -> Result<crate::macos_outer_identity::OuterIdentity, FinalizeError> {
    let app = if phase == MacosMaintenancePhase::InstallationComplete {
        request
            .ready
            .parent()
            .ok_or(FinalizeError)?
            .join("predecessor.app")
    } else {
        request.installed_app.clone()
    };
    let scratch = identity_scratch(request)?;
    let identity =
        crate::macos_outer_identity::inspect(&app, scratch).map_err(|_| FinalizeError)?;
    verify_outer_signer(&app, &identity, scratch)?;
    Ok(identity)
}

fn identity_scratch(request: &FinalizeRequest) -> Result<&Path, FinalizeError> {
    validated_staging(request)
}

fn verify_bundle_architectures(app: &Path, architecture: &str) -> Result<(), FinalizeError> {
    let mut pending = vec![(app.to_path_buf(), 0_u8)];
    let mut entries = 0_u32;
    while let Some((path, depth)) = pending.pop() {
        if depth > 32 || entries >= 10_000 {
            return Err(FinalizeError);
        }
        entries += 1;
        let metadata = fs::symlink_metadata(&path).map_err(|_| FinalizeError)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|_| FinalizeError)? {
                pending.push((entry.map_err(|_| FinalizeError)?.path(), depth + 1));
            }
        } else if metadata.is_file() {
            let mut header = Vec::with_capacity(4_096);
            File::open(&path)
                .map_err(|_| FinalizeError)?
                .take(4_096)
                .read_to_end(&mut header)
                .map_err(|_| FinalizeError)?;
            if crate::macho::exact_architecture(&header, architecture).map_err(|_| FinalizeError)?
                == Some(false)
            {
                return Err(FinalizeError);
            }
        }
    }
    Ok(())
}

fn verify_code(path: &Path, requirement: &str) -> Result<(), FinalizeError> {
    let mut command = Command::new("/usr/bin/codesign");
    command
        .args(["--verify", "--strict", &format!("-R={requirement}")])
        .arg(path);
    run_command_bounded(&mut command, Duration::from_secs(5))
}

fn read_owner_handoff_pipe() -> Result<talking_quill_owner_protocol::Bytes32, FinalizeError> {
    let mut file = unsafe { File::from_raw_fd(3) };
    read_owner_handoff(&mut file)
}

fn read_owner_handoff(
    reader: &mut impl Read,
) -> Result<talking_quill_owner_protocol::Bytes32, FinalizeError> {
    let mut bytes = [0_u8; 32];
    reader.read_exact(&mut bytes).map_err(|_| FinalizeError)?;
    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing).map_err(|_| FinalizeError)? != 0
        || bytes.iter().all(|byte| *byte == 0)
    {
        return Err(FinalizeError);
    }
    Ok(talking_quill_owner_protocol::Bytes32::new(bytes))
}

fn authenticate_request(
    request: &FinalizeRequest,
    config: &InstalledConfig,
    owner_handoff: talking_quill_owner_protocol::Bytes32,
) -> Result<MacosMaintenanceRecord, FinalizeError> {
    let record = super::keychain::read_maintenance_record()
        .map_err(|_| FinalizeError)?
        .ok_or(FinalizeError)?;
    if !handoff_matches_record(owner_handoff, record)
        || !request_fields_match_record(request, config.release_build_digest, record)?
    {
        return Err(FinalizeError);
    }
    let expected_app = config
        .gateway
        .canonical_executable_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(FinalizeError)?;
    if expected_app != request.installed_app {
        return Err(FinalizeError);
    }
    Ok(record)
}

fn handoff_matches_record(
    owner_handoff: talking_quill_owner_protocol::Bytes32,
    record: MacosMaintenanceRecord,
) -> bool {
    owner_handoff == record.owner_handoff
}

fn request_fields_match_record(
    request: &FinalizeRequest,
    installed_build: talking_quill_owner_protocol::Bytes32,
    record: MacosMaintenanceRecord,
) -> Result<bool, FinalizeError> {
    let active_build_matches = match record.phase {
        MacosMaintenancePhase::InProgress | MacosMaintenancePhase::RolledBack => {
            record.source_build == installed_build
        }
        MacosMaintenancePhase::InstallationComplete => record.target_build == Some(installed_build),
    };
    Ok(active_build_matches
        && record.operation == request.operation.wire()
        && record.transaction == decode_digest(&request.transaction)?
        && request.source == hex(record.source_build.as_bytes())
        && record.target_build == request.target.as_deref().map(decode_digest).transpose()?
        && record.target_owner
            == request
                .target_owner
                .as_deref()
                .map(decode_digest)
                .transpose()?)
}

fn copy_predecessor(
    request: &FinalizeRequest,
    config: &InstalledConfig,
    trusted_outer: &crate::macos_outer_identity::OuterIdentity,
    reporter: &mut FinalizerReporter,
) -> Result<PathBuf, FinalizeError> {
    let backup = request
        .ready
        .parent()
        .ok_or(FinalizeError)?
        .join("predecessor.app");
    if backup.exists() {
        return Err(FinalizeError);
    }
    let mut child = Command::new("/usr/bin/ditto")
        .args(["--noqtn"])
        .arg(&request.installed_app)
        .arg(&backup)
        .spawn()
        .map_err(|_| FinalizeError)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| FinalizeError)? {
            break status;
        }
        if Instant::now() >= deadline || reporter.cancelled()? {
            let _ = terminate_child_bounded(&mut child);
            let _ = fs::remove_dir_all(&backup);
            return Err(FinalizeError);
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !status.success()
        || verify_outer_signer(&backup, trusted_outer, identity_scratch(request)?).is_err()
        || hash_file(&backup.join("Contents/Resources/helper/talking-quill-helper"))?
            != hex(config.gateway.executable_sha256.as_bytes())
        || hash_file(&backup.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner"))?
            != hex(config.owner.executable_sha256.as_bytes())
        || hash_file(&backup.join("Contents/MacOS/talking-quill-macos-service-bridge"))?
            != hex(config.bridge.executable_sha256.as_bytes())
    {
        let _ = fs::remove_dir_all(&backup);
        return Err(FinalizeError);
    }
    Ok(backup)
}

fn wait_for_process_exit(pid: libc::pid_t, timeout: Duration) -> Result<(), FinalizeError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(FinalizeError)
}

fn install_rollback_candidate(request: &FinalizeRequest) -> Result<PathBuf, FinalizeError> {
    let candidate = request.candidate_app.as_ref().ok_or(FinalizeError)?;
    let parent = request.ready.parent().ok_or(FinalizeError)?;
    let backup = parent.join("predecessor.app");
    if backup.exists() {
        return Err(FinalizeError);
    }
    fs::rename(&request.installed_app, &backup).map_err(|_| FinalizeError)?;
    if fs::rename(candidate, &request.installed_app).is_err() {
        let _ = fs::rename(&backup, &request.installed_app);
        return Err(FinalizeError);
    }
    Ok(backup)
}

fn recover_after_unregistration_under_lock(
    request: &FinalizeRequest,
    backup: Option<&Path>,
    config: &InstalledConfig,
    record: MacosMaintenanceRecord,
    trusted_outer: &crate::macos_outer_identity::OuterIdentity,
) -> Result<(), FinalizeError> {
    let source_gateway = hex(config.gateway.executable_sha256.as_bytes());
    let source_owner = hex(config.owner.executable_sha256.as_bytes());
    let source_bridge = hex(config.bridge.executable_sha256.as_bytes());
    let installed_is_source = hash_file(&config.gateway.canonical_executable_path).ok()
        == Some(source_gateway)
        && hash_file(&config.owner.canonical_executable_path).ok() == Some(source_owner)
        && hash_file(&config.bridge.canonical_executable_path).ok() == Some(source_bridge)
        && verify_outer_signer(
            &request.installed_app,
            trusted_outer,
            identity_scratch(request)?,
        )
        .is_ok();
    if recovery_action(installed_is_source, backup.is_some_and(Path::exists))
        == RecoveryAction::Unrecoverable
    {
        return Err(FinalizeError);
    }
    if !installed_is_source {
        let backup = backup.filter(|path| path.exists()).ok_or(FinalizeError)?;
        let failed = request
            .ready
            .parent()
            .ok_or(FinalizeError)?
            .join("failed-target.app");
        if failed.exists() {
            fs::remove_dir_all(&failed).map_err(|_| FinalizeError)?;
        }
        if request.installed_app.exists() && fs::rename(&request.installed_app, &failed).is_err() {
            return Err(FinalizeError);
        }
        if fs::rename(backup, &request.installed_app).is_err() {
            let _ = fs::rename(&failed, &request.installed_app);
            return Err(FinalizeError);
        }
    }
    verify_outer_signer(
        &request.installed_app,
        trusted_outer,
        identity_scratch(request)?,
    )?;
    let rolled_back = record.with_phase(MacosMaintenancePhase::RolledBack);
    // The synced journal precedes the consumable Keychain latch. A journal
    // failure therefore cannot expose RolledBack to owner startup.
    commit_rollback_before_authority(
        || write_durable(request, "rolled_back"),
        || {
            super::keychain::write_maintenance_latch(
                &rolled_back.encode().map_err(|_| FinalizeError)?,
            )
            .map_err(|_| FinalizeError)?;
            super::provisioning::provision_if_missing(config).map_err(|_| FinalizeError)
        },
    )
}

fn commit_rollback_before_authority(
    persist_synced_rollback: impl FnOnce() -> Result<(), FinalizeError>,
    restore_authority: impl FnOnce() -> Result<(), FinalizeError>,
) -> Result<(), FinalizeError> {
    persist_synced_rollback()?;
    restore_authority()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecoveryAction {
    KeepInstalledSource,
    RestoreBackup,
    Unrecoverable,
}

const fn recovery_action(installed_is_source: bool, backup_exists: bool) -> RecoveryAction {
    if installed_is_source {
        RecoveryAction::KeepInstalledSource
    } else if backup_exists {
        RecoveryAction::RestoreBackup
    } else {
        RecoveryAction::Unrecoverable
    }
}

const RECOVERY_AUTHORITY_DEADLINE: Duration = Duration::from_secs(90);

fn recover_after_unregistration_unlocked(request: &FinalizeRequest) -> Result<(), FinalizeError> {
    let deadline = Instant::now() + RECOVERY_AUTHORITY_DEADLINE;
    loop {
        let recovered = InstalledConfig::load().ok().is_some_and(|config| {
            MacosLoginItemService.ensure_registered(&config).is_ok()
                && probe_with_installed_gateway_once(request, &request.source).is_ok()
                && super::keychain::read_maintenance_record().is_ok_and(|record| record.is_none())
        });
        if recovered {
            let failed = request
                .ready
                .parent()
                .ok_or(FinalizeError)?
                .join("failed-target.app");
            if failed.exists() {
                fs::remove_dir_all(failed).map_err(|_| FinalizeError)?;
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            // Keep the synced RolledBack journal/latch for startup reconciliation.
            return Err(FinalizeError);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn durable_removal_pending() -> Result<bool, FinalizeError> {
    let home = std::env::var_os("HOME").ok_or(FinalizeError)?;
    let path = PathBuf::from(home)
        .join("Library/Application Support/Talking Quill/KeyboardOwner/removal-required-v1");
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1
                && metadata.uid() == unsafe { libc::geteuid() } =>
        {
            Ok(true)
        }
        Ok(_) => Err(FinalizeError),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(FinalizeError),
    }
}

pub(super) fn resume_durable_removal_if_needed(
    config: &InstalledConfig,
) -> Result<(), FinalizeError> {
    let session = config.socket_path.parent().ok_or(FinalizeError)?;
    let owner_root = session
        .parent()
        .and_then(Path::parent)
        .ok_or(FinalizeError)?;
    let poison = owner_root.join("removal-required-v1");
    if !poison.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&poison).map_err(|_| FinalizeError)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(FinalizeError);
    }
    let proof = MacosLoginItemService
        .unregister(config)
        .map_err(|_| FinalizeError)?;
    if super::keychain::delete_after_unregistration(proof).is_err()
        && !super::keychain::fixed_items_absent().map_err(|_| FinalizeError)?
    {
        return Err(FinalizeError);
    }
    let run_root = session.parent().ok_or(FinalizeError)?;
    if run_root.exists() {
        let run_metadata = fs::symlink_metadata(run_root).map_err(|_| FinalizeError)?;
        if !run_metadata.is_dir()
            || run_metadata.file_type().is_symlink()
            || run_metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(FinalizeError);
        }
        fs::remove_dir_all(run_root).map_err(|_| FinalizeError)?;
    }
    fs::remove_file(&poison).map_err(|_| FinalizeError)?;
    File::open(owner_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FinalizeError)
}

pub fn probe_installed_target(expected_build: &str) -> Result<(), FinalizeError> {
    if !digest(expected_build) {
        return Err(FinalizeError);
    }
    let mut connector = super::connector::MacosOwnerConnector::default();
    let connected = connector.connect_capture().map_err(|_| FinalizeError)?;
    (connected.build_id == expected_build)
        .then_some(())
        .ok_or(FinalizeError)
}

fn drain_target_for_rollback(
    request: &FinalizeRequest,
    config: &InstalledConfig,
) -> Result<(), FinalizeError> {
    let gateway = request
        .installed_app
        .join("Contents/Resources/helper/talking-quill-helper");
    let target = request.target.as_deref().ok_or(FinalizeError)?;
    let mut command = Command::new(gateway);
    command.args([
        "--owner-maintenance",
        "rollback",
        &request.transaction,
        target,
        &request.source,
        &hex(config.owner.executable_sha256.as_bytes()),
    ]);
    run_command_bounded(&mut command, Duration::from_secs(10))
}

fn probe_with_installed_gateway(
    request: &FinalizeRequest,
    build: &str,
) -> Result<(), FinalizeError> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if probe_with_installed_gateway_once(request, build).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(FinalizeError)
}

fn probe_with_installed_gateway_once(
    request: &FinalizeRequest,
    build: &str,
) -> Result<(), FinalizeError> {
    let gateway = request
        .installed_app
        .join("Contents/Resources/helper/talking-quill-helper");
    let mut command = Command::new(gateway);
    command.args(["--macos-owner-probe", build]);
    run_command_bounded(&mut command, Duration::from_secs(5))
}

fn wait_for_authenticated_target(request: &FinalizeRequest) -> Result<(), FinalizeError> {
    let target = request.target.as_deref().ok_or(FinalizeError)?;
    probe_with_installed_gateway(request, target)
}

fn wait_for_replacement(
    request: &FinalizeRequest,
    candidate: &Candidate,
    trusted_outer: &crate::macos_outer_identity::OuterIdentity,
) -> Result<(), FinalizeError> {
    let gateway = request
        .installed_app
        .join("Contents/Resources/helper/talking-quill-helper");
    let owner = request.installed_app.join("Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner");
    let bridge = request
        .installed_app
        .join("Contents/MacOS/talking-quill-macos-service-bridge");
    let deadline = Instant::now() + Duration::from_secs(300);
    while Instant::now() < deadline {
        if hash_file(&gateway).ok().as_deref() == Some(&candidate.gateway_sha)
            && hash_file(&owner).ok().as_deref() == Some(&candidate.owner_sha)
            && hash_file(&bridge).ok().as_deref() == Some(&candidate.bridge_sha)
            && verify_outer_bundle(
                &request.installed_app,
                request.architecture.as_deref().ok_or(FinalizeError)?,
                trusted_outer,
                identity_scratch(request)?,
            )
            .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(FinalizeError)
}

fn prepare_uninstall_staging_for_last_backup(
    request: &FinalizeRequest,
) -> Result<PathBuf, FinalizeError> {
    let staging = validated_staging(request)?;
    let predecessor = staging.join("predecessor.app");
    let cleanup = durable_owner_root()?.join("cleanup-v1");
    create_private_directory(&cleanup)?;
    let retained = cleanup.join(format!("{}.app", request.source));
    if retained.exists() || !predecessor.is_dir() {
        return Err(FinalizeError);
    }
    fs::rename(&predecessor, &retained).map_err(|_| FinalizeError)?;
    File::open(&cleanup)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FinalizeError)?;
    if fs::remove_dir_all(staging).is_err() {
        let _ = fs::rename(&retained, &predecessor);
        return Err(FinalizeError);
    }
    Ok(retained)
}

fn finish_committed_uninstall(
    request: &FinalizeRequest,
    retained: &Path,
    cleanup_deadline: Instant,
) -> Result<(), FinalizeError> {
    loop {
        match delete_uninstall_tombstone_once(retained) {
            TombstoneCleanup::Complete => break,
            TombstoneCleanup::Pending => sleep_before_deadline(cleanup_deadline)?,
        }
    }
    // Uninstall staging (including request.complete) was deliberately removed.
    loop {
        if delete_durable(request).is_ok() {
            return Ok(());
        }
        sleep_before_deadline(cleanup_deadline)?;
    }
}

fn sleep_before_deadline(deadline: Instant) -> Result<(), FinalizeError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(FinalizeError)?;
    if remaining.is_zero() {
        return Err(FinalizeError);
    }
    std::thread::sleep(remaining.min(Duration::from_secs(2)));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TombstoneCleanup {
    Complete,
    Pending,
}

fn delete_uninstall_tombstone_once(retained: &Path) -> TombstoneCleanup {
    let existed = retained.exists();
    if !existed {
        return TombstoneCleanup::Complete;
    }
    let removed = fs::remove_dir_all(retained).is_ok();
    tombstone_cleanup_result(existed, removed, retained.exists())
}

const fn tombstone_cleanup_result(
    existed: bool,
    remove_succeeded: bool,
    remains: bool,
) -> TombstoneCleanup {
    if !existed || (remove_succeeded && !remains) {
        TombstoneCleanup::Complete
    } else {
        TombstoneCleanup::Pending
    }
}

fn validated_staging(request: &FinalizeRequest) -> Result<&Path, FinalizeError> {
    let staging = request.ready.parent().ok_or(FinalizeError)?;
    let metadata = fs::symlink_metadata(staging).map_err(|_| FinalizeError)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || staging.parent().is_none()
    {
        return Err(FinalizeError);
    }
    Ok(staging)
}

fn wait_for_path_until(
    path: &Path,
    exists: bool,
    cleanup_deadline: Instant,
    cancelled: &CancellationCommitState,
) -> Result<(), FinalizeError> {
    while Instant::now() < cleanup_deadline {
        ensure_not_cancelled(cancelled)?;
        if path.exists() == exists {
            // Close the observation race: cancellation wins until credential
            // deletion, even if bundle removal became visible simultaneously.
            ensure_not_cancelled(cancelled)?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(FinalizeError)
}

fn ensure_not_cancelled(cancelled: &CancellationCommitState) -> Result<(), FinalizeError> {
    (!cancelled.cancelled()).then_some(()).ok_or(FinalizeError)
}

fn durable_owner_root() -> Result<PathBuf, FinalizeError> {
    let home = std::env::var_os("HOME").ok_or(FinalizeError)?;
    Ok(PathBuf::from(home).join("Library/Application Support/Talking Quill/KeyboardOwner"))
}

fn durable_path(request: &FinalizeRequest) -> Result<PathBuf, FinalizeError> {
    Ok(durable_owner_root()?
        .join("maintenance-v1")
        .join(format!("{}.json", request.transaction)))
}

fn delete_durable(request: &FinalizeRequest) -> Result<(), FinalizeError> {
    delete_durable_path(&durable_path(request)?)
}

fn delete_durable_path(path: &Path) -> Result<(), FinalizeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1
                && metadata.uid() == unsafe { libc::geteuid() } =>
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(FinalizeError),
            }
        }
        Ok(_) => return Err(FinalizeError),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(FinalizeError),
    }
    File::open(path.parent().ok_or(FinalizeError)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FinalizeError)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UninstallCleanupJournal {
    version: u8,
    transaction: String,
    operation: String,
    source: String,
    phase: String,
    tombstone: String,
    staging_removed: bool,
    source_release_policy: PolicyBlob,
    source_release_policy_signature: PolicySignature,
}

fn write_uninstall_cleanup_journal(
    request: &FinalizeRequest,
    config: &InstalledConfig,
    phase: &str,
    tombstone: &Path,
) -> Result<(), FinalizeError> {
    let expected_tombstone = durable_owner_root()?
        .join("cleanup-v1")
        .join(format!("{}.app", request.source));
    if request.operation != Operation::Uninstall
        || !matches!(
            phase,
            "uninstall_commit_intent" | "uninstall_cleanup_pending"
        )
        || tombstone != expected_tombstone
        || request.source != hex(config.release_build_digest.as_bytes())
    {
        return Err(FinalizeError);
    }
    super::cms::verify(
        &config.gateway_release_policy,
        &config.gateway_release_policy_signature,
    )
    .map_err(|_| FinalizeError)?;
    let journal = UninstallCleanupJournal {
        version: 1,
        transaction: request.transaction.clone(),
        operation: "uninstall".into(),
        source: request.source.clone(),
        phase: phase.into(),
        tombstone: format!("cleanup-v1/{}.app", request.source),
        staging_removed: !request.ready.parent().ok_or(FinalizeError)?.exists(),
        source_release_policy: config.gateway_release_policy.clone(),
        source_release_policy_signature: config.gateway_release_policy_signature.clone(),
    };
    if !journal.staging_removed {
        return Err(FinalizeError);
    }
    write_durable_bytes(
        request,
        &serde_json::to_vec(&journal).map_err(|_| FinalizeError)?,
    )
}

fn write_durable(request: &FinalizeRequest, phase: &str) -> Result<(), FinalizeError> {
    let body = format!(
        "{{\"transaction\":\"{}\",\"operation\":\"{}\",\"source\":\"{}\",\"target\":{},\"targetOwner\":{},\"phase\":\"{}\"}}\n",
        request.transaction,
        match request.operation {
            Operation::Update => "update",
            Operation::Rollback => "rollback",
            Operation::Uninstall => "uninstall",
        },
        request.source,
        request
            .target
            .as_ref()
            .map_or("null".into(), |v| format!("\"{v}\"")),
        request
            .target_owner
            .as_ref()
            .map_or("null".into(), |v| format!("\"{v}\"")),
        phase
    );
    write_durable_bytes(request, body.as_bytes())
}

fn write_durable_bytes(request: &FinalizeRequest, body: &[u8]) -> Result<(), FinalizeError> {
    write_durable_path(&durable_path(request)?, &request.transaction, body)
}

fn write_durable_path(path: &Path, transaction: &str, body: &[u8]) -> Result<(), FinalizeError> {
    if !digest(transaction)
        || path.file_name().and_then(|name| name.to_str()) != Some(&format!("{transaction}.json"))
    {
        return Err(FinalizeError);
    }
    let directory = path.parent().ok_or(FinalizeError)?;
    create_private_directory(directory)?;
    let temporary = directory.join(format!(".{}.new-{}", transaction, std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(|_| FinalizeError)?;
    file.write_all(body)
        .and_then(|_| file.sync_all())
        .map_err(|_| FinalizeError)?;
    fs::rename(&temporary, path).map_err(|_| FinalizeError)?;
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| FinalizeError)
}

fn create_private_directory(path: &Path) -> Result<(), FinalizeError> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| FinalizeError)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(FinalizeError),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| FinalizeError)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(FinalizeError);
    }
    Ok(())
}

/// Runs from a newly installed, authenticated gateway before provisioning. It
/// needs no deleted Keychain state: authority is the current sealed gateway plus
/// the pinned CMS policy retained in each strict journal. Deletion is confined
/// to cleanup-v1/<CMS-authenticated source digest>.app beneath the private owner root.
pub fn resume_committed_uninstall_cleanup() -> Result<(), FinalizeError> {
    let config = InstalledConfig::load().map_err(|_| FinalizeError)?;
    let owner_root = durable_owner_root()?;
    let owner_metadata = match fs::symlink_metadata(&owner_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) => metadata,
        Err(_) => return Err(FinalizeError),
    };
    if !owner_metadata.is_dir()
        || owner_metadata.file_type().is_symlink()
        || owner_metadata.uid() != unsafe { libc::geteuid() }
        || owner_metadata.mode() & 0o077 != 0
    {
        return Err(FinalizeError);
    }
    let maintenance = owner_root.join("maintenance-v1");
    match fs::symlink_metadata(&maintenance) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0 => {}
        _ => return Err(FinalizeError),
    }
    for entry in fs::read_dir(&maintenance).map_err(|_| FinalizeError)? {
        let entry = entry.map_err(|_| FinalizeError)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(FinalizeError);
        };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let path = maintenance.join(name);
        let bytes = read_regular(&path, 128 * 1024)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| FinalizeError)?;
        let phase = value.get("phase").and_then(serde_json::Value::as_str);
        if !matches!(
            phase,
            Some("uninstall_commit_intent" | "uninstall_cleanup_pending")
        ) {
            continue;
        }
        let mut journal: UninstallCleanupJournal =
            serde_json::from_value(value).map_err(|_| FinalizeError)?;
        resume_exact_uninstall_journal(&config, &owner_root, &path, name, &mut journal)?;
    }
    Ok(())
}

fn resume_exact_uninstall_journal(
    config: &InstalledConfig,
    owner_root: &Path,
    journal_path: &Path,
    journal_name: &str,
    journal: &mut UninstallCleanupJournal,
) -> Result<(), FinalizeError> {
    if journal.version != 1
        || journal.operation != "uninstall"
        || !digest(&journal.transaction)
        || journal_name != format!("{}.json", journal.transaction)
        || !digest(&journal.source)
        || !journal.staging_removed
        || journal.tombstone != format!("cleanup-v1/{}.app", journal.source)
        || !matches!(
            journal.phase.as_str(),
            "uninstall_commit_intent" | "uninstall_cleanup_pending"
        )
    {
        return Err(FinalizeError);
    }
    super::cms::verify(
        &journal.source_release_policy,
        &journal.source_release_policy_signature,
    )
    .map_err(|_| FinalizeError)?;
    let policy = journal
        .source_release_policy
        .decode()
        .map_err(|_| FinalizeError)?;
    if hex(policy.release_build_digest.as_bytes()) != journal.source {
        return Err(FinalizeError);
    }
    let items_absent = super::keychain::fixed_items_absent().map_err(|_| FinalizeError)?;
    match resume_credential_action(&journal.phase, items_absent)? {
        ResumeCredentialAction::EstablishDurableCommit => {
            if !items_absent {
                let proof = MacosLoginItemService
                    .unregister(config)
                    .map_err(|_| FinalizeError)?;
                super::keychain::delete_after_unregistration(proof).map_err(|_| FinalizeError)?;
            }
            if !super::keychain::fixed_items_absent().map_err(|_| FinalizeError)? {
                return Err(FinalizeError);
            }
            // Tombstone authority begins only after absence is proven and the
            // committed cleanup phase is durably replaced and synced.
            journal.phase = "uninstall_cleanup_pending".into();
            persist_cleanup_pending(journal_path, journal)?;
        }
        ResumeCredentialAction::RemoveCommittedCleanup => {}
    }
    let cleanup = owner_root.join("cleanup-v1");
    match fs::symlink_metadata(&cleanup) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0 => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(FinalizeError),
    }
    // The tombstone is derived only from the release digest inside the
    // successfully verified CMS policy, never from an attacker-selected path.
    let tombstone = cleanup.join(format!("{}.app", journal.source));
    remove_exact_cleanup_tombstone(&tombstone)?;
    delete_durable_path(journal_path)?;
    if let Ok(mut entries) = fs::read_dir(&cleanup)
        && entries.next().is_none()
    {
        match fs::remove_dir(&cleanup) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(FinalizeError),
        }
    }
    File::open(owner_root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| FinalizeError)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResumeCredentialAction {
    EstablishDurableCommit,
    RemoveCommittedCleanup,
}

fn resume_credential_action(
    phase: &str,
    fixed_items_absent: bool,
) -> Result<ResumeCredentialAction, FinalizeError> {
    match (phase, fixed_items_absent) {
        ("uninstall_commit_intent", _) | ("uninstall_cleanup_pending", false) => {
            Ok(ResumeCredentialAction::EstablishDurableCommit)
        }
        ("uninstall_cleanup_pending", true) => Ok(ResumeCredentialAction::RemoveCommittedCleanup),
        _ => Err(FinalizeError),
    }
}

fn persist_cleanup_pending(
    journal_path: &Path,
    journal: &UninstallCleanupJournal,
) -> Result<(), FinalizeError> {
    if journal.phase != "uninstall_cleanup_pending" {
        return Err(FinalizeError);
    }
    write_durable_path(
        journal_path,
        &journal.transaction,
        &serde_json::to_vec(journal).map_err(|_| FinalizeError)?,
    )
}

fn remove_exact_cleanup_tombstone(tombstone: &Path) -> Result<(), FinalizeError> {
    match fs::symlink_metadata(tombstone) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() } =>
        {
            fs::remove_dir_all(tombstone).map_err(|_| FinalizeError)?;
        }
        Ok(_) => return Err(FinalizeError),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(FinalizeError),
    }
    (!tombstone.exists()).then_some(()).ok_or(FinalizeError)
}

fn write_marker_if_missing(path: &Path, value: &str) -> Result<(), FinalizeError> {
    if path.exists() {
        return (fs::read(path).map_err(|_| FinalizeError)? == value.as_bytes())
            .then_some(())
            .ok_or(FinalizeError);
    }
    write_marker(path, value)
}

fn write_marker(path: &Path, value: &str) -> Result<(), FinalizeError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| FinalizeError)?;
    file.write_all(value.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| FinalizeError)
}
fn read_regular(path: &Path, max: usize) -> Result<Vec<u8>, FinalizeError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| FinalizeError)?;
    let metadata = file.metadata().map_err(|_| FinalizeError)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > max as u64
    {
        return Err(FinalizeError);
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FinalizeError)?;
    (bytes.len() <= max).then_some(bytes).ok_or(FinalizeError)
}
fn hash_file(path: &Path) -> Result<String, FinalizeError> {
    let bytes = read_regular(path, 128 * 1024 * 1024)?;
    Ok(hex(&Sha256::digest(bytes)))
}
fn normal_path(value: &str) -> Result<PathBuf, FinalizeError> {
    let path = PathBuf::from(value);
    (path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_))))
    .then_some(path)
    .ok_or(FinalizeError)
}
fn decode_digest(value: &str) -> Result<talking_quill_owner_protocol::Bytes32, FinalizeError> {
    if !digest(value) {
        return Err(FinalizeError);
    }
    let mut bytes = [0_u8; 32];
    for (target, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *target = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| FinalizeError)?, 16)
            .map_err(|_| FinalizeError)?;
    }
    Ok(talking_quill_owner_protocol::Bytes32::new(bytes))
}
fn digest(value: &str) -> bool {
    hex_len(value, 32)
}
fn hex_len(value: &str, bytes: usize) -> bool {
    value.len() == bytes * 2
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const CANCELLABLE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMITTING: u8 = 2;
const COMMITTED: u8 = 3;

#[derive(Debug)]
struct CancellationCommitState(AtomicU8);

impl CancellationCommitState {
    fn new() -> Self {
        Self(AtomicU8::new(CANCELLABLE))
    }

    fn cancel(&self) -> bool {
        self.0
            .compare_exchange(CANCELLABLE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == CANCELLED
    }

    fn begin_commit(&self) -> Result<(), FinalizeError> {
        self.0
            .compare_exchange(CANCELLABLE, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| FinalizeError)
    }

    fn finish_commit(&self) -> Result<(), FinalizeError> {
        self.0
            .compare_exchange(COMMITTING, COMMITTED, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| FinalizeError)
    }

    fn seal(&self) {
        let state = self.0.load(Ordering::Acquire);
        if state == COMMITTING {
            let _ = self.finish_commit();
        } else if state == CANCELLABLE {
            let _ = self.0.compare_exchange(
                CANCELLABLE,
                COMMITTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

fn cancellation_observed(pipe: &mut File) -> Result<bool, FinalizeError> {
    let mut byte = [0_u8; 1];
    match pipe.read(&mut byte) {
        Ok(0) | Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(_) => Err(FinalizeError),
    }
}

struct FinalizerReporter {
    output: File,
    cancellation: Arc<CancellationCommitState>,
    cancellation_pipe: Arc<Mutex<File>>,
    handoff: talking_quill_owner_protocol::Bytes32,
    transaction: String,
    terminal: bool,
}

impl FinalizerReporter {
    fn new(
        handoff: talking_quill_owner_protocol::Bytes32,
        transaction: String,
    ) -> Result<Self, FinalizeError> {
        let cancellation_pipe = unsafe { File::from_raw_fd(5) };
        let flags = unsafe { libc::fcntl(cancellation_pipe.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe {
                libc::fcntl(
                    cancellation_pipe.as_raw_fd(),
                    libc::F_SETFL,
                    flags | libc::O_NONBLOCK,
                )
            } != 0
        {
            return Err(FinalizeError);
        }
        let cancellation = Arc::new(CancellationCommitState::new());
        let cancellation_pipe = Arc::new(Mutex::new(cancellation_pipe));
        let monitor = Arc::clone(&cancellation);
        let monitor_pipe = Arc::clone(&cancellation_pipe);
        std::thread::Builder::new()
            .name("tq-finalizer-cancel".into())
            .spawn(move || {
                loop {
                    let observed = monitor_pipe
                        .lock()
                        .map_err(|_| ())
                        .and_then(|mut pipe| cancellation_observed(&mut pipe).map_err(|_| ()));
                    match observed {
                        Ok(true) | Err(()) => {
                            monitor.cancel();
                            return;
                        }
                        Ok(false) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            })
            .map_err(|_| FinalizeError)?;
        Ok(Self {
            output: unsafe { File::from_raw_fd(4) },
            cancellation,
            cancellation_pipe,
            handoff,
            transaction,
            terminal: false,
        })
    }

    fn cancelled(&self) -> Result<bool, FinalizeError> {
        Ok(self.cancelled_now())
    }

    fn cancelled_now(&self) -> bool {
        self.cancellation.cancelled()
    }

    const fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn cancellation_state(&self) -> &CancellationCommitState {
        &self.cancellation
    }

    fn begin_credential_commit(&self) -> Result<(), FinalizeError> {
        let mut pipe = self.cancellation_pipe.lock().map_err(|_| FinalizeError)?;
        if cancellation_observed(&mut pipe)? {
            self.cancellation.cancel();
            return Err(FinalizeError);
        }
        self.cancellation.begin_commit()
    }

    fn finish_credential_commit(&self) -> Result<(), FinalizeError> {
        self.cancellation.finish_commit()
    }

    fn ready(&mut self) -> Result<(), FinalizeError> {
        self.write_terminal("ready")
    }

    fn uninstall_ready(&mut self) -> Result<(), FinalizeError> {
        self.write("uninstall_ready")
    }

    fn complete(&mut self) -> Result<(), FinalizeError> {
        self.write_terminal("complete")
    }

    fn error(&mut self) -> Result<(), FinalizeError> {
        self.write_terminal("error")
    }

    fn cleanup_pending(&mut self) -> Result<(), FinalizeError> {
        self.write_terminal("cleanup_pending")
    }

    fn write_terminal(&mut self, state: &str) -> Result<(), FinalizeError> {
        // Mark terminal before writing so a closed status pipe can never cause
        // Drop to attempt a second write to the same pipe.
        self.terminal = true;
        self.cancellation.seal();
        self.write(state)
    }

    fn write(&mut self, state: &str) -> Result<(), FinalizeError> {
        let mut authenticator = <Hmac<Sha256> as KeyInit>::new_from_slice(self.handoff.as_bytes())
            .map_err(|_| FinalizeError)?;
        authenticator.update(b"talking-quill/macos-finalizer-status/v1\0");
        authenticator.update(self.transaction.as_bytes());
        authenticator.update(state.as_bytes());
        let mac = hex(&authenticator.finalize().into_bytes());
        writeln!(
            self.output,
            "{{\"version\":1,\"transaction\":\"{}\",\"state\":\"{}\",\"mac\":\"{}\"}}",
            self.transaction, state, mac
        )
        .and_then(|_| self.output.flush())
        .map_err(|_| FinalizeError)
    }
}

impl Drop for FinalizerReporter {
    fn drop(&mut self) {
        if !self.terminal {
            self.terminal = true;
            self.cancellation.seal();
            let _ = self.write("error");
        }
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the macOS owner maintenance transaction failed closed")]
pub struct FinalizeError;

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_value(value: char) -> String {
        std::iter::repeat_n(value, 64).collect()
    }

    #[test]
    fn finalizer_arguments_bind_exact_operation_and_safe_paths() {
        let root = "/tmp/talking-quill-transaction";
        let valid = vec![
            "update".into(),
            digest_value('1'),
            digest_value('2'),
            digest_value('3'),
            digest_value('4'),
            "x64".into(),
            format!("{root}/Talking Quill.app"),
            "/Applications/Talking Quill.app".into(),
            format!("{root}/ready"),
            format!("{root}/complete"),
            "42".into(),
        ];
        assert!(FinalizeRequest::parse(&valid).is_ok());
        let mut traversal = valid.clone();
        traversal[6] = format!("{root}/../candidate.app");
        assert!(FinalizeRequest::parse(&traversal).is_err());
        let mut incomplete = valid;
        incomplete[4] = "-".into();
        assert!(FinalizeRequest::parse(&incomplete).is_err());
    }

    #[test]
    fn finalizer_authority_requires_exact_owner_persisted_in_progress_record() {
        let root = "/tmp/talking-quill-authority";
        let values = vec![
            "update".into(),
            digest_value('1'),
            digest_value('2'),
            digest_value('3'),
            digest_value('4'),
            "x64".into(),
            format!("{root}/Talking Quill.app"),
            "/Applications/Talking Quill.app".into(),
            format!("{root}/ready"),
            format!("{root}/complete"),
            "42".into(),
        ];
        let request = FinalizeRequest::parse(&values).unwrap();
        let record = MacosMaintenanceRecord::in_progress(
            MacosMaintenanceOperation::Update,
            decode_digest(&values[1]).unwrap(),
            decode_digest(&values[2]).unwrap(),
            Some(decode_digest(&values[3]).unwrap()),
            Some(decode_digest(&values[4]).unwrap()),
            talking_quill_owner_protocol::Bytes32::new([9; 32]),
        )
        .unwrap();
        assert!(handoff_matches_record(
            talking_quill_owner_protocol::Bytes32::new([9; 32]),
            record,
        ));
        assert!(!handoff_matches_record(
            talking_quill_owner_protocol::Bytes32::new([8; 32]),
            record,
        ));
        assert!(
            request_fields_match_record(&request, decode_digest(&values[2]).unwrap(), record)
                .unwrap()
        );
        assert!(
            !request_fields_match_record(
                &request,
                decode_digest(&values[2]).unwrap(),
                record.with_phase(MacosMaintenancePhase::InstallationComplete),
            )
            .unwrap()
        );
        let mut wrong = values.clone();
        wrong[4] = digest_value('5');
        assert!(
            !request_fields_match_record(
                &FinalizeRequest::parse(&wrong).unwrap(),
                decode_digest(&values[2]).unwrap(),
                record,
            )
            .unwrap()
        );
    }

    #[test]
    fn handoff_pipe_rejects_missing_short_zero_and_trailing_values() {
        assert!(read_owner_handoff(&mut &[][..]).is_err());
        assert!(read_owner_handoff(&mut &[1_u8; 31][..]).is_err());
        assert!(read_owner_handoff(&mut &[0_u8; 32][..]).is_err());
        assert!(read_owner_handoff(&mut &[1_u8; 33][..]).is_err());
        assert_eq!(
            read_owner_handoff(&mut &[7_u8; 32][..]).unwrap().as_bytes(),
            &[7_u8; 32],
        );
    }

    #[test]
    fn rollback_total_order_is_synced_journal_then_latch_then_authority() {
        let events = std::cell::RefCell::new(Vec::new());
        commit_rollback_before_authority(
            || {
                events.borrow_mut().push("journal_file_and_directory_sync");
                Ok(())
            },
            || {
                events.borrow_mut().push("keychain_latch_then_authority");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            events.into_inner(),
            [
                "journal_file_and_directory_sync",
                "keychain_latch_then_authority"
            ]
        );
    }

    #[test]
    fn rollback_journal_write_or_sync_failure_never_restores_authority() {
        for injected_stage in ["write", "file_sync", "directory_sync"] {
            let authority_restored = std::cell::Cell::new(false);
            let result = commit_rollback_before_authority(
                || Err(FinalizeError),
                || {
                    authority_restored.set(true);
                    Ok(())
                },
            );
            assert!(result.is_err(), "{injected_stage}");
            assert!(!authority_restored.get(), "{injected_stage}");
        }
    }

    #[test]
    fn every_post_unregister_failure_has_source_or_backup_recovery() {
        for stage in [
            "backup",
            "journal",
            "ready_marker",
            "replacement",
            "acl",
            "complete_record",
            "registration",
            "probe",
            "record_clear",
            "completion_marker",
        ] {
            assert_eq!(
                recovery_action(true, false),
                RecoveryAction::KeepInstalledSource,
                "{stage}",
            );
            assert_eq!(
                recovery_action(false, true),
                RecoveryAction::RestoreBackup,
                "{stage}",
            );
            assert_eq!(
                recovery_action(false, false),
                RecoveryAction::Unrecoverable,
                "{stage}",
            );
        }
    }

    #[test]
    fn committed_uninstall_never_treats_partial_tombstone_removal_as_success() {
        assert_eq!(
            tombstone_cleanup_result(false, false, false),
            TombstoneCleanup::Complete
        );
        assert_eq!(
            tombstone_cleanup_result(true, false, true),
            TombstoneCleanup::Pending
        );
        assert_eq!(
            tombstone_cleanup_result(true, true, true),
            TombstoneCleanup::Pending
        );
        assert_eq!(
            tombstone_cleanup_result(true, true, false),
            TombstoneCleanup::Complete
        );
    }

    #[test]
    fn crash_before_keychain_deletion_never_authorizes_tombstone_removal() {
        assert_eq!(
            resume_credential_action("uninstall_commit_intent", false).unwrap(),
            ResumeCredentialAction::EstablishDurableCommit
        );
        assert_eq!(
            resume_credential_action("uninstall_commit_intent", true).unwrap(),
            ResumeCredentialAction::EstablishDurableCommit
        );
        assert_eq!(
            resume_credential_action("uninstall_cleanup_pending", true).unwrap(),
            ResumeCredentialAction::RemoveCommittedCleanup
        );
        assert_eq!(
            resume_credential_action("uninstall_cleanup_pending", false).unwrap(),
            ResumeCredentialAction::EstablishDurableCommit
        );
    }

    #[test]
    fn crash_reboot_tombstone_cleanup_is_idempotent() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tmp")
            .join(format!(
                "committed-uninstall-restart-{}",
                std::process::id()
            ));
        let tombstone = root.join(format!("{}.app", digest_value('c')));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(tombstone.join("Contents/partial")).expect("partial tombstone");
        fs::write(tombstone.join("Contents/partial/file"), b"partial").expect("partial file");
        remove_exact_cleanup_tombstone(&tombstone).expect("reboot cleanup");
        remove_exact_cleanup_tombstone(&tombstone).expect("second launch is idempotent");
        fs::remove_dir_all(root).expect("remove restart fixture");
    }

    #[test]
    fn path_wait_cancels_promptly_and_cancellation_wins_removal_race() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tmp")
            .join(format!("cancel-wait-{}", std::process::id()));
        fs::write(&path, b"present").expect("path fixture");
        let cancelled = Arc::new(CancellationCommitState::new());
        let signal = Arc::clone(&cancelled);
        let path_for_thread = path.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            assert!(signal.cancel());
            let _ = fs::remove_file(path_for_thread);
        });
        let started = Instant::now();
        assert!(
            wait_for_path_until(
                &path,
                false,
                Instant::now() + Duration::from_secs(2),
                &cancelled,
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        worker.join().expect("cancellation worker");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cancellation_and_commit_cas_have_exactly_one_winner() {
        let cancellation_wins = CancellationCommitState::new();
        assert!(cancellation_wins.cancel());
        assert!(cancellation_wins.begin_commit().is_err());
        assert!(cancellation_wins.cancelled());

        let commit_wins = CancellationCommitState::new();
        commit_wins.begin_commit().expect("commit CAS");
        assert!(!commit_wins.cancel());
        assert!(!commit_wins.cancelled());
        commit_wins.finish_commit().expect("commit completion");
        assert_eq!(commit_wins.0.load(Ordering::Acquire), COMMITTED);
    }

    #[test]
    fn deletion_failure_stays_non_cancellable_and_fail_closed() {
        let state = CancellationCommitState::new();
        state.begin_commit().expect("commit CAS");
        assert!(!state.cancel());
        assert_eq!(state.0.load(Ordering::Acquire), COMMITTING);
    }

    #[test]
    fn uninstall_shape_has_no_target_or_candidate_authority() {
        let root = "/tmp/talking-quill-uninstall";
        let values = vec![
            "uninstall".into(),
            digest_value('a'),
            digest_value('b'),
            "-".into(),
            "-".into(),
            "-".into(),
            "-".into(),
            "/Applications/Talking Quill.app".into(),
            format!("{root}/ready"),
            format!("{root}/complete"),
            "42".into(),
        ];
        let parsed = FinalizeRequest::parse(&values).expect("strict uninstall");
        assert!(parsed.target.is_none());
        assert!(parsed.target_owner.is_none());
        assert!(parsed.candidate_app.is_none());
    }
}
