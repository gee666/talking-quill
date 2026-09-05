//! Shared transactions fixtures.

use super::*;

pub(super) fn engine_with_ctrl_shift_x_candidate() -> TransactionEngine {
    let compiled =
        CompiledActivationConfig::compile(ConfigRevision::new(1), true, bindings()).unwrap();
    let mut engine = TransactionEngine::new(compiled);
    let mut sides = ModifierSides::default();
    for (side, key_code) in [
        (ModifierSide::LeftCtrl, LEFT_CONTROL_KEY_CODE),
        (ModifierSide::LeftShift, LEFT_SHIFT_KEY_CODE),
    ] {
        sides.insert(side);
        let event = NormalizedEvent {
            key: KeyIdentity::Modifier(side),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: key_code,
                scan_code: u32::from(key_code),
                ..NativeKey::default()
            },
            observed_at_ms: 1,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(0, sides, false),
        };
        let Turn::Complete {
            engine: next,
            completion: Completion::Event(_),
        } = engine.begin(EngineInput::Event(event))
        else {
            panic!("modifier turn must complete");
        };
        engine = next;
    }
    let x_bit = 1_u32 << u32::from(ActivationKey::X.index());
    let x = NormalizedEvent {
        key: KeyIdentity::Letter(ActivationKey::X),
        phase: PhysicalPhase::Down,
        source: InputSource::test_physical(),
        native: NativeKey {
            virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            scan_code: u32::from(LETTER_KEY_CODES[usize::from(ActivationKey::X.index())]),
            ..NativeKey::default()
        },
        observed_at_ms: 2,
        config_revision: ConfigRevision::new(1),
        gate: GateState::Open,
        snapshot: PhysicalSnapshot::new(x_bit, sides, false),
    };
    let Turn::Complete {
        engine,
        completion: Completion::Event(outcome),
    } = engine.begin(EngineInput::Event(x))
    else {
        panic!("ordered prefix must remain a candidate");
    };
    assert_eq!(outcome.disposition, EventDisposition::CaptureCurrent);
    engine
}

pub(super) fn full_replay_batch(current_phase: PhysicalPhase) -> ReplayBatch {
    let mut journal = EventJournal::new();
    let x = LETTER_KEY_CODES[usize::from(ActivationKey::X.index())];
    let y = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
    for index in 0..JOURNAL_CAPACITY {
        let current = index + 1 == JOURNAL_CAPACITY;
        journal
            .push(ReplayRecord {
                key: if current && current_phase == PhysicalPhase::Down {
                    KeyIdentity::Other(y)
                } else {
                    KeyIdentity::Letter(ActivationKey::X)
                },
                native: NativeKey {
                    virtual_key: if current && current_phase == PhysicalPhase::Down {
                        y
                    } else {
                        x
                    },
                    scan_code: u32::from(if current && current_phase == PhysicalPhase::Down {
                        y
                    } else {
                        x
                    }),
                    extended: false,
                    platform_flags: 0,
                },
                phase: if current {
                    current_phase
                } else if index == 0 {
                    PhysicalPhase::Down
                } else {
                    PhysicalPhase::Repeat
                },
                observed_at_ms: index as u64 + 1,
            })
            .unwrap();
    }
    journal.replay_batch().unwrap()
}

pub(super) fn replay_continuation_for_test() -> Continuation {
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(_),
        continuation,
    } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Cancel(
        CancelReason::InvalidContinuation,
    )))
    else {
        panic!("candidate cancellation provides a replay continuation");
    };
    continuation
}
