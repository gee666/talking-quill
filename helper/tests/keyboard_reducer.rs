use talking_quill_helper::keyboard::{
    ActivationKey, EventPhase, HelperEvent, KeyInput, KeyPhase, KeyboardReducer, PhysicalKey,
    PhysicalKeyTracker, SessionKey,
};

fn input(key: PhysicalKey, phase: KeyPhase, alt: bool, shift: bool, injected: bool) -> KeyInput {
    KeyInput {
        key,
        phase,
        alt,
        shift,
        disallowed_modifiers: false,
        repeat: false,
        injected,
    }
}

fn step(
    reducer: &mut KeyboardReducer,
    input: KeyInput,
    configured: ActivationKey,
    capture: bool,
    delivery_succeeds: bool,
) -> (Option<HelperEvent>, bool) {
    step_with_activation(reducer, input, configured, true, capture, delivery_succeeds)
}

fn step_with_activation(
    reducer: &mut KeyboardReducer,
    input: KeyInput,
    configured: ActivationKey,
    activation_enabled: bool,
    capture: bool,
    delivery_succeeds: bool,
) -> (Option<HelperEvent>, bool) {
    let plan = reducer.plan(input, configured, activation_enabled, capture);
    let event = plan.event();
    let swallowed = reducer.apply(plan, delivery_succeeds);
    (event, swallowed)
}

#[test]
fn activation_down_repeats_and_up_are_emitted_once_and_swallowed() {
    let mut reducer = KeyboardReducer::default();
    let down = input(
        PhysicalKey::Letter(ActivationKey::Z),
        KeyPhase::Down,
        true,
        false,
        false,
    );
    assert_eq!(
        step(&mut reducer, down, ActivationKey::Z, false, true),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
            }),
            true,
        )
    );
    assert_eq!(
        step(&mut reducer, down, ActivationKey::Z, false, true),
        (None, true)
    );
    assert_eq!(
        step(
            &mut reducer,
            input(
                PhysicalKey::Letter(ActivationKey::Z),
                KeyPhase::Up,
                false,
                false,
                false,
            ),
            ActivationKey::Z,
            false,
            true,
        ),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: EventPhase::Up,
                shift: false,
            }),
            true,
        )
    );
}

#[test]
fn shift_state_is_remembered_until_activation_up() {
    let mut reducer = KeyboardReducer::default();
    let (event, swallowed) = step(
        &mut reducer,
        input(
            PhysicalKey::Letter(ActivationKey::A),
            KeyPhase::Down,
            true,
            true,
            false,
        ),
        ActivationKey::A,
        false,
        true,
    );
    assert_eq!(
        event,
        Some(HelperEvent::Activation {
            key: ActivationKey::A,
            phase: EventPhase::Down,
            shift: true,
        })
    );
    assert!(swallowed);
    assert_eq!(
        step(
            &mut reducer,
            input(
                PhysicalKey::Letter(ActivationKey::A),
                KeyPhase::Up,
                false,
                false,
                false,
            ),
            ActivationKey::B,
            false,
            true,
        ),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift: true,
            }),
            true,
        )
    );
}

#[test]
fn activation_requires_alt_and_the_configured_letter() {
    let mut reducer = KeyboardReducer::default();
    for event in [
        input(
            PhysicalKey::Letter(ActivationKey::Z),
            KeyPhase::Down,
            false,
            false,
            false,
        ),
        input(
            PhysicalKey::Letter(ActivationKey::Y),
            KeyPhase::Down,
            true,
            false,
            false,
        ),
        input(PhysicalKey::Other, KeyPhase::Down, true, true, false),
    ] {
        assert_eq!(
            step(&mut reducer, event, ActivationKey::Z, false, true),
            (None, false)
        );
    }
}

#[test]
fn activation_enable_and_modifier_matrix_is_exhaustive() {
    for mask in 0_u8..16 {
        let enabled = mask & 0b0001 != 0;
        let alt = mask & 0b0010 != 0;
        let shift = mask & 0b0100 != 0;
        let disallowed = mask & 0b1000 != 0;
        let mut reducer = KeyboardReducer::default();
        let mut down = input(
            PhysicalKey::Letter(ActivationKey::Z),
            KeyPhase::Down,
            alt,
            shift,
            false,
        );
        down.disallowed_modifiers = disallowed;

        let (event, swallowed) =
            step_with_activation(&mut reducer, down, ActivationKey::Z, enabled, false, true);
        if enabled && alt && !disallowed {
            assert_eq!(
                event,
                Some(HelperEvent::Activation {
                    key: ActivationKey::Z,
                    phase: EventPhase::Down,
                    shift,
                }),
                "mask {mask:04b}"
            );
            assert!(swallowed, "mask {mask:04b}");
        } else {
            assert_eq!(event, None, "mask {mask:04b}");
            assert!(!swallowed, "mask {mask:04b}");
        }
    }
}

#[test]
fn passing_activation_sequence_stays_passing_across_configuration_changes() {
    let mut reducer = KeyboardReducer::default();
    let mut down = input(
        PhysicalKey::Letter(ActivationKey::A),
        KeyPhase::Down,
        true,
        false,
        false,
    );
    down.disallowed_modifiers = true;
    assert_eq!(
        step_with_activation(&mut reducer, down, ActivationKey::A, true, false, true,),
        (None, false)
    );

    let allowed_down = input(
        PhysicalKey::Letter(ActivationKey::A),
        KeyPhase::Down,
        true,
        false,
        false,
    );
    assert_eq!(
        step_with_activation(
            &mut reducer,
            allowed_down,
            ActivationKey::A,
            true,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(
        step_with_activation(
            &mut reducer,
            input(
                PhysicalKey::Letter(ActivationKey::A),
                KeyPhase::Up,
                false,
                false,
                false,
            ),
            ActivationKey::B,
            false,
            false,
            true,
        ),
        (None, false)
    );

    assert!(
        step_with_activation(
            &mut reducer,
            allowed_down,
            ActivationKey::A,
            true,
            false,
            true,
        )
        .1
    );
}

#[test]
fn captured_activation_sequence_stays_captured_across_configuration_changes() {
    let mut reducer = KeyboardReducer::default();
    let down = input(
        PhysicalKey::Letter(ActivationKey::A),
        KeyPhase::Down,
        true,
        true,
        false,
    );
    assert!(step_with_activation(&mut reducer, down, ActivationKey::A, true, false, true,).1);

    let mut changed_repeat = down;
    changed_repeat.disallowed_modifiers = true;
    assert_eq!(
        step_with_activation(
            &mut reducer,
            changed_repeat,
            ActivationKey::B,
            false,
            false,
            true,
        ),
        (None, true)
    );
    let mut changed_up = input(
        PhysicalKey::Letter(ActivationKey::A),
        KeyPhase::Up,
        false,
        false,
        false,
    );
    changed_up.disallowed_modifiers = true;
    assert_eq!(
        step_with_activation(
            &mut reducer,
            changed_up,
            ActivationKey::B,
            false,
            false,
            true,
        ),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift: true,
            }),
            true,
        )
    );
}

#[test]
fn delivered_activation_down_forces_matching_up_swallow_on_delivery_failure() {
    for up_delivery_succeeds in [false, true] {
        let mut reducer = KeyboardReducer::default();
        let key = PhysicalKey::Letter(ActivationKey::A);
        assert!(
            step_with_activation(
                &mut reducer,
                input(key, KeyPhase::Down, true, false, false),
                ActivationKey::A,
                true,
                false,
                true,
            )
            .1
        );
        assert_eq!(
            step_with_activation(
                &mut reducer,
                input(key, KeyPhase::Up, false, false, false),
                ActivationKey::B,
                false,
                false,
                up_delivery_succeeds,
            ),
            (
                Some(HelperEvent::Activation {
                    key: ActivationKey::A,
                    phase: EventPhase::Up,
                    shift: false,
                }),
                true,
            ),
            "up delivery succeeds: {up_delivery_succeeds}"
        );

        // The physical sequence is complete in either outcome. In production,
        // a failed up also closes the callback gate before this later input.
        assert!(
            step_with_activation(
                &mut reducer,
                input(key, KeyPhase::Down, true, false, false),
                ActivationKey::A,
                true,
                false,
                true,
            )
            .1
        );
    }
}

#[test]
fn delivered_session_down_forces_matching_up_swallow_on_delivery_failure() {
    for (physical, session) in [
        (PhysicalKey::Escape, SessionKey::Escape),
        (PhysicalKey::Enter, SessionKey::Enter),
    ] {
        for up_delivery_succeeds in [false, true] {
            let mut reducer = KeyboardReducer::default();
            assert!(
                step_with_activation(
                    &mut reducer,
                    input(physical, KeyPhase::Down, false, false, false),
                    ActivationKey::Z,
                    false,
                    true,
                    true,
                )
                .1
            );
            assert_eq!(
                step_with_activation(
                    &mut reducer,
                    input(physical, KeyPhase::Up, false, false, false),
                    ActivationKey::Z,
                    false,
                    false,
                    up_delivery_succeeds,
                ),
                (
                    Some(HelperEvent::SessionKey {
                        key: session,
                        phase: EventPhase::Up,
                    }),
                    true,
                ),
                "{session:?}, up delivery succeeds: {up_delivery_succeeds}"
            );
        }
    }
}

#[test]
fn fail_open_balances_all_delivered_downs_and_clears_stale_sequences() {
    let mut reducer = KeyboardReducer::default();
    let activation_key = PhysicalKey::Letter(ActivationKey::A);
    assert!(
        step_with_activation(
            &mut reducer,
            input(activation_key, KeyPhase::Down, true, false, false),
            ActivationKey::A,
            true,
            false,
            true,
        )
        .1
    );
    assert!(
        step_with_activation(
            &mut reducer,
            input(PhysicalKey::Escape, KeyPhase::Down, false, false, false),
            ActivationKey::A,
            true,
            true,
            true,
        )
        .1
    );

    assert_eq!(
        reducer.fail_open_balancing_events(),
        [
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift: false,
            }),
            Some(HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            }),
            None,
        ]
    );
    assert_eq!(
        step_with_activation(
            &mut reducer,
            input(activation_key, KeyPhase::Up, false, false, false),
            ActivationKey::A,
            false,
            false,
            true,
        ),
        (None, false)
    );
    assert_eq!(
        step_with_activation(
            &mut reducer,
            input(PhysicalKey::Escape, KeyPhase::Up, false, false, false),
            ActivationKey::A,
            false,
            false,
            true,
        ),
        (None, false)
    );
}

#[test]
fn session_capture_is_independent_of_activation_modifiers_and_enablement() {
    let mut reducer = KeyboardReducer::default();
    let mut escape = input(PhysicalKey::Escape, KeyPhase::Down, true, true, false);
    escape.disallowed_modifiers = true;
    assert_eq!(
        step_with_activation(&mut reducer, escape, ActivationKey::Z, false, true, true,),
        (
            Some(HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            }),
            true,
        )
    );
}

#[test]
fn registered_hotkey_down_and_passive_hook_up_form_one_balanced_activation() {
    for shift in [false, true] {
        let mut reducer = KeyboardReducer::default();
        let key = PhysicalKey::Letter(ActivationKey::A);
        let low_level_down = input(key, KeyPhase::Down, true, shift, false);
        let passive = reducer.plan_passive_hook(low_level_down, ActivationKey::A, true, false);
        assert!(passive.event().is_none());
        assert!(!reducer.apply(passive, true));

        let hotkey =
            reducer.plan_registered_hotkey(ActivationKey::A, shift, ActivationKey::A, true);
        assert_eq!(
            hotkey.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Down,
                shift,
            })
        );
        assert!(reducer.apply(hotkey, true));

        let duplicate =
            reducer.plan_registered_hotkey(ActivationKey::A, shift, ActivationKey::A, true);
        assert!(duplicate.event().is_none());
        let _ = reducer.apply(duplicate, true);

        let mut repeat = low_level_down;
        repeat.repeat = true;
        let passive_repeat = reducer.plan_passive_hook(repeat, ActivationKey::B, false, false);
        assert!(passive_repeat.event().is_none());
        assert!(!reducer.apply(passive_repeat, true));

        let up = reducer.plan_passive_hook(
            input(key, KeyPhase::Up, false, false, false),
            ActivationKey::B,
            false,
            false,
        );
        assert_eq!(
            up.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift,
            })
        );
        assert!(reducer.apply(up, false));
    }
}

#[test]
fn failed_registered_hotkey_down_leaves_passive_hook_sequence_open() {
    let mut reducer = KeyboardReducer::default();
    let hotkey = reducer.plan_registered_hotkey(ActivationKey::A, false, ActivationKey::A, true);
    assert!(!reducer.apply(hotkey, false));

    let up = reducer.plan_passive_hook(
        input(
            PhysicalKey::Letter(ActivationKey::A),
            KeyPhase::Up,
            false,
            false,
            false,
        ),
        ActivationKey::A,
        true,
        false,
    );
    assert!(up.event().is_none());
    assert!(!reducer.apply(up, true));
}

#[test]
fn passive_hook_never_captures_unrelated_alt_input() {
    let mut reducer = KeyboardReducer::default();
    for key in [
        PhysicalKey::Letter(ActivationKey::A),
        PhysicalKey::Letter(ActivationKey::B),
        PhysicalKey::Other,
    ] {
        let plan = reducer.plan_passive_hook(
            input(key, KeyPhase::Down, true, true, false),
            ActivationKey::A,
            true,
            false,
        );
        assert!(plan.event().is_none());
        assert!(!reducer.apply(plan, true));
    }
}

#[test]
fn failed_activation_delivery_passes_the_complete_sequence_through() {
    let mut reducer = KeyboardReducer::default();
    let down = input(
        PhysicalKey::Letter(ActivationKey::Z),
        KeyPhase::Down,
        true,
        false,
        false,
    );
    assert_eq!(
        step(&mut reducer, down, ActivationKey::Z, false, false),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
            }),
            false,
        )
    );
    assert_eq!(
        step(&mut reducer, down, ActivationKey::Z, false, true),
        (None, false)
    );
    assert_eq!(
        step(
            &mut reducer,
            input(
                PhysicalKey::Letter(ActivationKey::Z),
                KeyPhase::Up,
                false,
                false,
                false,
            ),
            ActivationKey::Z,
            false,
            true,
        ),
        (None, false)
    );
}

#[test]
fn escape_and_enter_are_captured_only_for_sequences_started_in_session_mode() {
    for (physical, session) in [
        (PhysicalKey::Escape, SessionKey::Escape),
        (PhysicalKey::Enter, SessionKey::Enter),
    ] {
        let mut reducer = KeyboardReducer::default();
        let down = input(physical, KeyPhase::Down, false, false, false);
        assert_eq!(
            step(&mut reducer, down, ActivationKey::Z, false, true),
            (None, false)
        );
        assert_eq!(
            step(&mut reducer, down, ActivationKey::Z, true, true),
            (
                Some(HelperEvent::SessionKey {
                    key: session,
                    phase: EventPhase::Down,
                }),
                true,
            )
        );
        assert_eq!(
            step(&mut reducer, down, ActivationKey::Z, false, true),
            (None, true)
        );
        assert_eq!(
            step(
                &mut reducer,
                input(physical, KeyPhase::Up, false, false, false),
                ActivationKey::Z,
                false,
                true,
            ),
            (
                Some(HelperEvent::SessionKey {
                    key: session,
                    phase: EventPhase::Up,
                }),
                true,
            )
        );
    }
}

#[test]
fn failed_session_key_delivery_fails_open_until_key_up() {
    let mut reducer = KeyboardReducer::default();
    let down = input(PhysicalKey::Escape, KeyPhase::Down, false, false, false);
    assert!(!step(&mut reducer, down, ActivationKey::Z, true, false).1);
    assert_eq!(
        step(&mut reducer, down, ActivationKey::Z, true, true),
        (None, false)
    );
    assert_eq!(
        step(
            &mut reducer,
            input(PhysicalKey::Escape, KeyPhase::Up, false, false, false),
            ActivationKey::Z,
            true,
            true,
        ),
        (None, false)
    );
}

#[test]
fn physical_tracker_derives_down_down_up_transitions() {
    let mut tracker = PhysicalKeyTracker::default();
    let key = PhysicalKey::Letter(ActivationKey::Z);
    assert!(!tracker.observe(key, KeyPhase::Down));
    assert!(tracker.observe(key, KeyPhase::Down));
    assert!(!tracker.observe(key, KeyPhase::Up));
    assert!(!tracker.observe(key, KeyPhase::Down));
    assert!(!tracker.observe(key, KeyPhase::Up));

    assert!(!tracker.observe(PhysicalKey::Escape, KeyPhase::Down));
    assert!(tracker.observe(PhysicalKey::Escape, KeyPhase::Down));
    assert!(!tracker.observe(PhysicalKey::Escape, KeyPhase::Up));
    assert!(!tracker.observe(PhysicalKey::Other, KeyPhase::Down));
}

#[test]
fn keys_held_before_gate_open_do_not_become_new_captures() {
    let mut tracker = PhysicalKeyTracker::default();
    let mut reducer = KeyboardReducer::default();
    let letter = PhysicalKey::Letter(ActivationKey::Z);

    // The native callback observes this event while its gate is closed and
    // deliberately does not send it through the reducer.
    assert!(!tracker.observe(letter, KeyPhase::Down));

    // Once the gate opens, the next physical down is derived as a repeat even
    // if Alt or configuration changed while the key remained held.
    let mut held_down = input(letter, KeyPhase::Down, true, false, false);
    held_down.repeat = tracker.observe(letter, KeyPhase::Down);
    assert_eq!(
        step(&mut reducer, held_down, ActivationKey::Z, true, true,),
        (None, false)
    );
    assert!(!tracker.observe(letter, KeyPhase::Up));
    assert_eq!(
        step(
            &mut reducer,
            input(letter, KeyPhase::Up, false, false, false),
            ActivationKey::Z,
            true,
            true,
        ),
        (None, false)
    );

    let mut fresh_down = input(letter, KeyPhase::Down, true, false, false);
    fresh_down.repeat = tracker.observe(letter, KeyPhase::Down);
    assert_eq!(
        step(&mut reducer, fresh_down, ActivationKey::Z, true, true,),
        (
            Some(HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
            }),
            true,
        )
    );
}

#[test]
fn session_capture_does_not_capture_an_escape_held_before_enable() {
    let mut tracker = PhysicalKeyTracker::default();
    let mut reducer = KeyboardReducer::default();
    assert!(!tracker.observe(PhysicalKey::Escape, KeyPhase::Down));

    let mut held = input(PhysicalKey::Escape, KeyPhase::Down, false, false, false);
    held.repeat = tracker.observe(PhysicalKey::Escape, KeyPhase::Down);
    assert_eq!(
        step(&mut reducer, held, ActivationKey::Z, true, true),
        (None, false)
    );
    assert!(!tracker.observe(PhysicalKey::Escape, KeyPhase::Up));
}

#[test]
fn orphaned_native_repeats_do_not_start_captured_sequences() {
    let mut reducer = KeyboardReducer::default();
    let mut activation = input(
        PhysicalKey::Letter(ActivationKey::Z),
        KeyPhase::Down,
        true,
        false,
        false,
    );
    activation.repeat = true;
    assert_eq!(
        step(&mut reducer, activation, ActivationKey::Z, true, true),
        (None, false)
    );

    let mut escape = input(PhysicalKey::Escape, KeyPhase::Down, false, false, false);
    escape.repeat = true;
    assert_eq!(
        step(&mut reducer, escape, ActivationKey::Z, true, true),
        (None, false)
    );
}

#[test]
fn injected_events_always_pass_through() {
    let mut reducer = KeyboardReducer::default();
    assert_eq!(
        step(
            &mut reducer,
            input(
                PhysicalKey::Letter(ActivationKey::Z),
                KeyPhase::Down,
                true,
                true,
                true,
            ),
            ActivationKey::Z,
            true,
            true,
        ),
        (None, false)
    );
}

#[test]
fn exact_multi_bindings_emit_the_selected_key_and_shift_without_mode_aliasing() {
    let bindings = talking_quill_helper::keyboard::ActivationBindings::from_exact(&[
        (ActivationKey::A, false),
        (ActivationKey::Q, true),
        (ActivationKey::Z, false),
    ])
    .unwrap();
    for (key, shift) in [
        (ActivationKey::A, false),
        (ActivationKey::Q, true),
        (ActivationKey::Z, false),
    ] {
        let mut reducer = KeyboardReducer::default();
        let plan = reducer.plan_bindings(
            input(PhysicalKey::Letter(key), KeyPhase::Down, true, shift, false),
            bindings,
            true,
            false,
        );
        assert_eq!(
            plan.event(),
            Some(HelperEvent::Activation {
                key,
                phase: EventPhase::Down,
                shift,
            })
        );
        assert!(reducer.apply(plan, true));
    }

    let plan = KeyboardReducer::default().plan_bindings(
        input(
            PhysicalKey::Letter(ActivationKey::Q),
            KeyPhase::Down,
            true,
            false,
            false,
        ),
        bindings,
        true,
        false,
    );
    assert_eq!(plan.event(), None);
}
