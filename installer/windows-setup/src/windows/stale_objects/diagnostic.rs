//! Append-only, identity-bound stale diagnostic events.
use super::*;

#[cfg(feature = "stale-schema2-cleanup")]
pub(in super::super) const STALE_SCHEMA2_DIAGNOSTIC_STAGE_CODES: &[&str] = &[
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
pub(in super::super) struct StaleSchema2Diagnostic {
    pub(in super::super) file: File,
    pub(in super::super) parent: PathBuf,
    pub(in super::super) identity: String,
    pub(in super::super) operation: String,
    pub(in super::super) sequence: u32,
    pub(in super::super) chain: [u8; 32],
}

#[cfg(feature = "stale-schema2-cleanup")]
impl StaleSchema2Diagnostic {
    pub(in super::super) fn open() -> Result<Self> {
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

    pub(in super::super) fn record(
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
pub(in super::super) fn diagnostic_stage<T, F>(
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
