use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;

use talking_quill_keyboard_core::transactional::{CancelReason, TransactionMetrics};

/// JavaScript's largest exactly representable integer. Runtime diagnostics are
/// deliberately aggregate-only and saturate here before crossing JSON-RPC.
pub(crate) const MAX_OBSERVABILITY_COUNTER: u64 = 9_007_199_254_740_991;

#[derive(Debug)]
pub(crate) struct TransactionObservability {
    publication: AtomicU64,
    started: AtomicU64,
    committed: AtomicU64,
    replayed: AtomicU64,
    cancelled: AtomicU64,
    cancellation_reasons: [AtomicU64; CancelReason::COUNT],
    journal_high_water: AtomicU64,
    replay_attempted: AtomicU64,
    replay_succeeded: AtomicU64,
    replay_partial: AtomicU64,
    replay_failed: AtomicU64,
    dummy_attempted: AtomicU64,
    dummy_succeeded: AtomicU64,
    dummy_partial: AtomicU64,
    dummy_failed: AtomicU64,
    target_validation_fallbacks: AtomicU64,
    modifier_wait_duration_ms_total: AtomicU64,
    modifier_wait_duration_ms_max: AtomicU64,
    modifier_timeouts: AtomicU64,
    shutdown_ownership_deadlines: AtomicU64,
    hook_installed: AtomicU64,
    pump_alive: AtomicU64,
    hc_action_callbacks: AtomicU64,
    physical_callbacks: AtomicU64,
    physical_callbacks_filtered: AtomicU64,
    registered_candidate_callbacks: AtomicU64,
    registered_match_callbacks: AtomicU64,
    registered_release_callbacks: AtomicU64,
    callback_channel_accepted: AtomicU64,
    callback_channel_rejected: AtomicU64,
    adapter_dequeued: AtomicU64,
}

impl TransactionObservability {
    pub(crate) fn new() -> Self {
        Self {
            publication: AtomicU64::new(0),
            started: AtomicU64::new(0),
            committed: AtomicU64::new(0),
            replayed: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            cancellation_reasons: std::array::from_fn(|_| AtomicU64::new(0)),
            journal_high_water: AtomicU64::new(0),
            replay_attempted: AtomicU64::new(0),
            replay_succeeded: AtomicU64::new(0),
            replay_partial: AtomicU64::new(0),
            replay_failed: AtomicU64::new(0),
            dummy_attempted: AtomicU64::new(0),
            dummy_succeeded: AtomicU64::new(0),
            dummy_partial: AtomicU64::new(0),
            dummy_failed: AtomicU64::new(0),
            target_validation_fallbacks: AtomicU64::new(0),
            modifier_wait_duration_ms_total: AtomicU64::new(0),
            modifier_wait_duration_ms_max: AtomicU64::new(0),
            modifier_timeouts: AtomicU64::new(0),
            shutdown_ownership_deadlines: AtomicU64::new(0),
            hook_installed: AtomicU64::new(0),
            pump_alive: AtomicU64::new(0),
            hc_action_callbacks: AtomicU64::new(0),
            physical_callbacks: AtomicU64::new(0),
            physical_callbacks_filtered: AtomicU64::new(0),
            registered_candidate_callbacks: AtomicU64::new(0),
            registered_match_callbacks: AtomicU64::new(0),
            registered_release_callbacks: AtomicU64::new(0),
            callback_channel_accepted: AtomicU64::new(0),
            callback_channel_rejected: AtomicU64::new(0),
            adapter_dequeued: AtomicU64::new(0),
        }
    }

    /// Publishes one authoritative reducer snapshot without callback locking.
    /// A reducer continuation can be cloned for rollback, so only completed
    /// owner turns call this method and monotonic maxima prevent stale clones
    /// from moving counters backwards.
    pub(crate) fn publish(&self, metrics: TransactionMetrics) {
        // Native owners are single-writer. Odd/even generations give RPC
        // readers a coherent aggregate snapshot without callback locking.
        self.publication.fetch_add(1, Ordering::AcqRel);
        publish_max(&self.started, metrics.started);
        publish_max(&self.committed, metrics.committed);
        publish_max(&self.replayed, metrics.replayed);
        publish_max(&self.cancelled, metrics.cancelled);
        for (destination, value) in self
            .cancellation_reasons
            .iter()
            .zip(metrics.cancellation_reasons)
        {
            publish_max(destination, value);
        }
        publish_max(&self.journal_high_water, metrics.journal_high_water);
        publish_max(&self.replay_attempted, metrics.replay_attempted);
        publish_max(&self.replay_succeeded, metrics.replay_succeeded);
        publish_max(&self.replay_partial, metrics.replay_partial);
        publish_max(&self.replay_failed, metrics.replay_failed);
        publish_max(&self.dummy_attempted, metrics.dummy_attempted);
        publish_max(&self.dummy_succeeded, metrics.dummy_succeeded);
        publish_max(&self.dummy_partial, metrics.dummy_partial);
        publish_max(&self.dummy_failed, metrics.dummy_failed);
        self.publication.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn record_target_validation_fallback(&self) {
        self.publish_native(|| increment_atomic(&self.target_validation_fallbacks));
    }

    fn record_modifier_wait(&self, duration: Duration) {
        self.publish_native(|| {
            let elapsed = u64::try_from(duration.as_millis())
                .unwrap_or(MAX_OBSERVABILITY_COUNTER)
                .min(MAX_OBSERVABILITY_COUNTER);
            add_atomic(&self.modifier_wait_duration_ms_total, elapsed);
            // Publish the total before the maximum. AcqRel also carries the
            // total associated with an earlier, larger maximum through later
            // fetch_max calls from other native writers.
            self.modifier_wait_duration_ms_max
                .fetch_max(elapsed, Ordering::AcqRel);
        });
    }

    pub(crate) fn record_modifier_timeout(&self) {
        self.publish_native(|| increment_atomic(&self.modifier_timeouts));
    }

    pub(crate) fn record_shutdown_ownership_deadline(&self) {
        self.publish_native(|| increment_atomic(&self.shutdown_ownership_deadlines));
    }

    #[cfg(windows)]
    pub(crate) fn record_hook_installed(&self) {
        self.publish_native(|| self.hook_installed.store(1, Ordering::Relaxed));
    }

    #[cfg(windows)]
    pub(crate) fn record_pump_alive(&self) {
        self.publish_native(|| self.pump_alive.store(1, Ordering::Relaxed));
    }

    #[cfg(windows)]
    pub(crate) fn record_hc_action_callback(&self) {
        self.publish_native(|| increment_atomic(&self.hc_action_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_physical_callback(&self) {
        self.publish_native(|| increment_atomic(&self.physical_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_physical_callback_filtered(&self) {
        self.publish_native(|| increment_atomic(&self.physical_callbacks_filtered));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_candidate_callback(&self) {
        self.publish_native(|| increment_atomic(&self.registered_candidate_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_match_callback(&self) {
        self.publish_native(|| increment_atomic(&self.registered_match_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_release_callback(&self) {
        self.publish_native(|| increment_atomic(&self.registered_release_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_callback_channel_accepted(&self) {
        self.publish_native(|| increment_atomic(&self.callback_channel_accepted));
    }

    #[cfg(windows)]
    pub(crate) fn record_callback_channel_rejected(&self) {
        self.publish_native(|| increment_atomic(&self.callback_channel_rejected));
    }

    #[cfg(windows)]
    pub(crate) fn record_adapter_dequeued(&self) {
        self.publish_native(|| increment_atomic(&self.adapter_dequeued));
    }

    fn publish_native(&self, update: impl FnOnce()) {
        // Native aggregate fields are independent monotonic atomics and have
        // multiple writers (hook, adapter, runtime). Do not enter the
        // transaction reducer's single-writer seqlock here: overlapping native
        // writers could otherwise make its generation temporarily even.
        update();
    }

    pub(crate) fn snapshot(&self) -> TransactionObservabilitySnapshot {
        loop {
            let before = self.publication.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let reason = |reason: CancelReason| load(&self.cancellation_reasons[reason.index()]);
            let (modifier_wait_duration_ms_total, modifier_wait_duration_ms_max) =
                self.load_modifier_wait_durations_after_max(|| {});
            let snapshot = TransactionObservabilitySnapshot {
                transactions: TransactionCounters {
                    started: load(&self.started),
                    committed: load(&self.committed),
                    replayed: load(&self.replayed),
                    cancelled: load(&self.cancelled),
                    journal_high_water: load(&self.journal_high_water),
                    cancellation_reasons: CancellationReasonCounters {
                        invalid_continuation: reason(CancelReason::InvalidContinuation),
                        modifier_changed: reason(CancelReason::ModifierChanged),
                        alt_gr: reason(CancelReason::AltGr),
                        journal_overflow: reason(CancelReason::JournalOverflow),
                        configuration_replaced: reason(CancelReason::ConfigurationReplaced),
                        revision_mismatch: reason(CancelReason::RevisionMismatch),
                        gate_closed: reason(CancelReason::GateClosed),
                        shutdown: reason(CancelReason::Shutdown),
                        helper_disconnected: reason(CancelReason::HelperDisconnected),
                        secure_desktop: reason(CancelReason::SecureDesktop),
                        timeout: reason(CancelReason::Timeout),
                        activation_delivery_failed: reason(CancelReason::ActivationDeliveryFailed),
                        neutralization_failed: reason(CancelReason::NeutralizationFailed),
                        replay_failed: reason(CancelReason::ReplayFailed),
                        effect_protocol_violation: reason(CancelReason::EffectProtocolViolation),
                        physical_state_mismatch: reason(CancelReason::PhysicalStateMismatch),
                        target_changed: reason(CancelReason::TargetChanged),
                    },
                },
                replay: EffectOutcomeCounters {
                    attempted: load(&self.replay_attempted),
                    succeeded: load(&self.replay_succeeded),
                    partial: load(&self.replay_partial),
                    failed: load(&self.replay_failed),
                },
                dummy: EffectOutcomeCounters {
                    attempted: load(&self.dummy_attempted),
                    succeeded: load(&self.dummy_succeeded),
                    partial: load(&self.dummy_partial),
                    failed: load(&self.dummy_failed),
                },
                registered_input: RegisteredInputCounters {
                    hook_installed: load(&self.hook_installed),
                    pump_alive: load(&self.pump_alive),
                    hc_action_callbacks: load(&self.hc_action_callbacks),
                    physical_callbacks: load(&self.physical_callbacks),
                    physical_callbacks_filtered: load(&self.physical_callbacks_filtered),
                    registered_candidate_callbacks: load(&self.registered_candidate_callbacks),
                    registered_match_callbacks: load(&self.registered_match_callbacks),
                    registered_release_callbacks: load(&self.registered_release_callbacks),
                    callback_channel_accepted: load(&self.callback_channel_accepted),
                    callback_channel_rejected: load(&self.callback_channel_rejected),
                    adapter_dequeued: load(&self.adapter_dequeued),
                },
                native_paste: NativePasteCounters {
                    target_validation_fallbacks: load(&self.target_validation_fallbacks),
                    modifier_wait_duration_ms_total,
                    modifier_wait_duration_ms_max,
                    modifier_timeouts: load(&self.modifier_timeouts),
                    shutdown_ownership_deadlines: load(&self.shutdown_ownership_deadlines),
                },
            };
            if self.publication.load(Ordering::Acquire) == before {
                return snapshot;
            }
        }
    }

    fn load_modifier_wait_durations_after_max(&self, after_max: impl FnOnce()) -> (u64, u64) {
        // The maximum is the publication edge for this related pair. Reading
        // it first means a concurrent writer can only make the following total
        // newer. Acquire pairs with record_modifier_wait's AcqRel fetch_max,
        // so a maximum already observed always carries its contributing total.
        let maximum = self
            .modifier_wait_duration_ms_max
            .load(Ordering::Acquire)
            .min(MAX_OBSERVABILITY_COUNTER);
        after_max();
        let total = load(&self.modifier_wait_duration_ms_total);
        (total, maximum)
    }
}

impl Default for TransactionObservability {
    fn default() -> Self {
        Self::new()
    }
}

fn publish_max(counter: &AtomicU64, value: u64) {
    counter.fetch_max(value.min(MAX_OBSERVABILITY_COUNTER), Ordering::Relaxed);
}

fn increment_atomic(counter: &AtomicU64) {
    add_atomic(counter, 1);
}

fn add_atomic(counter: &AtomicU64, increment: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(
            value
                .saturating_add(increment)
                .min(MAX_OBSERVABILITY_COUNTER),
        )
    });
}

fn load(counter: &AtomicU64) -> u64 {
    counter
        .load(Ordering::Relaxed)
        .min(MAX_OBSERVABILITY_COUNTER)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionObservabilitySnapshot {
    pub transactions: TransactionCounters,
    pub replay: EffectOutcomeCounters,
    pub dummy: EffectOutcomeCounters,
    pub registered_input: RegisteredInputCounters,
    pub native_paste: NativePasteCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredInputCounters {
    pub hook_installed: u64,
    pub pump_alive: u64,
    pub hc_action_callbacks: u64,
    pub physical_callbacks: u64,
    pub physical_callbacks_filtered: u64,
    pub registered_candidate_callbacks: u64,
    pub registered_match_callbacks: u64,
    pub registered_release_callbacks: u64,
    pub callback_channel_accepted: u64,
    pub callback_channel_rejected: u64,
    pub adapter_dequeued: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePasteCounters {
    pub target_validation_fallbacks: u64,
    pub modifier_wait_duration_ms_total: u64,
    pub modifier_wait_duration_ms_max: u64,
    pub modifier_timeouts: u64,
    pub shutdown_ownership_deadlines: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionCounters {
    pub started: u64,
    pub committed: u64,
    pub replayed: u64,
    pub cancelled: u64,
    pub journal_high_water: u64,
    pub cancellation_reasons: CancellationReasonCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutcomeCounters {
    pub attempted: u64,
    pub succeeded: u64,
    pub partial: u64,
    pub failed: u64,
}

/// Owner-thread observation of time spent waiting specifically for physical
/// and logical modifiers to become neutral. Target validation, clipboard work,
/// queueing, and injection are deliberately outside this interval.
pub(crate) struct ModifierNeutralWait {
    observability: Arc<TransactionObservability>,
    started_at: Option<Instant>,
}

impl ModifierNeutralWait {
    pub(crate) fn new(observability: Arc<TransactionObservability>) -> Self {
        Self {
            observability,
            started_at: None,
        }
    }

    pub(crate) fn start(&mut self) {
        self.start_at(Instant::now());
    }

    pub(crate) fn finish(&mut self) {
        self.finish_at(Instant::now());
    }

    fn start_at(&mut self, now: Instant) {
        let _ = self.started_at.get_or_insert(now);
    }

    fn finish_at(&mut self, now: Instant) {
        let Some(started_at) = self.started_at.take() else {
            return;
        };
        self.observability
            .record_modifier_wait(now.saturating_duration_since(started_at));
    }
}

impl Drop for ModifierNeutralWait {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancellationReasonCounters {
    pub invalid_continuation: u64,
    pub modifier_changed: u64,
    pub alt_gr: u64,
    pub journal_overflow: u64,
    pub configuration_replaced: u64,
    pub revision_mismatch: u64,
    pub gate_closed: u64,
    pub shutdown: u64,
    pub helper_disconnected: u64,
    pub secure_desktop: u64,
    pub timeout: u64,
    pub activation_delivery_failed: u64,
    pub neutralization_failed: u64,
    pub replay_failed: u64,
    pub effect_protocol_violation: u64,
    pub physical_state_mismatch: u64,
    pub target_changed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_contains_only_fixed_aggregate_fields() {
        let observability = TransactionObservability::new();
        let mut metrics = TransactionMetrics {
            started: 2,
            committed: 1,
            replayed: 1,
            cancelled: 1,
            journal_high_water: 3,
            replay_attempted: 2,
            replay_succeeded: 1,
            replay_partial: 1,
            dummy_attempted: 1,
            dummy_succeeded: 1,
            ..TransactionMetrics::default()
        };
        metrics.cancellation_reasons[CancelReason::InvalidContinuation.index()] = 1;
        observability.publish(metrics);

        let value = serde_json::to_value(observability.snapshot()).unwrap();
        assert_eq!(value["transactions"]["journalHighWater"], 3);
        assert_eq!(
            value["transactions"]["cancellationReasons"]["invalidContinuation"],
            1
        );
        assert_eq!(value["replay"]["partial"], 1);
        assert_eq!(value["dummy"]["succeeded"], 1);
        let encoded = value.to_string();
        assert!(!encoded.contains("shortcut"));
        assert!(!encoded.contains("targetToken"));
        assert!(!encoded.contains("targetEvidence"));
        assert!(!encoded.contains("clipboard"));
        assert!(!encoded.contains("keyStream"));
    }

    #[test]
    fn stale_snapshots_never_decrease_published_counters() {
        let observability = TransactionObservability::new();
        observability.publish(TransactionMetrics {
            started: 4,
            journal_high_water: 5,
            ..TransactionMetrics::default()
        });
        observability.publish(TransactionMetrics {
            started: 2,
            journal_high_water: 1,
            ..TransactionMetrics::default()
        });
        let snapshot = observability.snapshot();
        assert_eq!(snapshot.transactions.started, 4);
        assert_eq!(snapshot.transactions.journal_high_water, 5);
    }

    #[test]
    fn concurrent_reads_never_observe_a_torn_transaction_publication() {
        let observability = std::sync::Arc::new(TransactionObservability::new());
        let writer = std::sync::Arc::clone(&observability);
        let thread = std::thread::spawn(move || {
            for value in 1..=10_000 {
                let mut metrics = TransactionMetrics {
                    started: value,
                    cancelled: value,
                    replay_attempted: value,
                    replay_succeeded: value,
                    ..TransactionMetrics::default()
                };
                metrics.cancellation_reasons[CancelReason::InvalidContinuation.index()] = value;
                writer.publish(metrics);
            }
        });

        while !thread.is_finished() {
            let snapshot = observability.snapshot();
            assert_eq!(
                snapshot.transactions.started,
                snapshot.transactions.cancelled
            );
            assert_eq!(
                snapshot.transactions.cancelled,
                snapshot
                    .transactions
                    .cancellation_reasons
                    .invalid_continuation
            );
            assert_eq!(snapshot.replay.attempted, snapshot.replay.succeeded);
        }
        thread.join().unwrap();
    }

    #[test]
    fn native_paste_reasons_distinguish_timeout_from_other_modifier_conflicts() {
        let observability = TransactionObservability::new();
        observability.record_target_validation_fallback();
        observability.record_modifier_timeout();
        let snapshot = observability.snapshot();
        assert_eq!(snapshot.native_paste.target_validation_fallbacks, 1);
        assert_eq!(snapshot.native_paste.modifier_timeouts, 1);
    }

    #[test]
    fn modifier_wait_measures_only_the_explicit_platform_wait_interval() {
        let observability = Arc::new(TransactionObservability::new());
        let baseline = Instant::now();
        let mut wait = ModifierNeutralWait::new(Arc::clone(&observability));
        wait.start_at(baseline + Duration::from_millis(40));
        wait.start_at(baseline + Duration::from_millis(90));
        wait.finish_at(baseline + Duration::from_millis(115));
        wait.finish_at(baseline + Duration::from_millis(500));

        let snapshot = observability.snapshot();
        assert_eq!(snapshot.native_paste.modifier_wait_duration_ms_total, 75);
        assert_eq!(snapshot.native_paste.modifier_wait_duration_ms_max, 75);
    }

    #[test]
    fn native_wait_snapshot_derives_pair_across_a_forced_concurrent_update() {
        let observability = Arc::new(TransactionObservability::new());
        observability.record_modifier_wait(Duration::from_millis(10));
        let writer = Arc::clone(&observability);
        let start = Arc::new(std::sync::Barrier::new(2));
        let finished = Arc::new(std::sync::Barrier::new(2));
        let writer_start = Arc::clone(&start);
        let writer_finished = Arc::clone(&finished);
        let thread = std::thread::spawn(move || {
            writer_start.wait();
            writer.record_modifier_wait(Duration::from_millis(100));
            writer_finished.wait();
        });

        let (total, maximum) = observability.load_modifier_wait_durations_after_max(|| {
            start.wait();
            finished.wait();
        });
        thread.join().unwrap();

        assert_eq!(maximum, 10);
        assert_eq!(total, 110);
        assert!(maximum <= total);
        let current = observability.snapshot().native_paste;
        assert_eq!(current.modifier_wait_duration_ms_max, 100);
        assert_eq!(current.modifier_wait_duration_ms_total, 110);
    }

    #[test]
    fn concurrent_native_wait_reads_preserve_total_maximum_invariants() {
        const WRITERS: u64 = 4;
        const WAITS_PER_WRITER: u64 = 2_500;

        let observability = Arc::new(TransactionObservability::new());
        let start = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let finish = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let completed = Arc::new(AtomicU64::new(0));
        let threads = (0..WRITERS)
            .map(|writer_index| {
                let writer = Arc::clone(&observability);
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let completed = Arc::clone(&completed);
                std::thread::spawn(move || {
                    start.wait();
                    for duration in 1..=WAITS_PER_WRITER {
                        writer.record_modifier_wait(Duration::from_millis(duration + writer_index));
                    }
                    completed.fetch_add(1, Ordering::Release);
                    finish.wait();
                })
            })
            .collect::<Vec<_>>();
        start.wait();

        while completed.load(Ordering::Acquire) != WRITERS {
            let snapshot = observability.snapshot().native_paste;
            assert!(
                snapshot.modifier_wait_duration_ms_max <= snapshot.modifier_wait_duration_ms_total,
                "maximum {} exceeded total {}",
                snapshot.modifier_wait_duration_ms_max,
                snapshot.modifier_wait_duration_ms_total
            );
        }
        finish.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        let snapshot = observability.snapshot().native_paste;

        let expected_total = WRITERS * WAITS_PER_WRITER * (WAITS_PER_WRITER + 1) / 2
            + WAITS_PER_WRITER * WRITERS * (WRITERS - 1) / 2;
        assert_eq!(snapshot.modifier_wait_duration_ms_total, expected_total);
        assert_eq!(
            snapshot.modifier_wait_duration_ms_max,
            WAITS_PER_WRITER + WRITERS - 1
        );
    }

    #[test]
    fn native_wait_total_and_maximum_saturate_without_breaking_invariant() {
        let observability = TransactionObservability::new();
        observability
            .modifier_wait_duration_ms_total
            .store(MAX_OBSERVABILITY_COUNTER - 5, Ordering::Relaxed);
        observability
            .modifier_wait_duration_ms_max
            .store(5, Ordering::Relaxed);

        observability.record_modifier_wait(Duration::MAX);

        let snapshot = observability.snapshot().native_paste;
        assert_eq!(
            snapshot.modifier_wait_duration_ms_total,
            MAX_OBSERVABILITY_COUNTER
        );
        assert_eq!(
            snapshot.modifier_wait_duration_ms_max,
            MAX_OBSERVABILITY_COUNTER
        );
    }

    #[test]
    fn unfinished_modifier_wait_is_recorded_when_platform_work_terminates() {
        let observability = Arc::new(TransactionObservability::new());
        let mut wait = ModifierNeutralWait::new(Arc::clone(&observability));
        wait.start_at(Instant::now() - Duration::from_millis(5));
        drop(wait);

        assert!(
            observability
                .snapshot()
                .native_paste
                .modifier_wait_duration_ms_total
                >= 5
        );
    }
}
