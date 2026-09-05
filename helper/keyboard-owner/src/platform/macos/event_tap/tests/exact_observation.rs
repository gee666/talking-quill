//! Exact observation contracts.

use super::*;

#[test]
fn replay_observation_requires_exact_record_shape_flags_and_order() {
    let mut journal = talking_quill_keyboard_core::transactional::EventJournal::new();
    for (phase, flags) in [
        (PhysicalPhase::Down, 0x10_u64),
        (PhysicalPhase::Up, 0x20_u64),
    ] {
        journal
            .push(ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::V),
                native: NativeKey {
                    virtual_key: 9,
                    platform_flags: flags,
                    ..NativeKey::default()
                },
                phase,
                observed_at_ms: 1,
            })
            .unwrap();
    }
    let mut keyboard = CallbackKeyboard::default();
    assert!(keyboard.begin_replay_observation(
        ExpectedReplayBatch::Replay(journal.replay_batch().unwrap()),
        injection::OperationToken::for_test(1),
    ));
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 9, false, 0x10),
        GapBarrierObservation::Forged
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 9, false, 0x10),
        GapBarrierObservation::Down
    );
    assert_eq!(
        keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 9, false, 0x20),
        GapBarrierObservation::Complete
    );
    assert!(keyboard.replay_observation.is_none());
}

#[test]
fn delayed_operation_tokens_cannot_advance_replay_gap_barrier_paste_barrier_or_paste() {
    let identity = injection::InjectionIdentity::for_test(42);
    for (old_generation, current_generation) in [(1, 2), (3, 4), (5, 6), (7, 8)] {
        let old = injection::OperationToken::for_test(old_generation);
        let current = injection::OperationToken::for_test(current_generation);
        assert!(!injection::token_matches(
            identity,
            Some(current),
            old.marker(),
            identity.source_pid,
        ));
    }
}

#[test]
fn tagged_pair_requires_exact_down_up_shape_and_order() {
    let mut state = 0;
    assert_eq!(
        observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, false, 0, 0,),
        GapBarrierObservation::Forged
    );
    assert_eq!(state, 0);
    assert_eq!(
        observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_DOWN, 127, 127, false, 0, 0,),
        GapBarrierObservation::Down
    );
    assert_eq!(
        observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, true, 0, 0,),
        GapBarrierObservation::Forged
    );
    assert_eq!(
        observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, false, 0, 0,),
        GapBarrierObservation::Complete
    );
}
