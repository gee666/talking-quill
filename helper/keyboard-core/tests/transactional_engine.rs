use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationKey, ProfileId, Shortcut, ShortcutModifiers,
    transactional::{
        ActivationNotice, CancelReason, CompiledActivationConfig, Completion, ConfigRevision,
        Control, EffectOutcome, EffectRequest, EngineInput, EventDisposition, GateState,
        InputSource, JOURNAL_CAPACITY, KeyIdentity, MAX_EFFECTS_PER_TURN, MenuNeutralizationPolicy,
        ModifierSide, NativeKey, NormalizedEvent, PhysicalPhase, PhysicalSnapshot, ShutdownState,
        TransactionEngine, Turn,
    },
};

fn modifiers(ctrl: bool, alt: bool, shift: bool, meta: bool) -> ShortcutModifiers {
    ShortcutModifiers {
        ctrl,
        alt,
        shift,
        meta,
    }
}

fn binding(
    profile: ProfileId,
    modifiers: ShortcutModifiers,
    keys: &[ActivationKey],
) -> ActivationBinding {
    ActivationBinding::new(profile, Shortcut::new(modifiers, keys).unwrap())
}

fn canonical_config(revision: u64, enabled: bool) -> CompiledActivationConfig {
    let alt = modifiers(false, true, false, false);
    let bindings = ActivationBindings::new(&[
        binding(ProfileId::GENERAL, alt, &[ActivationKey::X]),
        binding(
            ProfileId::PROMPT,
            alt,
            &[ActivationKey::X, ActivationKey::P],
        ),
        binding(
            ProfileId::PROMPT_TO_ENGLISH,
            alt,
            &[ActivationKey::X, ActivationKey::Q],
        ),
        binding(
            ProfileId::MARKDOWN,
            alt,
            &[ActivationKey::X, ActivationKey::M],
        ),
        binding(
            ProfileId::TRANSLATE_TO_ENGLISH,
            alt,
            &[ActivationKey::X, ActivationKey::T],
        ),
    ])
    .unwrap();
    CompiledActivationConfig::compile(ConfigRevision::new(revision), enabled, bindings).unwrap()
}

fn single_config(
    revision: u64,
    modifiers: ShortcutModifiers,
    key: ActivationKey,
) -> CompiledActivationConfig {
    let bindings =
        ActivationBindings::new(&[binding(ProfileId::GENERAL, modifiers, &[key])]).unwrap();
    CompiledActivationConfig::compile(ConfigRevision::new(revision), true, bindings).unwrap()
}

fn native(key: KeyIdentity) -> NativeKey {
    match key {
        KeyIdentity::Letter(letter) => NativeKey {
            virtual_key: u16::from(letter.index()) + 0x41,
            scan_code: u32::from(letter.index()) + 1,
            extended: false,
            platform_flags: 0x10,
        },
        KeyIdentity::Modifier(side) => NativeKey {
            virtual_key: 0xA0 + u16::from(side as u8),
            scan_code: u32::from(side as u8) + 0x1D,
            extended: matches!(
                side,
                ModifierSide::RightCtrl
                    | ModifierSide::RightAlt
                    | ModifierSide::LeftMeta
                    | ModifierSide::RightMeta
            ),
            platform_flags: 0x20,
        },
        KeyIdentity::Escape => NativeKey {
            virtual_key: 0x1B,
            ..NativeKey::default()
        },
        KeyIdentity::Enter => NativeKey {
            virtual_key: 0x0D,
            ..NativeKey::default()
        },
        KeyIdentity::Other(code) => NativeKey {
            virtual_key: code,
            ..NativeKey::default()
        },
    }
}

fn snapshot_after(
    engine: &TransactionEngine,
    key: KeyIdentity,
    phase: PhysicalPhase,
) -> PhysicalSnapshot {
    let mut letters = engine.physical_letters();
    let mut modifier_sides = engine.physical_modifiers();
    match key {
        KeyIdentity::Letter(letter) => match phase {
            PhysicalPhase::Down | PhysicalPhase::Repeat => letters |= 1_u32 << letter.index(),
            PhysicalPhase::Up => letters &= !(1_u32 << letter.index()),
        },
        KeyIdentity::Modifier(side) => match phase {
            PhysicalPhase::Down | PhysicalPhase::Repeat => modifier_sides.insert(side),
            PhysicalPhase::Up => modifier_sides.remove(side),
        },
        KeyIdentity::Escape | KeyIdentity::Enter | KeyIdentity::Other(_) => {}
    }
    PhysicalSnapshot::new(letters, modifier_sides, false)
}

fn event(
    engine: &TransactionEngine,
    key: KeyIdentity,
    phase: PhysicalPhase,
    at: u64,
) -> NormalizedEvent {
    NormalizedEvent::physical(
        key,
        phase,
        native(key),
        at,
        engine.config().revision(),
        GateState::Open,
        snapshot_after(engine, key, phase),
    )
}

fn success(effect: EffectRequest) -> EffectOutcome {
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

fn drive(
    turn: Turn,
    mut respond: impl FnMut(EffectRequest, usize) -> EffectOutcome,
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
                let index = effects.len();
                effects.push(effect);
                assert!(effects.len() <= MAX_EFFECTS_PER_TURN);
                turn = continuation.resume(respond(effect, index));
            }
        }
    }
}

fn apply_event(
    engine: TransactionEngine,
    key: KeyIdentity,
    phase: PhysicalPhase,
    at: u64,
) -> (TransactionEngine, Completion, Vec<EffectRequest>) {
    let input = event(&engine, key, phase, at);
    drive(engine.begin(EngineInput::Event(input)), |effect, _| {
        success(effect)
    })
}

fn apply_control(
    engine: TransactionEngine,
    control: Control,
) -> (TransactionEngine, Completion, Vec<EffectRequest>) {
    drive(engine.begin(EngineInput::Control(control)), |effect, _| {
        success(effect)
    })
}

fn disposition(completion: Completion) -> EventDisposition {
    let Completion::Event(outcome) = completion else {
        panic!("expected event completion, got {completion:?}");
    };
    outcome.disposition
}

#[test]
fn target_reservation_hint_is_true_only_at_an_activation_boundary() {
    let engine = TransactionEngine::new(single_config(
        1,
        modifiers(true, false, false, false),
        ActivationKey::A,
    ));
    assert!(!engine.event_may_request_activation(KeyIdentity::Other(1), PhysicalPhase::Down));
    assert!(
        !engine.event_may_request_activation(
            KeyIdentity::Letter(ActivationKey::B),
            PhysicalPhase::Down
        )
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftCtrl),
        PhysicalPhase::Down,
        1,
    );
    assert!(
        engine.event_may_request_activation(
            KeyIdentity::Letter(ActivationKey::A),
            PhysicalPhase::Down
        )
    );
}

#[test]
fn terminating_physical_event_is_reposted_as_the_final_replay_record() {
    let engine = TransactionEngine::new(canonical_config(1, true));
    let engine = press_alt(engine, ModifierSide::LeftAlt);
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(effects.is_empty());

    let terminating = event(&engine, KeyIdentity::Other(99), PhysicalPhase::Down, 20);
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(terminating)),
        |effect, _| success(effect),
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    let EffectRequest::Replay(batch) = effects[0] else {
        panic!("candidate cancellation must replay");
    };
    assert_eq!(batch.entries().len(), 2);
    assert_eq!(
        batch.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::X)
    );
    assert_eq!(batch.entries()[1].key, KeyIdentity::Other(99));
    assert_eq!(
        engine.foreground_letters(),
        1_u32 << ActivationKey::X.index()
    );
}

#[test]
fn mouse_focus_boundary_cancels_candidate_and_replays_before_adapter_repost() {
    let engine = TransactionEngine::new(canonical_config(1, true));
    let engine = press_alt(engine, ModifierSide::LeftAlt);
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) =
        apply_control(engine, Control::Cancel(CancelReason::InvalidContinuation));
    assert!(matches!(completion, Completion::Control(_)));
    assert_eq!(effects.len(), 1);
    let EffectRequest::Replay(batch) = effects[0] else {
        panic!("mouse boundary must request replay first");
    };
    assert_eq!(
        batch.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::X)
    );
    assert_eq!(engine.journal_len(), 0);
    // The macOS adapter suppresses and proxy-reposts the mouse only after this
    // control completion, so no focus edge can overtake the replay effect.
}

#[test]
fn macos_not_required_policy_never_requests_or_fakes_menu_neutralization() {
    let engine = TransactionEngine::new(single_config(
        1,
        modifiers(false, true, false, false),
        ActivationKey::X,
    ))
    .with_menu_neutralization_policy(MenuNeutralizationPolicy::NotRequired);
    let engine = press_alt(engine, ModifierSide::LeftAlt);
    let (_engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], EffectRequest::DeliverActivation(_)));
}

fn press_alt(engine: TransactionEngine, side: ModifierSide) -> TransactionEngine {
    let (engine, completion, effects) =
        apply_event(engine, KeyIdentity::Modifier(side), PhysicalPhase::Down, 1);
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    engine
}

#[test]
fn alt_x_p_captures_every_letter_commits_once_and_balances_sides() {
    let mut engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        100,
    );
    assert_eq!(engine.foreground_letters(), 0);
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());

    let (next, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        200,
    );
    engine = next;
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(matches!(effects[0], EffectRequest::NeutralizeMenu(_)));
    assert_eq!(
        effects[1],
        EffectRequest::DeliverActivation(ActivationNotice::Down {
            binding: binding(
                ProfileId::PROMPT,
                modifiers(false, true, false, false),
                &[ActivationKey::X, ActivationKey::P],
            ),
        })
    );
    assert_eq!(engine.foreground_letters(), 0);
    assert_eq!(
        engine.owned_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::P.index())
    );

    let (next, _, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Up,
        450,
    );
    engine = next;
    assert_eq!(
        effects,
        vec![EffectRequest::DeliverActivation(ActivationNotice::Up {
            binding: binding(
                ProfileId::PROMPT,
                modifiers(false, true, false, false),
                &[ActivationKey::X, ActivationKey::P],
            ),
            held_ms: 250,
        })]
    );
    (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        500,
    );
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(engine.foreground_letters(), 0);

    let (engine, completion, _) = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftAlt),
        PhysicalPhase::Up,
        600,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert_eq!(engine.physical_modifiers().bits(), 0);
    assert_eq!(engine.foreground_modifiers().bits(), 0);
}

#[test]
fn shorter_shared_prefix_completes_on_release_and_saturates_duration() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        u64::MAX,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(effects.is_empty());

    let (_, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        1,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(matches!(effects[0], EffectRequest::NeutralizeMenu(_)));
    assert_eq!(
        effects[1],
        EffectRequest::DeliverActivation(ActivationNotice::Complete {
            binding: binding(
                ProfileId::GENERAL,
                modifiers(false, true, false, false),
                &[ActivationKey::X],
            ),
            held_ms: 0,
        })
    );
}

#[test]
fn wrong_suffix_is_replayed_once_in_original_order_and_originals_are_captured() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        20,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    let [EffectRequest::Replay(batch)] = effects.as_slice() else {
        panic!("expected one replay, got {effects:?}");
    };
    assert_eq!(
        batch
            .entries()
            .iter()
            .map(|record| record.key)
            .collect::<Vec<_>>(),
        vec![
            KeyIdentity::Letter(ActivationKey::X),
            KeyIdentity::Letter(ActivationKey::Y),
        ]
    );
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(
        engine.foreground_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::Y.index())
    );

    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Up,
        30,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        40,
    );
    assert_eq!(engine.foreground_letters(), 0);
}

#[test]
fn repeat_overflow_replays_current_repeat_exactly_once_after_bounded_prefix() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (mut engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    for index in 1..JOURNAL_CAPACITY - 1 {
        (engine, _, _) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Repeat,
            index as u64 + 1,
        );
    }
    assert_eq!(engine.journal_len(), JOURNAL_CAPACITY - 1);

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        999,
    );
    let Completion::Event(outcome) = completion else {
        unreachable!()
    };
    assert_eq!(outcome.disposition, EventDisposition::CaptureCurrent);
    assert_eq!(outcome.cancellation, Some(CancelReason::JournalOverflow));
    let [EffectRequest::Replay(batch)] = effects.as_slice() else {
        panic!("overflow must cause exactly one replay");
    };
    assert_eq!(batch.len(), JOURNAL_CAPACITY);
    assert_eq!(
        batch
            .entries()
            .iter()
            .filter(|record| record.observed_at_ms == 999)
            .count(),
        1,
        "the current repeat has one replay replacement and is never dropped"
    );
    assert_eq!(batch.entries().last().unwrap().phase, PhysicalPhase::Repeat);
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::X.index());

    let (engine, completion, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        1_000,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert_eq!(engine.foreground_letters(), 0);
}

#[test]
fn failed_initial_delivery_discards_candidate_and_drains_exact_held_ups() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    );
    let (mut engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::DeliverActivation(_) => EffectOutcome::ActivationDelivered(false),
            _ => success(effect),
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [
            EffectRequest::NeutralizeMenu(_),
            EffectRequest::DeliverActivation(_)
        ]
    ));
    assert!(matches!(completion, Completion::Event(outcome)
        if outcome.cancellation == Some(CancelReason::ActivationDeliveryFailed)
            && outcome.disposition == EventDisposition::CaptureCurrent
            && outcome.terminal));
    assert!(!engine.admission_open());
    assert_eq!(engine.foreground_letters(), 0);
    assert_eq!(
        engine.owned_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::P.index())
    );
    for (key, phase) in [
        (ActivationKey::X, PhysicalPhase::Repeat),
        (ActivationKey::X, PhysicalPhase::Up),
        (ActivationKey::P, PhysicalPhase::Up),
    ] {
        let (next, completion, effects) = apply_event(engine, KeyIdentity::Letter(key), phase, 30);
        assert!(effects.is_empty());
        assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
        engine = next;
    }
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn delivery_queue_failure_never_requests_replay_or_cleanup() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::DeliverActivation(_) => EffectOutcome::ActivationDelivered(false),
            _ => success(effect),
        },
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, EffectRequest::DeliverActivation(_)))
    );
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        EffectRequest::Replay(_) | EffectRequest::CleanupInjected(_)
    )));
    assert!(matches!(completion, Completion::Event(outcome)
        if outcome.cancellation == Some(CancelReason::ActivationDeliveryFailed)
            && outcome.terminal));
    assert!(!engine.admission_open());
    assert_eq!(
        engine.owned_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::P.index())
    );
    assert_eq!(engine.foreground_letters(), 0);
}

#[test]
fn activation_up_failure_never_replays_and_enters_exact_ownership_drain() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Up,
        30,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::DeliverActivation(ActivationNotice::Up { .. }) => {
                EffectOutcome::ActivationDelivered(false)
            }
            _ => success(effect),
        },
    );
    assert_eq!(effects.len(), 1);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, EffectRequest::Replay(_)))
    );
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());
    assert!(!engine.admission_open());
    assert!(matches!(completion, Completion::Event(outcome) if outcome.terminal));

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        40,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(effects.is_empty());
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn config_replacement_replays_candidate_and_fences_every_held_key_and_modifier() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let replacement = single_config(2, modifiers(true, false, true, false), ActivationKey::G);
    let (engine, completion, effects) = apply_control(engine, Control::ReplaceConfig(replacement));
    assert!(matches!(effects.as_slice(), [EffectRequest::Replay(_)]));
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(engine.config(), replacement);
    assert_ne!(engine.fenced_letters(), 0);
    assert_ne!(engine.fenced_modifiers().bits(), 0);
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::X.index());
}

#[test]
fn accepted_binding_snapshot_survives_replacement_until_activation_up() {
    let old_config = single_config(1, modifiers(true, false, true, false), ActivationKey::G);
    let mut engine = TransactionEngine::new(old_config);
    for side in [ModifierSide::LeftCtrl, ModifierSide::RightShift] {
        engine = apply_event(engine, KeyIdentity::Modifier(side), PhysicalPhase::Down, 1).0;
    }
    let (engine, _, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::G),
        PhysicalPhase::Down,
        10,
    );
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::DeliverActivation(_)]
    ));
    let replacement = single_config(2, modifiers(true, false, true, false), ActivationKey::H);
    let (engine, _, _) = apply_control(engine, Control::ReplaceConfig(replacement));
    let (engine, _, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::G),
        PhysicalPhase::Up,
        20,
    );
    let [EffectRequest::DeliverActivation(ActivationNotice::Up { binding, .. })] =
        effects.as_slice()
    else {
        panic!("old accepted binding must deliver its up");
    };
    assert_eq!(
        binding.shortcut(),
        old_config.bindings().iter().next().unwrap().shortcut()
    );
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn close_barrier_retains_candidate_until_ordered_cancellation_while_shutdown_drains() {
    let candidate = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (candidate, _, _) = apply_event(
        candidate,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let retained_journal = candidate.journal_len();
    let (candidate, completion, effects) = apply_control(
        candidate,
        Control::CloseAdmission(CancelReason::HelperDisconnected),
    );
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Control(outcome) if outcome.applied && outcome.shutdown == ShutdownState::Running
    ));
    assert_eq!(candidate.journal_len(), retained_journal);
    assert_ne!(candidate.owned_letters(), 0);
    assert!(!candidate.admission_open());

    let (cancelled, completion, effects) = apply_control(
        candidate,
        Control::CancelCandidate(CancelReason::HelperDisconnected),
    );
    assert!(matches!(effects.as_slice(), [EffectRequest::Replay(_)]));
    assert!(matches!(
        completion,
        Completion::Control(outcome) if outcome.applied
    ));
    assert_eq!(cancelled.journal_len(), 0);
    assert_eq!(cancelled.owned_letters(), 0);
    assert!(!cancelled.admission_open());

    let candidate = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (candidate, _, _) = apply_event(
        candidate,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (shutdown, completion, effects) = apply_control(candidate, Control::Shutdown);
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Control(outcome)
            if matches!(outcome.shutdown, ShutdownState::Draining { owned_letters } if owned_letters != 0)
    ));
    assert_ne!(shutdown.owned_letters(), 0);
    assert_eq!(shutdown.journal_len(), 0);
}

#[test]
fn closing_before_unsubmitted_replay_preserves_candidate_for_ordered_retry() {
    let candidate = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (candidate, _, _) = apply_event(
        candidate,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let retained_journal = candidate.journal_len();
    let pending = candidate.begin(EngineInput::Control(Control::CancelCandidate(
        CancelReason::HelperDisconnected,
    )));
    assert!(matches!(
        pending,
        Turn::NeedEffect {
            effect: EffectRequest::Replay(_),
            ..
        }
    ));
    let closed = pending.close_before_unsubmitted_effect(CancelReason::GateClosed);
    let Turn::Complete {
        engine: retained,
        completion: Completion::Control(outcome),
    } = closed
    else {
        panic!("closure cancels the unsubmitted effect without discarding the candidate");
    };
    assert!(!outcome.applied);
    assert!(!retained.admission_open());
    assert_eq!(retained.journal_len(), retained_journal);
    assert_ne!(retained.owned_letters(), 0);

    let (cancelled, completion, effects) = apply_control(
        retained,
        Control::CancelCandidate(CancelReason::HelperDisconnected),
    );
    assert!(matches!(effects.as_slice(), [EffectRequest::Replay(_)]));
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(cancelled.journal_len(), 0);
}

#[test]
fn shutdown_is_safe_at_idle_modifier_candidate_repeat_and_post_replay_boundaries() {
    let idle = TransactionEngine::new(canonical_config(1, true));
    let (idle, completion, effects) = apply_control(idle, Control::Shutdown);
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Control(outcome) if outcome.shutdown == ShutdownState::Quiescent
    ));
    assert_eq!(idle.owned_letters(), 0);

    let modifier = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (modifier, completion, effects) = apply_control(modifier, Control::Shutdown);
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Control(outcome) if outcome.shutdown == ShutdownState::Quiescent
    ));
    assert_eq!(modifier.owned_letters(), 0);

    for with_repeat in [false, true] {
        let candidate = press_alt(
            TransactionEngine::new(canonical_config(1, true)),
            ModifierSide::LeftAlt,
        );
        let (mut candidate, _, _) = apply_event(
            candidate,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            10,
        );
        if with_repeat {
            candidate = apply_event(
                candidate,
                KeyIdentity::Letter(ActivationKey::X),
                PhysicalPhase::Repeat,
                11,
            )
            .0;
        }
        let (candidate, completion, effects) = apply_control(candidate, Control::Shutdown);
        assert!(effects.is_empty());
        assert!(matches!(
            completion,
            Completion::Control(outcome)
                if matches!(outcome.shutdown, ShutdownState::Draining { owned_letters } if owned_letters != 0)
        ));
        assert_ne!(candidate.owned_letters(), 0);
        assert_eq!(candidate.journal_len(), 0);
    }

    let candidate = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let candidate = apply_event(
        candidate,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    )
    .0;
    let (replayed, _, _) = apply_event(
        candidate,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        20,
    );
    assert_eq!(replayed.owned_letters(), 0);
    let (replayed, completion, effects) = apply_control(replayed, Control::Shutdown);
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Control(outcome) if outcome.shutdown == ShutdownState::Quiescent
    ));
    assert_eq!(replayed.owned_letters(), 0);
}

#[test]
fn shutdown_after_commit_is_drain_only_until_every_owned_up() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    );
    let (mut engine, completion, effects) = apply_control(engine, Control::Shutdown);
    assert!(effects.is_empty());
    assert!(
        matches!(completion, Completion::Control(outcome) if matches!(outcome.shutdown, ShutdownState::Draining { .. }))
    );
    for key in [ActivationKey::P, ActivationKey::X] {
        let (next, completion, effects) =
            apply_event(engine, KeyIdentity::Letter(key), PhysicalPhase::Up, 30);
        engine = next;
        assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
        assert!(effects.is_empty());
    }
    assert_eq!(engine.shutdown_state(), ShutdownState::Quiescent);
}

#[test]
fn proven_gap_reconcile_clears_only_committed_keys_no_longer_native_held() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    );
    let (engine, _, _) = apply_control(engine, Control::Shutdown);

    let x_only = PhysicalSnapshot::new(
        1_u32 << u32::from(ActivationKey::X.index()),
        engine.physical_modifiers(),
        false,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Control(Control::Reconcile(x_only))),
        |effect, _| success(effect),
    );
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(
        engine.owned_letters(),
        1_u32 << u32::from(ActivationKey::X.index())
    );

    let released = PhysicalSnapshot::new(0, engine.physical_modifiers(), false);
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Control(Control::Reconcile(released))),
        |effect, _| success(effect),
    );
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(engine.shutdown_state(), ShutdownState::Terminal);
}

#[test]
fn dual_modifier_sides_preserve_candidate_but_altgr_cancels_it() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::RightAlt),
        PhysicalPhase::Down,
        11,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftAlt),
        PhysicalPhase::Up,
        12,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());

    let altgr = event(
        &engine,
        KeyIdentity::Modifier(ModifierSide::RightCtrl),
        PhysicalPhase::Down,
        13,
    )
    .with_alt_gr(true);
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(altgr)), |effect, _| {
            success(effect)
        });
    assert!(matches!(effects.as_slice(), [EffectRequest::Replay(_)]));
    assert!(
        matches!(completion, Completion::Event(outcome) if outcome.cancellation == Some(CancelReason::AltGr))
    );
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn pending_exact_completes_when_alt_is_released_before_its_letter() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftAlt),
        PhysicalPhase::Up,
        20,
    );
    assert!(matches!(
        effects.as_slice(),
        [
            EffectRequest::NeutralizeMenu(_),
            EffectRequest::DeliverActivation(ActivationNotice::Complete {
                binding: _,
                held_ms: 10
            })
        ]
    ));
    assert!(
        matches!(completion, Completion::Event(outcome) if outcome.disposition == EventDisposition::PassCurrent && outcome.cancellation.is_none())
    );
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());
    assert_eq!(engine.foreground_letters(), 0);
    assert_eq!(engine.foreground_modifiers().bits(), 0);

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        30,
    );
    assert!(effects.is_empty());
    assert!(
        matches!(completion, Completion::Event(outcome) if outcome.disposition == EventDisposition::CaptureCurrent && outcome.cancellation.is_none())
    );
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn observed_deferred_release_discards_an_uncommitted_unbalanced_journal() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, _) = drive(
        engine.begin(EngineInput::Control(Control::CloseAdmission(
            CancelReason::GateClosed,
        ))),
        |effect, _| success(effect),
    );
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Control(Control::ReconcileObserved(
            PhysicalSnapshot::default(),
        ))),
        |effect, _| success(effect),
    );
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(engine.owned_letters(), 0);
    assert_eq!(engine.journal_len(), 0);
    assert!(!engine.admission_open());
}

#[test]
fn menu_neutralization_is_deduplicated_until_last_alt_side_releases() {
    let mut engine = press_alt(
        TransactionEngine::new(single_config(
            1,
            modifiers(false, true, false, false),
            ActivationKey::A,
        )),
        ModifierSide::LeftAlt,
    );
    for round in 0..2 {
        let (next, _, effects) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::A),
            PhysicalPhase::Down,
            10 + round * 10,
        );
        engine = next;
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, EffectRequest::NeutralizeMenu(_)))
                .count(),
            usize::from(round == 0)
        );
        engine = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::A),
            PhysicalPhase::Up,
            11 + round * 10,
        )
        .0;
    }
    engine = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftAlt),
        PhysicalPhase::Up,
        40,
    )
    .0;
    engine = press_alt(engine, ModifierSide::RightAlt);
    let (_, _, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        50,
    );
    assert!(matches!(effects[0], EffectRequest::NeutralizeMenu(_)));
}

#[test]
fn revision_mismatch_fences_held_input_and_replay_paste_never_mutate_or_activate() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let stale = NormalizedEvent::physical(
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        native(KeyIdentity::Letter(ActivationKey::X)),
        10,
        ConfigRevision::new(99),
        GateState::Open,
        snapshot_after(
            &engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
        ),
    );
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(stale)), |effect, _| {
            success(effect)
        });
    assert!(effects.is_empty());
    assert!(
        matches!(completion, Completion::Event(outcome) if outcome.cancellation == Some(CancelReason::RevisionMismatch))
    );
    assert_ne!(engine.fenced_letters(), 0);
    assert_ne!(engine.fenced_modifiers().bits(), 0);

    for source in [InputSource::HelperReplay, InputSource::HelperPaste] {
        let before = engine.clone();
        let injected = event(
            &engine,
            KeyIdentity::Letter(ActivationKey::P),
            PhysicalPhase::Down,
            20,
        )
        .with_source(source);
        let (after, completion, effects) = drive(
            engine.clone().begin(EngineInput::Event(injected)),
            |effect, _| success(effect),
        );
        assert_eq!(disposition(completion), EventDisposition::PassCurrent);
        assert!(effects.is_empty());
        assert_eq!(after, before);
    }
}

#[test]
fn escape_and_enter_are_reposted_after_activation_replay_for_session_delivery() {
    for key in [KeyIdentity::Escape, KeyIdentity::Enter] {
        let engine = press_alt(
            TransactionEngine::new(canonical_config(1, true)),
            ModifierSide::LeftAlt,
        );
        let (engine, _, _) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            10,
        );
        let (engine, completion, effects) = apply_event(engine, key, PhysicalPhase::Down, 20);
        assert!(matches!(effects.as_slice(), [EffectRequest::Replay(_)]));
        assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
        assert_eq!(engine.owned_letters(), 0);
    }
}

#[test]
fn all_twenty_six_letters_track_without_aliasing_or_overflow() {
    let mut engine = TransactionEngine::new(canonical_config(1, false));
    for index in 0..26 {
        let key = ActivationKey::from_index(index).unwrap();
        engine = apply_event(
            engine,
            KeyIdentity::Letter(key),
            PhysicalPhase::Down,
            u64::from(index),
        )
        .0;
    }
    assert_eq!(engine.physical_letters(), (1_u32 << 26) - 1);
    assert_eq!(engine.foreground_letters(), (1_u32 << 26) - 1);
    for index in (0..26).rev() {
        let key = ActivationKey::from_index(index).unwrap();
        engine = apply_event(
            engine,
            KeyIdentity::Letter(key),
            PhysicalPhase::Up,
            100 + u64::from(index),
        )
        .0;
    }
    assert_eq!(engine.physical_letters(), 0);
    assert_eq!(engine.foreground_letters(), 0);
}

#[test]
fn partial_replay_requests_only_injected_cleanup_and_enters_terminal_drain() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        20,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::Replay(_) => EffectOutcome::ReplayAccepted { accepted: 1 },
            EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                accepted: batch.len(),
            },
            _ => success(effect),
        },
    );
    assert!(matches!(effects[0], EffectRequest::Replay(_)));
    let EffectRequest::CleanupInjected(cleanup) = effects[1] else {
        panic!("partial replay must clean accepted X-down");
    };
    assert_eq!(cleanup.len(), 1);
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.cancellation == Some(CancelReason::ReplayFailed)
                && outcome.disposition == EventDisposition::CaptureCurrent
                && outcome.terminal
    ));
    assert!(!engine.admission_open());
    assert_eq!(
        engine.owned_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::Y.index())
    );
}

#[test]
fn closed_gate_discards_candidate_passes_unowned_current_and_drains_owned_up() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let closed = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    )
    .with_gate(GateState::Closed);
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(closed)), |effect, _| {
            success(effect)
        });
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Event(outcome)
        if outcome.disposition == EventDisposition::PassCurrent && outcome.terminal));
    assert!(!engine.admission_open());
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());

    let owned_up = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        30,
    )
    .with_gate(GateState::Closed);
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(owned_up)), |effect, _| {
            success(effect)
        });
    assert!(effects.is_empty());
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn every_dual_modifier_family_balances_for_both_release_orders() {
    for (left, right) in [
        (ModifierSide::LeftCtrl, ModifierSide::RightCtrl),
        (ModifierSide::LeftAlt, ModifierSide::RightAlt),
        (ModifierSide::LeftShift, ModifierSide::RightShift),
        (ModifierSide::LeftMeta, ModifierSide::RightMeta),
    ] {
        for releases in [[left, right], [right, left]] {
            let mut engine = TransactionEngine::new(canonical_config(1, false));
            for side in [left, right] {
                engine = apply_event(engine, KeyIdentity::Modifier(side), PhysicalPhase::Down, 1).0;
            }
            assert!(engine.physical_modifiers().contains(left));
            assert!(engine.physical_modifiers().contains(right));
            assert_eq!(engine.physical_modifiers(), engine.foreground_modifiers());
            for side in releases {
                engine = apply_event(engine, KeyIdentity::Modifier(side), PhysicalPhase::Up, 2).0;
            }
            assert_eq!(engine.physical_modifiers().bits(), 0);
            assert_eq!(engine.foreground_modifiers().bits(), 0);
        }
    }
}

#[test]
fn wrong_order_and_extra_held_key_cannot_become_a_delayed_match() {
    let mut engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let mut all_effects = Vec::new();
    for (key, phase) in [
        (ActivationKey::Q, PhysicalPhase::Down),
        (ActivationKey::X, PhysicalPhase::Down),
        (ActivationKey::Q, PhysicalPhase::Up),
        (ActivationKey::P, PhysicalPhase::Down),
    ] {
        let (next, _, effects) = apply_event(engine, KeyIdentity::Letter(key), phase, 10);
        engine = next;
        all_effects.extend(effects);
    }
    assert!(
        !all_effects
            .iter()
            .any(|effect| matches!(effect, EffectRequest::DeliverActivation(_)))
    );
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn neutralization_failure_discards_candidate_and_drains_exact_up() {
    let engine = press_alt(
        TransactionEngine::new(single_config(
            1,
            modifiers(false, true, false, false),
            ActivationKey::A,
        )),
        ModifierSide::LeftAlt,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 0 },
            _ => success(effect),
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::NeutralizeMenu(_)]
    ));
    assert!(matches!(completion, Completion::Event(outcome)
        if outcome.cancellation == Some(CancelReason::NeutralizationFailed)
            && outcome.disposition == EventDisposition::CaptureCurrent
            && outcome.terminal));
    assert!(!engine.admission_open());
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::A.index());
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Up,
        11,
    );
    assert!(effects.is_empty());
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn external_injection_is_input_equivalent_and_supplies_a_match_step() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let external = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::P),
        PhysicalPhase::Down,
        20,
    )
    .with_source(InputSource::External);
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(external)), |effect, _| {
            success(effect)
        });
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, EffectRequest::DeliverActivation(_)))
    );
    assert!(
        matches!(completion, Completion::Event(outcome) if outcome.disposition == EventDisposition::CaptureCurrent)
    );
    assert_ne!(engine.owned_letters(), 0);
}

#[test]
fn mismatched_effect_outcome_closes_admission_deterministically() {
    let mut engine = TransactionEngine::new(single_config(
        1,
        modifiers(true, false, false, false),
        ActivationKey::A,
    ));
    engine = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftCtrl),
        PhysicalPhase::Down,
        1,
    )
    .0;
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        2,
    );
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(input)), |_effect, _| {
            EffectOutcome::Neutralized { accepted: 2 }
        });
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::DeliverActivation(_)]
    ));
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.cancellation == Some(CancelReason::EffectProtocolViolation)
                && outcome.disposition == EventDisposition::CaptureCurrent
                && outcome.terminal
    ));
    assert!(!engine.admission_open());
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::A.index());
}

#[test]
fn partial_replay_preserves_pass_current_added_modifier_disposition() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        11,
    );
    let input = event(
        &engine,
        KeyIdentity::Modifier(ModifierSide::LeftShift),
        PhysicalPhase::Down,
        12,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::Replay(_) => EffectOutcome::ReplayAccepted { accepted: 1 },
            EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                accepted: batch.len(),
            },
            _ => success(effect),
        },
    );
    assert!(matches!(effects[0], EffectRequest::Replay(_)));
    assert!(matches!(effects[1], EffectRequest::CleanupInjected(_)));
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.disposition == EventDisposition::PassCurrent
                && outcome.cancellation == Some(CancelReason::ReplayFailed)
                && outcome.terminal
    ));
    assert!(
        engine
            .foreground_modifiers()
            .contains(ModifierSide::LeftShift)
    );
}

fn three_key_config() -> CompiledActivationConfig {
    let shortcut = Shortcut::new(
        modifiers(true, false, false, false),
        &[ActivationKey::A, ActivationKey::B, ActivationKey::C],
    )
    .unwrap();
    CompiledActivationConfig::compile(
        ConfigRevision::new(1),
        true,
        ActivationBindings::new(&[ActivationBinding::new(ProfileId::GENERAL, shortcut)]).unwrap(),
    )
    .unwrap()
}

fn wrong_third_key_candidate() -> (TransactionEngine, NormalizedEvent) {
    let mut engine = TransactionEngine::new(three_key_config());
    engine = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftCtrl),
        PhysicalPhase::Down,
        1,
    )
    .0;
    for key in [ActivationKey::A, ActivationKey::B] {
        engine = apply_event(engine, KeyIdentity::Letter(key), PhysicalPhase::Down, 2).0;
    }
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        3,
    );
    (engine, input)
}

#[test]
fn failed_cleanup_retains_exact_unaccepted_suffix_for_bounded_retry() {
    for cleanup_accepted in 0..=2 {
        let (engine, input) = wrong_third_key_candidate();
        let (engine, completion, effects) = drive(
            engine.begin(EngineInput::Event(input)),
            |effect, _| match effect {
                EffectRequest::Replay(_) => EffectOutcome::ReplayAccepted { accepted: 2 },
                EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupAccepted {
                    accepted: cleanup_accepted,
                },
                _ => success(effect),
            },
        );
        let EffectRequest::CleanupInjected(requested) = effects[1] else {
            panic!("accepted A/B downs require reverse cleanup");
        };
        assert_eq!(requested.len(), 2);
        assert!(matches!(
            completion,
            Completion::Event(outcome)
                if outcome.disposition == EventDisposition::CaptureCurrent
                    && outcome.cancellation == Some(CancelReason::ReplayFailed)
        ));
        let remaining = engine.pending_injected_cleanup();
        assert_eq!(
            remaining.map_or(0, |batch| batch.len()),
            2 - cleanup_accepted
        );

        if remaining.is_some() {
            let (retried, completion, effects) = apply_control(engine, Control::RetryCleanup);
            assert!(matches!(
                effects.as_slice(),
                [EffectRequest::CleanupInjected(_)]
            ));
            assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
            assert!(retried.pending_injected_cleanup().is_none());
        }
    }
}

#[test]
fn partial_menu_neutralization_retains_dummy_cleanup_until_retry() {
    let engine = press_alt(
        TransactionEngine::new(single_config(
            1,
            modifiers(false, true, false, false),
            ActivationKey::A,
        )),
        ModifierSide::LeftAlt,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        10,
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(input)),
        |effect, _| match effect {
            EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 1 },
            EffectRequest::CleanupMenuNeutralization(_) => {
                EffectOutcome::MenuCleanupAccepted { accepted: 0 }
            }
            _ => success(effect),
        },
    );
    assert!(matches!(effects[0], EffectRequest::NeutralizeMenu(_)));
    assert!(matches!(
        effects[1],
        EffectRequest::CleanupMenuNeutralization(_)
    ));
    assert_eq!(effects.len(), 2);
    assert!(matches!(completion, Completion::Event(outcome) if outcome.terminal));
    assert!(engine.pending_menu_cleanup().is_some());
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::A.index());

    let (engine, completion, effects) = apply_control(engine, Control::RetryCleanup);
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::CleanupMenuNeutralization(_)]
    ));
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert!(engine.pending_menu_cleanup().is_none());
}

#[test]
fn startup_snapshot_fences_preheld_modifier_and_snapshot_mismatch_fails_closed() {
    let mut sides = talking_quill_keyboard_core::transactional::ModifierSides::default();
    sides.insert(ModifierSide::LeftAlt);
    let snapshot = PhysicalSnapshot::new(0, sides, false);
    let engine = TransactionEngine::with_physical_snapshot(canonical_config(1, true), snapshot);
    assert_eq!(engine.physical_modifiers(), sides);
    assert_eq!(engine.foreground_modifiers(), sides);
    assert_eq!(engine.fenced_modifiers(), sides);

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    assert_eq!(engine.owned_letters(), 0);

    let mut inconsistent_sides = sides;
    inconsistent_sides.insert(ModifierSide::RightCtrl);
    let inconsistent = NormalizedEvent::physical(
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        native(KeyIdentity::Letter(ActivationKey::Y)),
        2,
        engine.config().revision(),
        GateState::Open,
        PhysicalSnapshot::new(
            (1_u32 << ActivationKey::X.index()) | (1_u32 << ActivationKey::Y.index()),
            inconsistent_sides,
            false,
        ),
    );
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Event(inconsistent)),
        |effect, _| success(effect),
    );
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.cancellation == Some(CancelReason::PhysicalStateMismatch)
                && outcome.disposition == EventDisposition::PassCurrent
                && outcome.terminal
    ));
    assert!(!engine.admission_open());
}

#[test]
fn cancellation_reason_matrix_closes_terminal_reasons_and_rejects_committed_timeout() {
    let idle = TransactionEngine::new(canonical_config(1, true));
    let (idle, completion, _) =
        apply_control(idle, Control::Cancel(CancelReason::HelperDisconnected));
    assert!(!idle.admission_open());
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));

    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    let (engine, completion, effects) =
        apply_control(engine, Control::Cancel(CancelReason::SecureDesktop));
    assert!(effects.is_empty());
    assert!(!engine.admission_open());
    assert_ne!(engine.owned_letters(), 0);
    assert_eq!(engine.journal_len(), 0);
    assert!(matches!(
        completion,
        Completion::Control(outcome)
            if outcome.applied && outcome.shutdown == ShutdownState::Terminal
    ));

    let mut committed = TransactionEngine::new(single_config(
        1,
        modifiers(true, false, false, false),
        ActivationKey::A,
    ));
    committed = apply_event(
        committed,
        KeyIdentity::Modifier(ModifierSide::LeftCtrl),
        PhysicalPhase::Down,
        1,
    )
    .0;
    committed = apply_event(
        committed,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        2,
    )
    .0;
    let (committed, completion, effects) =
        apply_control(committed, Control::Cancel(CancelReason::Timeout));
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
    assert!(committed.admission_open());

    let (committed, completion, effects) =
        apply_control(committed, Control::Cancel(CancelReason::HelperDisconnected));
    assert!(effects.is_empty());
    assert!(!committed.admission_open());
    assert_ne!(committed.owned_letters(), 0);
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
}

#[test]
fn partial_overflow_preserves_unsubmitted_current_original() {
    for overflowing_phase in [PhysicalPhase::Repeat, PhysicalPhase::Up] {
        let engine = press_alt(
            TransactionEngine::new(canonical_config(1, true)),
            ModifierSide::LeftAlt,
        );
        let (mut engine, _, _) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            1,
        );
        for index in 1..JOURNAL_CAPACITY - 1 {
            (engine, _, _) = apply_event(
                engine,
                KeyIdentity::Letter(ActivationKey::X),
                PhysicalPhase::Repeat,
                index as u64 + 1,
            );
        }
        let input = event(
            &engine,
            KeyIdentity::Letter(ActivationKey::X),
            overflowing_phase,
            999,
        );
        let (engine, completion, effects) = drive(
            engine.begin(EngineInput::Event(input)),
            |effect, _| match effect {
                EffectRequest::Replay(_) => EffectOutcome::ReplayAccepted { accepted: 1 },
                EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                    accepted: batch.len(),
                },
                _ => success(effect),
            },
        );
        assert!(matches!(effects[0], EffectRequest::Replay(_)));
        assert!(matches!(effects[1], EffectRequest::CleanupInjected(_)));
        assert!(matches!(
            completion,
            Completion::Event(outcome)
                if outcome.disposition == EventDisposition::PassCurrent
                    && outcome.cancellation == Some(CancelReason::ReplayFailed)
        ));
        if overflowing_phase == PhysicalPhase::Repeat {
            assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());
            let (engine, completion, _) = apply_event(
                engine,
                KeyIdentity::Letter(ActivationKey::X),
                PhysicalPhase::Up,
                1_000,
            );
            assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
            assert_eq!(engine.owned_letters(), 0);
        } else {
            assert_eq!(engine.owned_letters(), 0);
        }
    }
}

#[test]
fn candidate_reconcile_balances_replay_and_never_drains_released_keys() {
    for replay_accepted in [0_usize, 1, 2] {
        let engine = press_alt(
            TransactionEngine::new(canonical_config(1, true)),
            ModifierSide::LeftAlt,
        );
        let (engine, _, _) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            10,
        );
        let (engine, _, _) = apply_event(
            engine,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Repeat,
            11,
        );
        let snapshot = PhysicalSnapshot::new(0, engine.physical_modifiers(), false);
        let (engine, completion, effects) = drive(
            engine.begin(EngineInput::Control(Control::Reconcile(snapshot))),
            |effect, _| match effect {
                EffectRequest::Replay(batch) => EffectOutcome::ReplayAccepted {
                    accepted: replay_accepted.min(batch.len()),
                },
                EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                    accepted: batch.len(),
                },
                _ => success(effect),
            },
        );
        assert!(matches!(effects[0], EffectRequest::Replay(_)));
        if replay_accepted > 0 {
            assert!(matches!(effects[1], EffectRequest::CleanupInjected(_)));
        }
        assert_eq!(engine.owned_letters(), 0);
        if replay_accepted == 2 {
            assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
        } else {
            assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
        }
    }
}

#[test]
fn candidate_snapshot_mismatch_balances_missing_owned_down_before_reposting_current() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let mismatched = NormalizedEvent::physical(
        KeyIdentity::Other(0x70),
        PhysicalPhase::Down,
        native(KeyIdentity::Other(0x70)),
        20,
        engine.config().revision(),
        GateState::Open,
        PhysicalSnapshot::new(0, engine.physical_modifiers(), false),
    );
    let (engine, completion, effects) =
        drive(engine.begin(EngineInput::Event(mismatched)), |effect, _| {
            success(effect)
        });
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.disposition == EventDisposition::PassCurrent
                && outcome.cancellation == Some(CancelReason::PhysicalStateMismatch)
                && outcome.terminal
    ));
    assert_eq!(engine.owned_letters(), 0);
}

#[test]
fn impossible_letter_and_modifier_phases_fail_closed_and_reconcile() {
    let cases = [
        (KeyIdentity::Letter(ActivationKey::A), PhysicalPhase::Repeat),
        (KeyIdentity::Letter(ActivationKey::A), PhysicalPhase::Up),
        (
            KeyIdentity::Modifier(ModifierSide::LeftCtrl),
            PhysicalPhase::Repeat,
        ),
        (
            KeyIdentity::Modifier(ModifierSide::LeftCtrl),
            PhysicalPhase::Up,
        ),
    ];
    for (key, phase) in cases {
        let engine = TransactionEngine::new(canonical_config(1, false));
        let snapshot = match key {
            KeyIdentity::Letter(letter) if phase == PhysicalPhase::Repeat => {
                PhysicalSnapshot::new(1_u32 << letter.index(), engine.physical_modifiers(), false)
            }
            KeyIdentity::Modifier(side) if phase == PhysicalPhase::Repeat => {
                let mut sides = engine.physical_modifiers();
                sides.insert(side);
                PhysicalSnapshot::new(0, sides, false)
            }
            _ => PhysicalSnapshot::default(),
        };
        let malformed = NormalizedEvent::physical(
            key,
            phase,
            native(key),
            1,
            engine.config().revision(),
            GateState::Open,
            snapshot,
        );
        let (engine, completion, effects) =
            drive(engine.begin(EngineInput::Event(malformed)), |effect, _| {
                success(effect)
            });
        assert!(effects.is_empty());
        assert!(matches!(
            completion,
            Completion::Event(outcome)
                if outcome.cancellation == Some(CancelReason::PhysicalStateMismatch)
                    && outcome.terminal
        ));
        assert!(!engine.admission_open());
    }

    let snapshot =
        PhysicalSnapshot::new(1_u32 << ActivationKey::A.index(), Default::default(), false);
    let engine = TransactionEngine::with_physical_snapshot(canonical_config(1, false), snapshot);
    let duplicate = NormalizedEvent::physical(
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        native(KeyIdentity::Letter(ActivationKey::A)),
        2,
        engine.config().revision(),
        GateState::Open,
        snapshot,
    );
    let (_, completion, _) = drive(engine.begin(EngineInput::Event(duplicate)), |effect, _| {
        success(effect)
    });
    assert!(matches!(completion, Completion::Event(outcome)
        if outcome.cancellation == Some(CancelReason::PhysicalStateMismatch)));
}

#[test]
fn terminal_menu_cleanup_retry_never_creates_a_replay_obligation() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        20,
    );
    let (engine, _, effects) =
        drive(
            engine.begin(EngineInput::Event(input)),
            |effect, _| match effect {
                EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 1 },
                EffectRequest::CleanupMenuNeutralization(_) => {
                    EffectOutcome::MenuCleanupAccepted { accepted: 0 }
                }
                EffectRequest::Replay(_) => EffectOutcome::ReplayAccepted { accepted: 1 },
                EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupAccepted { accepted: 0 },
                _ => success(effect),
            },
        );
    assert!(matches!(
        effects.as_slice(),
        [
            EffectRequest::NeutralizeMenu(_),
            EffectRequest::CleanupMenuNeutralization(_)
        ]
    ));
    assert!(engine.pending_menu_cleanup().is_some());
    assert!(engine.pending_injected_cleanup().is_none());

    let (engine, completion, effects) = apply_control(engine, Control::RetryCleanup);
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::CleanupMenuNeutralization(_)]
    ));
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert!(engine.pending_menu_cleanup().is_none());
    assert!(engine.pending_injected_cleanup().is_none());
}

#[test]
fn generic_shutdown_cancel_uses_shutdown_semantics_and_internal_reasons_are_rejected() {
    let engine = TransactionEngine::new(canonical_config(1, true));
    let (engine, completion, effects) =
        apply_control(engine, Control::Cancel(CancelReason::Shutdown));
    assert!(effects.is_empty());
    assert_eq!(engine.shutdown_state(), ShutdownState::Quiescent);
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));

    let engine = TransactionEngine::new(canonical_config(1, true));
    let (engine, completion, effects) =
        apply_control(engine, Control::Cancel(CancelReason::ConfigurationReplaced));
    assert!(effects.is_empty());
    assert!(engine.admission_open());
    assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
}

#[test]
fn callback_failure_never_replays_into_a_changed_target() {
    let engine = press_alt(
        TransactionEngine::new(single_config(
            1,
            modifiers(false, true, false, false),
            ActivationKey::X,
        )),
        ModifierSide::LeftAlt,
    );
    let input = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    let Turn::NeedEffect {
        effect: EffectRequest::NeutralizeMenu(_),
        continuation,
    } = engine.begin(EngineInput::Event(input))
    else {
        panic!("Alt activation neutralizes before callback delivery");
    };
    let Turn::NeedEffect {
        effect: EffectRequest::DeliverActivation(_),
        continuation,
    } = continuation.resume(EffectOutcome::Neutralized { accepted: 2 })
    else {
        panic!("activation callback follows neutralization");
    };
    let Turn::Complete { engine, completion } =
        continuation.resume(EffectOutcome::ActivationDelivered(false))
    else {
        panic!("callback failure terminalizes without replay");
    };
    assert!(matches!(
        completion,
        Completion::Event(outcome)
            if outcome.cancellation == Some(CancelReason::ActivationDeliveryFailed)
                && outcome.disposition == EventDisposition::CaptureCurrent
                && outcome.terminal
    ));
    assert_eq!(engine.metrics().replay_attempted, 0);
    assert_eq!(engine.owned_letters(), 1 << ActivationKey::X.index());
}

#[test]
fn changed_candidate_target_discards_journal_and_enters_owned_up_drain() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        10,
    );
    assert_eq!(disposition(completion), EventDisposition::CaptureCurrent);
    assert!(effects.is_empty());

    let turn = engine.begin(EngineInput::Control(Control::ReplaceConfig(
        canonical_config(2, true),
    )));
    let Turn::NeedEffect {
        effect: EffectRequest::Replay(_),
        continuation,
    } = turn
    else {
        panic!("candidate replacement requires replay validation");
    };
    let Turn::Complete { engine, completion } =
        continuation.resume(EffectOutcome::ReplaySuppressedTargetChanged)
    else {
        panic!("target-change suppression completes without native effects");
    };
    assert!(matches!(
        completion,
        Completion::Control(outcome)
            if !outcome.applied
                && outcome.cancellation == Some(CancelReason::TargetChanged)
                && outcome.shutdown == ShutdownState::Terminal
    ));
    assert!(!engine.admission_open());
    assert_ne!(engine.owned_letters(), 0);
    assert_eq!(engine.journal_len(), 0);
    assert_eq!(engine.metrics().cancelled, 1);
    assert_eq!(
        engine.metrics().cancellation_reasons[CancelReason::TargetChanged as usize],
        1
    );
    assert_eq!(engine.metrics().replay_attempted, 0);
}

#[test]
fn helper_replay_same_key_bypasses_an_active_candidate_without_recursion() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        2,
    );
    let journal_before = engine.journal_len();
    let helper_replay = event(
        &engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        3,
    )
    .with_source(InputSource::HelperReplay);
    let (after, completion, effects) = drive(
        engine.begin(EngineInput::Event(helper_replay)),
        |effect, _| success(effect),
    );
    assert!(effects.is_empty());
    assert!(matches!(
        completion,
        Completion::Event(outcome) if outcome.disposition == EventDisposition::PassCurrent
    ));
    assert_eq!(after.journal_len(), journal_before);
}

#[test]
fn reconciliation_cleanup_retry_preserves_intervening_live_physical_state() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        2,
    );
    let snapshot = PhysicalSnapshot::new(0, engine.physical_modifiers(), false);
    let (engine, completion, effects) = drive(
        engine.begin(EngineInput::Control(Control::Reconcile(snapshot))),
        |effect, _| match effect {
            EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupAccepted { accepted: 0 },
            _ => success(effect),
        },
    );
    assert!(matches!(effects[0], EffectRequest::Replay(_)));
    assert!(matches!(effects[1], EffectRequest::CleanupInjected(_)));
    assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::X.index());
    assert!(engine.pending_injected_cleanup().is_some());

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::Y),
        PhysicalPhase::Down,
        3,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    assert_eq!(
        engine.foreground_letters(),
        (1 << ActivationKey::X.index()) | (1 << ActivationKey::Y.index())
    );

    let (engine, completion, effects) = apply_control(engine, Control::RetryCleanup);
    assert!(matches!(
        effects.as_slice(),
        [EffectRequest::CleanupInjected(_)]
    ));
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::Y.index());
    assert_eq!(engine.physical_letters(), 1 << ActivationKey::Y.index());
    assert_eq!(engine.foreground_modifiers(), snapshot.modifiers);
    assert!(engine.pending_injected_cleanup().is_none());
}

#[test]
fn cleanup_retry_defers_new_same_key_hold_and_physical_up_satisfies_obligation() {
    let engine = press_alt(
        TransactionEngine::new(canonical_config(1, true)),
        ModifierSide::LeftAlt,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        1,
    );
    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Repeat,
        2,
    );
    let snapshot = PhysicalSnapshot::new(0, engine.physical_modifiers(), false);
    let (engine, _, _) = drive(
        engine.begin(EngineInput::Control(Control::Reconcile(snapshot))),
        |effect, _| match effect {
            EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupAccepted { accepted: 0 },
            _ => success(effect),
        },
    );
    assert!(engine.pending_injected_cleanup().is_some());

    let (engine, completion, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Down,
        3,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    let (engine, completion, effects) = apply_control(engine, Control::RetryCleanup);
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
    assert!(engine.pending_injected_cleanup().is_some());

    let (engine, completion, effects) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::X),
        PhysicalPhase::Up,
        4,
    );
    assert_eq!(disposition(completion), EventDisposition::PassCurrent);
    assert!(effects.is_empty());
    assert!(engine.pending_injected_cleanup().is_none());
    assert_eq!(engine.foreground_letters(), 0);

    let (_, completion, effects) = apply_control(engine, Control::RetryCleanup);
    assert!(effects.is_empty());
    assert!(matches!(completion, Completion::Control(outcome) if outcome.applied));
}

#[test]
fn retry_cleanup_partitions_blocked_key_from_immediately_safe_keys() {
    let mut engine = TransactionEngine::new(three_key_config());
    engine = apply_event(
        engine,
        KeyIdentity::Modifier(ModifierSide::LeftCtrl),
        PhysicalPhase::Down,
        1,
    )
    .0;
    for key in [ActivationKey::A, ActivationKey::B] {
        engine = apply_event(engine, KeyIdentity::Letter(key), PhysicalPhase::Down, 2).0;
    }
    let snapshot = PhysicalSnapshot::new(0, engine.physical_modifiers(), false);
    let (engine, _, effects) = drive(
        engine.begin(EngineInput::Control(Control::Reconcile(snapshot))),
        |effect, _| match effect {
            EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupAccepted { accepted: 0 },
            _ => success(effect),
        },
    );
    let EffectRequest::CleanupInjected(initial) = effects[1] else {
        panic!("both missing replay downs require cleanup");
    };
    assert_eq!(initial.len(), 2);

    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Down,
        3,
    );
    let (engine, completion, effects) = apply_control(engine, Control::RetryCleanup);
    let [EffectRequest::CleanupInjected(ready)] = effects.as_slice() else {
        panic!("unblocked B cleanup must proceed");
    };
    assert_eq!(ready.len(), 1);
    assert_eq!(
        ready.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::B)
    );
    assert!(matches!(completion, Completion::Control(outcome) if !outcome.applied));
    let retained = engine.pending_injected_cleanup().unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(
        retained.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::A)
    );
    assert_eq!(engine.foreground_letters(), 1 << ActivationKey::A.index());

    let (engine, _, _) = apply_event(
        engine,
        KeyIdentity::Letter(ActivationKey::A),
        PhysicalPhase::Up,
        4,
    );
    assert!(engine.pending_injected_cleanup().is_none());
    assert_eq!(engine.foreground_letters(), 0);
}

#[test]
fn aggregate_metrics_cover_commit_replay_dummy_and_cancellation_without_key_payloads() {
    let mut committed = TransactionEngine::new(single_config(
        1,
        modifiers(false, true, false, false),
        ActivationKey::X,
    ));
    let (next, _, _) = drive(
        committed.clone().begin(EngineInput::Event(event(
            &committed,
            KeyIdentity::Modifier(ModifierSide::LeftAlt),
            PhysicalPhase::Down,
            1,
        ))),
        |effect, _| success(effect),
    );
    committed = next;
    let (committed, _, effects) = drive(
        committed.clone().begin(EngineInput::Event(event(
            &committed,
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            2,
        ))),
        |effect, _| success(effect),
    );
    assert!(matches!(effects[0], EffectRequest::NeutralizeMenu(_)));
    let metrics = committed.metrics();
    assert_eq!(metrics.started, 1);
    assert_eq!(metrics.committed, 1);
    assert_eq!(metrics.journal_high_water, 1);
    assert_eq!(metrics.dummy_attempted, 1);
    assert_eq!(metrics.dummy_succeeded, 1);

    let mut replayed = TransactionEngine::new(canonical_config(1, true));
    for (key, phase, at) in [
        (
            KeyIdentity::Modifier(ModifierSide::LeftAlt),
            PhysicalPhase::Down,
            1,
        ),
        (
            KeyIdentity::Letter(ActivationKey::X),
            PhysicalPhase::Down,
            2,
        ),
    ] {
        let (next, _, _) = drive(
            replayed
                .clone()
                .begin(EngineInput::Event(event(&replayed, key, phase, at))),
            |effect, _| success(effect),
        );
        replayed = next;
    }
    let (replayed, _, effects) = drive(
        replayed.clone().begin(EngineInput::Event(event(
            &replayed,
            KeyIdentity::Letter(ActivationKey::Y),
            PhysicalPhase::Down,
            3,
        ))),
        |effect, _| success(effect),
    );
    assert!(matches!(effects[0], EffectRequest::Replay(_)));
    let metrics = replayed.metrics();
    assert_eq!(metrics.started, 1);
    assert_eq!(metrics.replayed, 1);
    assert_eq!(metrics.cancelled, 1);
    assert_eq!(
        metrics.cancellation_reasons[CancelReason::InvalidContinuation as usize],
        1
    );
    assert_eq!(metrics.replay_attempted, 1);
    assert_eq!(metrics.replay_succeeded, 1);
    assert_eq!(metrics.journal_high_water, 2);
}
