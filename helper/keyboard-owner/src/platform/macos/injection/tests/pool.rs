use super::*;

#[test]
fn callback_effect_entrypoints_require_the_prebuilt_native_pool() {
    let _: fn(&mut NativeEventPool, ReplayBatch) -> Option<PreparedReplay> = prepare_replay;
    let _: fn(&mut NativeEventPool, usize, &[DeferredEvent]) -> Option<PreparedDeferredEvents> =
        prepare_deferred_events;
    let _: fn(&mut NativeEventPool, CleanupBatch) -> Option<PreparedReplay> = prepare_cleanup;
    let _: fn(&mut NativeEventPool) -> Option<OperationToken> = post_gap_barrier;
    let _: fn(&mut NativeEventPool) -> Option<OperationToken> = post_paste_barrier;
    assert_eq!(
        NATIVE_EVENT_POOL_CAPACITY,
        JOURNAL_CAPACITY
            + BARRIER_EVENT_COUNT
            + PASTE_BARRIER_EVENT_COUNT
            + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
    );
}

#[test]
fn native_pool_ranges_are_fixed_disjoint_and_cover_maximum_batch_plus_pairs() {
    assert_eq!(REPLAY_POOL_START, 0);
    assert_eq!(BARRIER_POOL_START, JOURNAL_CAPACITY);
    assert_eq!(
        PASTE_BARRIER_POOL_START,
        JOURNAL_CAPACITY + BARRIER_EVENT_COUNT
    );
    assert_eq!(
        DEFERRED_POOL_START,
        JOURNAL_CAPACITY + BARRIER_EVENT_COUNT + PASTE_BARRIER_EVENT_COUNT
    );
    let deferred_tail_pool_start = DEFERRED_POOL_START + DEFERRED_EDGE_CAPACITY;
    assert_eq!(
        deferred_tail_pool_start + DEFERRED_EDGE_CAPACITY,
        NATIVE_EVENT_POOL_CAPACITY
    );
    assert_eq!(
        NATIVE_EVENT_POOL_CAPACITY,
        JOURNAL_CAPACITY
            + BARRIER_EVENT_COUNT
            + PASTE_BARRIER_EVENT_COUNT
            + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
    );
    const {
        assert!(
            DEFERRED_POOL_START + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
                == NATIVE_EVENT_POOL_CAPACITY
        )
    };
}

#[test]
fn mach_ticks_are_converted_to_cgevent_nanoseconds_without_losing_the_abi_width() {
    assert_eq!(mach_ticks_to_event_timestamp(5, 3, 2), Some(7));
    assert_eq!(
        mach_ticks_to_event_timestamp(u64::MAX, 1, 1),
        Some(u64::MAX)
    );
    assert_eq!(mach_ticks_to_event_timestamp(1, 0, 1), None);
    assert_eq!(mach_ticks_to_event_timestamp(1, 1, 0), None);
    assert_eq!(mach_ticks_to_event_timestamp(u64::MAX, u32::MAX, 1), None);
}

#[test]
fn every_reused_batch_and_pair_post_refreshes_timestamp_immediately_and_in_order() {
    let replay = [
        REPLAY_POOL_START,
        REPLAY_POOL_START + 1,
        REPLAY_POOL_START + 2,
    ];
    let cleanup_reusing_replay_slots = [REPLAY_POOL_START, REPLAY_POOL_START + 1];
    let barrier = [BARRIER_POOL_START, BARRIER_POOL_START + 1];
    let raw_times = RefCell::new([100_u64, 99, 101, 98, 102, 102, 101].into_iter());
    let operations = RefCell::new(Vec::new());
    let mut last_post_timestamp = 0;

    for events in [
        replay.as_slice(),
        cleanup_reusing_replay_slots.as_slice(),
        barrier.as_slice(),
    ] {
        post_events_with_fresh_timestamps(
            events,
            &mut last_post_timestamp,
            || raw_times.borrow_mut().next().expect("one time per post"),
            |event, timestamp| {
                operations
                    .borrow_mut()
                    .push(PostOperation::SetTimestamp(event, timestamp));
            },
            |event| operations.borrow_mut().push(PostOperation::Post(event)),
        );
    }

    assert_eq!(
        operations.into_inner(),
        vec![
            PostOperation::SetTimestamp(REPLAY_POOL_START, 100),
            PostOperation::Post(REPLAY_POOL_START),
            PostOperation::SetTimestamp(REPLAY_POOL_START + 1, 100),
            PostOperation::Post(REPLAY_POOL_START + 1),
            PostOperation::SetTimestamp(REPLAY_POOL_START + 2, 101),
            PostOperation::Post(REPLAY_POOL_START + 2),
            PostOperation::SetTimestamp(REPLAY_POOL_START, 101),
            PostOperation::Post(REPLAY_POOL_START),
            PostOperation::SetTimestamp(REPLAY_POOL_START + 1, 102),
            PostOperation::Post(REPLAY_POOL_START + 1),
            PostOperation::SetTimestamp(BARRIER_POOL_START, 102),
            PostOperation::Post(BARRIER_POOL_START),
            PostOperation::SetTimestamp(BARRIER_POOL_START + 1, 102),
            PostOperation::Post(BARRIER_POOL_START + 1),
        ]
    );
    assert_eq!(last_post_timestamp, 102);
    assert!(raw_times.borrow_mut().next().is_none());
}

#[test]
fn cgevent_timestamp_ffi_round_trips_exact_uint64_value() {
    // SAFETY: the event is created and released exactly once in this test;
    // setting/getting its scalar timestamp does not post or require input
    // monitoring/accessibility permission.
    let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 127, false) };
    assert!(!event.is_null());
    let expected = 0x0123_4567_89AB_CDEF;
    let actual = unsafe {
        ffi::CGEventSetTimestamp(event, expected);
        let actual = ffi::CGEventGetTimestamp(event);
        ffi::CFRelease(event.cast_const());
        actual
    };
    assert_eq!(actual, expected);
}

#[test]
fn full_pool_initialization_creates_exactly_the_fixed_startup_capacity() {
    let next = RefCell::new(0_usize);
    let events = initialize_event_refs(
        || {
            let mut next = next.borrow_mut();
            *next += 1;
            *next as *mut c_void
        },
        |_| panic!("successful initialization must not release early"),
    )
    .unwrap();
    assert_eq!(*next.borrow(), NATIVE_EVENT_POOL_CAPACITY);
    assert!(events.iter().all(|event| !event.is_null()));
}

#[test]
fn partial_pool_initialization_releases_every_created_prefix_entry() {
    let next = RefCell::new(0_usize);
    let released = RefCell::new(Vec::new());
    let result = initialize_event_refs(
        || {
            let mut next = next.borrow_mut();
            let index = *next;
            *next += 1;
            if index == 5 {
                null_mut()
            } else {
                (index + 1) as *mut c_void
            }
        },
        |event| released.borrow_mut().push(event as usize),
    );
    assert_eq!(result.unwrap_err(), 5);
    assert_eq!(&*released.borrow(), &[1, 2, 3, 4, 5]);
}
