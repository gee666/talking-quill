//! Retain and diagnose stale lifecycle objects before cleanup.
use super::*;

pub(super) struct RetainedStaleObject {
    pub(super) file: File,
    pub(super) identity: String,
    pub(super) path: PathBuf,
    pub(super) directory: bool,
}

impl RetainedStaleObject {
    pub(super) fn open(path: &Path, directory: bool) -> Result<Self> {
        Self::open_with_share(path, directory, 0)
    }

    pub(super) fn open_lifecycle(path: &Path) -> Result<Self> {
        // Delete sharing permits POSIX unlink while a duplicate retains the same verified file
        // object. Read and write sharing remain denied, so an active lifecycle owner conflicts.
        Self::open_with_share(path, false, FILE_SHARE_DELETE)
    }

    pub(super) fn open_with_share(path: &Path, directory: bool, share: u32) -> Result<Self> {
        let raw = unsafe {
            CreateFileW(
                wide(path.as_os_str()).as_ptr(),
                FILE_GENERIC_READ | DELETE,
                share,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_REJECTED,
                "A stale fixture object is active or unavailable.",
            ));
        }
        let file = unsafe { File::from_raw_handle(raw) };
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity is invalid.",
            ));
        }
        let identity = file_identity_text(&file)?;
        Ok(Self {
            file,
            identity,
            path: path.to_owned(),
            directory,
        })
    }

    pub(super) fn verify(&self) -> Result<()> {
        if file_identity_text(&self.file)? != self.identity {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity changed.",
            ));
        }
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(self.file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != self.directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture type changed.",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_protected_acl(&self) -> Result<()> {
        if !protected_handle_acl_is_exact(&self.file, self.directory)? {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture ACL is not exact.",
            ));
        }
        Ok(())
    }

    pub(super) fn names(&self) -> Result<Vec<String>> {
        if !self.directory {
            return Err(fail(
                EXIT_REJECTED,
                "A retained file has no child inventory.",
            ));
        }
        let mut names = retained_directory_names(self.file.as_raw_handle())
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        names.sort_unstable();
        Ok(names)
    }

    pub(super) fn read_all(&mut self) -> Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).map_err(io_failure)?;
        Ok(bytes)
    }

    pub(super) fn rename(&mut self, destination: &Path) -> Result<()> {
        rename_handle(self.file.as_raw_handle(), destination)?;
        self.path = destination.to_owned();
        Ok(())
    }

    pub(super) fn mark_posix_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(super) fn mark_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(super) fn finish_deleted(self) -> Result<()> {
        let path = self.path.clone();
        drop(self);
        if path_present(&path)? {
            return Err(fail(
                EXIT_FAILURE,
                "Windows retained a handle-deleted stale object.",
            ));
        }
        Ok(())
    }

    pub(super) fn delete(self) -> Result<()> {
        self.mark_deleted()?;
        self.finish_deleted()
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) const STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES: &[&str] = &[
    "request.exact-argv",
    "token.identity",
    "self.path",
    "self.open",
    "self.acl",
    "self.identity",
    "self.sha256",
    "package.tqpkg2",
    "package.source-binding",
    "audit.environment",
    "audit.path",
    "audit.acl",
    "audit.open",
    "audit.initialize",
    "audit.event",
    "mutex.availability",
    "paths.known-folders",
    "lifecycle-lock.availability",
    "registry.inventory",
    "active-state.inventory",
    "fixture.identity",
    "fixture.sha256",
    "image.stability",
    "diagnostic.complete",
    "cleanup.rejected.before-audit",
    "cleanup.rejected.after-audit",
];

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) struct StaleSchema2Diagnostic {
    pub(super) file: File,
    pub(super) parent: PathBuf,
    pub(super) identity: String,
    pub(super) operation: String,
    pub(super) sequence: u32,
    pub(super) chain: [u8; 32],
}

#[cfg(feature = "stale-schema2-cleanup")]
impl StaleSchema2Diagnostic {
    pub(super) fn open() -> Result<Self> {
        let path = std::env::var_os("TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| {
                fail(
                    EXIT_REJECTED,
                    "TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH is required.",
                )
            })?;
        if !path.is_absolute() {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnostic path must be absolute.",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic path has no parent."))?
            .to_owned();
        assert_plain_directory(&parent)?;
        let mut file = OpenOptions::new()
            .append(true)
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
            .open(&path)
            .map_err(|_| {
                fail(
                    EXIT_REJECTED,
                    "Administrator must pre-create the protected diagnostic file.",
                )
            })?;
        if !protected_file_handle_acl_is_exact(&file)? {
            return Err(fail(
                EXIT_REJECTED,
                "Stale schema-2 diagnostic is not administrator protected.",
            ));
        }
        let identity = file_identity_text(&file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut prior = Vec::new();
        file.read_to_end(&mut prior).map_err(io_failure)?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| {
            fail(
                EXIT_REJECTED,
                "Diagnostic operation randomness is unavailable.",
            )
        })?;
        let operation = hex_hash(&nonce);
        let mut initial = Sha256::new();
        initial.update(b"TalkingQuill/stale-schema2-diagnostic-chain/v1\0");
        initial.update(identity.as_bytes());
        initial.update(&prior);
        initial.update(operation.as_bytes());
        Ok(Self {
            file,
            parent,
            identity,
            operation,
            sequence: 0,
            chain: initial.finalize().into(),
        })
    }

    pub(super) fn record(
        &mut self,
        stage_code: &str,
        outcome: &str,
        evidence: serde_json::Value,
    ) -> Result<()> {
        if !STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES.contains(&stage_code)
            || !matches!(outcome, "passed" | "rejected")
        {
            return Err(fail(EXIT_REJECTED, "Diagnostic stage code is invalid."));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| fail(EXIT_REJECTED, "Diagnostic sequence is exhausted."))?;
        let previous = hex_hash(&self.chain);
        let unsigned = serde_json::json!({
            "schemaVersion": 1,
            "operation": "stale-schema2-diagnostic",
            "operationId": self.operation,
            "sequence": self.sequence,
            "diagnosticIdentity": self.identity,
            "stageCode": stage_code,
            "outcome": outcome,
            "evidence": evidence,
            "previousSha256": previous,
        });
        let bytes = serde_json::to_vec(&unsigned)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic event serialization failed."))?;
        let mut event_hash = Sha256::new();
        event_hash.update(self.chain);
        event_hash.update(&bytes);
        self.chain = event_hash.finalize().into();
        let mut event = unsigned;
        event
            .as_object_mut()
            .expect("diagnostic event is an object")
            .insert(
                "eventSha256".to_owned(),
                serde_json::Value::String(hex_hash(&self.chain)),
            );
        let mut line = serde_json::to_vec(&event)
            .map_err(|_| fail(EXIT_REJECTED, "Diagnostic event serialization failed."))?;
        line.push(b'\n');
        self.file.write_all(&line).map_err(io_failure)?;
        self.file.sync_all().map_err(io_failure)?;
        flush_setup_directory(&self.parent)
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn diagnostic_stage<T, F>(
    diagnostic: &mut StaleSchema2Diagnostic,
    stage_code: &str,
    operation: F,
) -> Result<T>
where
    F: FnOnce() -> Result<(T, serde_json::Value)>,
{
    match operation() {
        Ok((value, evidence)) => {
            diagnostic.record(stage_code, "passed", evidence)?;
            Ok(value)
        }
        Err(error) => {
            let evidence = serde_json::json!({ "error": error.message });
            diagnostic.record(stage_code, "rejected", evidence)?;
            Err(fail(EXIT_REJECTED, error.message))
        }
    }
}
