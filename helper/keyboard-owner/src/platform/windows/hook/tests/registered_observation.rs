use super::*;

#[test]
fn registered_observation_callback_is_exact_release_only_and_always_passes_through() {
    let (context, outbound, terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );

    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(
        outbound.try_recv().is_err(),
        "candidate or match emitted proof"
    );
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    let observed = context.observability.snapshot().registered_input;
    assert_eq!(
        (
            observed.registered_candidate_callbacks,
            observed.registered_match_callbacks,
            observed.registered_release_callbacks,
            observed.callback_channel_accepted,
        ),
        (1, 1, 1, 1),
    );
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 1 }
    ));
    assert!(outbound.try_recv().is_err());
    assert!(terminal.try_recv().is_err());

    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.journal_len(), 0);
    assert!(keyboard.dispatcher.active.is_none());
    assert!(keyboard.candidate_target.is_none());
    assert!(keyboard.pending_paste_cleanup.is_empty());
    drop(keyboard);
    let counters = context.observability.snapshot().registered_input;
    assert_eq!(counters.registered_candidate_callbacks, 1);
    assert_eq!(counters.registered_match_callbacks, 1);
    assert_eq!(counters.registered_release_callbacks, 1);
    assert_eq!(counters.callback_channel_accepted, 1);
    assert_eq!(counters.callback_channel_rejected, 0);
}

#[test]
fn registered_observation_callback_rejection_never_succeeds_or_activates() {
    let (context, outbound, _terminal) = test_context(0);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());
    let keyboard = context.keyboard.lock().unwrap();
    assert_eq!(keyboard.transactional.journal_len(), 0);
    assert!(keyboard.dispatcher.active.is_none());
    assert!(keyboard.candidate_target.is_none());
    drop(keyboard);
    let counters = context.observability.snapshot().registered_input;
    assert_eq!(counters.callback_channel_accepted, 0);
    assert_eq!(counters.callback_channel_rejected, 1);
}

#[test]
fn registered_observation_handles_repeats_and_cancels_modifier_changes_or_wrong_releases() {
    let (context, outbound, _terminal) = test_context(8);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: full_bindings(),
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));

    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Down));
    assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Up));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
    assert!(outbound.try_recv().is_err());

    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 3 }
    ));
}

#[test]
fn registered_observation_preserves_shared_prefix_candidate_sets() {
    let shared = ActivationBindings::new(&[
        ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X],
            ),
        ),
        ActivationBinding::new(
            ProfileId::PROMPT,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X, ActivationKey::P],
            ),
        ),
    ])
    .unwrap();
    let (context, outbound, _terminal) = test_context(4);
    apply_config(
        &context,
        ActivationConfig {
            enabled: false,
            bindings: shared,
        },
    );
    assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 1 }
    ));
    assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
    assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
    assert!(matches!(
        outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
        NativeEvent::RegisteredObservation { generation: 2 }
    ));
}
