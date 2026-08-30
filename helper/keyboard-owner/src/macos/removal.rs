#![cfg(target_os = "macos")]

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
#[cfg(feature = "macos-native-lifecycle-fixture")]
use std::sync::atomic::{AtomicU8, Ordering};

/// Drag-to-Trash cleanup runs only after the runtime, endpoint, singleton and
/// maintenance handles have been dropped. A durable poison file keeps capture
/// closed across retries/relaunches until every owned artifact is gone.
pub fn finish_removed_install(
    runtime: &Path,
    bridge: &mut super::RemovalServiceBridge,
) -> Result<(), RemovalError> {
    let owner_root = runtime
        .parent()
        .and_then(Path::parent)
        .ok_or(RemovalError)?;
    let poison = owner_root.join("removal-required-v1");
    create_poison(&poison)?;
    cleanup_once(runtime, bridge)?;
    fs::remove_file(&poison).map_err(|_| RemovalError)?;
    sync_directory(owner_root)
}

pub fn prepare_removed_install(runtime: &Path) -> Result<(), RemovalError> {
    let owner_root = runtime
        .parent()
        .and_then(Path::parent)
        .ok_or(RemovalError)?;
    create_poison(&owner_root.join("removal-required-v1"))
}

pub fn removal_poisoned(runtime: &Path) -> bool {
    runtime
        .parent()
        .and_then(Path::parent)
        .is_some_and(|root| fs::symlink_metadata(root.join("removal-required-v1")).is_ok())
}

/// Completes the crash/reboot case without loading bundle policy. Keychain
/// deletion is possible only after an authenticated unregister proof, so both
/// fixed no-UI queries being absent are durable evidence that native cleanup
/// already crossed that boundary. Otherwise the poison is retained for an
/// installed owner or a later reinstall to retry with the authenticated bridge.
pub fn finish_poisoned_without_bundle(runtime: &Path) -> Result<bool, RemovalError> {
    let owner_root = runtime
        .parent()
        .and_then(Path::parent)
        .ok_or(RemovalError)?;
    let poison = owner_root.join("removal-required-v1");
    valid_poison(&poison)?;
    let fixed_items_absent =
        super::native_keychain::fixed_items_absent_without_ui().map_err(|_| RemovalError)?;
    if preconfig_cleanup_decision(fixed_items_absent) == PreconfigCleanup::RetryAfterReinstall {
        return Ok(false);
    }
    cleanup_runtime(runtime)?;
    fs::remove_file(&poison).map_err(|_| RemovalError)?;
    sync_directory(owner_root)?;
    Ok(true)
}

fn cleanup_once(
    runtime: &Path,
    bridge: &mut super::RemovalServiceBridge,
) -> Result<(), RemovalError> {
    #[cfg(feature = "macos-native-lifecycle-fixture")]
    if inject_permissioned_cleanup_failure()? {
        return Err(RemovalError);
    }
    let proof = bridge.unregister().map_err(|_| RemovalError)?;
    super::delete_after_unregistration(proof).map_err(|_| RemovalError)?;
    cleanup_runtime(runtime)?;
    #[cfg(feature = "macos-native-lifecycle-fixture")]
    lifecycle_fixture_event("success")?;
    Ok(())
}

#[cfg(feature = "macos-native-lifecycle-fixture")]
static LIFECYCLE_FIXTURE_ATTEMPT: AtomicU8 = AtomicU8::new(0);

#[cfg(feature = "macos-native-lifecycle-fixture")]
fn inject_permissioned_cleanup_failure() -> Result<bool, RemovalError> {
    if std::env::var("TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE").as_deref()
        != Ok("permissioned-ci-v1")
    {
        return Ok(false);
    }
    let attempt = LIFECYCLE_FIXTURE_ATTEMPT.fetch_add(1, Ordering::AcqRel);
    lifecycle_fixture_event(if attempt == 0 { "injected" } else { "retry" })?;
    Ok(attempt == 0)
}

#[cfg(feature = "macos-native-lifecycle-fixture")]
fn lifecycle_fixture_event(event: &str) -> Result<(), RemovalError> {
    let path = PathBuf::from(
        std::env::var_os("TALKING_QUILL_MACOS_REMOVAL_RETRY_TRACE").ok_or(RemovalError)?,
    );
    if !path.is_absolute()
        || !path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
        || !path.to_string_lossy().contains("/tmp/")
    {
        return Err(RemovalError);
    }
    let mut trace = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| RemovalError)?;
    writeln!(trace, "{} {event}", std::process::id())
        .and_then(|_| trace.sync_all())
        .map_err(|_| RemovalError)
}

fn cleanup_runtime(runtime: &Path) -> Result<(), RemovalError> {
    let uid = unsafe { libc::geteuid() };
    let run_root = runtime.parent().ok_or(RemovalError)?;
    let owner_root = run_root.parent().ok_or(RemovalError)?;
    match fs::symlink_metadata(runtime) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
                return Err(RemovalError);
            }
            // LOCK_EX excludes both a live owner and maintenance. Validate the
            // retained inode before moving the whole generation out of the
            // public name, then clean only that private tombstone.
            let authority = acquire_cleanup_authority(&runtime.join("maintenance.lock"), uid)?;
            let tombstone = run_root.join(format!(
                ".owner-runtime-removed-{}-{:016x}",
                std::process::id(),
                getrandom::u64().map_err(|_| RemovalError)?
            ));
            fs::rename(runtime, &tombstone).map_err(|_| RemovalError)?;
            for name in ["owner-v1.sock", "owner-v1.lock", "maintenance.lock"] {
                remove_validated_runtime_component(&tombstone.join(name), uid)?;
            }
            match fs::read_dir(&tombstone) {
                Ok(mut entries) => {
                    if entries.next().is_some() {
                        return Err(RemovalError);
                    }
                }
                Err(_) => return Err(RemovalError),
            }
            fs::remove_dir(&tombstone).map_err(|_| RemovalError)?;
            drop(authority);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(RemovalError),
    }
    match fs::symlink_metadata(run_root) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != uid {
                return Err(RemovalError);
            }
            match fs::read_dir(run_root) {
                Ok(mut entries) => {
                    if entries.next().is_none() {
                        match fs::remove_dir(run_root) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(_) => return Err(RemovalError),
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(RemovalError),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(RemovalError),
    }
    sync_directory(owner_root)
}

fn acquire_cleanup_authority(path: &Path, uid: u32) -> Result<File, RemovalError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| RemovalError)?;
    let retained = file.metadata().map_err(|_| RemovalError)?;
    let named = fs::symlink_metadata(path).map_err(|_| RemovalError)?;
    if !retained.is_file()
        || named.file_type().is_symlink()
        || retained.uid() != uid
        || retained.nlink() != 1
        || retained.dev() != named.dev()
        || retained.ino() != named.ino()
        || unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
    {
        return Err(RemovalError);
    }
    Ok(file)
}

fn remove_validated_runtime_component(path: &Path, uid: u32) -> Result<(), RemovalError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.nlink() == 1
                && (metadata.is_file() || metadata.file_type().is_socket()) =>
        {
            match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(RemovalError),
            }
        }
        Ok(_) => Err(RemovalError),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RemovalError),
    }
}

fn valid_poison(path: &Path) -> Result<(), RemovalError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RemovalError)?;
    (metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.nlink() == 1
        && metadata.uid() == unsafe { libc::geteuid() })
    .then_some(())
    .ok_or(RemovalError)
}

fn create_poison(path: &Path) -> Result<(), RemovalError> {
    if path.exists() {
        return valid_poison(path);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| RemovalError)?;
    file.write_all(b"capture-disabled-until-removal-complete\n")
        .and_then(|_| file.sync_all())
        .map_err(|_| RemovalError)?;
    sync_directory(path.parent().ok_or(RemovalError)?)
}

fn sync_directory(path: &Path) -> Result<(), RemovalError> {
    OpenOptions::new()
        .read(true)
        .open(PathBuf::from(path))
        .and_then(|file| file.sync_all())
        .map_err(|_| RemovalError)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreconfigCleanup {
    FinishWithoutBundle,
    RetryAfterReinstall,
}

const fn preconfig_cleanup_decision(fixed_items_absent: bool) -> PreconfigCleanup {
    if fixed_items_absent {
        PreconfigCleanup::FinishWithoutBundle
    } else {
        PreconfigCleanup::RetryAfterReinstall
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("removed macOS installation cleanup remains durably poisoned")]
pub struct RemovalError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reboot_after_authenticated_native_cleanup_finishes_without_bundle() {
        assert_eq!(
            preconfig_cleanup_decision(true),
            PreconfigCleanup::FinishWithoutBundle
        );
    }

    #[test]
    fn missing_bundle_before_native_cleanup_stays_poisoned_for_reinstall() {
        assert_eq!(
            preconfig_cleanup_decision(false),
            PreconfigCleanup::RetryAfterReinstall
        );
    }

    #[test]
    fn runtime_cleanup_never_unlinks_a_live_owner_generation() {
        use std::os::fd::AsRawFd as _;

        let root = std::env::temp_dir().join(format!(
            "talking-quill-removal-live-owner-{}",
            std::process::id()
        ));
        let runtime = root.join("run").join("501.7");
        fs::create_dir_all(&runtime).expect("runtime");
        for name in ["owner-v1.lock", "owner-v1.sock"] {
            File::create(runtime.join(name)).expect("component");
        }
        let maintenance = File::create(runtime.join("maintenance.lock")).expect("maintenance");
        assert_eq!(
            unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        assert!(cleanup_runtime(&runtime).is_err());
        assert!(runtime.join("owner-v1.sock").exists());
        assert!(runtime.join("owner-v1.lock").exists());
        assert!(runtime.join("maintenance.lock").exists());
        assert_eq!(
            unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_UN) },
            0
        );
        drop(maintenance);
        cleanup_runtime(&runtime).expect("cleanup after owner release");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_cleanup_accepts_already_removed_components_and_retries() {
        let owner_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tmp")
            .join(format!("removal-idempotence-{}", std::process::id()));
        let runtime = owner_root.join("run-v1/42");
        let _ = fs::remove_dir_all(&owner_root);
        fs::create_dir_all(&runtime).expect("runtime fixture");
        fs::write(runtime.join("owner-v1.lock"), b"lock").expect("one remaining component");
        cleanup_runtime(&runtime).expect("first cleanup");
        cleanup_runtime(&runtime).expect("idempotent cleanup after NotFound");
        fs::remove_dir_all(owner_root).expect("remove fixture");
    }
}
