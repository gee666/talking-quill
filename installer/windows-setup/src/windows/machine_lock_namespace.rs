//! Production and isolated-test lock namespaces. These names and ACLs are authorization policy.
use super::*;

#[cfg(any(
    not(any(test, feature = "machine-lock-test-namespace")),
    feature = "stale-schema2-cleanup"
))]
pub(super) const MACHINE_LOCK_DIRECTORY_SDDL: &str = "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
pub(super) const MACHINE_LOCK_FILE_SDDL: &str = "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) const TEST_MACHINE_LOCK_DIRECTORY_SDDL: &str =
    "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;AU)";
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) const TEST_MACHINE_LOCK_FILE_SDDL: &str =
    "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;AU)";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_directory_sddl() -> &'static str {
    TEST_MACHINE_LOCK_DIRECTORY_SDDL
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_directory_sddl() -> &'static str {
    MACHINE_LOCK_DIRECTORY_SDDL
}
#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_file_sddl() -> &'static str {
    TEST_MACHINE_LOCK_FILE_SDDL
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_file_sddl() -> &'static str {
    MACHINE_LOCK_FILE_SDDL
}

pub(super) const MACHINE_LOCK_RETIRED_PREFIX: &str = "retired:";
pub(super) const MACHINE_LOCK_REGISTRY_KEY: &str = r"Software\Talking Quill\RecoveryStateLockV1";
pub(super) const MACHINE_LOCK_REGISTRY_VALUE: &str = "DirectorySuffix";
pub(super) const MACHINE_LOCK_DIRECTORY_PREFIX: &str = ".Talking Quill.machine-lock-";
pub(super) const MACHINE_LOCK_PENDING_PREFIX: &str = ".Talking Quill.machine-lock-pending-";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) const MACHINE_LOCK_TEST_ID_ENV: &str = "TQ_MACHINE_LOCK_TEST_NAMESPACE_ID";

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_test_id() -> Result<&'static str> {
    static ID: OnceLock<String> = OnceLock::new();
    let value = ID.get_or_init(|| {
        std::env::var(MACHINE_LOCK_TEST_ID_ENV)
            .expect("machine-lock tests require the wrapper namespace environment")
    });
    validate_machine_lock_suffix(value)?;
    Ok(value)
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_registry_hive() -> HKEY {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_some() {
        HKEY_CURRENT_USER
    } else {
        HKEY_LOCAL_MACHINE
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_registry_hive() -> HKEY {
    HKEY_LOCAL_MACHINE
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_registry_key() -> Result<String> {
    match std::env::var_os(MACHINE_LOCK_TEST_ID_ENV) {
        Some(_) => Ok(format!(
            r"Software\Talking Quill Tests\{}\RecoveryStateLockV1",
            machine_lock_test_id()?
        )),
        None => Ok(MACHINE_LOCK_REGISTRY_KEY.to_owned()),
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_registry_key() -> Result<String> {
    Ok(MACHINE_LOCK_REGISTRY_KEY.to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_registry_parent() -> Result<String> {
    match std::env::var_os(MACHINE_LOCK_TEST_ID_ENV) {
        Some(_) => Ok(format!(
            r"Software\Talking Quill Tests\{}",
            machine_lock_test_id()?
        )),
        None => Ok(r"Software\Talking Quill".to_owned()),
    }
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_registry_parent() -> Result<String> {
    Ok(r"Software\Talking Quill".to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_program_data(production: &Path) -> Result<PathBuf> {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_none() {
        return Ok(production.to_owned());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tmp/machine-lock-tests/windows-setup")
        .join(machine_lock_test_id()?);
    if !root.is_dir() {
        return Err(fail(EXIT_FAILURE, "machine-lock test outer root is absent"));
    }
    Ok(root)
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_program_data(production: &Path) -> Result<PathBuf> {
    Ok(production.to_owned())
}

#[cfg(any(test, feature = "machine-lock-test-namespace"))]
pub(super) fn machine_lock_mutex_names() -> Result<[String; 2]> {
    if std::env::var_os(MACHINE_LOCK_TEST_ID_ENV).is_none() {
        return Ok([
            r"Global\TalkingQuill.NativeSetup.V2".to_owned(),
            r"Global\TalkingQuill.UpdateRecovery.State.V1".to_owned(),
        ]);
    }
    let id = machine_lock_test_id()?;
    Ok([
        format!(r"Local\TalkingQuill.Tests.{id}.NativeSetup.V2"),
        format!(r"Local\TalkingQuill.Tests.{id}.UpdateRecovery.State.V1"),
    ])
}
#[cfg(not(any(test, feature = "machine-lock-test-namespace")))]
pub(super) fn machine_lock_mutex_names() -> Result<[String; 2]> {
    Ok([
        r"Global\TalkingQuill.NativeSetup.V2".to_owned(),
        r"Global\TalkingQuill.UpdateRecovery.State.V1".to_owned(),
    ])
}
