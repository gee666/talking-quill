//! Measures only the explicit native modifier-neutral wait interval.
use super::*;
use std::{sync::Arc, time::Instant};

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

#[cfg(test)]
mod tests {
    use super::*;
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
        let midpoint = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let resume = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let concurrent_updates_finished = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let finish = Arc::new(std::sync::Barrier::new(WRITERS as usize + 1));
        let completed = Arc::new(AtomicU64::new(0));
        let threads = (0..WRITERS)
            .map(|writer_index| {
                let writer = Arc::clone(&observability);
                let start = Arc::clone(&start);
                let midpoint = Arc::clone(&midpoint);
                let resume = Arc::clone(&resume);
                let concurrent_updates_finished = Arc::clone(&concurrent_updates_finished);
                let finish = Arc::clone(&finish);
                let completed = Arc::clone(&completed);
                std::thread::spawn(move || {
                    start.wait();
                    for duration in 1..=WAITS_PER_WRITER {
                        writer.record_modifier_wait(Duration::from_millis(duration + writer_index));
                        if duration == WAITS_PER_WRITER / 2 {
                            midpoint.wait();
                            resume.wait();
                        } else if duration == WAITS_PER_WRITER / 2 + 1 {
                            concurrent_updates_finished.wait();
                        }
                    }
                    completed.fetch_add(1, Ordering::Release);
                    finish.wait();
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        midpoint.wait();
        let (concurrent_total, concurrent_maximum) = observability
            .load_modifier_wait_durations_after_max(|| {
                resume.wait();
                concurrent_updates_finished.wait();
            });
        assert!(concurrent_maximum <= concurrent_total);
        assert!(concurrent_total > 0);

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
