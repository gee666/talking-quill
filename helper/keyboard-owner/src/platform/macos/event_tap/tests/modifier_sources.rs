//! Modifier sources contracts.

use super::*;

#[test]
fn side_specific_modifier_models_keep_both_sides_independent_for_every_family() {
    for (left, right) in [
        (LEFT_CONTROL_KEY_CODE, RIGHT_CONTROL_KEY_CODE),
        (LEFT_OPTION_KEY_CODE, RIGHT_OPTION_KEY_CODE),
        (LEFT_SHIFT_KEY_CODE, RIGHT_SHIFT_KEY_CODE),
        (LEFT_COMMAND_KEY_CODE, RIGHT_COMMAND_KEY_CODE),
    ] {
        for (pid, release_order) in [(301, [left, right]), (302, [right, left])] {
            let mut journal = RecoveryEdgeJournal::default();
            assert_eq!(
                journal
                    .modifier_side_transition(InputSource::External, pid, left, None)
                    .map(|transition| transition.0),
                Some(true)
            );
            assert_eq!(
                journal
                    .modifier_side_transition(InputSource::External, pid, right, None)
                    .map(|transition| transition.0),
                Some(true)
            );
            for key_code in release_order {
                assert_eq!(
                    journal
                        .modifier_side_transition(InputSource::External, pid, key_code, None)
                        .map(|transition| transition.0),
                    Some(false)
                );
            }

            let mut physical = RecoveryEdgeJournal::default();
            physical.seed_modifier_side(InputSource::test_physical(), -1, left, false);
            physical.seed_modifier_side(InputSource::test_physical(), -1, right, false);
            assert_eq!(
                physical
                    .modifier_side_transition(InputSource::test_physical(), -1, left, None)
                    .map(|transition| transition.0),
                Some(true)
            );
            assert_eq!(
                physical
                    .modifier_side_transition(InputSource::test_physical(), -1, right, None)
                    .map(|transition| transition.0),
                Some(true)
            );
            for key_code in release_order {
                assert_eq!(
                    physical
                        .modifier_side_transition(InputSource::test_physical(), -1, key_code, None,)
                        .map(|transition| transition.0),
                    Some(false)
                );
            }
        }
    }
}

#[test]
fn ninth_external_pid_still_cancels_candidate_and_recovery_retains_scalar() {
    let (context, _outbound, _commands) = test_context();
    {
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.transactional = engine_with_ctrl_shift_x_candidate();
        for key_code in [
            LEFT_CONTROL_KEY_CODE,
            LEFT_SHIFT_KEY_CODE,
            LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
        ] {
            let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
        }
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
        let _ = keyboard
            .modifiers
            .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
    }
    {
        let mut journal = context.recovery_edges.lock().unwrap();
        for pid in 1..=RECOVERY_EXTERNAL_SOURCE_SLOTS as i64 {
            assert_eq!(
                journal.observe_normal_external_modifier(pid, LEFT_CONTROL_KEY_CODE),
                Some(true)
            );
        }
        assert!(journal.source_slot(InputSource::External, 99).is_none());
    }
    TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
    let _captured = process_transactional_event(
        &context,
        CallbackEvent {
            event_ref: null_mut(),
            marker: 0,
            event_type: ffi::K_CG_EVENT_FLAGS_CHANGED,
            key_code: LEFT_SHIFT_KEY_CODE,
            native_repeat: false,
            flags: ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            event_timestamp: 10,
            source: InputSource::External,
            source_pid: 99,
            test_physical_source: false,
        },
        None,
    );
    assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
    assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));

    let event = tagged_keyboard_event(
        ffi::K_CG_EVENT_FLAGS_CHANGED,
        LEFT_SHIFT_KEY_CODE,
        ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
        0,
    );
    let disposition = defer_callback_edge(
        &context,
        ffi::K_CG_EVENT_FLAGS_CHANGED,
        event,
        InputSource::External,
        99,
        0,
        false,
    );
    assert_eq!(disposition, CurrentEdgeDisposition::Owned);
    let journal = context.recovery_edges.lock().unwrap();
    assert!(
        journal.pending[..journal.pending_len]
            .iter()
            .any(|edge| edge.source_pid == 99)
    );
    for pid in 1..=RECOVERY_EXTERNAL_SOURCE_SLOTS as i64 {
        assert!(
            journal
                .existing_source_slot(InputSource::External, pid)
                .is_some()
        );
    }
    drop(journal);
    unsafe { ffi::CFRelease(event.cast_const()) };
}
