//! Random identities and process generation preparation.

use super::*;

pub(super) trait EntropySource: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> io::Result<()>;
}

pub(super) struct OsEntropy;

impl EntropySource for OsEntropy {
    fn fill(&self, bytes: &mut [u8]) -> io::Result<()> {
        getrandom::fill(bytes).map_err(io::Error::other)
    }
}

pub(super) fn prepare_process_identities(
    state: &mut State,
    entropy: &dyn EntropySource,
) -> io::Result<()> {
    if !valid_identity(&state.journal.journal_id)
        || !valid_identity(&state.journal.journal_nonce)
        || state.journal.journal_id == state.journal.journal_nonce
    {
        let (journal_id, journal_nonce) = new_journal_identities(entropy)?;
        state.journal.journal_id = journal_id;
        state.journal.journal_nonce = journal_nonce;
        state.generation_prepared = false;
    }
    if !valid_identity(&state.stream_id)
        || state.stream_id == state.journal.journal_id
        || state.stream_id == state.journal.journal_nonce
    {
        let stream_id = fresh_identity(entropy)?;
        if stream_id == state.journal.journal_id || stream_id == state.journal.journal_nonce {
            return Err(io::Error::other("diagnostic identity collision"));
        }
        state.stream_id = stream_id;
        state.generation_prepared = false;
    }
    if !state.generation_prepared {
        state.journal.process_generation = state.journal.process_generation.saturating_add(1);
        state.generation_prepared = true;
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
    }
    Ok(())
}

pub(super) fn new_journal_identities(entropy: &dyn EntropySource) -> io::Result<(String, String)> {
    let journal_id = fresh_identity(entropy)?;
    let journal_nonce = fresh_identity(entropy)?;
    if journal_id == journal_nonce {
        return Err(io::Error::other("diagnostic identity collision"));
    }
    Ok((journal_id, journal_nonce))
}

fn fresh_identity(entropy: &dyn EntropySource) -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    entropy.fill(&mut bytes)?;
    let identity = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !valid_identity(&identity) {
        return Err(io::Error::other("invalid diagnostic entropy output"));
    }
    Ok(identity)
}

pub(super) fn os_random_identity() -> io::Result<String> {
    fresh_identity(&OsEntropy)
}

#[cfg(test)]
pub(super) fn random_identity() -> io::Result<String> {
    os_random_identity()
}

pub(super) fn uninitialized_journal() -> JournalFile {
    JournalFile {
        version: JOURNAL_VERSION,
        journal_id: String::new(),
        journal_nonce: String::new(),
        process_generation: 0,
        writer_start_failures: 0,
        synchronization_recoveries: 0,
        durability_failures: 1,
        entries: Vec::new(),
    }
}

pub(super) fn valid_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && !value.bytes().all(|byte| byte == b'0')
        && !value.bytes().all(|byte| byte == b'f')
}
