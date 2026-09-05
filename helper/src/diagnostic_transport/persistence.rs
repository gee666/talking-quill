//! Snapshot persistence and durable waiter completion.

use super::*;

pub(super) fn persistence_loop(shared: Arc<Shared>) {
    loop {
        let needs_initial_load = lock_state(&shared).needs_initial_load;
        if needs_initial_load {
            match shared.store.load() {
                Ok(Some(mut loaded)) => {
                    let mut state = lock_state(&shared);
                    merge_pending_into_loaded(&state.journal, &mut loaded);
                    state.journal = loaded;
                    state.needs_initial_load = false;
                    state.journal_available = false;
                    state.generation_prepared = false;
                    state.stream_id.clear();
                    state.journal_dirty = true;
                    state.journal_revision = state.journal_revision.saturating_add(1);
                    shared.changed.notify_all();
                }
                Ok(None) => {
                    let mut state = lock_state(&shared);
                    state.needs_initial_load = false;
                    state.journal_available = false;
                    state.journal_dirty = true;
                    state.journal_revision = state.journal_revision.saturating_add(1);
                    shared.changed.notify_all();
                }
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    if shared.store.quarantine_corrupt().is_ok() {
                        let mut state = lock_state(&shared);
                        state.needs_initial_load = false;
                        state.journal_available = false;
                        state.generation_prepared = false;
                        state.journal.journal_id.clear();
                        state.journal.journal_nonce.clear();
                        state.stream_id.clear();
                        state.journal.durability_failures =
                            state.journal.durability_failures.saturating_add(1);
                        state.journal_dirty = true;
                        state.journal_revision = state.journal_revision.saturating_add(1);
                        shared.changed.notify_all();
                    } else {
                        std::thread::sleep(RETRY_INTERVAL);
                    }
                }
                Err(_) => {
                    std::thread::sleep(RETRY_INTERVAL);
                }
            }
            continue;
        }

        let identities_ready = {
            let mut state = lock_state(&shared);
            let result = prepare_process_identities(&mut state, shared.entropy.as_ref());
            if result.is_err() {
                state.journal_available = false;
                state.journal.durability_failures =
                    state.journal.durability_failures.saturating_add(1);
                state.journal_dirty = true;
            }
            result.is_ok()
        };
        if !identities_ready {
            shared.changed.notify_all();
            std::thread::sleep(RETRY_INTERVAL);
            continue;
        }

        let (snapshot, revision) = {
            let mut state = lock_state(&shared);
            while !state.journal_dirty {
                state = match shared.changed.wait(state) {
                    Ok(updated) => updated,
                    Err(poisoned) => {
                        let mut updated = poisoned.into_inner();
                        updated.journal.synchronization_recoveries =
                            updated.journal.synchronization_recoveries.saturating_add(1);
                        updated.journal_dirty = true;
                        updated.journal_revision = updated.journal_revision.saturating_add(1);
                        shared.state.clear_poison();
                        updated
                    }
                };
            }
            (state.journal.clone(), state.journal_revision)
        };

        match shared.store.store(&snapshot) {
            Ok(()) => {
                let durable = snapshot
                    .entries
                    .iter()
                    .map(|entry| (entry.dimensions.clone(), entry.counter))
                    .collect::<BTreeMap<_, _>>();
                let mut state = lock_state(&shared);
                state.durable_totals = durable;
                state.journal_available = snapshot.valid()
                    && state.generation_prepared
                    && valid_identity(&state.stream_id)
                    && state.stream_id != snapshot.journal_id
                    && state.stream_id != snapshot.journal_nonce;
                if state.journal_revision == revision {
                    state.journal_dirty = false;
                }
                let mut pending = VecDeque::new();
                while let Some(waiter) = state.commit_waiters.pop_front() {
                    if target_is_durable(&waiter.target, &state.durable_totals) {
                        let _ = waiter.reply.try_send(Ok(()));
                    } else {
                        pending.push_back(waiter);
                    }
                }
                state.commit_waiters = pending;
                shared.changed.notify_all();
            }
            Err(_) => {
                let mut state = lock_state(&shared);
                state.journal_available = false;
                state.journal.durability_failures =
                    state.journal.durability_failures.saturating_add(1);
                state.journal_dirty = true;
                state.journal_revision = state.journal_revision.saturating_add(1);
                shared.changed.notify_all();
                drop(state);
                std::thread::sleep(RETRY_INTERVAL);
            }
        }
    }
}

fn merge_pending_into_loaded(pending: &JournalFile, loaded: &mut JournalFile) {
    loaded.writer_start_failures = loaded
        .writer_start_failures
        .saturating_add(pending.writer_start_failures);
    loaded.synchronization_recoveries = loaded
        .synchronization_recoveries
        .saturating_add(pending.synchronization_recoveries);
    loaded.durability_failures = loaded
        .durability_failures
        .saturating_add(pending.durability_failures);
    for pending_entry in &pending.entries {
        match loaded
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&pending_entry.dimensions))
        {
            Ok(index) => {
                loaded.entries[index].counter.total = loaded.entries[index]
                    .counter
                    .total
                    .saturating_add(pending_entry.counter.total);
                loaded.entries[index].counter.overflowed |= pending_entry.counter.overflowed;
            }
            Err(index) => loaded.entries.insert(index, pending_entry.clone()),
        }
    }
}

fn target_is_durable(
    target: &CommitTarget,
    durable: &BTreeMap<OwnerDiagnosticKey, JournalCounter>,
) -> bool {
    match target {
        CommitTarget::Report(key, total) => durable
            .get(key)
            .is_some_and(|counter| counter.total >= *total),
        CommitTarget::Acknowledgement(key, count) => durable
            .get(key)
            .is_some_and(|counter| counter.acknowledged >= *count),
    }
}
