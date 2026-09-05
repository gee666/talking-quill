//! Reporting, acknowledgement, and bounded durability waits.

use super::*;

impl DiagnosticTransport {
    pub(super) fn start(writer: impl Write + Send + 'static, store: Arc<dyn JournalStore>) -> Self {
        Self::start_with_options(writer, store, Arc::new(OsEntropy), true)
    }

    #[cfg(test)]
    pub(super) fn start_with_entropy(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        entropy: Arc<dyn EntropySource>,
    ) -> Self {
        Self::start_with_options(writer, store, entropy, true)
    }

    #[cfg(test)]
    pub(super) fn start_with_spawn(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        permit_spawn: bool,
    ) -> Self {
        Self::start_with_options(writer, store, Arc::new(OsEntropy), permit_spawn)
    }

    fn start_with_options(
        writer: impl Write + Send + 'static,
        store: Arc<dyn JournalStore>,
        entropy: Arc<dyn EntropySource>,
        permit_spawn: bool,
    ) -> Self {
        let loaded = store.load();
        let (journal, needs_initial_load) = match loaded {
            Ok(Some(journal)) => (journal, false),
            Ok(None) => (
                JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal()),
                false,
            ),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let quarantined = store.quarantine_corrupt().is_ok();
                let mut journal =
                    JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal());
                journal.durability_failures = 1;
                (journal, !quarantined)
            }
            Err(_) => {
                let mut journal =
                    JournalFile::new(entropy.as_ref()).unwrap_or_else(|_| uninitialized_journal());
                journal.durability_failures = 1;
                (journal, true)
            }
        };
        let durable_totals = journal
            .entries
            .iter()
            .map(|entry| (entry.dimensions.clone(), entry.counter))
            .collect();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                journal,
                durable_totals,
                journal_dirty: true,
                journal_available: false,
                needs_initial_load,
                journal_revision: 1,
                commit_waiters: VecDeque::new(),
                persistence_worker_started: false,
                generation_prepared: false,
                stream_id: String::new(),
                raw: VecDeque::new(),
                raw_dropped: 0,
            }),
            changed: Condvar::new(),
            store,
            entropy,
        });
        let persistence_shared = Arc::clone(&shared);
        let persistence_spawned = std::thread::Builder::new()
            .name("talking-quill-diagnostic-journal".into())
            .spawn(move || persistence_loop(persistence_shared))
            .is_ok();
        {
            let mut state = lock_state(&shared);
            state.persistence_worker_started = persistence_spawned;
            shared.changed.notify_all();
        }
        let worker_shared = Arc::clone(&shared);
        let spawned = permit_spawn
            && std::thread::Builder::new()
                .name("talking-quill-diagnostic-writer".into())
                .spawn(move || writer_loop(worker_shared, writer))
                .is_ok();
        if !spawned {
            let mut state = lock_state(&shared);
            state.journal.writer_start_failures =
                state.journal.writer_start_failures.saturating_add(1);
            state.journal_dirty = true;
            state.journal_revision = state.journal_revision.saturating_add(1);
            shared.changed.notify_all();
        }
        Self {
            shared,
            writer_started: spawned,
        }
    }

    pub(super) fn report_owner(
        &self,
        diagnostic: OwnerClientDiagnostic,
        health_refresh: &'static str,
        owner_process_state: OwnerProcessState,
    ) -> Result<(), DiagnosticDurabilityError> {
        let key =
            OwnerDiagnosticKey::from_diagnostic(diagnostic, health_refresh, owner_process_state);
        let (reply_tx, reply_rx) = bounded(1);
        let mut state = lock_state(&self.shared);
        let total = match state
            .journal
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&key))
        {
            Ok(index) => {
                let counter = &mut state.journal.entries[index].counter;
                match counter.total.checked_add(1) {
                    Some(total) => counter.total = total,
                    None => counter.overflowed = true,
                }
                counter.total
            }
            Err(index) => {
                state.journal.entries.insert(
                    index,
                    JournalEntry {
                        dimensions: key.clone(),
                        counter: JournalCounter {
                            total: 1,
                            ..JournalCounter::default()
                        },
                    },
                );
                1
            }
        };
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
        if !state.persistence_worker_started {
            self.shared.changed.notify_all();
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if state.commit_waiters.len() >= COMMIT_WAITER_CAPACITY {
            self.shared.changed.notify_all();
            return Err(DiagnosticDurabilityError::QueueFull);
        }
        state.commit_waiters.push_back(CommitWaiter {
            target: CommitTarget::Report(key, total),
            reply: reply_tx,
        });
        self.shared.changed.notify_all();
        drop(state);
        reply_rx
            .recv_timeout(DURABILITY_WAIT)
            .unwrap_or(Err(DiagnosticDurabilityError::TimedOut))
    }

    pub(super) fn report_raw(&self, line: String) {
        let mut state = lock_state(&self.shared);
        if state.raw.len() < RAW_QUEUE_CAPACITY {
            state.raw.push_back(line);
        } else {
            state.raw_dropped = state.raw_dropped.saturating_add(1);
        }
        self.shared.changed.notify_all();
    }

    pub(super) fn acknowledge(&self, ack: &DiagnosticAck) -> Result<(), DiagnosticDurabilityError> {
        if !valid_identity(&ack.journal_id)
            || !valid_identity(&ack.journal_nonce)
            || !ack.dimensions.valid()
        {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        let Ok(count) = ack.count.parse::<u128>() else {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        };
        let mut state = lock_state(&self.shared);
        if !state.generation_prepared || !valid_identity(&state.stream_id) {
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if ack.journal_id != state.journal.journal_id
            || ack.journal_nonce != state.journal.journal_nonce
        {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        let Ok(index) = state
            .journal
            .entries
            .binary_search_by(|entry| entry.dimensions.cmp(&ack.dimensions))
        else {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        };
        let previous = state.journal.entries[index].counter.acknowledged;
        let total = state.journal.entries[index].counter.total;
        if count < previous || count > total {
            return Err(DiagnosticDurabilityError::InvalidAcknowledgement);
        }
        if !state.persistence_worker_started {
            return Err(DiagnosticDurabilityError::WorkerUnavailable);
        }
        if state.commit_waiters.len() >= COMMIT_WAITER_CAPACITY {
            return Err(DiagnosticDurabilityError::QueueFull);
        }
        let (reply_tx, reply_rx) = bounded(1);
        state.journal.entries[index].counter.acknowledged = count;
        state.journal_dirty = true;
        state.journal_revision = state.journal_revision.saturating_add(1);
        state.commit_waiters.push_back(CommitWaiter {
            target: CommitTarget::Acknowledgement(ack.dimensions.clone(), count),
            reply: reply_tx,
        });
        self.shared.changed.notify_all();
        drop(state);
        reply_rx
            .recv_timeout(DURABILITY_WAIT)
            .unwrap_or(Err(DiagnosticDurabilityError::TimedOut))
    }

    pub(crate) fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock_state(&self.shared);
        loop {
            if !state.journal_dirty
                && pending_owner(&state).is_none()
                && state.raw.is_empty()
                && state.raw_dropped == 0
            {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || !self.writer_started {
                return false;
            }
            let waited = self
                .shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(now));
            match waited {
                Ok((updated, result)) => {
                    state = updated;
                    if result.timed_out() {
                        return !state.journal_dirty
                            && pending_owner(&state).is_none()
                            && state.raw.is_empty()
                            && state.raw_dropped == 0;
                    }
                }
                Err(poisoned) => {
                    let (mut updated, _) = poisoned.into_inner();
                    updated.journal.synchronization_recoveries =
                        updated.journal.synchronization_recoveries.saturating_add(1);
                    updated.journal_dirty = true;
                    updated.journal_revision = updated.journal_revision.saturating_add(1);
                    self.shared.state.clear_poison();
                    self.shared.changed.notify_all();
                    state = updated;
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) fn poison_state(&self) {
        let shared = Arc::clone(&self.shared);
        let _ = std::thread::spawn(move || {
            let _guard = shared.state.lock().unwrap();
            panic!("injected diagnostic state poison");
        })
        .join();
    }
}
