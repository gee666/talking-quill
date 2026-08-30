use proptest::prelude::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationKey, ProfileId, Shortcut, ShortcutModifiers,
    transactional::{
        CompiledActivationConfig, Completion, ConfigRevision, Control, EffectOutcome,
        EffectRequest, EngineInput, EventDisposition, InputSource, JOURNAL_CAPACITY, KeyIdentity,
        MAX_EFFECTS_PER_TURN, ModifierSide, NativeKey, NormalizedEvent, PhysicalPhase,
        PhysicalSnapshot, TransactionEngine, Turn,
    },
};

fn config(revision: u64) -> CompiledActivationConfig {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        Shortcut::new(
            ShortcutModifiers {
                ctrl: true,
                alt: false,
                shift: false,
                meta: false,
            },
            &[ActivationKey::A],
        )
        .unwrap(),
    );
    CompiledActivationConfig::compile(
        ConfigRevision::new(revision),
        true,
        ActivationBindings::new(&[binding]).unwrap(),
    )
    .unwrap()
}

fn prefix_config(revision: u64) -> CompiledActivationConfig {
    let binding = ActivationBinding::new(
        ProfileId::GENERAL,
        Shortcut::new(
            ShortcutModifiers {
                ctrl: true,
                alt: false,
                shift: false,
                meta: false,
            },
            &[ActivationKey::A, ActivationKey::B, ActivationKey::C],
        )
        .unwrap(),
    );
    CompiledActivationConfig::compile(
        ConfigRevision::new(revision),
        true,
        ActivationBindings::new(&[binding]).unwrap(),
    )
    .unwrap()
}

fn native(key: KeyIdentity) -> NativeKey {
    NativeKey {
        virtual_key: match key {
            KeyIdentity::Letter(letter) => u16::from(letter.index()) + 0x41,
            KeyIdentity::Modifier(side) => 0xA0 + u16::from(side as u8),
            KeyIdentity::Escape => 0x1B,
            KeyIdentity::Enter => 0x0D,
            KeyIdentity::Other(code) => code,
        },
        scan_code: match key {
            KeyIdentity::Letter(letter) => u32::from(letter.index()) + 1,
            KeyIdentity::Modifier(side) => u32::from(side as u8) + 0x1D,
            _ => 0,
        },
        extended: false,
        platform_flags: 0,
    }
}

fn accepted(effect: EffectRequest) -> EffectOutcome {
    match effect {
        EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 2 },
        EffectRequest::CleanupMenuNeutralization(_) => {
            EffectOutcome::MenuCleanupAccepted { accepted: 1 }
        }
        EffectRequest::DeliverActivation(_) => EffectOutcome::ActivationDelivered(true),
        EffectRequest::Replay(batch) => EffectOutcome::ReplayAccepted {
            accepted: batch.len(),
        },
        EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
            accepted: batch.len(),
        },
    }
}

fn drive_with(
    turn: Turn,
    mut respond: impl FnMut(EffectRequest) -> EffectOutcome,
) -> (TransactionEngine, Completion, Vec<EffectRequest>) {
    let mut turn = turn;
    let mut effects = Vec::new();
    loop {
        match turn {
            Turn::Complete { engine, completion } => {
                assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
                return (engine, completion, effects);
            }
            Turn::NeedEffect {
                effect,
                continuation,
            } => {
                effects.push(effect);
                assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
                turn = continuation.resume(respond(effect));
            }
        }
    }
}

fn drive(turn: Turn) -> (TransactionEngine, Completion, Vec<EffectRequest>) {
    drive_with(turn, accepted)
}

fn physical_event(
    engine: &TransactionEngine,
    key: KeyIdentity,
    phase: PhysicalPhase,
    timestamp: u64,
) -> NormalizedEvent {
    let mut letters = engine.physical_letters();
    let mut modifiers = engine.physical_modifiers();
    match key {
        KeyIdentity::Letter(letter) => match phase {
            PhysicalPhase::Down | PhysicalPhase::Repeat => letters |= 1_u32 << letter.index(),
            PhysicalPhase::Up => letters &= !(1_u32 << letter.index()),
        },
        KeyIdentity::Modifier(side) => match phase {
            PhysicalPhase::Down | PhysicalPhase::Repeat => modifiers.insert(side),
            PhysicalPhase::Up => modifiers.remove(side),
        },
        KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
    }
    NormalizedEvent::physical(
        key,
        phase,
        native(key),
        timestamp,
        engine.config().revision(),
        talking_quill_keyboard_core::transactional::GateState::Open,
        PhysicalSnapshot::new(letters, modifiers, false),
    )
}

fn side_from_index(index: u8) -> ModifierSide {
    match index % 8 {
        0 => ModifierSide::LeftCtrl,
        1 => ModifierSide::RightCtrl,
        2 => ModifierSide::LeftAlt,
        3 => ModifierSide::RightAlt,
        4 => ModifierSide::LeftShift,
        5 => ModifierSide::RightShift,
        6 => ModifierSide::LeftMeta,
        _ => ModifierSide::RightMeta,
    }
}

fn assert_core_invariants(engine: &TransactionEngine) {
    assert_eq!(engine.owned_letters() & !engine.physical_letters(), 0);
    assert_eq!(engine.foreground_letters() & !engine.physical_letters(), 0);
    assert_eq!(
        engine.foreground_modifiers().bits() & !engine.physical_modifiers().bits(),
        0
    );
    assert!(engine.journal_len() <= JOURNAL_CAPACITY);
}

fn visible_index(key: KeyIdentity) -> Option<usize> {
    match key {
        KeyIdentity::Letter(letter) => Some(usize::from(letter.index())),
        KeyIdentity::Modifier(side) => Some(26 + usize::from(side as u8)),
        KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => None,
    }
}

fn apply_visible(counts: &mut [u8; 34], key: KeyIdentity, phase: PhysicalPhase) {
    let Some(index) = visible_index(key) else {
        return;
    };
    match phase {
        PhysicalPhase::Down => {
            assert_eq!(counts[index], 0, "duplicate visible down for {key:?}");
            counts[index] = 1;
        }
        PhysicalPhase::Repeat => {
            assert_eq!(counts[index], 1, "visible repeat without down for {key:?}");
        }
        PhysicalPhase::Up => {
            assert_eq!(counts[index], 1, "visible up without down for {key:?}");
            counts[index] = 0;
        }
    }
}

fn apply_turn_trace(
    counts: &mut [u8; 34],
    input: EngineInput,
    effects: &[EffectRequest],
    completion: Completion,
) {
    for effect in effects {
        if let EffectRequest::Replay(batch) = effect {
            for record in batch.entries() {
                apply_visible(counts, record.key, record.phase);
            }
        }
    }
    if let (EngineInput::Event(event), Completion::Event(outcome)) = (input, completion)
        && event.source.is_physical()
        && outcome.disposition == EventDisposition::PassCurrent
    {
        apply_visible(counts, event.key, event.phase);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bounded_physical_and_control_streams_preserve_ownership_and_balance(
        operations in prop::collection::vec((0_u8..11, any::<u8>(), any::<u8>()), 0..160)
    ) {
        let mut engine = TransactionEngine::new(config(1));
        let mut timestamp = 0_u64;
        let mut visible = [0_u8; 34];

        for (kind, identity, action) in operations {
            timestamp = timestamp.saturating_add(u64::from(action));
            let input = match kind {
                0..=3 => {
                    let key = ActivationKey::from_index(identity % 4).unwrap();
                    let bit = 1_u32 << key.index();
                    let held = engine.physical_letters() & bit != 0;
                    let phase = match action % 3 {
                        0 if held => PhysicalPhase::Repeat,
                        0 => PhysicalPhase::Down,
                        1 if held => PhysicalPhase::Up,
                        1 => PhysicalPhase::Down,
                        _ if held => PhysicalPhase::Repeat,
                        _ => PhysicalPhase::Down,
                    };
                    EngineInput::Event(physical_event(
                        &engine,
                        KeyIdentity::Letter(key),
                        phase,
                        timestamp,
                    ))
                }
                4..=5 => {
                    let side = side_from_index(identity);
                    let held = engine.physical_modifiers().contains(side);
                    let phase = match action % 3 {
                        0 if held => PhysicalPhase::Repeat,
                        0 => PhysicalPhase::Down,
                        1 if held => PhysicalPhase::Up,
                        1 => PhysicalPhase::Down,
                        _ if held => PhysicalPhase::Repeat,
                        _ => PhysicalPhase::Down,
                    };
                    EngineInput::Event(physical_event(
                        &engine,
                        KeyIdentity::Modifier(side),
                        phase,
                        timestamp,
                    ))
                }
                6 => {
                    let source = match action % 3 {
                        0 => InputSource::HelperReplay,
                        1 => InputSource::HelperPaste,
                        _ => InputSource::External,
                    };
                    let external_held = engine.physical_letters()
                        & (1_u32 << ActivationKey::A.index())
                        != 0;
                    let phase = if source == InputSource::External && external_held {
                        PhysicalPhase::Up
                    } else {
                        PhysicalPhase::Down
                    };
                    EngineInput::Event(
                        physical_event(
                            &engine,
                            KeyIdentity::Letter(ActivationKey::A),
                            phase,
                            timestamp,
                        )
                        .with_source(source),
                    )
                }
                7 if action % 2 == 0 => EngineInput::Control(Control::Cancel(
                    talking_quill_keyboard_core::transactional::CancelReason::Timeout,
                )),
                7 => {
                    let revision = engine.config().revision().get().saturating_add(1);
                    EngineInput::Control(Control::ReplaceConfig(config(revision)))
                }
                8 => EngineInput::Control(Control::Shutdown),
                9 => EngineInput::Control(Control::CloseAdmission(
                    talking_quill_keyboard_core::transactional::CancelReason::HelperDisconnected,
                )),
                _ => {
                    let mut stale = physical_event(
                        &engine,
                        KeyIdentity::Letter(ActivationKey::D),
                        if engine.physical_letters() & (1_u32 << ActivationKey::D.index()) == 0 {
                            PhysicalPhase::Down
                        } else {
                            PhysicalPhase::Up
                        },
                        timestamp,
                    );
                    stale.config_revision = ConfigRevision::new(
                        stale.config_revision.get().saturating_add(1),
                    );
                    EngineInput::Event(stale)
                }
            };

            let first_turn = engine.clone().begin(input);
            let second_turn = engine.clone().begin(input);
            prop_assert_eq!(&first_turn, &second_turn, "pure planning must be deterministic");
            let (next, completion, effects) = drive(first_turn);
            apply_turn_trace(&mut visible, input, &effects, completion);
            prop_assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
            prop_assert!(!(effects.iter().any(|effect| matches!(effect, EffectRequest::Replay(_)))
                && effects.iter().any(|effect| matches!(effect, EffectRequest::DeliverActivation(_)))),
                "accepted input cannot both activate and replay");
            assert_core_invariants(&next);
            engine = next;
        }

        // Well-formed final releases must leave no physical, logical, or owned
        // letter/modifier state. Release letters before modifiers so pending
        // exact prefixes resolve through their ordinary completion boundary.
        for index in 0..26 {
            let key = ActivationKey::from_index(index).unwrap();
            if engine.physical_letters() & (1_u32 << key.index()) != 0 {
                let input = physical_event(
                    &engine,
                    KeyIdentity::Letter(key),
                    PhysicalPhase::Up,
                    u64::MAX,
                );
                let turn_input = EngineInput::Event(input);
                let (next, completion, effects) = drive(engine.begin(turn_input));
                apply_turn_trace(&mut visible, turn_input, &effects, completion);
                engine = next;
                assert_core_invariants(&engine);
            }
        }
        for index in 0..8 {
            let side = side_from_index(index);
            if engine.physical_modifiers().contains(side) {
                let input = physical_event(
                    &engine,
                    KeyIdentity::Modifier(side),
                    PhysicalPhase::Up,
                    u64::MAX,
                );
                let turn_input = EngineInput::Event(input);
                let (next, completion, effects) = drive(engine.begin(turn_input));
                apply_turn_trace(&mut visible, turn_input, &effects, completion);
                engine = next;
                assert_core_invariants(&engine);
            }
        }
        let (engine, _, _) = drive(engine.begin(EngineInput::Control(Control::Shutdown)));
        prop_assert_eq!(engine.physical_letters(), 0);
        prop_assert_eq!(engine.foreground_letters(), 0);
        prop_assert_eq!(engine.owned_letters(), 0);
        prop_assert_eq!(engine.physical_modifiers().bits(), 0);
        prop_assert_eq!(engine.foreground_modifiers().bits(), 0);
        prop_assert_eq!(visible, [0_u8; 34]);
    }

    #[test]
    fn generated_partial_replay_and_cleanup_counts_retain_exact_degraded_ownership(
        replay_accepted in 0_usize..5,
        cleanup_accepted in 0_usize..5,
    ) {
        let mut engine = TransactionEngine::new(prefix_config(1));
        for key in [
            KeyIdentity::Modifier(ModifierSide::LeftCtrl),
            KeyIdentity::Letter(ActivationKey::A),
            KeyIdentity::Letter(ActivationKey::B),
        ] {
            let input = physical_event(&engine, key, PhysicalPhase::Down, 1);
            engine = drive(engine.begin(EngineInput::Event(input))).0;
        }
        let wrong = physical_event(
            &engine,
            KeyIdentity::Letter(ActivationKey::Y),
            PhysicalPhase::Down,
            2,
        );
        let (engine, completion, effects) = drive_with(
            engine.begin(EngineInput::Event(wrong)),
            |effect| match effect {
                EffectRequest::Replay(batch) => {
                    let mut accepted_count = replay_accepted.min(batch.len() + 1);
                    if accepted_count == batch.len() {
                        accepted_count += 1;
                    }
                    EffectOutcome::ReplayAccepted {
                        accepted: accepted_count,
                    }
                }
                EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                    accepted: cleanup_accepted.min(batch.len() + 1),
                },
                _ => accepted(effect),
            },
        );
        prop_assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
        prop_assert!(matches!(completion, Completion::Event(outcome)
            if outcome.disposition == EventDisposition::CaptureCurrent && outcome.terminal));
        prop_assert_eq!(engine.owned_letters() & !engine.physical_letters(), 0);
        prop_assert!(engine.pending_injected_cleanup().map_or(0, |batch| batch.len()) <= 3);
        prop_assert!(!engine.admission_open());
    }

    #[test]
    fn repeat_storms_cancel_bounded_candidates_without_advancing_or_wrapping(
        repeat_count in 0_usize..2_000
    ) {
        let mut engine = TransactionEngine::new(prefix_config(1));
        let ctrl = physical_event(
            &engine,
            KeyIdentity::Modifier(ModifierSide::LeftCtrl),
            PhysicalPhase::Down,
            0,
        );
        engine = drive(engine.begin(EngineInput::Event(ctrl))).0;
        // A begins the held A+B+C prefix. Repeats are journaled but never
        // advance it; the first overflow replays once and fences the held key.
        let down = physical_event(
            &engine,
            KeyIdentity::Letter(ActivationKey::A),
            PhysicalPhase::Down,
            1,
        );
        engine = drive(engine.begin(EngineInput::Event(down))).0;
        let mut replay_count = 0_usize;
        for index in 0..repeat_count {
            let repeat = physical_event(
                &engine,
                KeyIdentity::Letter(ActivationKey::A),
                PhysicalPhase::Repeat,
                index as u64 + 2,
            );
            let (next, _, effects) = drive(engine.begin(EngineInput::Event(repeat)));
            let activated = effects
                .iter()
                .any(|effect| matches!(effect, EffectRequest::DeliverActivation(_)));
            prop_assert!(!activated);
            replay_count += effects
                .iter()
                .filter(|effect| matches!(effect, EffectRequest::Replay(_)))
                .count();
            prop_assert!(next.journal_len() <= JOURNAL_CAPACITY);
            engine = next;
        }
        prop_assert!(replay_count <= 1);
        if repeat_count >= JOURNAL_CAPACITY {
            prop_assert_eq!(replay_count, 1);
            prop_assert_eq!(engine.owned_letters(), 0);
        }
    }

    #[test]
    fn generated_reconciliation_and_malformed_phases_fail_safe(
        held_bits in 0_u32..(1_u32 << 4),
        modifier_bits in any::<u8>(),
        malformed_key in 0_u8..4,
    ) {
        let snapshot = PhysicalSnapshot::new(
            held_bits,
            talking_quill_keyboard_core::transactional::ModifierSides::from_bits(modifier_bits),
            false,
        );
        let engine = TransactionEngine::new(prefix_config(1));
        let (engine, completion, effects) = drive(
            engine.begin(EngineInput::Control(Control::Reconcile(snapshot))),
        );
        prop_assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
        prop_assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
        prop_assert_eq!(engine.physical_letters(), held_bits);
        prop_assert_eq!(engine.physical_modifiers().bits(), modifier_bits);

        let key = ActivationKey::from_index(malformed_key).unwrap();
        let bit = 1_u32 << key.index();
        let malformed_phase = if held_bits & bit == 0 {
            PhysicalPhase::Repeat
        } else {
            PhysicalPhase::Down
        };
        let malformed = NormalizedEvent::physical(
            KeyIdentity::Letter(key),
            malformed_phase,
            native(KeyIdentity::Letter(key)),
            1,
            engine.config().revision(),
            talking_quill_keyboard_core::transactional::GateState::Open,
            snapshot,
        );
        let (engine, completion, effects) = drive(engine.begin(EngineInput::Event(malformed)));
        prop_assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
        prop_assert!(matches!(completion, Completion::Event(outcome)
            if outcome.cancellation == Some(
                talking_quill_keyboard_core::transactional::CancelReason::PhysicalStateMismatch
            ) && outcome.terminal));
        prop_assert!(!engine.admission_open());
        prop_assert_eq!(engine.owned_letters() & !engine.physical_letters(), 0);
    }
}

#[test]
fn altgr_never_satisfies_plain_ctrl_alt_binding() {
    let shortcut = Shortcut::new(
        ShortcutModifiers {
            ctrl: true,
            alt: true,
            shift: false,
            meta: false,
        },
        &[ActivationKey::A],
    )
    .unwrap();
    let binding = ActivationBinding::new(ProfileId::GENERAL, shortcut);
    let configured = CompiledActivationConfig::compile(
        ConfigRevision::new(1),
        true,
        ActivationBindings::new(&[binding]).unwrap(),
    )
    .unwrap();
    let mut engine = TransactionEngine::new(configured);
    for side in [ModifierSide::LeftCtrl, ModifierSide::RightAlt] {
        let input = physical_event(&engine, KeyIdentity::Modifier(side), PhysicalPhase::Down, 1)
            .with_alt_gr(true);
        engine = drive(engine.begin(EngineInput::Event(input))).0;
    }
    let input = physical_event(
        &engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        2,
    )
    .with_alt_gr(true);
    let (engine, _completion, effects) = drive(engine.begin(EngineInput::Event(input)));
    assert!(effects.is_empty());
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::A.index());
}
