//! Bind stale-state cleanup evidence to retained objects and source identity.
use super::*;

pub(super) struct StaleCleanupAudit {
    pub(super) file: File,
    pub(super) parent: PathBuf,
    pub(super) identity: String,
    pub(super) operation: String,
    pub(super) chain: [u8; 32],
}

impl StaleCleanupAudit {
    pub(super) fn open() -> Result<Self> {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_AUDIT_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| fail(EXIT_REJECTED, "TQ_STALE_SCHEMA2_AUDIT_PATH is required."))?;
        if !path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale cleanup audit path must be absolute.",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Audit path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        let file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected cleanup audit file.",
                )
            })?;
        if !protected_file_handle_acl_is_exact(&file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Cleanup audit is not administrator protected.",
            ));
        }
        Self::from_retained(file, parent)
    }

    pub(super) fn from_retained(mut file: File, parent: PathBuf) -> Result<Self> {
        let identity = file_identity_text(&file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut prior = Vec::new();
        file.read_to_end(&mut prior).map_err(io_failure)?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|_| fail(EXIT_FAILURE, "Audit operation randomness is unavailable."))?;
        let operation: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut initial = Sha256::new();
        initial.update(b"TalkingQuill/stale-schema2-audit-chain/v1\0");
        initial.update(identity.as_bytes());
        initial.update(&prior);
        initial.update(operation.as_bytes());
        Ok(Self {
            file,
            parent,
            identity,
            operation,
            chain: initial.finalize().into(),
        })
    }

    pub(super) fn record(&mut self, stage: &str, binding: &str, proof: &str) -> Result<()> {
        let previous = hex_hash(&self.chain);
        let mut event = Sha256::new();
        event.update(self.chain);
        event.update(stage.as_bytes());
        event.update(binding.as_bytes());
        event.update(proof.as_bytes());
        self.chain = event.finalize().into();
        let line = format!(
            "{{\"schemaVersion\":1,\"operation\":\"stale-schema2-cleanup\",\"operationId\":\"{}\",\"auditIdentity\":\"{}\",\"stage\":\"{stage}\",\"bindingSha256\":\"{binding}\",\"proofSha256\":\"{proof}\",\"previousSha256\":\"{previous}\",\"eventSha256\":\"{}\"}}\n",
            self.operation,
            self.identity,
            hex_hash(&self.chain),
        );
        self.file.write_all(line.as_bytes()).map_err(io_failure)?;
        self.file.sync_all().map_err(io_failure)?;
        flush_setup_directory(&self.parent)
    }
}

pub(super) fn retained_binding(objects: &[&RetainedStaleObject], suffix: &str) -> String {
    let mut entries = objects
        .iter()
        .map(|object| format!("{}:{}", object.path.display(), object.identity))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/stale-schema2-cleanup-binding/v1\0");
    hash.update(suffix.as_bytes());
    for entry in entries {
        hash.update(entry.as_bytes());
        hash.update([0]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    hex_hash(&digest)
}

pub(super) fn active_state_proof(
    program_files: &Path,
    program_data: &Path,
    system: &Path,
    allow_coordination: bool,
    authenticated_parent: bool,
) -> Result<String> {
    let fixed_paths = [
        program_files.join("Talking Quill"),
        program_files.join(".Talking Quill.native-staging"),
        program_files.join(".Talking Quill.native-backup"),
        program_files.join(".Talking Quill.native-transaction-v2.json"),
        program_files.join(".Talking Quill.maintenance-generation-v1"),
        program_data.join("Talking Quill/KeyboardAuthority"),
        program_data.join("Talking Quill/.KeyboardAuthority.retirement-quarantine"),
        system.join("Tasks/TalkingQuillKeyboardAuthority"),
    ];
    let uninstall_present = registry_key_present(HKEY_LOCAL_MACHINE, UNINSTALL_KEY)?;
    let app_path_present = registry_key_present(HKEY_LOCAL_MACHINE, APP_PATH_KEY)?;
    let run_absent = no_owned_run_values()?;
    let processes_absent =
        no_talking_quill_process_except_authenticated_pair(authenticated_parent)?;
    let services_absent = no_owned_service_keys()?;
    let tasks_absent = no_owned_task_files(system)?;
    let legacy_service_present = terminal_service_exists("TalkingQuillKeyboardAuthority")?;
    if fixed_paths
        .iter()
        .any(|path| path_present(path).unwrap_or(true))
        || uninstall_present
        || app_path_present
        || !run_absent
        || !processes_absent
        || !services_absent
        || !tasks_absent
        || legacy_service_present
    {
        return Err(fail(
            EXIT_REJECTED,
            "Active or unknown Talking Quill state blocks stale cleanup.",
        ));
    }
    let mut evidence = fixed_paths
        .iter()
        .map(|path| format!("absent:{}", path.display()))
        .collect::<Vec<_>>();
    evidence.extend([
        format!("uninstall-present:{uninstall_present}"),
        format!("app-path-present:{app_path_present}"),
        format!("run-absent:{run_absent}"),
        format!("processes-absent:{processes_absent}"),
        format!("services-absent:{services_absent}"),
        format!("tasks-absent:{tasks_absent}"),
        format!("legacy-service-present:{legacy_service_present}"),
    ]);
    let mut program_files_inventory = Vec::new();
    for entry in fs::read_dir(program_files).map_err(io_failure)? {
        let name = entry
            .map_err(io_failure)?
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase();
        program_files_inventory.push(name.clone());
        if name.starts_with("talking quill maintenance-")
            || name.starts_with(".talking quill.native-transaction-v2.tmp-")
        {
            return Err(fail(
                EXIT_REJECTED,
                "Program Files recovery residue blocks stale cleanup.",
            ));
        }
    }
    program_files_inventory.sort_unstable();
    evidence.extend(
        program_files_inventory
            .into_iter()
            .map(|name| format!("pf:{name}")),
    );
    let mut program_data_inventory = Vec::new();
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let name = entry
            .map_err(io_failure)?
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase();
        program_data_inventory.push(name.clone());
        let coordination = name == "talking quill update recovery"
            || name.starts_with(".talking quill.machine-lock-")
            || name.starts_with(".talking quill.machine-lifecycle-retained-");
        let other = name.starts_with(".talking quill.update-")
            || name.starts_with(".talking quill.uninstall-finalizer-")
            || name.starts_with(".talking quill.terminal-")
            || name.starts_with("talking quill terminal");
        if other || (coordination && !allow_coordination) {
            return Err(fail(
                EXIT_REJECTED,
                "ProgramData recovery residue blocks stale cleanup.",
            ));
        }
    }
    program_data_inventory.sort_unstable();
    evidence.extend(
        program_data_inventory
            .into_iter()
            .map(|name| format!("pd:{name}")),
    );
    let talking_quill_registry =
        registry_key_present(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?;
    if !allow_coordination && talking_quill_registry {
        return Err(fail(
            EXIT_REJECTED,
            "Talking Quill registry residue remains.",
        ));
    }
    evidence.push(format!(
        "talking-quill-registry-present:{talking_quill_registry}"
    ));
    evidence.push(format!("allow-coordination:{allow_coordination}"));
    evidence.push(format!("authenticated-parent:{authenticated_parent}"));
    evidence.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/stale-schema2-machine-proof/v2\0");
    for entry in evidence {
        hash.update(entry.as_bytes());
        hash.update([0]);
    }
    let digest: [u8; 32] = hash.finalize().into();
    Ok(hex_hash(&digest))
}

pub(super) fn exact_stale_coordination_inventory(
    program_data: &Path,
    suffix: &str,
    schema2_recovery: bool,
) -> Result<()> {
    let expected_lock = format!("{MACHINE_LOCK_DIRECTORY_PREFIX}{suffix}").to_ascii_lowercase();
    let mut observed = Vec::new();
    for entry in fs::read_dir(program_data).map_err(io_failure)? {
        let name = entry
            .map_err(io_failure)?
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase();
        if name == "talking quill update recovery"
            || name.starts_with(".talking quill.machine-lock-")
            || name.starts_with(".talking quill.machine-lifecycle-retained-")
        {
            observed.push(name);
        }
    }
    observed.sort_unstable();
    let mut expected = vec![expected_lock];
    if schema2_recovery {
        expected.push("talking quill update recovery".to_owned());
        expected.sort_unstable();
    }
    if observed != expected {
        return Err(fail(
            EXIT_REJECTED,
            "Stale coordination inventory is not the exact admitted topology.",
        ));
    }
    Ok(())
}

pub(super) fn exact_cleanup_registry(suffix: &str) -> Result<()> {
    if exact_machine_lock_publication()?
        .as_ref()
        .map(|publication| publication.suffix.as_str())
        != Some(suffix)
        || registry_subkeys(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")?
            != Some(vec!["RecoveryStateLockV1".to_owned()])
        || registry_value_names(HKEY_LOCAL_MACHINE, r"Software\Talking Quill")? != Some(Vec::new())
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cleanup registry inventory is not exact.",
        ));
    }
    Ok(())
}

pub(super) fn complete_stale_cleanup_zero_state(
    audit: &mut StaleCleanupAudit,
    binding: &str,
    program_files: &Path,
    program_data: &Path,
    system: &Path,
    authenticated_parent: bool,
) -> Result<()> {
    let zero = active_state_proof(
        program_files,
        program_data,
        system,
        false,
        authenticated_parent,
    )?;
    audit.record("completed", binding, &zero)
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn direct_cleanup_source_identity() -> Result<(&'static str, &'static str)> {
    let commit = option_env!("TALKING_QUILL_RELEASE_COMMIT").ok_or_else(|| {
        fail(
            EXIT_REJECTED,
            "Direct cleanup build source commit is unavailable.",
        )
    })?;
    let tree = option_env!("TALKING_QUILL_RELEASE_TREE").ok_or_else(|| {
        fail(
            EXIT_REJECTED,
            "Direct cleanup build source tree is unavailable.",
        )
    })?;
    let exact_git_identity = |value: &str| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if !exact_git_identity(commit) || !exact_git_identity(tree) {
        return Err(fail(
            EXIT_REJECTED,
            "Direct cleanup build source identity is invalid.",
        ));
    }
    Ok((commit, tree))
}
