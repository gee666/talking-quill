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
