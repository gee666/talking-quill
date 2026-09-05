use super::*;
use std::sync::Arc;
#[cfg(windows)]
#[derive(Clone, Copy)]
enum RegisteredSubsetStage {
    Leaf,
    Physical,
}

#[cfg(windows)]
fn forced_registered_subset_snapshot(
    before_subset: fn(&TransactionObservability),
    publish_subset: fn(&TransactionObservability),
    stage: RegisteredSubsetStage,
) -> (RegisteredInputCounters, RegisteredInputCounters) {
    let observability = Arc::new(TransactionObservability::new());
    let writer = Arc::clone(&observability);
    let base_recorded = Arc::new(std::sync::Barrier::new(2));
    let publish = Arc::new(std::sync::Barrier::new(2));
    let subset_recorded = Arc::new(std::sync::Barrier::new(2));
    let writer_base_recorded = Arc::clone(&base_recorded);
    let writer_publish = Arc::clone(&publish);
    let writer_subset_recorded = Arc::clone(&subset_recorded);
    let thread = std::thread::spawn(move || {
        before_subset(&writer);
        writer_base_recorded.wait();
        writer_publish.wait();
        publish_subset(&writer);
        writer_subset_recorded.wait();
    });
    let interleave = || {
        base_recorded.wait();
        publish.wait();
        subset_recorded.wait();
    };
    let forced = match stage {
        RegisteredSubsetStage::Leaf => {
            observability.load_registered_input_after_subsets(interleave, || {})
        }
        RegisteredSubsetStage::Physical => {
            observability.load_registered_input_after_subsets(|| {}, interleave)
        }
    };
    thread.join().unwrap();
    (forced, observability.snapshot().registered_input)
}

#[cfg(windows)]
#[test]
fn pump_snapshot_reads_subset_before_hook_installation_base() {
    let (forced, current) = forced_registered_subset_snapshot(
        TransactionObservability::record_hook_installed,
        TransactionObservability::record_pump_alive,
        RegisteredSubsetStage::Leaf,
    );
    assert_eq!((forced.hook_installed, forced.pump_alive), (1, 0));
    assert_eq!((current.hook_installed, current.pump_alive), (1, 1));
}

#[cfg(windows)]
#[test]
fn physical_snapshot_reads_subset_before_hc_action_base() {
    let (forced, current) = forced_registered_subset_snapshot(
        TransactionObservability::record_hc_action_callback,
        TransactionObservability::record_physical_callback,
        RegisteredSubsetStage::Physical,
    );
    assert_eq!(
        (forced.hc_action_callbacks, forced.physical_callbacks),
        (1, 0)
    );
    assert_eq!(
        (current.hc_action_callbacks, current.physical_callbacks),
        (1, 1)
    );
}

#[cfg(windows)]
#[test]
fn filtered_snapshot_reads_subset_before_physical_base() {
    fn record_physical_base(observability: &TransactionObservability) {
        observability.record_hc_action_callback();
        observability.record_physical_callback();
    }

    let (forced, current) = forced_registered_subset_snapshot(
        record_physical_base,
        TransactionObservability::record_physical_callback_filtered,
        RegisteredSubsetStage::Leaf,
    );
    assert_eq!(
        (
            forced.physical_callbacks,
            forced.physical_callbacks_filtered,
        ),
        (1, 0)
    );
    assert_eq!(
        (
            current.physical_callbacks,
            current.physical_callbacks_filtered,
        ),
        (1, 1)
    );
}

#[cfg(windows)]
#[test]
fn candidate_snapshot_reads_subset_before_physical_base() {
    fn record_physical_base(observability: &TransactionObservability) {
        observability.record_hc_action_callback();
        observability.record_physical_callback();
    }

    let (forced, current) = forced_registered_subset_snapshot(
        record_physical_base,
        TransactionObservability::record_registered_candidate_callback,
        RegisteredSubsetStage::Leaf,
    );
    assert_eq!(
        (
            forced.physical_callbacks,
            forced.registered_candidate_callbacks,
        ),
        (1, 0)
    );
    assert_eq!(
        (
            current.physical_callbacks,
            current.registered_candidate_callbacks,
        ),
        (1, 1)
    );
}

#[cfg(windows)]
#[test]
fn release_snapshot_reads_subset_before_match_base() {
    let (forced, current) = forced_registered_subset_snapshot(
        TransactionObservability::record_registered_match_callback,
        TransactionObservability::record_registered_release_callback,
        RegisteredSubsetStage::Leaf,
    );
    assert_eq!(
        (
            forced.registered_match_callbacks,
            forced.registered_release_callbacks,
        ),
        (1, 0)
    );
    assert_eq!(
        (
            current.registered_match_callbacks,
            current.registered_release_callbacks,
        ),
        (1, 1)
    );
}

#[cfg(windows)]
#[test]
fn concurrent_registered_subset_reads_preserve_all_call_order_invariants() {
    const WRITERS: usize = 4;
    const ITERATIONS: u64 = 2_000;

    let observability = Arc::new(TransactionObservability::new());
    observability.record_hook_installed();
    observability.record_pump_alive();
    let midpoint = Arc::new(std::sync::Barrier::new(WRITERS + 1));
    let resume = Arc::new(std::sync::Barrier::new(WRITERS + 1));
    let leaf_updates_finished = Arc::new(std::sync::Barrier::new(WRITERS + 1));
    let physical_updates_start = Arc::new(std::sync::Barrier::new(WRITERS + 1));
    let physical_updates_finished = Arc::new(std::sync::Barrier::new(WRITERS + 1));
    let threads = (0..WRITERS)
        .map(|_| {
            let writer = Arc::clone(&observability);
            let midpoint = Arc::clone(&midpoint);
            let resume = Arc::clone(&resume);
            let leaf_updates_finished = Arc::clone(&leaf_updates_finished);
            let physical_updates_start = Arc::clone(&physical_updates_start);
            let physical_updates_finished = Arc::clone(&physical_updates_finished);
            std::thread::spawn(move || {
                for iteration in 1..=ITERATIONS {
                    writer.record_hc_action_callback();
                    writer.record_physical_callback();
                    writer.record_physical_callback_filtered();
                    writer.record_registered_candidate_callback();
                    writer.record_registered_match_callback();
                    writer.record_registered_release_callback();
                    match iteration {
                        value if value == ITERATIONS / 2 => {
                            midpoint.wait();
                            resume.wait();
                        }
                        value if value == ITERATIONS / 2 + 1 => {
                            leaf_updates_finished.wait();
                            physical_updates_start.wait();
                        }
                        value if value == ITERATIONS / 2 + 2 => {
                            physical_updates_finished.wait();
                        }
                        _ => {}
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    midpoint.wait();
    let guaranteed_concurrent_sample = observability.load_registered_input_after_subsets(
        || {
            resume.wait();
            leaf_updates_finished.wait();
        },
        || {
            physical_updates_start.wait();
            physical_updates_finished.wait();
        },
    );
    assert_registered_subset_invariants(guaranteed_concurrent_sample);
    assert!(guaranteed_concurrent_sample.physical_callbacks > 0);
    while threads.iter().any(|thread| !thread.is_finished()) {
        assert_registered_subset_invariants(observability.snapshot().registered_input);
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert_registered_subset_invariants(observability.snapshot().registered_input);
}

#[cfg(windows)]
fn assert_registered_subset_invariants(snapshot: RegisteredInputCounters) {
    assert!(snapshot.pump_alive <= snapshot.hook_installed);
    assert!(snapshot.physical_callbacks <= snapshot.hc_action_callbacks);
    assert!(snapshot.physical_callbacks_filtered <= snapshot.physical_callbacks);
    assert!(snapshot.registered_candidate_callbacks <= snapshot.physical_callbacks);
    assert!(snapshot.registered_release_callbacks <= snapshot.registered_match_callbacks);
}
