use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

mod counters;
mod modifier_wait;
mod snapshot;
pub use counters::*;
pub(crate) use modifier_wait::ModifierNeutralWait;
#[cfg(all(test, windows))]
mod registered_input_tests;
#[cfg(test)]
mod tests;

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
        self.publish_native(|| self.pump_alive.store(1, Ordering::Release));
    }

    #[cfg(windows)]
    pub(crate) fn record_hc_action_callback(&self) {
        self.publish_native(|| increment_atomic(&self.hc_action_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_physical_callback(&self) {
        self.publish_native(|| increment_atomic_published(&self.physical_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_physical_callback_filtered(&self) {
        self.publish_native(|| increment_atomic_published(&self.physical_callbacks_filtered));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_candidate_callback(&self) {
        self.publish_native(|| increment_atomic_published(&self.registered_candidate_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_match_callback(&self) {
        self.publish_native(|| increment_atomic(&self.registered_match_callbacks));
    }

    #[cfg(windows)]
    pub(crate) fn record_registered_release_callback(&self) {
        self.publish_native(|| increment_atomic_published(&self.registered_release_callbacks));
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

#[cfg(windows)]
fn increment_atomic_published(counter: &AtomicU64) {
    add_atomic_with_order(counter, 1, Ordering::AcqRel);
}

fn add_atomic(counter: &AtomicU64, increment: u64) {
    add_atomic_with_order(counter, increment, Ordering::Relaxed);
}

fn add_atomic_with_order(counter: &AtomicU64, increment: u64, order: Ordering) {
    let _ = counter.fetch_update(order, Ordering::Relaxed, |value| {
        Some(
            value
                .saturating_add(increment)
                .min(MAX_OBSERVABILITY_COUNTER),
        )
    });
}

fn load_acquire(counter: &AtomicU64) -> u64 {
    counter
        .load(Ordering::Acquire)
        .min(MAX_OBSERVABILITY_COUNTER)
}

fn load(counter: &AtomicU64) -> u64 {
    counter
        .load(Ordering::Relaxed)
        .min(MAX_OBSERVABILITY_COUNTER)
}
