//! Journal schema and dimension validation.

use super::*;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct OwnerDiagnosticKey {
    pub(super) category: String,
    pub(super) operation: String,
    pub(super) correlation_status: String,
    pub(super) health_refresh: String,
    pub(super) transport_status: String,
    pub(super) owner_process_state: String,
}

impl OwnerDiagnosticKey {
    pub(super) fn from_diagnostic(
        diagnostic: OwnerClientDiagnostic,
        health_refresh: &'static str,
        owner_process_state: OwnerProcessState,
    ) -> Self {
        Self {
            category: diagnostic.category.into(),
            operation: diagnostic.operation.into(),
            correlation_status: diagnostic.correlation_status.into(),
            health_refresh: health_refresh.into(),
            transport_status: diagnostic.transport_status.into(),
            owner_process_state: owner_process_state.as_str().into(),
        }
    }

    pub(super) fn valid(&self) -> bool {
        one_of(
            &self.category,
            &[
                "connect",
                "disconnected",
                "uncertain",
                "protocol",
                "acquire_rejected",
                "rejected",
                "sequence_exhausted",
                "transport",
            ],
        ) && one_of(
            &self.operation,
            &[
                "capture.replace_configuration",
                "capture.set_enabled",
                "command.sequence",
                "connect.reconcile",
                "established_operation",
                "front_app.get",
                "front_app.metadata.get",
                "front_app.metadata_get",
                "health.get",
                "lease.acquire",
                "lease.release",
                "lease.renew",
                "observability.get",
                "paste.await_commit",
                "paste.inject",
                "permissions.get",
                "runtime.rollback",
                "service",
                "service.poll",
                "session.reconcile_off",
                "session.set_mode",
            ],
        ) && one_of(
            &self.correlation_status,
            &[
                "none",
                "not_established",
                "pending",
                "matched",
                "matched_initial_response",
                "mismatched",
                "unexpected_response",
                "unknown",
            ],
        ) && one_of(
            &self.health_refresh,
            &["not_attempted", "succeeded", "failed"],
        ) && one_of(
            &self.transport_status,
            &["open", "eof", "closed", "error", "backpressured", "unknown"],
        ) && one_of(&self.owner_process_state, &["running", "exited", "unknown"])
    }
}

fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

mod u128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct JournalCounter {
    #[serde(with = "u128_decimal")]
    pub(super) total: u128,
    #[serde(with = "u128_decimal")]
    pub(super) acknowledged: u128,
    pub(super) overflowed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct JournalEntry {
    pub(super) dimensions: OwnerDiagnosticKey,
    pub(super) counter: JournalCounter,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct JournalFile {
    pub(super) version: u8,
    pub(super) journal_id: String,
    pub(super) journal_nonce: String,
    pub(super) process_generation: u64,
    #[serde(with = "u128_decimal")]
    pub(super) writer_start_failures: u128,
    #[serde(with = "u128_decimal")]
    pub(super) synchronization_recoveries: u128,
    #[serde(with = "u128_decimal")]
    pub(super) durability_failures: u128,
    pub(super) entries: Vec<JournalEntry>,
}

impl JournalFile {
    pub(super) fn new(entropy: &dyn EntropySource) -> io::Result<Self> {
        let (journal_id, journal_nonce) = new_journal_identities(entropy)?;
        Ok(Self {
            version: JOURNAL_VERSION,
            journal_id,
            journal_nonce,
            process_generation: 0,
            writer_start_failures: 0,
            synchronization_recoveries: 0,
            durability_failures: 0,
            entries: Vec::new(),
        })
    }

    pub(super) fn valid(&self) -> bool {
        self.version == JOURNAL_VERSION
            && valid_identity(&self.journal_id)
            && valid_identity(&self.journal_nonce)
            && self.journal_id != self.journal_nonce
            && self.entries.len() <= 100_000
            && self.entries.iter().all(|entry| {
                entry.dimensions.valid() && entry.counter.acknowledged <= entry.counter.total
            })
            && self
                .entries
                .windows(2)
                .all(|pair| pair[0].dimensions < pair[1].dimensions)
    }
}
