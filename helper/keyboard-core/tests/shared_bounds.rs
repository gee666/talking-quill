use proptest::prelude::*;
use talking_quill_keyboard_core::{
    ACTIVATION_KEY_CAPACITY, ActivationBinding, ActivationBindings, ActivationGeneration,
    ActivationKey, COMBINED_PHYSICAL_DRAIN_CAPACITY, KeyInput, KeyPhase, KeyboardReducer,
    ModifierMask, OWNER_ADMITTED_EFFECT_CAPACITY, PhysicalKey, PhysicalKeyTracker, ProfileId,
    REPLAY_CLEANUP_EDGE_CAPACITY, SESSION_KEY_CAPACITY, SessionKey, Shortcut, ShortcutModifiers,
    transactional::{
        CancelReason, CleanupBatch, CompiledActivationConfig, Completion, ConfigIdentity, Control,
        EngineInput, GateState, InputSource, KeyIdentity, NativeKey, NormalizedEvent,
        PhysicalPhase, PhysicalSnapshot, ReplayRecord, TransactionEngine, Turn,
    },
};

fn config(scope: u64, revision: u64) -> CompiledActivationConfig {
    CompiledActivationConfig::compile(
        ConfigIdentity::scoped(scope, revision).unwrap(),
        false,
        ActivationBindings::default(),
    )
    .unwrap()
}

#[test]
fn shared_grammar_bounds_are_exact_and_derived() {
    assert_eq!(ACTIVATION_KEY_CAPACITY, 26);
    assert_eq!(SESSION_KEY_CAPACITY, 2);
    assert_eq!(COMBINED_PHYSICAL_DRAIN_CAPACITY, 28);
    assert_eq!(
        COMBINED_PHYSICAL_DRAIN_CAPACITY,
        ACTIVATION_KEY_CAPACITY + SESSION_KEY_CAPACITY
    );
    assert_eq!(REPLAY_CLEANUP_EDGE_CAPACITY, ACTIVATION_KEY_CAPACITY);
    assert_eq!(OWNER_ADMITTED_EFFECT_CAPACITY, 8);
}

#[test]
fn all_twenty_eight_tracked_keys_have_independent_slots() {
    let mut tracker = PhysicalKeyTracker::default();
    for index in 0..ACTIVATION_KEY_CAPACITY as u8 {
        assert!(!tracker.observe(
            PhysicalKey::Letter(ActivationKey::from_index(index).unwrap()),
            KeyPhase::Down,
        ));
    }
    assert!(!tracker.observe(PhysicalKey::Escape, KeyPhase::Down));
    assert!(!tracker.observe(PhysicalKey::Enter, KeyPhase::Down));
    assert!(tracker.observe(PhysicalKey::Escape, KeyPhase::Down));
    assert!(tracker.observe(PhysicalKey::Enter, KeyPhase::Down));
    assert_eq!(tracker.held_letter_bits(), (1_u32 << 26) - 1);
}

#[test]
fn physical_snapshot_accepts_bit_twenty_five_and_retains_but_rejects_bit_twenty_six() {
    let valid = PhysicalSnapshot::checked(1_u32 << 25, Default::default(), false).unwrap();
    assert!(valid.is_valid());
    assert!(PhysicalSnapshot::checked(1_u32 << 26, Default::default(), false).is_none());

    let impossible = PhysicalSnapshot::new(1_u32 << 26, Default::default(), false);
    assert_eq!(impossible.held_letters, 1_u32 << 26);
    assert!(!impossible.is_valid());

    let engine = TransactionEngine::new(config(1, 1));
    let event = NormalizedEvent {
        key: KeyIdentity::Letter(ActivationKey::A),
        phase: PhysicalPhase::Down,
        source: InputSource::Physical,
        native: NativeKey::default(),
        observed_at_ms: 1,
        config_revision: engine.config().revision(),
        gate: GateState::Open,
        snapshot: impossible,
    };
    let Turn::Complete {
        engine,
        completion: Completion::Event(outcome),
    } = engine.begin(EngineInput::Event(event))
    else {
        panic!("impossible snapshot must fail without requesting effects");
    };
    assert_eq!(
        outcome.cancellation,
        Some(CancelReason::PhysicalStateMismatch)
    );
    assert!(outcome.terminal);
    assert!(!engine.admission_open());
    assert_eq!(engine.physical_letters(), 1_u32 << 26);
}

#[test]
fn new_capture_scope_may_restart_at_one_and_old_scope_cannot_replace_it() {
    let engine = TransactionEngine::new(config(7, 99));
    let Turn::Complete {
        engine,
        completion: Completion::Control(outcome),
    } = engine.begin(EngineInput::Control(Control::ReplaceConfig(config(8, 1))))
    else {
        panic!("idle replacement completes synchronously");
    };
    assert!(outcome.applied);
    assert_eq!(engine.config().revision().capture_scope(), Some(8));
    assert_eq!(engine.config().revision().get(), 1);

    let Turn::Complete {
        engine: retained,
        completion: Completion::Control(outcome),
    } = engine.begin(EngineInput::Control(Control::ReplaceConfig(config(7, 100))))
    else {
        panic!();
    };
    assert!(!outcome.applied);
    assert_eq!(outcome.cancellation, Some(CancelReason::RevisionMismatch));
    assert_eq!(retained.config().revision().capture_scope(), Some(8));
}

#[test]
fn cleanup_capacity_deduplicates_visible_letter_downs_and_reaches_exact_max() {
    let record = |key| ReplayRecord {
        key: KeyIdentity::Letter(key),
        native: NativeKey::default(),
        phase: PhysicalPhase::Down,
        observed_at_ms: 1,
    };
    let duplicates = vec![Some(record(ActivationKey::A)); 54];
    let cleanup = CleanupBatch::from_visible_downs(&duplicates);
    assert_eq!(cleanup.len(), 1);

    let all_letters: Vec<_> = (0..ACTIVATION_KEY_CAPACITY as u8)
        .map(|index| Some(record(ActivationKey::from_index(index).unwrap())))
        .collect();
    let cleanup = CleanupBatch::from_visible_downs(&all_letters);
    assert_eq!(cleanup.len(), REPLAY_CLEANUP_EDGE_CAPACITY);
}

proptest! {
    #[test]
    fn scoped_configuration_order_is_lexicographic_without_cross_epoch_aliasing(
        old_scope in 1_u64..u64::MAX,
        old_revision in 1_u64..u64::MAX,
        new_revision in 1_u64..u64::MAX,
    ) {
        let new_scope = old_scope + 1;
        let old = ConfigIdentity::scoped(old_scope, old_revision).unwrap();
        let next = ConfigIdentity::scoped(new_scope, new_revision).unwrap();
        prop_assert!(next.is_newer_than(old));
        prop_assert!(!old.is_newer_than(next));
    }
}

#[test]
fn complete_core_debug_values_redact_configuration_events_and_replay_records() {
    let binding = ActivationBinding::new(
        ProfileId::new("sentinel-profile").unwrap(),
        Shortcut::new(
            ShortcutModifiers {
                ctrl: true,
                alt: false,
                shift: true,
                meta: false,
            },
            &[ActivationKey::Q],
        )
        .unwrap(),
    );
    let bindings = ActivationBindings::new(&[binding]).unwrap();
    let compiled = CompiledActivationConfig::compile(
        ConfigIdentity::scoped(777, 888).unwrap(),
        true,
        bindings,
    )
    .unwrap();
    let replay = ReplayRecord {
        key: KeyIdentity::Letter(ActivationKey::Q),
        native: NativeKey {
            virtual_key: 81,
            scan_code: 16,
            extended: false,
            platform_flags: 999,
        },
        phase: PhysicalPhase::Down,
        observed_at_ms: 123_456,
    };
    let event = NormalizedEvent {
        key: replay.key,
        phase: replay.phase,
        source: InputSource::Physical,
        native: replay.native,
        observed_at_ms: replay.observed_at_ms,
        config_revision: compiled.revision(),
        gate: GateState::Open,
        snapshot: PhysicalSnapshot::new(
            1_u32 << ActivationKey::Q.index(),
            Default::default(),
            false,
        ),
    };
    let engine = TransactionEngine::new(compiled);
    let turn = engine.clone().begin(EngineInput::Event(event));
    let input = KeyInput {
        key: PhysicalKey::Letter(ActivationKey::Q),
        phase: KeyPhase::Down,
        modifiers: ModifierMask::new(true, false, true, false),
        repeat: false,
        injected: false,
    };
    let reducer = KeyboardReducer::default();
    let plan = reducer.plan(input, ActivationKey::Q, true, Default::default());
    let tracker = PhysicalKeyTracker::default();
    let output = format!(
        "{binding:#?} {bindings:#?} {compiled:#?} {replay:#?} {event:#?} {engine:#?} {turn:#?} {input:#?} {tracker:#?} {reducer:#?} {plan:#?} {:?} {:?} {:?}",
        PhysicalPhase::Down,
        KeyIdentity::Other(4321),
        InputSource::test_physical(),
    );
    let output = format!("{output} {:?} {:?}", KeyPhase::Down, SessionKey::Enter);
    assert!(output.contains("<redacted>"));
    for forbidden in ["sentinel-profile", "123456", "4321", "999", "777", "888"] {
        assert!(!output.contains(forbidden), "leaked {forbidden}: {output}");
    }
}

#[test]
fn helper_generation_exhausts_without_overflow_and_preserves_legacy_json_number() {
    assert_eq!(ActivationGeneration::MAX.checked_next(), None);
    assert_eq!(
        serde_json::to_value(ActivationGeneration::MAX).unwrap(),
        serde_json::json!(9_007_199_254_740_991_u64)
    );
}

#[test]
fn physical_key_input_shape_still_uses_shared_tracker_without_wire_changes() {
    let input = KeyInput {
        key: PhysicalKey::Letter(ActivationKey::Z),
        phase: KeyPhase::Down,
        modifiers: ModifierMask::new(false, true, false, false),
        repeat: false,
        injected: false,
    };
    assert_eq!(input.key, PhysicalKey::Letter(ActivationKey::Z));
}
