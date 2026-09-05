//! Source retention contracts.

use super::*;

#[test]
fn external_source_slots_pin_buffer_exposure_and_fence_references() {
    let mut journal = RecoveryEdgeJournal::default();
    for pid in 10..18 {
        assert!(journal.source_slot(InputSource::External, pid).is_some());
    }
    assert!(journal.append(injection::DeferredEvent {
        source: InputSource::External,
        source_pid: 10,
        event_type: ffi::K_CG_EVENT_KEY_DOWN,
        ..injection::DeferredEvent::EMPTY
    }));
    assert!(journal.begin_submission(injection::OperationToken::for_test(388), 0, 1));
    assert!(journal.append(injection::DeferredEvent {
        source: InputSource::External,
        source_pid: 13,
        event_type: ffi::K_CG_EVENT_KEY_UP,
        ..injection::DeferredEvent::EMPTY
    }));
    let slot_11 = journal
        .existing_source_slot(InputSource::External, 11)
        .unwrap();
    journal.exposed_keys[slot_11][16] = true;
    let slot_12 = journal
        .existing_source_slot(InputSource::External, 12)
        .unwrap();
    journal.discard_key_fences[slot_12][17] = true;
    assert!(journal.source_slot(InputSource::External, 99).is_some());
    for pid in [10, 11, 12, 13] {
        assert!(
            journal
                .existing_source_slot(InputSource::External, pid)
                .is_some()
        );
    }
    assert!(journal.external_pid_referenced(10));
}

#[test]
fn deferred_flags_changed_streams_ignore_aggregate_side_flags() {
    fn assert_stream(source: InputSource, source_pid: i64, marker: i64) {
        let (context, _outbound, _commands) = test_context();
        let events = [
            tagged_keyboard_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_CONTROL_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                marker,
            ),
            tagged_keyboard_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                RIGHT_CONTROL_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                marker,
            ),
            // Aggregate Control remains set because right Control is held;
            // the left keycode nevertheless identifies a left-side up.
            tagged_keyboard_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_CONTROL_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                marker,
            ),
            tagged_keyboard_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                RIGHT_CONTROL_KEY_CODE,
                0,
                marker,
            ),
        ];
        for event in events {
            assert_eq!(
                defer_callback_edge(
                    &context,
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    event,
                    source,
                    source_pid,
                    marker,
                    false,
                ),
                CurrentEdgeDisposition::Owned
            );
        }
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 4);
        assert_eq!(
            journal.pending[..4]
                .iter()
                .map(|edge| edge.is_down)
                .collect::<Vec<_>>(),
            vec![true, true, false, false]
        );
        assert!(journal.hidden_balanced());
        drop(journal);
        unsafe {
            for event in events {
                ffi::CFRelease(event.cast_const());
            }
        }
    }

    assert_stream(InputSource::External, 550, 0);
    assert_stream(
        InputSource::test_physical(),
        -1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
}
