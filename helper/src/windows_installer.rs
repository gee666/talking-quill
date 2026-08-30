#![cfg(windows)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_ProgramData, FOLDERID_ProgramFiles, SHGetKnownFolderPath,
};

const EXIT_INVALID: i32 = 64;
const EXIT_REFUSED: i32 = 78;
const EXIT_FAILED: i32 = 79;
const LEGACY_SERVICE: &str = "TalkingQuillKeyboardAuthority";
const LEGACY_TASK: &str = "TalkingQuillKeyboardAuthority";

#[derive(Clone)]
struct LifecyclePaths {
    install: PathBuf,
    backup: PathBuf,
    orphan: PathBuf,
    transaction: PathBuf,
    legacy_authority: PathBuf,
    legacy_authority_quarantine: PathBuf,
    legacy_task_file: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailurePoint {
    None,
    AfterFreshStagingRecord,
    AfterStagingRecord,
    AfterPredecessorRename,
    AfterRestoringRecord,
    AfterPartialRemoval,
    AfterAuthoritySnapshot,
    AfterTaskDisable,
    AfterTaskStop,
    AfterServiceDisable,
    AfterServiceStop,
    AfterTaskFilePreserved,
    AfterAuthorityQuarantine,
    AfterDurableCommit,
    AfterServiceDelete,
    AfterTaskDelete,
    AfterTaskFileDelete,
    AfterAuthorityQuarantineDelete,
    AfterAuthorityRollbackRecord,
    AfterAuthorityRollbackNeutral,
    AfterPredecessorFilesRestored,
    AfterServiceActivityRestored,
    AfterTaskActivityRestored,
}

#[derive(Clone)]
struct ObservedProcess {
    pid: u32,
    name: String,
    path: Result<PathBuf, ()>,
}

trait MachineAdapter {
    fn processes(&mut self) -> Result<Vec<ObservedProcess>, i32>;
    fn pause(&mut self, duration: Duration);
    fn service_exists(&mut self) -> Result<bool, i32>;
    fn service_is_running(&mut self) -> Result<bool, i32>;
    fn service_definition(&mut self) -> Result<String, i32>;
    fn restore_service(&mut self, definition: &str) -> Result<(), i32>;
    fn service_start_mode(&mut self) -> Result<ServiceStartMode, i32>;
    fn set_service_start_mode(&mut self, mode: ServiceStartMode) -> Result<(), i32>;
    fn stop_service(&mut self) -> Result<(), i32>;
    fn start_service(&mut self) -> Result<(), i32>;
    fn delete_service(&mut self) -> Result<(), i32>;
    fn task_exists(&mut self) -> Result<bool, i32>;
    fn task_is_enabled(&mut self) -> Result<bool, i32>;
    fn task_is_running(&mut self) -> Result<bool, i32>;
    fn task_definition(&mut self) -> Result<String, i32>;
    fn restore_task(&mut self, definition: &str) -> Result<(), i32>;
    fn set_task_enabled(&mut self, enabled: bool) -> Result<(), i32>;
    fn stop_task(&mut self) -> Result<(), i32>;
    fn start_task(&mut self) -> Result<(), i32>;
    fn delete_task(&mut self) -> Result<(), i32>;
}

struct WindowsMachine;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ServiceStartMode {
    Auto,
    #[default]
    Demand,
    Disabled,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacySnapshot {
    service_existed: bool,
    service_was_running: bool,
    service_start_mode: Option<ServiceStartMode>,
    service_definition: Option<String>,
    task_existed: bool,
    task_was_enabled: bool,
    task_was_running: bool,
    task_definition: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Transaction {
    schema_version: u8,
    state: String,
    had_predecessor: bool,
    #[serde(default)]
    fresh_install: bool,
    #[serde(default)]
    repair: bool,
    #[serde(default)]
    legacy_snapshot: Option<LegacySnapshot>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    schema_version: u16,
    kind: String,
    version: String,
    platform: String,
    architecture: String,
    owner_mode: String,
    package_mode: String,
    roles: Vec<Role>,
    predecessor: Option<Predecessor>,
    fresh_install: Option<bool>,
    release_build_digest: String,
    package_layout_digest: String,
    update: serde_json::Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Predecessor {
    platform: String,
    architecture: String,
    version: String,
    release_build_digest: String,
    gateway_sha256: String,
    owner_sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Role {
    role: String,
    path: String,
    sha256: String,
    suppression_capable: bool,
}

pub fn run(arguments: &[std::ffi::OsString]) -> i32 {
    run_inner(arguments).unwrap_or_else(|code| code)
}

fn run_inner(arguments: &[std::ffi::OsString]) -> Result<i32, i32> {
    let mode = arguments
        .first()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix("--windows-installer-lifecycle-v1="))
        .ok_or(EXIT_INVALID)?;
    let expected_gateway = argument(arguments, "--expected-gateway=");
    let expected_owner = argument(arguments, "--expected-owner=");
    let expected_layout = argument(arguments, "--expected-layout=");
    let paths = production_paths()?;
    assert_plain_state(&paths)?;
    let mut machine = WindowsMachine;
    execute_lifecycle(
        mode,
        &paths,
        expected_gateway,
        expected_owner,
        expected_layout,
        FailurePoint::None,
        &mut machine,
    )?;
    Ok(0)
}

fn production_paths() -> Result<LifecyclePaths, i32> {
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let system = system_directory()?;
    Ok(LifecyclePaths {
        install: program_files.join("Talking Quill"),
        backup: program_files.join(".Talking Quill.stage1-backup"),
        orphan: program_files.join(".Talking Quill.stage1-ambiguous-replacement"),
        transaction: program_files.join(".Talking Quill.stage1-transaction.json"),
        legacy_authority: program_data.join("Talking Quill/KeyboardAuthority"),
        legacy_authority_quarantine: program_data
            .join("Talking Quill/.KeyboardAuthority.retirement-quarantine"),
        legacy_task_file: system.join(format!("Tasks/{LEGACY_TASK}")),
    })
}

fn execute_lifecycle(
    mode: &str,
    paths: &LifecyclePaths,
    expected_gateway: &str,
    expected_owner: &str,
    expected_layout: &str,
    failure: FailurePoint,
    machine: &mut impl MachineAdapter,
) -> Result<(), i32> {
    if matches!(mode, "install" | "uninstall") {
        wait_for_planned_runtime_exit(&paths.install, Duration::from_secs(30), machine, false)?;
    }
    match mode {
        "fresh-install" => {
            if paths.transaction.exists() {
                let pending = read_transaction(&paths.transaction)?;
                if !pending.fresh_install || pending.had_predecessor {
                    return Err(EXIT_REFUSED);
                }
                resolve_pending(paths, false, failure, machine)?;
            }
            if paths.install.exists()
                || paths.backup.exists()
                || paths.orphan.exists()
                || paths.legacy_authority.exists()
                || paths.legacy_authority_quarantine.exists()
                || paths.legacy_task_file.exists()
                || machine.service_exists()?
                || machine.task_exists()?
            {
                return Err(EXIT_REFUSED);
            }
            write_transaction(&paths.transaction, "staging", false, true, false)?;
            fail_at(failure, FailurePoint::AfterFreshStagingRecord)?;
            write_transaction(&paths.transaction, "prepared", false, true, false)?;
        }
        "install" | "repair" => {
            let repair = mode == "repair";
            resolve_pending(paths, false, failure, machine)?;
            if !paths.install.exists() {
                return Err(EXIT_REFUSED);
            }
            write_transaction(&paths.transaction, "staging", true, false, repair)?;
            fail_at(failure, FailurePoint::AfterStagingRecord)?;
            fs::rename(&paths.install, &paths.backup).map_err(|_| EXIT_FAILED)?;
            fail_at(failure, FailurePoint::AfterPredecessorRename)?;
            write_transaction(&paths.transaction, "prepared", true, false, repair)?;
        }
        "install-rollback" => resolve_pending(paths, true, failure, machine)?,
        "install-commit" => {
            let current = read_transaction(&paths.transaction)?;
            if !paths.install.is_dir() {
                return Err(EXIT_REFUSED);
            }
            if current.state == "committed" {
                finish_legacy_authority_retirement(paths, machine, failure)?;
                prove_legacy_authority_absent(paths, machine)?;
                return Ok(());
            }
            if !matches!(current.state.as_str(), "prepared" | "authority-preparing") {
                return Err(EXIT_REFUSED);
            }
            verify_candidate(
                &paths.install,
                &paths.backup,
                expected_gateway,
                expected_owner,
                expected_layout,
                current.fresh_install,
                current.repair,
            )?;
            let snapshot = prepare_legacy_authority_retirement(paths, machine, &current, failure)?;
            prove_legacy_authority_quiesced(paths, machine, &snapshot)?;
            wait_for_planned_runtime_exit(&paths.install, Duration::from_secs(30), machine, true)?;
            write_transaction_with_snapshot(
                &paths.transaction,
                "committed",
                current.had_predecessor,
                current.fresh_install,
                current.repair,
                Some(snapshot),
            )?;
            fail_at(failure, FailurePoint::AfterDurableCommit)?;
            finish_legacy_authority_retirement(paths, machine, failure)?;
            prove_legacy_authority_absent(paths, machine)?;
        }
        "install-retire-backup" => {
            let current = read_transaction(&paths.transaction)?;
            if current.state != "committed" || !paths.install.is_dir() {
                return Err(EXIT_REFUSED);
            }
            resolve_pending(paths, false, failure, machine)?;
        }
        "uninstall" => {
            resolve_pending(paths, false, failure, machine)?;
            retire_legacy_authority_after_commit(paths, machine, failure)?;
            if paths.backup.exists() || paths.transaction.exists() {
                return Err(EXIT_REFUSED);
            }
            remove_plain_tree(&paths.install)?;
            remove_plain_tree(&paths.orphan)?;
        }
        _ => return Err(EXIT_INVALID),
    }
    Ok(())
}

fn resolve_pending(
    paths: &LifecyclePaths,
    preserve_committed: bool,
    failure: FailurePoint,
    machine: &mut impl MachineAdapter,
) -> Result<(), i32> {
    if !paths.transaction.exists() {
        if paths.backup.exists() {
            if !paths.install.exists() {
                fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
            } else {
                if paths.orphan.exists() {
                    return Err(EXIT_REFUSED);
                }
                fs::rename(&paths.install, &paths.orphan).map_err(|_| EXIT_FAILED)?;
                fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
            }
        }
        return Ok(());
    }
    let current = read_transaction(&paths.transaction)?;
    match current.state.as_str() {
        "staging" => {
            if current.fresh_install && !current.had_predecessor {
                if paths.backup.exists() {
                    return Err(EXIT_REFUSED);
                }
                write_transaction(&paths.transaction, "restoring", false, true, current.repair)?;
                fail_at(failure, FailurePoint::AfterRestoringRecord)?;
                remove_plain_tree(&paths.install)?;
                fail_at(failure, FailurePoint::AfterPartialRemoval)?;
                return remove_transaction(&paths.transaction);
            }
            if !current.had_predecessor || current.fresh_install {
                return Err(EXIT_REFUSED);
            }
            match (paths.install.exists(), paths.backup.exists()) {
                (true, false) => remove_transaction(&paths.transaction),
                (false, true) => {
                    fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
                    remove_transaction(&paths.transaction)
                }
                _ => Err(EXIT_REFUSED),
            }
        }
        "restoring" => {
            if current.fresh_install && !current.had_predecessor {
                if paths.backup.exists() {
                    return Err(EXIT_REFUSED);
                }
                remove_plain_tree(&paths.install)?;
                fail_at(failure, FailurePoint::AfterPartialRemoval)?;
                return remove_transaction(&paths.transaction);
            }
            if !current.had_predecessor || current.fresh_install || !paths.backup.is_dir() {
                return Err(EXIT_REFUSED);
            }
            remove_plain_tree(&paths.install)?;
            fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
            remove_transaction(&paths.transaction)
        }
        "committed" => {
            if !paths.install.is_dir() {
                return Err(EXIT_REFUSED);
            }
            finish_legacy_authority_retirement(paths, machine, failure)?;
            prove_legacy_authority_absent(paths, machine)?;
            if preserve_committed {
                return Ok(());
            }
            remove_plain_tree(&paths.backup)?;
            remove_plain_tree(&paths.orphan)?;
            remove_transaction(&paths.transaction)
        }
        "authority-preparing" => {
            let snapshot = current.legacy_snapshot.clone().ok_or(EXIT_REFUSED)?;
            write_transaction_with_snapshot(
                &paths.transaction,
                "authority-rollback",
                current.had_predecessor,
                current.fresh_install,
                current.repair,
                Some(snapshot),
            )?;
            fail_at(failure, FailurePoint::AfterAuthorityRollbackRecord)?;
            recover_authority_rollback(
                paths,
                machine,
                &read_transaction(&paths.transaction)?,
                failure,
            )
        }
        "authority-rollback" => recover_authority_rollback(paths, machine, &current, failure),
        "prepared" => {
            if current.had_predecessor {
                if !paths.backup.is_dir() {
                    return Err(EXIT_REFUSED);
                }
                write_transaction(&paths.transaction, "restoring", true, false, current.repair)?;
                fail_at(failure, FailurePoint::AfterRestoringRecord)?;
                remove_plain_tree(&paths.install)?;
                fail_at(failure, FailurePoint::AfterPartialRemoval)?;
                fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
            } else {
                if paths.backup.exists() || !current.fresh_install {
                    return Err(EXIT_REFUSED);
                }
                write_transaction(&paths.transaction, "restoring", false, true, current.repair)?;
                fail_at(failure, FailurePoint::AfterRestoringRecord)?;
                remove_plain_tree(&paths.install)?;
                fail_at(failure, FailurePoint::AfterPartialRemoval)?;
            }
            remove_transaction(&paths.transaction)
        }
        _ => Err(EXIT_REFUSED),
    }
}

fn recover_authority_rollback(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
    current: &Transaction,
    failure: FailurePoint,
) -> Result<(), i32> {
    let snapshot = current.legacy_snapshot.as_ref().ok_or(EXIT_REFUSED)?;
    quiesce_legacy_registrations(machine)?;
    wait_for_planned_runtime_exit(&paths.install, Duration::from_secs(30), machine, true)?;
    fail_at(failure, FailurePoint::AfterAuthorityRollbackNeutral)?;
    restore_predecessor_files(paths, current)?;
    fail_at(failure, FailurePoint::AfterPredecessorFilesRestored)?;
    restore_quarantined_tree(&paths.legacy_authority_quarantine, &paths.legacy_authority)?;
    restore_legacy_activity(machine, snapshot, failure)?;
    remove_transaction(&paths.transaction)
}

fn restore_predecessor_files(paths: &LifecyclePaths, current: &Transaction) -> Result<(), i32> {
    if current.had_predecessor {
        if paths.backup.exists() {
            if !paths.backup.is_dir() {
                return Err(EXIT_REFUSED);
            }
            remove_plain_tree(&paths.install)?;
            fs::rename(&paths.backup, &paths.install).map_err(|_| EXIT_FAILED)?;
        } else if !paths.install.is_dir() {
            return Err(EXIT_REFUSED);
        }
    } else {
        if paths.backup.exists() || !current.fresh_install {
            return Err(EXIT_REFUSED);
        }
        remove_plain_tree(&paths.install)?;
    }
    Ok(())
}

fn fail_at(actual: FailurePoint, expected: FailurePoint) -> Result<(), i32> {
    if actual == expected {
        Err(EXIT_FAILED)
    } else {
        Ok(())
    }
}

fn argument<'a>(arguments: &'a [std::ffi::OsString], prefix: &str) -> &'a str {
    arguments
        .iter()
        .filter_map(|value| value.to_str())
        .find_map(|value| value.strip_prefix(prefix))
        .unwrap_or("")
}

fn assert_plain_state(paths: &LifecyclePaths) -> Result<(), i32> {
    for tree in [
        &paths.install,
        &paths.backup,
        &paths.orphan,
        &paths.legacy_authority,
        &paths.legacy_authority_quarantine,
    ] {
        assert_plain_tree(tree)?;
    }
    assert_plain_file(&paths.transaction)?;
    assert_plain_file(&paths.legacy_task_file)
}

fn assert_plain_tree(path: &Path) -> Result<(), i32> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| EXIT_REFUSED)?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_dir() {
        return Err(EXIT_REFUSED);
    }
    for entry in fs::read_dir(path).map_err(|_| EXIT_REFUSED)? {
        let entry = entry.map_err(|_| EXIT_REFUSED)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|_| EXIT_REFUSED)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(EXIT_REFUSED);
        }
        if metadata.is_dir() {
            assert_plain_tree(&child)?;
        }
    }
    Ok(())
}

fn assert_plain_file(path: &Path) -> Result<(), i32> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| EXIT_REFUSED)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(EXIT_REFUSED);
    }
    Ok(())
}

fn remove_plain_tree(path: &Path) -> Result<(), i32> {
    if !path.exists() {
        return Ok(());
    }
    assert_plain_tree(path)?;
    fs::remove_dir_all(path).map_err(|_| EXIT_FAILED)
}

fn write_transaction(
    path: &Path,
    state: &str,
    had_predecessor: bool,
    fresh_install: bool,
    repair: bool,
) -> Result<(), i32> {
    write_transaction_with_snapshot(path, state, had_predecessor, fresh_install, repair, None)
}

fn write_transaction_with_snapshot(
    path: &Path,
    state: &str,
    had_predecessor: bool,
    fresh_install: bool,
    repair: bool,
    legacy_snapshot: Option<LegacySnapshot>,
) -> Result<(), i32> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec(&Transaction {
        schema_version: 1,
        state: state.into(),
        had_predecessor,
        fresh_install,
        repair,
        legacy_snapshot,
    })
    .map_err(|_| EXIT_FAILED)?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| EXIT_FAILED)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| EXIT_FAILED)?;
        move_replace(&temporary, path)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn move_replace(from: &Path, to: &Path) -> Result<(), i32> {
    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain([0]).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain([0]).collect();
    if unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(EXIT_FAILED);
    }
    Ok(())
}

fn read_transaction(path: &Path) -> Result<Transaction, i32> {
    assert_plain_file(path)?;
    let value: Transaction = serde_json::from_slice(&fs::read(path).map_err(|_| EXIT_REFUSED)?)
        .map_err(|_| EXIT_REFUSED)?;
    if value.schema_version != 1
        || !matches!(
            value.state.as_str(),
            "staging"
                | "prepared"
                | "authority-preparing"
                | "authority-rollback"
                | "restoring"
                | "committed"
        )
    {
        return Err(EXIT_REFUSED);
    }
    Ok(value)
}

fn remove_transaction(path: &Path) -> Result<(), i32> {
    if path.exists() {
        fs::remove_file(path).map_err(|_| EXIT_FAILED)?;
    }
    Ok(())
}

fn wait_for_planned_runtime_exit(
    install: &Path,
    timeout: Duration,
    machine: &mut impl MachineAdapter,
    include_legacy_authority: bool,
) -> Result<(), i32> {
    let deadline = Instant::now() + timeout;
    loop {
        if !runtime_process_active(install, &machine.processes()?, include_legacy_authority)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(EXIT_REFUSED);
        }
        machine.pause(Duration::from_millis(100));
    }
}

fn runtime_process_active(
    install: &Path,
    processes: &[ObservedProcess],
    include_legacy_authority: bool,
) -> Result<bool, i32> {
    let expected = [
        install.join("Talking Quill.exe"),
        install.join("resources/helper/talking-quill-helper.exe"),
        install.join("resources/helper/talking-quill-keyboard-owner.exe"),
    ];
    for process in processes {
        if process.pid == std::process::id() {
            continue;
        }
        let name = process.name.to_ascii_lowercase();
        let legacy_authority = matches!(
            name.as_str(),
            "talking-quill-windows-keyboard-authority.exe"
                | "talking-quill-keyboard-authority.exe"
                | "talkingquillkeyboardauthority.exe"
        );
        if legacy_authority && !include_legacy_authority {
            continue;
        }
        if !legacy_authority
            && !matches!(
                name.as_str(),
                "talking quill.exe"
                    | "talking-quill-helper.exe"
                    | "talking-quill-keyboard-owner.exe"
            )
        {
            continue;
        }
        let path = match &process.path {
            Ok(path) => path,
            Err(()) => return Ok(true),
        };
        let observed = canonical_windows_path(path)?;
        for candidate in &expected {
            if candidate.exists() && canonical_windows_path(candidate)? == observed {
                return Ok(true);
            }
        }
        // A recognized runtime at any other path may still hold legacy keyboard
        // authority. Do not publish a successor while that process is alive.
        return Ok(true);
    }
    Ok(false)
}

use std::os::windows::io::AsRawHandle;

fn utf16_nul(value: &[u16]) -> String {
    let length = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..length])
}

fn canonical_windows_path(path: &Path) -> Result<String, i32> {
    let canonical = fs::canonicalize(path).map_err(|_| EXIT_REFUSED)?;
    let value = canonical.to_string_lossy();
    Ok(value
        .strip_prefix(r"\\?\")
        .unwrap_or(&value)
        .replace('/', "\\")
        .to_lowercase())
}

fn prepare_legacy_authority_retirement(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
    current: &Transaction,
    failure: FailurePoint,
) -> Result<LegacySnapshot, i32> {
    let snapshot = if current.state == "prepared" {
        let service_existed = machine.service_exists()?;
        let task_existed = machine.task_exists()?;
        let snapshot = LegacySnapshot {
            service_existed,
            service_was_running: service_existed && machine.service_is_running()?,
            service_start_mode: if service_existed {
                Some(machine.service_start_mode()?)
            } else {
                None
            },
            service_definition: if service_existed {
                Some(machine.service_definition()?)
            } else {
                None
            },
            task_existed,
            task_was_enabled: task_existed && machine.task_is_enabled()?,
            task_was_running: task_existed && machine.task_is_running()?,
            task_definition: if task_existed {
                Some(machine.task_definition()?)
            } else {
                None
            },
        };
        write_transaction_with_snapshot(
            &paths.transaction,
            "authority-preparing",
            current.had_predecessor,
            current.fresh_install,
            current.repair,
            Some(snapshot.clone()),
        )?;
        fail_at(failure, FailurePoint::AfterAuthoritySnapshot)?;
        snapshot
    } else {
        current.legacy_snapshot.clone().ok_or(EXIT_REFUSED)?
    };

    if snapshot.task_existed {
        if !machine.task_exists()? {
            return Err(EXIT_REFUSED);
        }
        machine.set_task_enabled(false)?;
        fail_at(failure, FailurePoint::AfterTaskDisable)?;
        if machine.task_is_running()? {
            machine.stop_task()?;
        }
        fail_at(failure, FailurePoint::AfterTaskStop)?;
    }
    if snapshot.service_existed {
        if !machine.service_exists()? {
            return Err(EXIT_REFUSED);
        }
        machine.set_service_start_mode(ServiceStartMode::Disabled)?;
        fail_at(failure, FailurePoint::AfterServiceDisable)?;
        if machine.service_is_running()? {
            machine.stop_service()?;
        }
        fail_at(failure, FailurePoint::AfterServiceStop)?;
    }
    assert_plain_file(&paths.legacy_task_file)?;
    fail_at(failure, FailurePoint::AfterTaskFilePreserved)?;
    quarantine_tree(&paths.legacy_authority, &paths.legacy_authority_quarantine)?;
    fail_at(failure, FailurePoint::AfterAuthorityQuarantine)?;
    Ok(snapshot)
}

fn quiesce_legacy_registrations(machine: &mut impl MachineAdapter) -> Result<(), i32> {
    if machine.task_exists()? {
        machine.set_task_enabled(false)?;
        if machine.task_is_running()? {
            machine.stop_task()?;
        }
    }
    if machine.service_exists()? {
        machine.set_service_start_mode(ServiceStartMode::Disabled)?;
        if machine.service_is_running()? {
            machine.stop_service()?;
        }
    }
    Ok(())
}

fn restore_legacy_activity(
    machine: &mut impl MachineAdapter,
    snapshot: &LegacySnapshot,
    failure: FailurePoint,
) -> Result<(), i32> {
    let service_definition = snapshot.service_definition.as_deref();
    if machine.service_exists()?
        && (service_definition.is_none()
            || machine.service_definition()? != service_definition.ok_or(EXIT_REFUSED)?)
    {
        retire_service_registration(machine)?;
    }
    if snapshot.service_existed && !machine.service_exists()? {
        machine.restore_service(service_definition.ok_or(EXIT_REFUSED)?)?;
    }
    if snapshot.service_existed {
        machine.set_service_start_mode(snapshot.service_start_mode.ok_or(EXIT_REFUSED)?)?;
        if snapshot.service_was_running && !machine.service_is_running()? {
            machine.start_service()?;
        }
    }
    fail_at(failure, FailurePoint::AfterServiceActivityRestored)?;

    let task_definition = snapshot.task_definition.as_deref();
    if machine.task_exists()?
        && (task_definition.is_none()
            || machine.task_definition()? != task_definition.ok_or(EXIT_REFUSED)?)
    {
        retire_task_registration(machine)?;
    }
    if snapshot.task_existed && !machine.task_exists()? {
        machine.restore_task(task_definition.ok_or(EXIT_REFUSED)?)?;
    }
    if snapshot.task_existed {
        machine.set_task_enabled(snapshot.task_was_enabled)?;
        if snapshot.task_was_running && !machine.task_is_running()? {
            machine.start_task()?;
        }
    }
    fail_at(failure, FailurePoint::AfterTaskActivityRestored)?;
    Ok(())
}

fn retire_service_registration(machine: &mut impl MachineAdapter) -> Result<(), i32> {
    machine.set_service_start_mode(ServiceStartMode::Disabled)?;
    if machine.service_is_running()? {
        machine.stop_service()?;
    }
    machine.delete_service()?;
    wait_until_absent(machine, true)
}

fn retire_task_registration(machine: &mut impl MachineAdapter) -> Result<(), i32> {
    machine.set_task_enabled(false)?;
    if machine.task_is_running()? {
        machine.stop_task()?;
    }
    machine.delete_task()?;
    wait_until_absent(machine, false)
}

fn finish_legacy_authority_retirement(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
    failure: FailurePoint,
) -> Result<(), i32> {
    if machine.service_exists()? {
        retire_service_registration(machine)?;
    }
    fail_at(failure, FailurePoint::AfterServiceDelete)?;
    if machine.task_exists()? {
        retire_task_registration(machine)?;
    }
    fail_at(failure, FailurePoint::AfterTaskDelete)?;
    remove_plain_file(&paths.legacy_task_file)?;
    fail_at(failure, FailurePoint::AfterTaskFileDelete)?;
    remove_plain_tree(&paths.legacy_authority)?;
    remove_plain_tree(&paths.legacy_authority_quarantine)?;
    fail_at(failure, FailurePoint::AfterAuthorityQuarantineDelete)?;
    wait_for_planned_runtime_exit(&paths.install, Duration::from_secs(30), machine, true)
}

fn prove_legacy_authority_quiesced(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
    snapshot: &LegacySnapshot,
) -> Result<(), i32> {
    let service_exists = machine.service_exists()?;
    let task_exists = machine.task_exists()?;
    if service_exists != snapshot.service_existed
        || task_exists != snapshot.task_existed
        || (service_exists
            && (machine.service_start_mode()? != ServiceStartMode::Disabled
                || machine.service_is_running()?))
        || (task_exists && (machine.task_is_enabled()? || machine.task_is_running()?))
        || paths.legacy_authority.exists()
    {
        return Err(EXIT_FAILED);
    }
    assert_plain_tree(&paths.legacy_authority_quarantine)?;
    assert_plain_file(&paths.legacy_task_file)
}

fn retire_legacy_authority_after_commit(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
    failure: FailurePoint,
) -> Result<(), i32> {
    if machine.task_exists()? {
        machine.set_task_enabled(false)?;
        if machine.task_is_running()? {
            machine.stop_task()?;
        }
    }
    if machine.service_exists()? {
        machine.set_service_start_mode(ServiceStartMode::Disabled)?;
        if machine.service_is_running()? {
            machine.stop_service()?;
        }
    }
    finish_legacy_authority_retirement(paths, machine, failure)?;
    prove_legacy_authority_absent(paths, machine)
}

fn prove_legacy_authority_absent(
    paths: &LifecyclePaths,
    machine: &mut impl MachineAdapter,
) -> Result<(), i32> {
    if machine.service_exists()?
        || machine.task_exists()?
        || paths.legacy_task_file.exists()
        || paths.legacy_authority.exists()
        || paths.legacy_authority_quarantine.exists()
    {
        return Err(EXIT_FAILED);
    }
    Ok(())
}

fn quarantine_tree(source: &Path, quarantine: &Path) -> Result<(), i32> {
    if source.exists() && quarantine.exists() {
        return Err(EXIT_REFUSED);
    }
    if source.exists() {
        assert_plain_tree(source)?;
        fs::rename(source, quarantine).map_err(|_| EXIT_FAILED)?;
    }
    assert_plain_tree(quarantine)
}

fn restore_quarantined_tree(quarantine: &Path, source: &Path) -> Result<(), i32> {
    if quarantine.exists() && source.exists() {
        return Err(EXIT_REFUSED);
    }
    if quarantine.exists() {
        assert_plain_tree(quarantine)?;
        fs::rename(quarantine, source).map_err(|_| EXIT_FAILED)?;
    }
    Ok(())
}

fn remove_plain_file(path: &Path) -> Result<(), i32> {
    if path.exists() {
        assert_plain_file(path)?;
        fs::remove_file(path).map_err(|_| EXIT_FAILED)?;
    }
    Ok(())
}

fn wait_until_absent(machine: &mut impl MachineAdapter, service: bool) -> Result<(), i32> {
    for _ in 0..300 {
        let exists = if service {
            machine.service_exists()?
        } else {
            machine.task_exists()?
        };
        if !exists {
            return Ok(());
        }
        machine.pause(Duration::from_millis(100));
    }
    Err(EXIT_FAILED)
}

impl MachineAdapter for WindowsMachine {
    fn processes(&mut self) -> Result<Vec<ObservedProcess>, i32> {
        enumerate_processes()
    }

    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn service_exists(&mut self) -> Result<bool, i32> {
        service_exists()
    }

    fn service_is_running(&mut self) -> Result<bool, i32> {
        service_is_running()
    }

    fn service_definition(&mut self) -> Result<String, i32> {
        const QUERY: &str = "$ErrorActionPreference='Stop';$s=Get-CimInstance Win32_Service -Filter \"Name='TalkingQuillKeyboardAuthority'\";if($null-eq$s){exit 1060};$d=@((Get-Service 'TalkingQuillKeyboardAuthority').ServicesDependedOn|ForEach-Object{$_.Name});$o=[ordered]@{path=[string]$s.PathName;account=[string]$s.StartName;display=[string]$s.DisplayName;dependencies=$d};$j=$o|ConvertTo-Json -Compress;[Console]::Out.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($j)))";
        powershell_definition(QUERY)
    }

    fn restore_service(&mut self, definition: &str) -> Result<(), i32> {
        validate_definition(definition)?;
        let script = format!(
            "$ErrorActionPreference='Stop';$o=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{definition}'))|ConvertFrom-Json;$a=@('create','{LEGACY_SERVICE}','binPath=',$o.path,'start=','disabled','obj=',$o.account,'DisplayName=',$o.display);if($o.dependencies.Count-gt 0){{$a+=@('depend=',($o.dependencies-join'/'))}};& $env:SystemRoot\\System32\\sc.exe @a;exit $LASTEXITCODE"
        );
        if powershell_status(&script)? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn service_start_mode(&mut self) -> Result<ServiceStartMode, i32> {
        service_start_mode()
    }

    fn set_service_start_mode(&mut self, mode: ServiceStartMode) -> Result<(), i32> {
        let mode = match mode {
            ServiceStartMode::Auto => "auto",
            ServiceStartMode::Demand => "demand",
            ServiceStartMode::Disabled => "disabled",
        };
        if system_command("sc.exe", &["config", LEGACY_SERVICE, "start=", mode])? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn stop_service(&mut self) -> Result<(), i32> {
        for _ in 0..300 {
            if !service_exists()? || !service_is_running()? {
                return Ok(());
            }
            let status = system_command("sc.exe", &["stop", LEGACY_SERVICE])?;
            if !matches!(status, 0 | 1052 | 1061 | 1062) {
                return Err(EXIT_FAILED);
            }
            self.pause(Duration::from_millis(100));
        }
        Err(EXIT_FAILED)
    }

    fn start_service(&mut self) -> Result<(), i32> {
        if matches!(
            system_command("sc.exe", &["start", LEGACY_SERVICE])?,
            0 | 1056
        ) {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn delete_service(&mut self) -> Result<(), i32> {
        if matches!(
            system_command("sc.exe", &["delete", LEGACY_SERVICE])?,
            0 | 1072
        ) {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn task_exists(&mut self) -> Result<bool, i32> {
        task_exists()
    }

    fn task_is_enabled(&mut self) -> Result<bool, i32> {
        task_property("Enabled")
    }

    fn task_is_running(&mut self) -> Result<bool, i32> {
        task_property("Running")
    }

    fn task_definition(&mut self) -> Result<String, i32> {
        const QUERY: &str = "$ErrorActionPreference='Stop';$s=New-Object -ComObject Schedule.Service;$s.Connect();$t=$s.GetFolder('\\').GetTask('TalkingQuillKeyboardAuthority');[Console]::Out.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($t.Xml)))";
        powershell_definition(QUERY)
    }

    fn restore_task(&mut self, definition: &str) -> Result<(), i32> {
        validate_definition(definition)?;
        let script = format!(
            "$ErrorActionPreference='Stop';$x=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{definition}'));$s=New-Object -ComObject Schedule.Service;$s.Connect();$null=$s.GetFolder('\\').RegisterTask('{LEGACY_TASK}',$x,6,$null,$null,0,$null);exit 0"
        );
        if powershell_status(&script)? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn set_task_enabled(&mut self, enabled: bool) -> Result<(), i32> {
        let action = if enabled { "/Enable" } else { "/Disable" };
        if system_command("schtasks.exe", &["/Change", "/TN", LEGACY_TASK, action])? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn stop_task(&mut self) -> Result<(), i32> {
        for _ in 0..300 {
            if !task_exists()? || !task_property("Running")? {
                return Ok(());
            }
            let _ = system_command("schtasks.exe", &["/End", "/TN", LEGACY_TASK])?;
            self.pause(Duration::from_millis(100));
        }
        Err(EXIT_FAILED)
    }

    fn start_task(&mut self) -> Result<(), i32> {
        if system_command("schtasks.exe", &["/Run", "/TN", LEGACY_TASK])? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }

    fn delete_task(&mut self) -> Result<(), i32> {
        if system_command("schtasks.exe", &["/Delete", "/TN", LEGACY_TASK, "/F"])? == 0 {
            Ok(())
        } else {
            Err(EXIT_FAILED)
        }
    }
}

fn enumerate_processes() -> Result<Vec<ObservedProcess>, i32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(EXIT_FAILED);
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.cast()) };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut present = unsafe { Process32FirstW(snapshot.as_raw_handle().cast(), &mut entry) } != 0;
    let mut processes = Vec::new();
    while present {
        let name = utf16_nul(&entry.szExeFile);
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "talking quill.exe"
                | "talking-quill-helper.exe"
                | "talking-quill-keyboard-owner.exe"
                | "talking-quill-windows-keyboard-authority.exe"
                | "talking-quill-keyboard-authority.exe"
                | "talkingquillkeyboardauthority.exe"
        ) {
            let process =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
            let path = if process.is_null() {
                Err(())
            } else {
                let mut buffer = vec![0_u16; 32_768];
                let mut length = buffer.len() as u32;
                let queried = unsafe {
                    QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length)
                } != 0;
                unsafe { CloseHandle(process) };
                if queried {
                    Ok(PathBuf::from(String::from_utf16_lossy(
                        &buffer[..length as usize],
                    )))
                } else {
                    Err(())
                }
            };
            processes.push(ObservedProcess {
                pid: entry.th32ProcessID,
                name,
                path,
            });
        }
        present = unsafe { Process32NextW(snapshot.as_raw_handle().cast(), &mut entry) } != 0;
    }
    Ok(processes)
}

fn service_exists() -> Result<bool, i32> {
    match system_command("sc.exe", &["query", LEGACY_SERVICE])? {
        0 => Ok(true),
        1060 => Ok(false),
        _ => Err(EXIT_FAILED),
    }
}

fn service_is_running() -> Result<bool, i32> {
    let output = system_command_output("sc.exe", &["query", LEGACY_SERVICE])?;
    if output.status.code() != Some(0) {
        return Err(EXIT_FAILED);
    }
    Ok(!String::from_utf8_lossy(&output.stdout).contains("STOPPED"))
}

fn service_start_mode() -> Result<ServiceStartMode, i32> {
    let output = system_command_output("sc.exe", &["qc", LEGACY_SERVICE])?;
    if output.status.code() != Some(0) {
        return Err(EXIT_FAILED);
    }
    let output = String::from_utf8_lossy(&output.stdout);
    if output.contains("AUTO_START") {
        Ok(ServiceStartMode::Auto)
    } else if output.contains("DEMAND_START") {
        Ok(ServiceStartMode::Demand)
    } else if output.contains("DISABLED") {
        Ok(ServiceStartMode::Disabled)
    } else {
        Err(EXIT_FAILED)
    }
}

fn task_exists() -> Result<bool, i32> {
    const QUERY: &str = "$ErrorActionPreference='Stop';try{$s=New-Object -ComObject Schedule.Service;$s.Connect();$null=$s.GetFolder('\\').GetTask('TalkingQuillKeyboardAuthority');exit 0}catch{if(([uint32]$_.Exception.HResult)-eq 0x80070002){exit 1060};exit 79}";
    match system_command(
        "WindowsPowerShell/v1.0/powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            QUERY,
        ],
    )? {
        0 => Ok(true),
        1060 => Ok(false),
        _ => Err(EXIT_FAILED),
    }
}

fn validate_definition(definition: &str) -> Result<(), i32> {
    if definition.is_empty()
        || definition.len() > 64 * 1024
        || !definition
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err(EXIT_REFUSED);
    }
    Ok(())
}

fn powershell_status(script: &str) -> Result<i32, i32> {
    system_command(
        "WindowsPowerShell/v1.0/powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
    )
}

fn powershell_definition(script: &str) -> Result<String, i32> {
    let output = system_command_output(
        "WindowsPowerShell/v1.0/powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
    )?;
    if output.status.code() != Some(0) {
        return Err(EXIT_FAILED);
    }
    let definition = String::from_utf8(output.stdout).map_err(|_| EXIT_FAILED)?;
    let definition = definition.trim().to_owned();
    validate_definition(&definition)?;
    Ok(definition)
}

fn task_property(property: &str) -> Result<bool, i32> {
    let condition = match property {
        "Enabled" => "$t.Enabled",
        "Running" => "$t.State -eq 2 -or $t.State -eq 4",
        _ => return Err(EXIT_FAILED),
    };
    let query = format!(
        "$ErrorActionPreference='Stop';try{{$s=New-Object -ComObject Schedule.Service;$s.Connect();$t=$s.GetFolder('\\').GetTask('{LEGACY_TASK}');if({condition}){{exit 0}}else{{exit 1}}}}catch{{exit 79}}"
    );
    match system_command(
        "WindowsPowerShell/v1.0/powershell.exe",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &query,
        ],
    )? {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(EXIT_FAILED),
    }
}

fn system_command(name: &str, arguments: &[&str]) -> Result<i32, i32> {
    let status = Command::new(system_directory()?.join(name))
        .args(arguments)
        .status()
        .map_err(|_| EXIT_FAILED)?;
    Ok(status.code().unwrap_or(EXIT_FAILED))
}

fn system_command_output(name: &str, arguments: &[&str]) -> Result<std::process::Output, i32> {
    Command::new(system_directory()?.join(name))
        .args(arguments)
        .output()
        .map_err(|_| EXIT_FAILED)
}

fn system_directory() -> Result<PathBuf, i32> {
    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(EXIT_FAILED);
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

fn verify_candidate(
    install: &Path,
    predecessor_install: &Path,
    expected_gateway: &str,
    expected_owner: &str,
    expected_layout: &str,
    expected_fresh_install: bool,
    expected_repair: bool,
) -> Result<(), i32> {
    #[cfg(test)]
    let _ = predecessor_install;
    let bytes = fs::read(install.join("resources/keyboard-owner-release-v1.json"))
        .map_err(|_| EXIT_REFUSED)?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| EXIT_REFUSED)?;
    if manifest.schema_version != 1
        || manifest.kind != "talking-quill-local-owner-release"
        || manifest.platform != "win"
        || manifest.owner_mode != "local-unsigned-enabled"
        || !matches!(manifest.package_mode.as_str(), "fresh" | "update")
        || manifest.version.is_empty()
        || !matches!(manifest.architecture.as_str(), "x64" | "arm64")
        || manifest.roles.len() != 2
        || (expected_fresh_install
            && (manifest.package_mode != "fresh"
                || manifest.fresh_install != Some(true)
                || manifest.predecessor.is_some()))
        || (!expected_fresh_install
            && (manifest.package_mode != "update"
                || manifest.fresh_install.is_some()
                || manifest.predecessor.is_none()))
        || manifest.release_build_digest != manifest.package_layout_digest
        || canonical_layout(&manifest)? != manifest.package_layout_digest
        || !manifest.update.is_object()
    {
        return Err(EXIT_REFUSED);
    }
    if expected_repair {
        verify_installed_candidate(predecessor_install, &manifest)?;
    }
    #[cfg(not(test))]
    if !expected_repair {
        if let Some(predecessor) = &manifest.predecessor {
            verify_installed_predecessor(predecessor_install, predecessor)?;
        } else if predecessor_install.exists() {
            return Err(EXIT_REFUSED);
        }
    }
    let gateway = &manifest.roles[0];
    let owner = &manifest.roles[1];
    if gateway.role != "gateway"
        || gateway.path != "resources/helper/talking-quill-helper.exe"
        || gateway.suppression_capable
        || owner.role != "owner"
        || owner.path != "resources/helper/talking-quill-keyboard-owner.exe"
        || !owner.suppression_capable
        || (!expected_layout.is_empty() && expected_layout != manifest.package_layout_digest)
        || (!expected_gateway.is_empty() && expected_gateway != gateway.sha256)
        || (!expected_owner.is_empty() && expected_owner != owner.sha256)
        || hash_path(&install.join(&gateway.path))? != gateway.sha256
        || hash_path(&install.join(&owner.path))? != owner.sha256
    {
        return Err(EXIT_REFUSED);
    }
    Ok(())
}

fn verify_installed_candidate(install: &Path, expected: &Manifest) -> Result<(), i32> {
    let bytes = fs::read(install.join("resources/keyboard-owner-release-v1.json"))
        .map_err(|_| EXIT_REFUSED)?;
    let installed: Manifest = serde_json::from_slice(&bytes).map_err(|_| EXIT_REFUSED)?;
    if installed.schema_version != expected.schema_version
        || installed.kind != expected.kind
        || installed.version != expected.version
        || installed.platform != expected.platform
        || installed.architecture != expected.architecture
        || installed.owner_mode != expected.owner_mode
        || installed.package_mode != expected.package_mode
        || installed.release_build_digest != expected.release_build_digest
        || installed.package_layout_digest != expected.package_layout_digest
        || canonical_layout(&installed)? != installed.package_layout_digest
        || installed.roles.len() != 2
        || installed.roles[0].sha256 != expected.roles[0].sha256
        || installed.roles[1].sha256 != expected.roles[1].sha256
        || hash_path(&install.join(&installed.roles[0].path))? != expected.roles[0].sha256
        || hash_path(&install.join(&installed.roles[1].path))? != expected.roles[1].sha256
    {
        return Err(EXIT_REFUSED);
    }
    Ok(())
}

#[cfg(not(test))]
fn verify_installed_predecessor(install: &Path, expected: &Predecessor) -> Result<(), i32> {
    let bytes = fs::read(install.join("resources/keyboard-owner-release-v1.json"))
        .map_err(|_| EXIT_REFUSED)?;
    let installed: Manifest = serde_json::from_slice(&bytes).map_err(|_| EXIT_REFUSED)?;
    if installed.schema_version != 1
        || installed.platform != expected.platform
        || installed.architecture != expected.architecture
        || installed.version != expected.version
        || installed.release_build_digest != expected.release_build_digest
        || installed.package_layout_digest != expected.release_build_digest
        || canonical_layout(&installed)? != installed.package_layout_digest
        || installed.roles.len() != 2
        || installed.roles[0].role != "gateway"
        || installed.roles[0].sha256 != expected.gateway_sha256
        || installed.roles[1].role != "owner"
        || installed.roles[1].sha256 != expected.owner_sha256
        || hash_path(&install.join(&installed.roles[0].path))? != expected.gateway_sha256
        || hash_path(&install.join(&installed.roles[1].path))? != expected.owner_sha256
    {
        return Err(EXIT_REFUSED);
    }
    Ok(())
}

fn canonical_layout(manifest: &Manifest) -> Result<String, i32> {
    let mut hash = Sha256::new();
    hash.update(b"talking-quill/package-layout/v1\0");
    for (name, value) in [
        ("version", manifest.version.as_str()),
        ("platform", manifest.platform.as_str()),
        ("architecture", manifest.architecture.as_str()),
        ("ownerMode", manifest.owner_mode.as_str()),
        ("packageMode", manifest.package_mode.as_str()),
    ] {
        frame(&mut hash, name, value)?;
    }
    for role in &manifest.roles {
        frame(
            &mut hash,
            "role",
            &format!(
                "{}\0{}\0{}\0{}",
                role.role, role.path, role.sha256, role.suppression_capable
            ),
        )?;
    }
    frame(
        &mut hash,
        "predecessorPresent",
        if manifest.predecessor.is_some() {
            "true"
        } else {
            "false"
        },
    )?;
    if let Some(predecessor) = &manifest.predecessor {
        for (name, value) in [
            ("predecessorPlatform", predecessor.platform.as_str()),
            ("predecessorArchitecture", predecessor.architecture.as_str()),
            ("predecessorVersion", predecessor.version.as_str()),
            (
                "predecessorReleaseBuildDigest",
                predecessor.release_build_digest.as_str(),
            ),
            (
                "predecessorGatewaySha256",
                predecessor.gateway_sha256.as_str(),
            ),
            ("predecessorOwnerSha256", predecessor.owner_sha256.as_str()),
        ] {
            frame(&mut hash, name, value)?;
        }
    }
    if manifest.fresh_install == Some(true) {
        frame(&mut hash, "freshInstall", "true")?;
    }
    Ok(hex_digest(hash.finalize().as_slice()))
}

fn frame(hash: &mut Sha256, name: &str, value: &str) -> Result<(), i32> {
    hash.update(
        u16::try_from(name.len())
            .map_err(|_| EXIT_REFUSED)?
            .to_be_bytes(),
    );
    hash.update(
        u32::try_from(value.len())
            .map_err(|_| EXIT_REFUSED)?
            .to_be_bytes(),
    );
    hash.update(name.as_bytes());
    hash.update(value.as_bytes());
    Ok(())
}

fn hash_path(path: &Path) -> Result<String, i32> {
    let mut file = File::open(path).map_err(|_| EXIT_REFUSED)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| EXIT_REFUSED)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex_digest(hash.finalize().as_slice()))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn known_folder(folder: &windows_sys::core::GUID) -> Result<PathBuf, i32> {
    let mut value = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut value) } < 0
        || value.is_null()
    {
        return Err(EXIT_FAILED);
    }
    let mut length = 0usize;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    let path = PathBuf::from(
        String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| EXIT_FAILED)?,
    );
    unsafe { CoTaskMemFree(value.cast()) };
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeMachine {
        process_generations: VecDeque<Result<Vec<ObservedProcess>, i32>>,
        service: bool,
        service_running: bool,
        service_start_mode: ServiceStartMode,
        task: bool,
        task_enabled: bool,
        task_running: bool,
        fail_delete_service: bool,
        pauses: usize,
        calls: Vec<&'static str>,
    }

    impl MachineAdapter for FakeMachine {
        fn processes(&mut self) -> Result<Vec<ObservedProcess>, i32> {
            self.calls.push("processes");
            if let Some(generation) = self.process_generations.pop_front() {
                return generation;
            }
            if self.service_running || self.task_running {
                return Ok(vec![ObservedProcess {
                    pid: std::process::id().saturating_add(1),
                    name: "talking-quill-windows-keyboard-authority.exe".into(),
                    path: Err(()),
                }]);
            }
            Ok(Vec::new())
        }

        fn pause(&mut self, _duration: Duration) {
            self.pauses += 1;
        }

        fn service_exists(&mut self) -> Result<bool, i32> {
            self.calls.push("service_exists");
            Ok(self.service)
        }

        fn service_is_running(&mut self) -> Result<bool, i32> {
            self.calls.push("service_is_running");
            Ok(self.service_running)
        }

        fn service_definition(&mut self) -> Result<String, i32> {
            self.calls.push("service_definition");
            Ok("c2VydmljZS12MQ==".into())
        }

        fn restore_service(&mut self, definition: &str) -> Result<(), i32> {
            self.calls.push("restore_service");
            if definition != "c2VydmljZS12MQ==" {
                return Err(EXIT_REFUSED);
            }
            self.service = true;
            Ok(())
        }

        fn service_start_mode(&mut self) -> Result<ServiceStartMode, i32> {
            self.calls.push("service_start_mode");
            Ok(self.service_start_mode)
        }

        fn set_service_start_mode(&mut self, mode: ServiceStartMode) -> Result<(), i32> {
            self.calls.push("set_service_start_mode");
            self.service_start_mode = mode;
            Ok(())
        }

        fn stop_service(&mut self) -> Result<(), i32> {
            self.calls.push("stop_service");
            self.service_running = false;
            Ok(())
        }

        fn start_service(&mut self) -> Result<(), i32> {
            self.calls.push("start_service");
            self.service_running = true;
            Ok(())
        }

        fn delete_service(&mut self) -> Result<(), i32> {
            self.calls.push("delete_service");
            if self.fail_delete_service {
                return Err(EXIT_FAILED);
            }
            self.service = false;
            Ok(())
        }

        fn task_exists(&mut self) -> Result<bool, i32> {
            self.calls.push("task_exists");
            Ok(self.task)
        }

        fn task_is_enabled(&mut self) -> Result<bool, i32> {
            self.calls.push("task_is_enabled");
            Ok(self.task_enabled)
        }

        fn task_is_running(&mut self) -> Result<bool, i32> {
            self.calls.push("task_is_running");
            Ok(self.task_running)
        }

        fn task_definition(&mut self) -> Result<String, i32> {
            self.calls.push("task_definition");
            Ok("dGFzay12MQ==".into())
        }

        fn restore_task(&mut self, definition: &str) -> Result<(), i32> {
            self.calls.push("restore_task");
            if definition != "dGFzay12MQ==" {
                return Err(EXIT_REFUSED);
            }
            self.task = true;
            Ok(())
        }

        fn set_task_enabled(&mut self, enabled: bool) -> Result<(), i32> {
            self.calls.push("set_task_enabled");
            self.task_enabled = enabled;
            Ok(())
        }

        fn stop_task(&mut self) -> Result<(), i32> {
            self.calls.push("stop_task");
            self.task_running = false;
            Ok(())
        }

        fn start_task(&mut self) -> Result<(), i32> {
            self.calls.push("start_task");
            self.task_running = true;
            Ok(())
        }

        fn delete_task(&mut self) -> Result<(), i32> {
            self.calls.push("delete_task");
            self.task = false;
            self.task_enabled = false;
            self.task_running = false;
            Ok(())
        }
    }

    struct Fixture {
        root: PathBuf,
        paths: LifecyclePaths,
    }

    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "talking-quill-rust-installer-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let paths = LifecyclePaths {
                install: root.join("Talking Quill"),
                backup: root.join(".Talking Quill.stage1-backup"),
                orphan: root.join(".Talking Quill.stage1-ambiguous-replacement"),
                transaction: root.join(".Talking Quill.stage1-transaction.json"),
                legacy_authority: root.join("legacy-authority"),
                legacy_authority_quarantine: root.join("legacy-authority-quarantine"),
                legacy_task_file: root.join("legacy-task"),
            };
            Self { root, paths }
        }

        fn write(&self, directory: &Path, value: &str) {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("marker"), value).unwrap();
        }

        fn execute(
            &self,
            mode: &str,
            failure: FailurePoint,
            machine: &mut FakeMachine,
        ) -> Result<(), i32> {
            execute_lifecycle(mode, &self.paths, "", "", "", failure, machine)
        }

        fn write_candidate(&self) {
            let gateway_path = self
                .paths
                .install
                .join("resources/helper/talking-quill-helper.exe");
            let owner_path = self
                .paths
                .install
                .join("resources/helper/talking-quill-keyboard-owner.exe");
            fs::create_dir_all(gateway_path.parent().unwrap()).unwrap();
            fs::write(&gateway_path, b"gateway").unwrap();
            fs::write(&owner_path, b"owner").unwrap();
            let mut manifest = Manifest {
                schema_version: 1,
                kind: "talking-quill-local-owner-release".into(),
                version: "1.2.3".into(),
                platform: "win".into(),
                architecture: "x64".into(),
                owner_mode: "local-unsigned-enabled".into(),
                package_mode: "update".into(),
                roles: vec![
                    Role {
                        role: "gateway".into(),
                        path: "resources/helper/talking-quill-helper.exe".into(),
                        sha256: hash_path(&gateway_path).unwrap(),
                        suppression_capable: false,
                    },
                    Role {
                        role: "owner".into(),
                        path: "resources/helper/talking-quill-keyboard-owner.exe".into(),
                        sha256: hash_path(&owner_path).unwrap(),
                        suppression_capable: true,
                    },
                ],
                predecessor: Some(Predecessor {
                    platform: "win".into(),
                    architecture: "x64".into(),
                    version: "1.2.2".into(),
                    release_build_digest: "11".repeat(32),
                    gateway_sha256: "22".repeat(32),
                    owner_sha256: "33".repeat(32),
                }),
                fresh_install: None,
                release_build_digest: String::new(),
                package_layout_digest: String::new(),
                update: serde_json::json!({}),
            };
            let layout = canonical_layout(&manifest).unwrap();
            manifest.release_build_digest = layout.clone();
            manifest.package_layout_digest = layout;
            fs::create_dir_all(self.paths.install.join("resources")).unwrap();
            fs::write(
                self.paths
                    .install
                    .join("resources/keyboard-owner-release-v1.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn authenticated_same_candidate_repair_accepts_exact_backup_and_replacement() {
        let fixture = Fixture::new();
        let mut machine = FakeMachine::default();
        fixture.write_candidate();
        assert_eq!(
            fixture.execute("repair", FailurePoint::None, &mut machine),
            Ok(())
        );
        fixture.write_candidate();
        assert_eq!(
            fixture.execute("install-commit", FailurePoint::None, &mut machine),
            Ok(())
        );
        let transaction = read_transaction(&fixture.paths.transaction).unwrap();
        assert!(transaction.repair);
        assert_eq!(transaction.state, "committed");
    }

    #[test]
    fn fresh_install_refuses_files_service_and_task_through_the_adapter() {
        for select in 0..8 {
            let fixture = Fixture::new();
            let mut machine = FakeMachine::default();
            match select {
                0 => fixture.write(&fixture.paths.install, "installed"),
                1 => fixture.write(&fixture.paths.backup, "backup"),
                2 => fixture.write(&fixture.paths.orphan, "orphan"),
                3 => fixture.write(&fixture.paths.legacy_authority, "authority"),
                4 => fs::write(&fixture.paths.legacy_task_file, "task").unwrap(),
                5 => fixture.write(&fixture.paths.legacy_authority_quarantine, "authority"),
                6 => machine.service = true,
                _ => machine.task = true,
            }
            assert_eq!(
                fixture.execute("fresh-install", FailurePoint::None, &mut machine),
                Err(EXIT_REFUSED)
            );
        }
    }

    #[test]
    fn fresh_staging_prepared_and_restoring_crashes_are_retryable() {
        let fixture = Fixture::new();
        let mut machine = FakeMachine::default();
        assert_eq!(
            fixture.execute(
                "fresh-install",
                FailurePoint::AfterFreshStagingRecord,
                &mut machine,
            ),
            Err(EXIT_FAILED)
        );
        assert_eq!(
            read_transaction(&fixture.paths.transaction).unwrap().state,
            "staging"
        );
        fixture
            .execute("fresh-install", FailurePoint::None, &mut machine)
            .unwrap();
        fixture.write(&fixture.paths.install, "partial");
        assert_eq!(
            fixture.execute(
                "install-rollback",
                FailurePoint::AfterRestoringRecord,
                &mut machine,
            ),
            Err(EXIT_FAILED)
        );
        assert_eq!(
            read_transaction(&fixture.paths.transaction).unwrap().state,
            "restoring"
        );
        fixture
            .execute("install-rollback", FailurePoint::None, &mut machine)
            .unwrap();
        assert!(!fixture.paths.install.exists());
        fixture
            .execute("fresh-install", FailurePoint::None, &mut machine)
            .unwrap();
        assert_eq!(
            read_transaction(&fixture.paths.transaction).unwrap().state,
            "prepared"
        );
    }

    #[test]
    fn runtime_enumeration_waits_for_exact_installed_path_and_propagates_failure() {
        let fixture = Fixture::new();
        fixture.write(&fixture.paths.install, "predecessor");
        fs::write(fixture.paths.install.join("Talking Quill.exe"), b"app").unwrap();
        let mut machine = FakeMachine::default();
        machine
            .process_generations
            .push_back(Ok(vec![ObservedProcess {
                pid: std::process::id().saturating_add(1),
                name: "Talking Quill.exe".into(),
                path: Ok(fixture.paths.install.join("Talking Quill.exe")),
            }]));
        machine.process_generations.push_back(Ok(Vec::new()));
        fixture
            .execute("install", FailurePoint::None, &mut machine)
            .unwrap();
        assert_eq!(machine.pauses, 1);

        let legacy = Fixture::new();
        legacy.write(&legacy.paths.install, "predecessor");
        let legacy_executable = legacy
            .root
            .join("talking-quill-windows-keyboard-authority.exe");
        fs::write(&legacy_executable, b"legacy").unwrap();
        let mut legacy_machine = FakeMachine::default();
        legacy_machine
            .process_generations
            .push_back(Ok(vec![ObservedProcess {
                pid: std::process::id().saturating_add(1),
                name: "talking-quill-windows-keyboard-authority.exe".into(),
                path: Ok(legacy_executable),
            }]));
        legacy_machine.process_generations.push_back(Ok(Vec::new()));
        wait_for_planned_runtime_exit(
            &legacy.paths.install,
            Duration::from_secs(1),
            &mut legacy_machine,
            true,
        )
        .unwrap();
        assert_eq!(legacy_machine.pauses, 1);

        let other = Fixture::new();
        other.write(&other.paths.install, "predecessor");
        let mut failed = FakeMachine::default();
        failed.process_generations.push_back(Err(EXIT_FAILED));
        assert_eq!(
            other.execute("install", FailurePoint::None, &mut failed),
            Err(EXIT_FAILED)
        );
        assert!(other.paths.install.exists());
    }

    #[test]
    fn update_crash_points_restore_the_predecessor() {
        for failure in [
            FailurePoint::AfterStagingRecord,
            FailurePoint::AfterPredecessorRename,
            FailurePoint::AfterRestoringRecord,
            FailurePoint::AfterPartialRemoval,
        ] {
            let fixture = Fixture::new();
            let mut machine = FakeMachine::default();
            fixture.write(&fixture.paths.install, "predecessor");
            let result = fixture.execute("install", failure, &mut machine);
            if matches!(
                failure,
                FailurePoint::AfterRestoringRecord | FailurePoint::AfterPartialRemoval
            ) {
                assert!(result.is_ok());
                fixture.write(&fixture.paths.install, "partial");
                assert_eq!(
                    fixture.execute("install-rollback", failure, &mut machine),
                    Err(EXIT_FAILED)
                );
            } else {
                assert_eq!(result, Err(EXIT_FAILED));
            }
            fixture
                .execute("install-rollback", FailurePoint::None, &mut machine)
                .unwrap();
            assert_eq!(
                fs::read_to_string(fixture.paths.install.join("marker")).unwrap(),
                "predecessor"
            );
        }
    }

    #[test]
    fn commit_quiesces_before_the_durable_decision_and_deletes_after_it() {
        let fixture = Fixture::new();
        let mut machine = FakeMachine {
            service: true,
            service_running: true,
            service_start_mode: ServiceStartMode::Auto,
            task: true,
            task_enabled: true,
            task_running: true,
            ..FakeMachine::default()
        };
        fixture.write(&fixture.paths.install, "predecessor");
        fixture
            .execute("install", FailurePoint::None, &mut machine)
            .unwrap();
        fixture.write_candidate();
        fixture.write(&fixture.paths.legacy_authority, "authority");
        fs::write(&fixture.paths.legacy_task_file, b"task").unwrap();
        fixture
            .execute("install-commit", FailurePoint::None, &mut machine)
            .unwrap();
        assert_eq!(
            read_transaction(&fixture.paths.transaction).unwrap().state,
            "committed"
        );
        assert!(!machine.service);
        assert!(!machine.task);
        assert!(!fixture.paths.legacy_task_file.exists());
        assert!(!fixture.paths.legacy_authority.exists());
        let stop = machine
            .calls
            .iter()
            .position(|call| *call == "stop_service")
            .unwrap();
        let delete = machine
            .calls
            .iter()
            .position(|call| *call == "delete_service")
            .unwrap();
        assert!(stop < delete);
    }

    #[test]
    fn every_precommit_retirement_crash_restores_authority_and_predecessor() {
        for failure in [
            FailurePoint::AfterAuthoritySnapshot,
            FailurePoint::AfterTaskDisable,
            FailurePoint::AfterTaskStop,
            FailurePoint::AfterServiceDisable,
            FailurePoint::AfterServiceStop,
            FailurePoint::AfterTaskFilePreserved,
            FailurePoint::AfterAuthorityQuarantine,
        ] {
            let fixture = Fixture::new();
            let mut machine = FakeMachine {
                service: true,
                service_running: true,
                service_start_mode: ServiceStartMode::Auto,
                task: true,
                task_enabled: true,
                task_running: true,
                ..FakeMachine::default()
            };
            fixture.write(&fixture.paths.install, "predecessor");
            fixture
                .execute("install", FailurePoint::None, &mut machine)
                .unwrap();
            fixture.write_candidate();
            fixture.write(&fixture.paths.legacy_authority, "authority");
            fs::write(&fixture.paths.legacy_task_file, b"task").unwrap();
            assert_eq!(
                fixture.execute("install-commit", failure, &mut machine),
                Err(EXIT_FAILED)
            );
            assert_eq!(
                read_transaction(&fixture.paths.transaction).unwrap().state,
                "authority-preparing"
            );
            fixture
                .execute("install-rollback", FailurePoint::None, &mut machine)
                .unwrap();
            assert!(machine.service && machine.service_running);
            assert_eq!(machine.service_start_mode, ServiceStartMode::Auto);
            assert!(machine.task && machine.task_enabled && machine.task_running);
            assert!(fixture.paths.legacy_authority.exists());
            assert!(fixture.paths.legacy_task_file.exists());
            assert_eq!(
                fs::read_to_string(fixture.paths.install.join("marker")).unwrap(),
                "predecessor"
            );
        }
    }

    #[test]
    fn running_legacy_authority_crash_recovery_restores_files_before_activity() {
        for failure in [
            FailurePoint::AfterAuthorityRollbackRecord,
            FailurePoint::AfterAuthorityRollbackNeutral,
            FailurePoint::AfterPredecessorFilesRestored,
            FailurePoint::AfterServiceActivityRestored,
            FailurePoint::AfterTaskActivityRestored,
        ] {
            let fixture = Fixture::new();
            let mut machine = FakeMachine {
                service: true,
                service_running: true,
                service_start_mode: ServiceStartMode::Auto,
                task: true,
                task_enabled: true,
                task_running: true,
                ..FakeMachine::default()
            };
            fixture.write(&fixture.paths.install, "predecessor");
            fixture
                .execute("install", FailurePoint::None, &mut machine)
                .unwrap();
            fixture.write_candidate();
            fixture.write(&fixture.paths.legacy_authority, "authority");
            fs::write(&fixture.paths.legacy_task_file, b"task").unwrap();
            assert_eq!(
                fixture.execute(
                    "install-commit",
                    FailurePoint::AfterAuthorityQuarantine,
                    &mut machine,
                ),
                Err(EXIT_FAILED)
            );
            assert_eq!(
                fixture.execute("install-rollback", failure, &mut machine),
                Err(EXIT_FAILED)
            );
            assert_eq!(
                read_transaction(&fixture.paths.transaction).unwrap().state,
                "authority-rollback"
            );
            if matches!(
                failure,
                FailurePoint::AfterPredecessorFilesRestored
                    | FailurePoint::AfterServiceActivityRestored
                    | FailurePoint::AfterTaskActivityRestored
            ) {
                assert_eq!(
                    fs::read_to_string(fixture.paths.install.join("marker")).unwrap(),
                    "predecessor"
                );
            }
            fixture
                .execute("install-rollback", FailurePoint::None, &mut machine)
                .unwrap();
            assert_eq!(
                fs::read_to_string(fixture.paths.install.join("marker")).unwrap(),
                "predecessor"
            );
            assert!(machine.service && machine.service_running);
            assert_eq!(machine.service_start_mode, ServiceStartMode::Auto);
            assert!(machine.task && machine.task_enabled && machine.task_running);
            assert!(!fixture.paths.transaction.exists());
        }
    }

    #[test]
    fn rollback_recreates_missing_snapshots_and_retires_unexpected_registrations() {
        let fixture = Fixture::new();
        let mut machine = FakeMachine {
            service: true,
            service_running: true,
            service_start_mode: ServiceStartMode::Auto,
            task: true,
            task_enabled: true,
            task_running: true,
            ..FakeMachine::default()
        };
        fixture.write(&fixture.paths.install, "predecessor");
        fixture
            .execute("install", FailurePoint::None, &mut machine)
            .unwrap();
        fixture.write_candidate();
        assert_eq!(
            fixture.execute(
                "install-commit",
                FailurePoint::AfterAuthoritySnapshot,
                &mut machine,
            ),
            Err(EXIT_FAILED)
        );
        machine.service = false;
        machine.service_running = false;
        machine.task = false;
        machine.task_enabled = false;
        machine.task_running = false;
        fixture
            .execute("install-rollback", FailurePoint::None, &mut machine)
            .unwrap();
        assert!(machine.service && machine.service_running);
        assert!(machine.task && machine.task_enabled && machine.task_running);

        let unexpected = Fixture::new();
        let mut machine = FakeMachine::default();
        unexpected.write(&unexpected.paths.install, "predecessor");
        unexpected
            .execute("install", FailurePoint::None, &mut machine)
            .unwrap();
        unexpected.write_candidate();
        assert_eq!(
            unexpected.execute(
                "install-commit",
                FailurePoint::AfterAuthoritySnapshot,
                &mut machine,
            ),
            Err(EXIT_FAILED)
        );
        machine.service = true;
        machine.service_running = true;
        machine.task = true;
        machine.task_enabled = true;
        machine.task_running = true;
        unexpected
            .execute("install-rollback", FailurePoint::None, &mut machine)
            .unwrap();
        assert!(!machine.service);
        assert!(!machine.task);
    }

    #[test]
    fn every_postcommit_retirement_crash_resumes_forward_without_mixed_authority() {
        for failure in [
            FailurePoint::AfterDurableCommit,
            FailurePoint::AfterServiceDelete,
            FailurePoint::AfterTaskDelete,
            FailurePoint::AfterTaskFileDelete,
            FailurePoint::AfterAuthorityQuarantineDelete,
        ] {
            let fixture = Fixture::new();
            let mut machine = FakeMachine {
                service: true,
                service_running: true,
                service_start_mode: ServiceStartMode::Auto,
                task: true,
                task_enabled: true,
                task_running: true,
                ..FakeMachine::default()
            };
            fixture.write(&fixture.paths.install, "predecessor");
            fixture
                .execute("install", FailurePoint::None, &mut machine)
                .unwrap();
            fixture.write_candidate();
            fixture.write(&fixture.paths.legacy_authority, "authority");
            fs::write(&fixture.paths.legacy_task_file, b"task").unwrap();
            assert_eq!(
                fixture.execute("install-commit", failure, &mut machine),
                Err(EXIT_FAILED)
            );
            assert_eq!(
                read_transaction(&fixture.paths.transaction).unwrap().state,
                "committed"
            );
            fixture
                .execute("install-commit", FailurePoint::None, &mut machine)
                .unwrap();
            assert!(!machine.service || !machine.service_running);
            assert!(!machine.task || (!machine.task_enabled && !machine.task_running));
            assert!(!fixture.paths.legacy_authority.exists());
            assert!(!fixture.paths.legacy_authority_quarantine.exists());
            assert!(!fixture.paths.legacy_task_file.exists());
            assert!(fixture.paths.install.exists());
        }
    }
}
