//! External ordering contracts.

use super::*;

#[test]
fn unpaired_external_down_submits_without_holding_shutdown_for_an_up() {
    let (context, _outbound, _commands) = test_context();
    context.state.stopping.store(true, Ordering::Release);
    context
        .state
        .recovery_deferred_mode
        .store(true, Ordering::Release);
    {
        let mut journal = context.recovery_edges.lock().unwrap();
        assert!(journal.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 16,
            is_down: true,
            source: InputSource::External,
            source_pid: 901,
            hidden_generation: true,
            ..injection::DeferredEvent::EMPTY
        }));
        assert_eq!(journal.ready_len(), 1);
        assert!(journal.external_collection_deadline.is_some());
        assert!(journal.begin_submission(injection::OperationToken::for_test(390), 0, 1));
        let edge = journal.expected().unwrap();
        journal.observe_foreground_exposure(edge);
        assert_eq!(
            journal.advance_observation(),
            GapBarrierObservation::Complete
        );
        assert!(!journal.has_pending());
        assert!(journal.external_collection_deadline.is_none());
    }
    assert!(try_clear_recovery_deferred_mode(&context));
    assert!(!pending_native_work(&context));

    let mut bounded = RecoveryEdgeJournal::default();
    let (generation, foreground_balance, hidden_generation) = bounded
        .observe_key_phase(InputSource::Physical, -1, 7, true, false, false)
        .unwrap();
    assert!(bounded.append(injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_DOWN,
        key_code: 7,
        is_down: true,
        source: InputSource::Physical,
        source_pid: -1,
        generation,
        foreground_balance,
        hidden_generation,
        ..injection::DeferredEvent::EMPTY
    }));
    assert!(bounded.append(injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_DOWN,
        key_code: 8,
        is_down: true,
        source: InputSource::External,
        source_pid: 903,
        ..injection::DeferredEvent::EMPTY
    }));
    bounded.external_collection_deadline = Some(Instant::now());
    assert!(bounded.expire_external_collection(Instant::now()));
    assert_eq!(bounded.pending_len, 1);
    assert_eq!(bounded.pending[0].source_pid, 903);
    assert!(bounded.physical_fences_pending());
    assert_eq!(bounded.ready_len(), 1);
}

#[test]
fn delayed_external_up_joins_next_batch_without_reordering_down() {
    let mut journal = RecoveryEdgeJournal::default();
    assert!(journal.append(injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_DOWN,
        key_code: 16,
        is_down: true,
        source: InputSource::External,
        source_pid: 902,
        ..injection::DeferredEvent::EMPTY
    }));
    assert!(journal.begin_submission(injection::OperationToken::for_test(391), 0, 1));
    assert!(journal.append(injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_UP,
        key_code: 16,
        source: InputSource::External,
        source_pid: 902,
        ..injection::DeferredEvent::EMPTY
    }));
    assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_DOWN);
    assert_eq!(journal.tail[0].event_type, ffi::K_CG_EVENT_KEY_UP);
    let down = journal.expected().unwrap();
    journal.observe_foreground_exposure(down);
    assert_eq!(
        journal.advance_observation(),
        GapBarrierObservation::Complete
    );
    assert_eq!(journal.pending_len, 1);
    assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
    assert_eq!(journal.ready_len(), 1);
}

#[test]
fn submitted_and_tail_buffers_roll_over_at_maximum_without_reordering() {
    fn edge(index: usize) -> injection::DeferredEvent {
        injection::DeferredEvent {
            event_type: if index.is_multiple_of(2) {
                ffi::K_CG_EVENT_KEY_DOWN
            } else {
                ffi::K_CG_EVENT_LEFT_MOUSE_UP
            },
            original_timestamp: index as u64,
            ..injection::DeferredEvent::EMPTY
        }
    }

    let mut journal = RecoveryEdgeJournal::default();
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        assert!(journal.append(edge(index)));
    }
    assert!(journal.begin_submission(
        injection::OperationToken::for_test(401),
        0,
        injection::DEFERRED_EDGE_CAPACITY,
    ));
    assert_eq!(journal.next_pool_bank, 1);
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        assert!(journal.append(edge(injection::DEFERRED_EDGE_CAPACITY + index)));
    }
    assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
    assert_eq!(journal.tail_len, injection::DEFERRED_EDGE_CAPACITY);
    assert!(!journal.overflow);
    for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
        let _ = journal.advance_observation();
    }
    assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
    assert_eq!(journal.tail_len, 0);
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        assert_eq!(
            journal.pending[index].original_timestamp,
            (injection::DEFERRED_EDGE_CAPACITY + index) as u64
        );
    }
    assert!(journal.begin_submission(
        injection::OperationToken::for_test(402),
        1,
        injection::DEFERRED_EDGE_CAPACITY,
    ));
    assert_eq!(journal.next_pool_bank, 0);
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        assert!(journal.append(edge(2 * injection::DEFERRED_EDGE_CAPACITY + index)));
    }
    for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
        let _ = journal.advance_observation();
    }
    assert_eq!(journal.pending[0].original_timestamp, 128);
    assert!(!journal.overflow);
}

#[test]
fn malformed_submitted_suffix_retains_balance_for_already_exposed_down() {
    let mut journal = RecoveryEdgeJournal::default();
    for (event_type, is_down) in [
        (ffi::K_CG_EVENT_KEY_DOWN, true),
        (ffi::K_CG_EVENT_KEY_UP, false),
    ] {
        assert!(journal.append(injection::DeferredEvent {
            event_type,
            key_code: 16,
            is_down,
            source: InputSource::External,
            source_pid: 710,
            ..injection::DeferredEvent::EMPTY
        }));
    }
    assert!(journal.begin_submission(injection::OperationToken::for_test(410), 0, 2));
    let down = journal.expected().unwrap();
    journal.observe_foreground_exposure(down);
    assert_eq!(journal.advance_observation(), GapBarrierObservation::Down);
    journal.abort_submission_to_overflow();
    journal.settle_overflow();
    assert_eq!(journal.pending_len, 1);
    assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
    assert!(journal.pending[0].foreground_balance);
}
