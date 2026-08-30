use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, ActivationKey,
    EventPhase, KeyInput, KeyPhase, KeyboardEvent, KeyboardReducer, ModifierMask,
    NativeTargetToken, PhysicalKey, PhysicalKeyTracker, ProfileId, SessionCaptureMode, SessionKey,
    Shortcut, ShortcutModifiers,
};

fn modifiers(ctrl: bool, alt: bool, shift: bool, meta: bool) -> ModifierMask {
    ModifierMask::new(ctrl, alt, shift, meta)
}

fn activation_context(generation: u64) -> ActivationContext {
    ActivationContext::target_unavailable(ActivationGeneration::new(generation).unwrap())
}

fn shortcut(mask: ModifierMask, keys: &[ActivationKey]) -> Shortcut {
    Shortcut::new(mask.into(), keys).unwrap()
}

fn bindings(shortcuts: &[Shortcut]) -> ActivationBindings {
    let values: Vec<_> = shortcuts
        .iter()
        .copied()
        .enumerate()
        .map(|(index, shortcut)| test_binding(index, shortcut))
        .collect();
    ActivationBindings::new(&values).unwrap()
}

fn test_binding(index: usize, shortcut: Shortcut) -> ActivationBinding {
    let alt = modifiers(false, true, false, false);
    let profile_id = if shortcut.modifier_mask() == alt {
        match shortcut.keys() {
            [ActivationKey::X] => ProfileId::GENERAL,
            [ActivationKey::X, ActivationKey::P] => ProfileId::PROMPT,
            [ActivationKey::X, ActivationKey::Q] => ProfileId::PROMPT_TO_ENGLISH,
            [ActivationKey::X, ActivationKey::M] => ProfileId::MARKDOWN,
            [ActivationKey::X, ActivationKey::T] => ProfileId::TRANSLATE_TO_ENGLISH,
            _ => indexed_profile_id(index),
        }
    } else {
        indexed_profile_id(index)
    };
    ActivationBinding::new(profile_id, shortcut)
}

fn indexed_profile_id(index: usize) -> ProfileId {
    match index {
        0 => ProfileId::GENERAL,
        1 => ProfileId::PROMPT,
        2 => ProfileId::PROMPT_TO_ENGLISH,
        3 => ProfileId::MARKDOWN,
        4 => ProfileId::TRANSLATE_TO_ENGLISH,
        _ => ProfileId::new(&format!("00000000-0000-4000-8000-{index:012x}")).unwrap(),
    }
}

fn letter(key: ActivationKey, phase: KeyPhase, mask: ModifierMask) -> KeyInput {
    KeyInput {
        key: PhysicalKey::Letter(key),
        phase,
        modifiers: mask,
        repeat: false,
        injected: false,
    }
}

fn control(key: PhysicalKey, phase: KeyPhase) -> KeyInput {
    KeyInput {
        key,
        phase,
        modifiers: ModifierMask::default(),
        repeat: false,
        injected: false,
    }
}

fn step(
    reducer: &mut KeyboardReducer,
    input: KeyInput,
    configured: ActivationBindings,
    enabled: bool,
    capture: bool,
    delivered: bool,
) -> (Option<KeyboardEvent>, bool) {
    step_at(reducer, configured, input, enabled, capture, delivered, 0)
}

fn step_at(
    reducer: &mut KeyboardReducer,
    configured: ActivationBindings,
    input: KeyInput,
    enabled: bool,
    capture: bool,
    delivered: bool,
    observed_at_ms: u64,
) -> (Option<KeyboardEvent>, bool) {
    let mode = if capture {
        SessionCaptureMode::Recording
    } else {
        SessionCaptureMode::Off
    };
    step_mode_at(
        reducer,
        configured,
        input,
        enabled,
        mode,
        delivered,
        observed_at_ms,
    )
}

fn step_mode_at(
    reducer: &mut KeyboardReducer,
    configured: ActivationBindings,
    input: KeyInput,
    enabled: bool,
    mode: SessionCaptureMode,
    delivered: bool,
    observed_at_ms: u64,
) -> (Option<KeyboardEvent>, bool) {
    let plan = reducer.plan_bindings_at(input, configured, enabled, mode, observed_at_ms);
    let event = plan.event();
    let swallowed = reducer.apply(plan, delivered);
    (event, swallowed)
}

#[test]
fn translate_to_english_binding_keeps_its_exact_full_chord_ownership() {
    let alt = modifiers(false, true, false, false);
    let family = [
        shortcut(alt, &[ActivationKey::X]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ];
    let configured = bindings(&family);
    let binding = ActivationBinding::new(ProfileId::TRANSLATE_TO_ENGLISH, family[3]);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::T, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        )
    );
}

#[test]
fn canonical_general_prefix_completes_on_release_with_physical_hold_duration() {
    let alt = modifiers(false, true, false, false);
    let general = shortcut(alt, &[ActivationKey::X]);
    let configured = bindings(&[
        general,
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ]);
    let binding = ActivationBinding::new(ProfileId::GENERAL, general);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            true,
            false,
            true,
            100,
        ),
        (None, false)
    );
    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Up, alt),
            true,
            false,
            true,
            725,
        ),
        (
            Some(KeyboardEvent::ActivationComplete {
                binding,
                context: activation_context(1),
                held_ms: 625,
            }),
            false,
        )
    );
}

#[test]
fn canonical_prompt_has_an_unambiguous_single_suffix() {
    let alt = modifiers(false, true, false, false);
    let prompt = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[
        shortcut(alt, &[ActivationKey::X]),
        prompt,
        shortcut(alt, &[ActivationKey::X, ActivationKey::Q]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ]);
    let binding = ActivationBinding::new(ProfileId::PROMPT, prompt);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        )
    );
}

#[test]
fn canonical_general_prefix_is_cancelled_by_modifier_release_before_x_release() {
    let alt = modifiers(false, true, false, false);
    let general = shortcut(alt, &[ActivationKey::X]);
    let configured = bindings(&[
        general,
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ]);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            true,
            false,
            true,
            100,
        ),
        (None, false)
    );
    reducer.observe_modifiers(ModifierMask::default());
    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Up, ModifierMask::default(),),
            true,
            false,
            true,
            300,
        ),
        (None, false),
    );
}

#[test]
fn canonical_general_prefix_is_cancelled_when_a_modifier_is_added() {
    let alt = modifiers(false, true, false, false);
    let general = shortcut(alt, &[ActivationKey::X]);
    let configured = bindings(&[
        general,
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ]);

    for added in [
        modifiers(true, true, false, false),
        modifiers(false, true, true, false),
        modifiers(false, true, false, true),
    ] {
        let mut reducer = KeyboardReducer::default();
        assert_eq!(
            step_at(
                &mut reducer,
                configured,
                letter(ActivationKey::X, KeyPhase::Down, alt),
                true,
                false,
                true,
                100,
            ),
            (None, false),
        );
        reducer.observe_modifiers(added);
        assert_eq!(
            step_at(
                &mut reducer,
                configured,
                letter(ActivationKey::X, KeyPhase::Up, added),
                true,
                false,
                true,
                300,
            ),
            (None, false),
        );
    }
}

#[test]
fn canonical_general_prefix_is_cancelled_when_alt_is_readded() {
    let alt = modifiers(false, true, false, false);
    let general = shortcut(alt, &[ActivationKey::X]);
    let configured = bindings(&[
        general,
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::M]),
        shortcut(alt, &[ActivationKey::X, ActivationKey::T]),
    ]);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            true,
            false,
            true,
            100,
        ),
        (None, false),
    );
    reducer.observe_modifiers(ModifierMask::default());
    reducer.observe_modifiers(alt);
    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::X, KeyPhase::Up, alt),
            true,
            false,
            true,
            300,
        ),
        (None, false),
    );
}

#[test]
fn alt_x_p_records_physical_order_passes_prefix_and_captures_only_trigger_sequence() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(reducer.held_letters(), &[ActivationKey::X]);

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        )
    );

    let mut repeat = letter(ActivationKey::P, KeyPhase::Down, alt);
    repeat.repeat = true;
    assert_eq!(
        step(&mut reducer, repeat, configured, true, false, true),
        (None, true)
    );

    // Prefix release passes and cannot alter the accepted shortcut snapshot.
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Up, ModifierMask::default()),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(reducer.held_letters(), &[ActivationKey::P]);

    // Modifier release also cannot change the matching up payload.
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Up, ModifierMask::default()),
            ActivationBindings::default(),
            false,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            true,
        )
    );
    assert!(reducer.held_letters().is_empty());
}

#[test]
fn ctrl_shift_p_requires_exact_complete_modifier_mask() {
    let required = modifiers(true, false, true, false);
    let accepted = shortcut(required, &[ActivationKey::P]);
    let configured = bindings(&[accepted]);

    for mask in 0_u8..16 {
        let actual = modifiers(
            mask & 0b0001 != 0,
            mask & 0b0010 != 0,
            mask & 0b0100 != 0,
            mask & 0b1000 != 0,
        );
        let mut reducer = KeyboardReducer::default();
        let result = step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, actual),
            configured,
            true,
            false,
            true,
        );
        if actual == required {
            assert_eq!(
                result,
                (
                    Some(KeyboardEvent::Activation {
                        binding: test_binding(0, accepted),
                        context: activation_context(1),
                        phase: EventPhase::Down,
                    }),
                    true,
                ),
                "modifier mask {mask:04b}"
            );
        } else {
            assert_eq!(result, (None, false), "modifier mask {mask:04b}");
        }
    }
}

#[test]
fn ordered_complete_held_sequence_rejects_wrong_order_missing_prefix_and_extra_keys() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);

    for downs in [
        vec![ActivationKey::P, ActivationKey::X],
        vec![ActivationKey::P],
        vec![ActivationKey::Q, ActivationKey::X, ActivationKey::P],
    ] {
        let mut reducer = KeyboardReducer::default();
        for key in downs {
            assert_eq!(
                step(
                    &mut reducer,
                    letter(key, KeyPhase::Down, alt),
                    configured,
                    true,
                    false,
                    true,
                ),
                (None, false)
            );
        }
    }
}

#[test]
fn fresh_down_after_releasing_an_extra_key_can_complete_the_exact_chord() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    for key in [ActivationKey::Q, ActivationKey::X] {
        assert!(
            !step(
                &mut reducer,
                letter(key, KeyPhase::Down, alt),
                configured,
                true,
                false,
                true,
            )
            .1
        );
    }
    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::Q, KeyPhase::Up, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert_eq!(reducer.held_letters(), &[ActivationKey::X]);
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        )
    );
}

#[test]
fn repeats_never_add_keys_or_trigger_but_accepted_trigger_repeats_are_swallowed() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    let mut orphaned_x = letter(ActivationKey::X, KeyPhase::Down, alt);
    orphaned_x.repeat = true;
    assert_eq!(
        step(&mut reducer, orphaned_x, configured, true, false, true),
        (None, false)
    );
    assert!(reducer.held_letters().is_empty());

    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    let mut orphaned_p = letter(ActivationKey::P, KeyPhase::Down, alt);
    orphaned_p.repeat = true;
    assert_eq!(
        step(&mut reducer, orphaned_p, configured, true, false, true),
        (None, false)
    );
    assert_eq!(reducer.held_letters(), &[ActivationKey::X]);

    assert!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert_eq!(
        step(&mut reducer, orphaned_p, configured, true, false, true),
        (None, true)
    );
}

#[test]
fn configuration_change_after_down_uses_the_exact_accepted_snapshot_on_up() {
    let alt = modifiers(false, true, false, false);
    let original = shortcut(alt, &[ActivationKey::A, ActivationKey::P]);
    let original_binding = ActivationBinding::new(ProfileId::GENERAL, original);
    let original_bindings = ActivationBindings::new(&[original_binding]).unwrap();
    let replacement_bindings =
        ActivationBindings::new(&[ActivationBinding::new(ProfileId::PROMPT, original)]).unwrap();
    let mut reducer = KeyboardReducer::default();

    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, alt),
            original_bindings,
            true,
            false,
            true,
        )
        .1
    );
    assert!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            original_bindings,
            true,
            false,
            true,
        )
        .1
    );

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Up, ModifierMask::default()),
            replacement_bindings,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: original_binding,
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            true,
        )
    );
}

#[test]
fn failed_activation_down_delivery_fails_open_for_trigger_repeats_and_up() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            false,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            false,
        )
    );
    let mut repeat = letter(ActivationKey::P, KeyPhase::Down, alt);
    repeat.repeat = true;
    assert_eq!(
        step(&mut reducer, repeat, configured, true, false, true),
        (None, false)
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Up, ModifierMask::default()),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
}

#[test]
fn duplicate_nonrepeat_down_never_retries_or_activates_a_different_trigger() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let configured = bindings(&[accepted]);

    for duplicate in [ActivationKey::X, ActivationKey::P] {
        let mut reducer = KeyboardReducer::default();
        assert!(
            !step(
                &mut reducer,
                letter(ActivationKey::X, KeyPhase::Down, alt),
                configured,
                true,
                false,
                true,
            )
            .1
        );
        // Failed trigger delivery intentionally retains the physical held state.
        assert!(
            !step(
                &mut reducer,
                letter(ActivationKey::P, KeyPhase::Down, alt),
                configured,
                true,
                false,
                false,
            )
            .1
        );

        assert_eq!(
            step(
                &mut reducer,
                letter(duplicate, KeyPhase::Down, alt),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
            "duplicate {duplicate:?}"
        );
    }
}

#[test]
fn delivered_activation_up_stays_swallowed_when_up_delivery_fails() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::A]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    assert!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Up, ModifierMask::default()),
            ActivationBindings::default(),
            false,
            false,
            false,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            true,
        )
    );
}

#[test]
fn injected_letters_and_wrong_ups_do_not_change_chord_state() {
    let alt = modifiers(false, true, false, false);
    let configured = bindings(&[shortcut(alt, &[ActivationKey::X, ActivationKey::P])]);
    let mut reducer = KeyboardReducer::default();

    let mut injected = letter(ActivationKey::X, KeyPhase::Down, alt);
    injected.injected = true;
    assert_eq!(
        step(&mut reducer, injected, configured, true, false, true),
        (None, false)
    );
    assert!(reducer.held_letters().is_empty());

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::Q, KeyPhase::Up, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert!(reducer.held_letters().is_empty());
    assert_eq!(reducer.modifiers(), ModifierMask::default());
}

#[test]
fn disabled_activation_still_tracks_physical_order_without_emitting() {
    let exact = modifiers(false, true, false, false);
    let configured = bindings(&[shortcut(exact, &[ActivationKey::A])]);
    let mut reducer = KeyboardReducer::default();
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, exact),
            configured,
            false,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(reducer.held_letters(), &[ActivationKey::A]);
}

#[test]
fn accepted_activation_retains_profile_and_shortcut_across_reassignment() {
    let alt = modifiers(false, true, false, false);
    let chord = shortcut(alt, &[ActivationKey::A, ActivationKey::P]);
    let original = ActivationBinding::new(ProfileId::GENERAL, chord);
    let reassigned = ActivationBinding::new(ProfileId::PROMPT, chord);
    let mut reducer = KeyboardReducer::default();
    let original_bindings = ActivationBindings::new(&[original]).unwrap();

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, alt),
            original_bindings,
            true,
            false,
            true,
        ),
        (None, false),
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            original_bindings,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: original,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Up, ModifierMask::default()),
            ActivationBindings::new(&[reassigned]).unwrap(),
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: original,
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            true,
        ),
    );
}

#[test]
fn modifier_sequence_must_be_nonempty_and_unchanged_until_all_letters_release() {
    let alt = modifiers(false, true, false, false);
    let chord = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let binding = ActivationBinding::new(ProfileId::PROMPT, chord);
    let configured = ActivationBindings::new(&[binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, ModifierMask::default()),
            configured,
            true,
            false,
            true,
        ),
        (None, false),
    );
    reducer.observe_modifiers(alt);
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false),
    );
    for key in [ActivationKey::P, ActivationKey::X] {
        assert!(
            !step(
                &mut reducer,
                letter(key, KeyPhase::Up, alt),
                configured,
                true,
                false,
                true,
            )
            .1
        );
    }

    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    reducer.observe_modifiers(modifiers(false, true, true, false));
    reducer.observe_modifiers(alt);
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false),
    );
    for key in [ActivationKey::P, ActivationKey::X] {
        let _ = step(
            &mut reducer,
            letter(key, KeyPhase::Up, alt),
            configured,
            true,
            false,
            true,
        );
    }

    assert!(
        !step(
            &mut reducer,
            letter(ActivationKey::X, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
}

#[test]
fn binding_revision_fences_passive_prefix_but_preserves_accepted_balance() {
    let alt = modifiers(false, true, false, false);
    let chord = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let binding = ActivationBinding::new(ProfileId::PROMPT, chord);
    let configured = ActivationBindings::new(&[binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    let _ = step(
        &mut reducer,
        letter(ActivationKey::X, KeyPhase::Down, alt),
        configured,
        false,
        false,
        true,
    );
    reducer.fence_activation_revision();
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        ),
        (None, false),
    );
    for key in [ActivationKey::P, ActivationKey::X] {
        let _ = step(
            &mut reducer,
            letter(key, KeyPhase::Up, alt),
            configured,
            true,
            false,
            true,
        );
    }

    let _ = step(
        &mut reducer,
        letter(ActivationKey::X, KeyPhase::Down, alt),
        configured,
        true,
        false,
        true,
    );
    let down = step(
        &mut reducer,
        letter(ActivationKey::P, KeyPhase::Down, alt),
        configured,
        true,
        false,
        true,
    );
    assert_eq!(
        down.0,
        Some(KeyboardEvent::Activation {
            binding,
            context: activation_context(1),
            phase: EventPhase::Down
        })
    );
    reducer.fence_activation_revision();
    reducer.observe_modifiers(ModifierMask::default());
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::P, KeyPhase::Up, ModifierMask::default()),
            ActivationBindings::default(),
            false,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding,
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            true,
        ),
    );
}

#[test]
fn session_capture_gate_behavior_is_preserved_and_modifier_independent() {
    for (physical, session) in [
        (PhysicalKey::Escape, SessionKey::Escape),
        (PhysicalKey::Enter, SessionKey::Enter),
    ] {
        let mut reducer = KeyboardReducer::default();
        let empty = ActivationBindings::default();
        assert_eq!(
            step(
                &mut reducer,
                control(physical, KeyPhase::Down),
                empty,
                false,
                false,
                true,
            ),
            (None, false)
        );
        // A sequence started before capture remains pass-through because the
        // native tracker marks its next observed down as a repeat.
        let mut preheld_repeat = control(physical, KeyPhase::Down);
        preheld_repeat.repeat = true;
        assert_eq!(
            step(&mut reducer, preheld_repeat, empty, false, true, true,),
            (None, false)
        );
        assert_eq!(
            step(
                &mut reducer,
                control(physical, KeyPhase::Up),
                empty,
                false,
                true,
                true,
            ),
            (None, false)
        );

        let mut fresh_with_all_modifiers = control(physical, KeyPhase::Down);
        fresh_with_all_modifiers.modifiers = modifiers(true, true, true, true);
        assert_eq!(
            step(
                &mut reducer,
                fresh_with_all_modifiers,
                empty,
                false,
                true,
                true,
            ),
            (
                Some(KeyboardEvent::SessionKey {
                    key: session,
                    phase: EventPhase::Down,
                }),
                true,
            )
        );
        assert_eq!(
            step(
                &mut reducer,
                control(physical, KeyPhase::Up),
                empty,
                false,
                false,
                false,
            ),
            (
                Some(KeyboardEvent::SessionKey {
                    key: session,
                    phase: EventPhase::Up,
                }),
                true,
            )
        );
    }
}

#[test]
fn proven_callback_gap_clears_only_hidden_session_key_ups() {
    let mut reducer = KeyboardReducer::default();
    let empty = ActivationBindings::default();
    for key in [PhysicalKey::Escape, PhysicalKey::Enter] {
        let (event, swallowed) = step(
            &mut reducer,
            control(key, KeyPhase::Down),
            empty,
            false,
            true,
            true,
        );
        assert!(event.is_some());
        assert!(swallowed);
    }
    assert!(reducer.is_capturing(PhysicalKey::Escape));
    assert!(reducer.is_capturing(PhysicalKey::Enter));

    assert_eq!(
        reducer.reconcile_hidden_session_releases(false, true),
        (true, false)
    );
    assert!(!reducer.is_capturing(PhysicalKey::Escape));
    assert!(reducer.is_capturing(PhysicalKey::Enter));

    assert_eq!(
        reducer.reconcile_hidden_session_releases(false, false),
        (false, true)
    );
    assert!(!reducer.is_capturing(PhysicalKey::Enter));
}

#[test]
fn capture_modes_filter_fresh_keys_without_losing_balancing_ownership() {
    let configured = ActivationBindings::default();

    let mut enter = KeyboardReducer::default();
    assert_eq!(
        step_mode_at(
            &mut enter,
            configured,
            control(PhysicalKey::Enter, KeyPhase::Down),
            false,
            SessionCaptureMode::Recording,
            true,
            0,
        ),
        (
            Some(KeyboardEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
    assert_eq!(
        step_mode_at(
            &mut enter,
            configured,
            control(PhysicalKey::Enter, KeyPhase::Up),
            false,
            SessionCaptureMode::CancelOnly,
            true,
            0,
        ),
        (
            Some(KeyboardEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            }),
            true,
        ),
    );
    assert_eq!(
        step_mode_at(
            &mut enter,
            configured,
            control(PhysicalKey::Enter, KeyPhase::Down),
            false,
            SessionCaptureMode::CancelOnly,
            true,
            0,
        ),
        (None, false),
    );

    let mut escape = KeyboardReducer::default();
    assert_eq!(
        step_mode_at(
            &mut escape,
            configured,
            control(PhysicalKey::Escape, KeyPhase::Down),
            false,
            SessionCaptureMode::CancelOnly,
            true,
            0,
        ),
        (
            Some(KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
    assert_eq!(
        step_mode_at(
            &mut escape,
            configured,
            control(PhysicalKey::Escape, KeyPhase::Up),
            false,
            SessionCaptureMode::Off,
            true,
            0,
        ),
        (
            Some(KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            }),
            true,
        ),
    );
}

#[test]
fn fail_open_balances_delivered_activation_and_session_downs_then_clears_state() {
    let alt = modifiers(false, true, false, false);
    let accepted = shortcut(alt, &[ActivationKey::A]);
    let configured = bindings(&[accepted]);
    let mut reducer = KeyboardReducer::default();

    assert!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, alt),
            configured,
            true,
            false,
            true,
        )
        .1
    );
    assert!(
        step(
            &mut reducer,
            control(PhysicalKey::Escape, KeyPhase::Down),
            configured,
            true,
            true,
            true,
        )
        .1
    );

    assert_eq!(
        reducer.fail_open_balancing_events(),
        [
            Some(KeyboardEvent::Activation {
                binding: test_binding(0, accepted),
                context: activation_context(1),
                phase: EventPhase::Up,
            }),
            Some(KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            }),
            None,
        ]
    );
    assert!(reducer.held_letters().is_empty());
}

#[test]
fn physical_tracker_derives_repeats_and_protects_keys_held_before_gate_open() {
    let mut tracker = PhysicalKeyTracker::default();
    let key = PhysicalKey::Letter(ActivationKey::Z);
    assert!(!tracker.observe(key, KeyPhase::Down));
    assert!(tracker.observe(key, KeyPhase::Down));
    assert!(!tracker.observe(key, KeyPhase::Up));

    // A native backend observes while its callback gate is closed. The next
    // down after opening is a repeat and cannot become a fresh trigger.
    assert!(!tracker.observe(key, KeyPhase::Down));
    let mut reducer = KeyboardReducer::default();
    let alt = modifiers(false, true, false, false);
    let configured = bindings(&[shortcut(alt, &[ActivationKey::Z])]);
    let mut held = letter(ActivationKey::Z, KeyPhase::Down, alt);
    held.repeat = tracker.observe(key, KeyPhase::Down);
    assert_eq!(
        step(&mut reducer, held, configured, true, false, true),
        (None, false)
    );
}

#[test]
fn shortcut_modifier_round_trip_is_exact() {
    let value = ShortcutModifiers {
        ctrl: true,
        alt: false,
        shift: true,
        meta: true,
    };
    let mask = ModifierMask::from(value);
    assert_eq!(ShortcutModifiers::from(mask), value);
    assert_eq!(mask, modifiers(true, false, true, true));
}

#[test]
fn activation_generations_are_stable_for_up_and_increase_for_the_next_activation() {
    let ctrl = modifiers(true, false, false, false);
    let shortcut = shortcut(ctrl, &[ActivationKey::A]);
    let binding = ActivationBinding::new(ProfileId::GENERAL, shortcut);
    let configured = ActivationBindings::new(&[binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    for generation in [1, 2] {
        assert_eq!(
            step(
                &mut reducer,
                letter(ActivationKey::A, KeyPhase::Down, ctrl),
                configured,
                true,
                false,
                true,
            ),
            (
                Some(KeyboardEvent::Activation {
                    binding,
                    context: activation_context(generation),
                    phase: EventPhase::Down,
                }),
                true,
            ),
        );
        assert_eq!(
            step(
                &mut reducer,
                letter(ActivationKey::A, KeyPhase::Up, ctrl),
                configured,
                true,
                false,
                true,
            ),
            (
                Some(KeyboardEvent::Activation {
                    binding,
                    context: activation_context(generation),
                    phase: EventPhase::Up,
                }),
                true,
            ),
        );
    }
}

#[test]
fn failed_activation_delivery_consumes_its_generation_without_retaining_ownership() {
    let ctrl = modifiers(true, false, false, false);
    let shortcut = shortcut(ctrl, &[ActivationKey::A]);
    let binding = ActivationBinding::new(ProfileId::GENERAL, shortcut);
    let configured = ActivationBindings::new(&[binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    let failed = step(
        &mut reducer,
        letter(ActivationKey::A, KeyPhase::Down, ctrl),
        configured,
        true,
        false,
        false,
    );
    assert_eq!(
        failed,
        (
            Some(KeyboardEvent::Activation {
                binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            false,
        ),
    );
    assert!(!reducer.has_captured_sequence());
    let _ = step(
        &mut reducer,
        letter(ActivationKey::A, KeyPhase::Up, ctrl),
        configured,
        true,
        false,
        true,
    );
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Down, ctrl),
            configured,
            true,
            false,
            true,
        )
        .0,
        Some(KeyboardEvent::Activation {
            binding,
            context: activation_context(2),
            phase: EventPhase::Down,
        }),
    );
}

#[test]
fn native_target_attachment_is_frozen_into_the_matching_up() {
    let ctrl = modifiers(true, false, false, false);
    let shortcut = shortcut(ctrl, &[ActivationKey::A]);
    let binding = ActivationBinding::new(ProfileId::GENERAL, shortcut);
    let configured = ActivationBindings::new(&[binding]).unwrap();
    let mut reducer = KeyboardReducer::default();
    let token = NativeTargetToken::new("native-target-1").unwrap();
    assert_eq!(format!("{token:?}"), "NativeTargetToken(<redacted>)");

    let mut down = reducer.plan_bindings(
        letter(ActivationKey::A, KeyPhase::Down, ctrl),
        configured,
        true,
        SessionCaptureMode::Off,
    );
    assert!(down.attach_native_target(token));
    let expected_context = activation_context(1).with_target_token(token);
    assert_eq!(
        down.event(),
        Some(KeyboardEvent::Activation {
            binding,
            context: expected_context,
            phase: EventPhase::Down,
        }),
    );
    assert!(!down.attach_native_target(NativeTargetToken::new("replacement").unwrap()));
    assert!(reducer.apply(down, true));

    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::A, KeyPhase::Up, ctrl),
            configured,
            true,
            false,
            true,
        )
        .0,
        Some(KeyboardEvent::Activation {
            binding,
            context: expected_context,
            phase: EventPhase::Up,
        }),
    );
    assert!(NativeTargetToken::new("").is_err());
    assert!(NativeTargetToken::new(&"é".repeat(32)).is_ok());
    assert!(NativeTargetToken::new(&"é".repeat(33)).is_err());
}

#[test]
fn noncanonical_shared_prefix_completes_with_one_generation_on_release() {
    let ctrl = modifiers(true, false, false, false);
    let shorter = shortcut(ctrl, &[ActivationKey::A]);
    let shorter_binding = ActivationBinding::new(indexed_profile_id(5), shorter);
    let longer_binding = ActivationBinding::new(
        indexed_profile_id(6),
        shortcut(ctrl, &[ActivationKey::A, ActivationKey::B]),
    );
    let configured = ActivationBindings::new(&[shorter_binding, longer_binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::A, KeyPhase::Down, ctrl),
            true,
            false,
            true,
            100,
        ),
        (None, false),
    );
    assert_eq!(
        step_at(
            &mut reducer,
            configured,
            letter(ActivationKey::A, KeyPhase::Up, ctrl),
            true,
            false,
            true,
            225,
        ),
        (
            Some(KeyboardEvent::ActivationComplete {
                binding: shorter_binding,
                context: activation_context(1),
                held_ms: 125,
            }),
            false,
        ),
    );
}

#[test]
fn shared_prefix_keeps_prefix_only_intermediate_steps_until_longest_completion() {
    let ctrl_shift = modifiers(true, false, true, false);
    let shorter_binding = ActivationBinding::new(
        indexed_profile_id(5),
        shortcut(ctrl_shift, &[ActivationKey::A]),
    );
    let longer_binding = ActivationBinding::new(
        indexed_profile_id(6),
        shortcut(
            ctrl_shift,
            &[ActivationKey::A, ActivationKey::B, ActivationKey::C],
        ),
    );
    let configured = ActivationBindings::new(&[shorter_binding, longer_binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    for key in [ActivationKey::A, ActivationKey::B] {
        assert_eq!(
            step(
                &mut reducer,
                letter(key, KeyPhase::Down, ctrl_shift),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
        );
    }
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::C, KeyPhase::Down, ctrl_shift),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: longer_binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
}

#[test]
fn abandoning_a_prefix_only_intermediate_cancels_the_shorter_pending_binding() {
    let ctrl_shift = modifiers(true, false, true, false);
    let shorter_binding = ActivationBinding::new(
        indexed_profile_id(5),
        shortcut(ctrl_shift, &[ActivationKey::A]),
    );
    let longer_binding = ActivationBinding::new(
        indexed_profile_id(6),
        shortcut(
            ctrl_shift,
            &[ActivationKey::A, ActivationKey::B, ActivationKey::C],
        ),
    );
    let configured = ActivationBindings::new(&[shorter_binding, longer_binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    for (key, phase) in [
        (ActivationKey::A, KeyPhase::Down),
        (ActivationKey::B, KeyPhase::Down),
        (ActivationKey::B, KeyPhase::Up),
        (ActivationKey::A, KeyPhase::Up),
    ] {
        assert_eq!(
            step(
                &mut reducer,
                letter(key, phase, ctrl_shift),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
        );
    }

    for key in [ActivationKey::A, ActivationKey::B] {
        assert_eq!(
            step(
                &mut reducer,
                letter(key, KeyPhase::Down, ctrl_shift),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
        );
    }
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::C, KeyPhase::Down, ctrl_shift),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: longer_binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
}

#[test]
fn releasing_a_non_trigger_prefix_cancels_an_exact_binding_with_longer_descendants() {
    let ctrl = modifiers(true, false, false, false);
    let shorter_binding = ActivationBinding::new(
        indexed_profile_id(5),
        shortcut(ctrl, &[ActivationKey::A, ActivationKey::B]),
    );
    let longer_binding = ActivationBinding::new(
        indexed_profile_id(6),
        shortcut(
            ctrl,
            &[ActivationKey::A, ActivationKey::B, ActivationKey::C],
        ),
    );
    let configured = ActivationBindings::new(&[shorter_binding, longer_binding]).unwrap();
    let mut reducer = KeyboardReducer::default();

    for (key, phase) in [
        (ActivationKey::A, KeyPhase::Down),
        (ActivationKey::B, KeyPhase::Down),
        (ActivationKey::A, KeyPhase::Up),
        (ActivationKey::B, KeyPhase::Up),
    ] {
        assert_eq!(
            step(
                &mut reducer,
                letter(key, phase, ctrl),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
        );
    }

    for key in [ActivationKey::A, ActivationKey::B] {
        assert_eq!(
            step(
                &mut reducer,
                letter(key, KeyPhase::Down, ctrl),
                configured,
                true,
                false,
                true,
            ),
            (None, false),
        );
    }
    assert_eq!(
        step(
            &mut reducer,
            letter(ActivationKey::C, KeyPhase::Down, ctrl),
            configured,
            true,
            false,
            true,
        ),
        (
            Some(KeyboardEvent::Activation {
                binding: longer_binding,
                context: activation_context(1),
                phase: EventPhase::Down,
            }),
            true,
        ),
    );
}

#[test]
fn complete_activation_target_attachment_is_one_shot() {
    let alt = modifiers(false, true, false, false);
    let general = shortcut(alt, &[ActivationKey::X]);
    let configured = bindings(&[
        general,
        shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
    ]);
    let binding = ActivationBinding::new(ProfileId::GENERAL, general);
    let mut reducer = KeyboardReducer::default();
    let down = reducer.plan_bindings_at(
        letter(ActivationKey::X, KeyPhase::Down, alt),
        configured,
        true,
        SessionCaptureMode::Off,
        100,
    );
    assert!(down.event().is_none());
    assert!(!reducer.apply(down, true));

    let mut complete = reducer.plan_bindings_at(
        letter(ActivationKey::X, KeyPhase::Up, alt),
        configured,
        true,
        SessionCaptureMode::Off,
        200,
    );
    let token = NativeTargetToken::new("complete-target").unwrap();
    assert!(complete.attach_native_target(token));
    assert!(!complete.attach_native_target(NativeTargetToken::new("replacement").unwrap()));
    assert_eq!(
        complete.event(),
        Some(KeyboardEvent::ActivationComplete {
            binding,
            context: activation_context(1).with_target_token(token),
            held_ms: 100,
        }),
    );
}
