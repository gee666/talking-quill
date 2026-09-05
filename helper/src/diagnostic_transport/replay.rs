//! Durable owner replay and bounded raw output.

use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnerReplayRecord<'a> {
    event: &'static str,
    journal_id: &'a str,
    journal_nonce: &'a str,
    stream_id: &'a str,
    process_generation: String,
    category: &'a str,
    operation: &'a str,
    correlation_status: &'a str,
    health_refresh: &'a str,
    transport_status: &'a str,
    owner_process_state: &'a str,
    count: String,
    counter_overflow: bool,
    durable: bool,
    durability_failures: String,
    writer_start_failures: String,
    synchronization_recoveries: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RawOverflowRecord {
    event: &'static str,
    dropped_count: String,
}

pub(super) fn pending_owner(state: &State) -> Option<(OwnerDiagnosticKey, JournalCounter)> {
    if !state.journal_available || !state.generation_prepared || !valid_identity(&state.stream_id) {
        return None;
    }
    state.durable_totals.iter().find_map(|(key, counter)| {
        (counter.total > counter.acknowledged || counter.overflowed)
            .then(|| (key.clone(), *counter))
    })
}

pub(super) fn writer_loop(shared: Arc<Shared>, mut writer: impl Write) {
    let mut last_sent: Option<(OwnerDiagnosticKey, u128, Instant)> = None;
    loop {
        enum Pending {
            Owner(OwnerDiagnosticKey, JournalCounter),
            Raw(String),
            RawOverflow(u128),
        }
        let pending = {
            let mut state = lock_state(&shared);
            loop {
                if let Some((key, counter)) = pending_owner(&state) {
                    let replay_due = last_sent.as_ref().is_none_or(|(sent_key, sent_count, at)| {
                        sent_key != &key
                            || *sent_count != counter.total
                            || at.elapsed() >= REPLAY_INTERVAL
                    });
                    if replay_due {
                        break Pending::Owner(key, counter);
                    }
                } else if state.raw_dropped != 0 {
                    break Pending::RawOverflow(state.raw_dropped);
                } else if let Some(line) = state.raw.front().cloned() {
                    break Pending::Raw(line);
                }
                let waited = shared.changed.wait_timeout(state, RETRY_INTERVAL);
                state = match waited {
                    Ok((updated, _)) => updated,
                    Err(poisoned) => {
                        let (mut updated, _) = poisoned.into_inner();
                        updated.journal.synchronization_recoveries =
                            updated.journal.synchronization_recoveries.saturating_add(1);
                        updated.journal_dirty = true;
                        updated.journal_revision = updated.journal_revision.saturating_add(1);
                        shared.state.clear_poison();
                        updated
                    }
                };
            }
        };

        let line = {
            let state = lock_state(&shared);
            match &pending {
                Pending::Owner(key, counter) => serde_json::to_string(&OwnerReplayRecord {
                    event: "helper.owner.connection.replay",
                    journal_id: &state.journal.journal_id,
                    journal_nonce: &state.journal.journal_nonce,
                    stream_id: &state.stream_id,
                    process_generation: state.journal.process_generation.to_string(),
                    category: &key.category,
                    operation: &key.operation,
                    correlation_status: &key.correlation_status,
                    health_refresh: &key.health_refresh,
                    transport_status: &key.transport_status,
                    owner_process_state: &key.owner_process_state,
                    count: counter.total.to_string(),
                    counter_overflow: counter.overflowed,
                    durable: state.journal_available && !state.journal_dirty,
                    durability_failures: state.journal.durability_failures.to_string(),
                    writer_start_failures: state.journal.writer_start_failures.to_string(),
                    synchronization_recoveries: state
                        .journal
                        .synchronization_recoveries
                        .to_string(),
                }),
                Pending::Raw(line) => Ok(line.trim_end_matches(['\r', '\n']).to_owned()),
                Pending::RawOverflow(count) => serde_json::to_string(&RawOverflowRecord {
                    event: "helper.diagnostic.raw_overflow",
                    dropped_count: count.to_string(),
                }),
            }
        };
        let Ok(line) = line else {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        };
        let framed = format!("\n{line}\n");
        if writer
            .write_all(framed.as_bytes())
            .and_then(|()| writer.flush())
            .is_err()
        {
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        }

        let mut state = lock_state(&shared);
        match pending {
            Pending::Owner(key, sent) => last_sent = Some((key, sent.total, Instant::now())),
            Pending::Raw(line) => {
                if state.raw.front() == Some(&line) {
                    state.raw.pop_front();
                }
            }
            Pending::RawOverflow(sent) => {
                state.raw_dropped = state.raw_dropped.saturating_sub(sent);
            }
        }
        shared.changed.notify_all();
    }
}
