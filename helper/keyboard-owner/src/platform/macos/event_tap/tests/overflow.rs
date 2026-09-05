//! Overflow contracts.

use super::*;

#[test]
fn overflow_keeps_only_foreground_physical_balance_and_owner_can_release() {
    let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
    let mut journal = RecoveryEdgeJournal::default();
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        let edge = injection::DeferredEvent {
            event_type: if index.is_multiple_of(2) {
                ffi::K_CG_EVENT_KEY_DOWN
            } else {
                ffi::K_CG_EVENT_KEY_UP
            },
            source: InputSource::Physical,
            source_pid: -1,
            ..injection::DeferredEvent::EMPTY
        };
        assert!(journal.append(edge));
    }
    let (generation, foreground_balance, hidden_generation) = journal
        .observe_key_phase(InputSource::Physical, -1, key_code, false, false, true)
        .unwrap();
    let balance = injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_UP,
        key_code,
        generation,
        source: InputSource::Physical,
        source_pid: -1,
        foreground_balance,
        hidden_generation,
        ..injection::DeferredEvent::EMPTY
    };
    assert!(!journal.append(balance));
    assert!(journal.overflow);
    assert_eq!(journal.pending_len, 1);
    assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
    assert!(journal.pending[0].foreground_balance);
    assert!(journal.hidden_balanced());
    assert!(journal.begin_submission(injection::OperationToken::for_test(499), 0, 1));
    assert_eq!(
        journal.advance_observation(),
        GapBarrierObservation::Complete
    );
    assert!(!journal.overflow);
    assert!(!journal.has_pending());

    let mut external = RecoveryEdgeJournal::default();
    for index in 0..injection::DEFERRED_EDGE_CAPACITY {
        assert!(external.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            is_down: true,
            source: InputSource::External,
            source_pid: 808,
            original_timestamp: index as u64,
            ..injection::DeferredEvent::EMPTY
        }));
    }
    assert!(!external.append(injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_UP,
        key_code,
        source: InputSource::External,
        source_pid: 808,
        original_timestamp: injection::DEFERRED_EDGE_CAPACITY as u64,
        ..injection::DeferredEvent::EMPTY
    }));
    assert_eq!(external.pending_len, injection::DEFERRED_EDGE_CAPACITY);
    assert_eq!(external.overflow_balance_len, 1);
    assert!(!external.physical_fences_pending());
    assert!(external.begin_submission(
        injection::OperationToken::for_test(500),
        0,
        injection::DEFERRED_EDGE_CAPACITY,
    ));
    for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
        let _ = external.advance_observation();
    }
    assert_eq!(external.pending_len, 1);
    assert_eq!(external.pending[0].original_timestamp, 64);
}

#[test]
fn physical_fences_clear_only_after_authoritative_gap_state_is_up() {
    let mut journal = RecoveryEdgeJournal::default();
    journal.discard_key_fences[0][16] = true;
    journal.discard_mouse_fences[0] = 1;
    journal.reconcile_physical_fences(|_| true, |_| true);
    assert!(journal.physical_fences_pending());
    journal.reconcile_physical_fences(|_| false, |_| false);
    assert!(!journal.physical_fences_pending());
}

#[test]
fn physical_overflow_fence_retains_owner_until_matching_up() {
    let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
    let identity_probe = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, key_code, 0, 0);
    let source_pid = unsafe {
        ffi::CGEventGetIntegerValueField(identity_probe, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
    };
    unsafe { ffi::CFRelease(identity_probe.cast_const()) };
    let (mut context, _outbound, _commands) = test_context();
    context.injection_identity = Some(injection::InjectionIdentity::for_test(source_pid + 1));
    context.keyboard.lock().unwrap().transactional = engine_with_ctrl_shift_x_candidate();
    context
        .state
        .recovery_deferred_mode
        .store(true, Ordering::Release);
    let context_ptr = (&raw const context).cast_mut().cast();

    for _ in 0..(injection::DEFERRED_EDGE_CAPACITY / 2) {
        for event_type in [ffi::K_CG_EVENT_KEY_DOWN, ffi::K_CG_EVENT_KEY_UP] {
            let event =
                tagged_keyboard_event(event_type, key_code, 0, TEST_RECOVERY_PHYSICAL_MARKER);
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert!(returned.is_null());
            unsafe { ffi::CFRelease(event.cast_const()) };
        }
    }
    {
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
        assert!(journal.hidden_balanced());
        assert!(journal.token.is_none());
    }

    let overflow_down = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let overflow_up = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_UP,
        key_code,
        0,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let returned_down = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_KEY_DOWN,
            overflow_down,
            context_ptr,
        )
    };
    assert!(returned_down.is_null());
    {
        let journal = context.recovery_edges.lock().unwrap();
        assert!(!journal.overflow);
        assert_eq!(journal.pending_len, 0, "no unexposed half may be replayed");
        assert!(journal.hidden_balanced());
        assert!(journal.key_is_discard_fenced(InputSource::test_physical(), source_pid, key_code,));
        assert!(journal.physical_fences_pending());
        assert!(journal.token.is_none());
    }
    assert_eq!(
        context.terminal.reason(),
        Some(TerminalReason::InputInjectionUnavailable)
    );

    context.keyboard.lock().unwrap().transactional = TransactionEngine::default();
    submit_deferred_edges_if_ready(&context);
    assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
    assert!(pending_native_work(&context));

    let returned_up =
        unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, overflow_up, context_ptr) };
    assert!(
        returned_up.is_null(),
        "discarded down's up is fenced, not exposed"
    );
    let journal = context.recovery_edges.lock().unwrap();
    assert!(!journal.overflow);
    assert_eq!(journal.pending_len, 0);
    assert!(journal.hidden_balanced());
    assert!(journal.token.is_none());
    assert!(!journal.key_is_discard_fenced(InputSource::test_physical(), source_pid, key_code,));
    drop(journal);
    assert!(!pending_native_work(&context));

    {
        let mut journal = context.recovery_edges.lock().unwrap();
        journal.discard_mouse_fences[1] = 1;
    }
    context
        .state
        .recovery_deferred_mode
        .store(true, Ordering::Release);
    assert!(pending_native_work(&context));
    let mouse_up = tagged_mouse_event(
        ffi::K_CG_EVENT_LEFT_MOUSE_UP,
        ffi::CGPoint { x: 40.0, y: 50.0 },
        0,
        1,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let returned_mouse = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_LEFT_MOUSE_UP,
            mouse_up,
            context_ptr,
        )
    };
    assert!(returned_mouse.is_null());
    assert!(!pending_native_work(&context));
    unsafe {
        ffi::CFRelease(mouse_up.cast_const());
        ffi::CFRelease(overflow_up.cast_const());
        ffi::CFRelease(overflow_down.cast_const());
    }
}
