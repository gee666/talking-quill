use super::*;

#[test]
fn simultaneous_enter_sources_latch_one_balanced_sequence_in_every_order() {
    for (first, second) in [
        (EnterSource::Main, EnterSource::Numpad),
        (EnterSource::Numpad, EnterSource::Main),
    ] {
        for release_accepted_first in [false, true] {
            let (context, outbound, _terminal) = test_context(4);
            context
                .state
                .session_capture_mode
                .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);

            assert!(enter(&context, first, KeyPhase::Down));
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: SessionKey::Enter,
                    phase: EventPhase::Down,
                },
            );
            assert!(enter(&context, first, KeyPhase::Down));
            assert!(!enter(&context, second, KeyPhase::Down));
            assert!(!enter(&context, second, KeyPhase::Down));
            assert!(outbound.try_recv().is_err());

            let releases = if release_accepted_first {
                [first, second]
            } else {
                [second, first]
            };
            for source in releases {
                assert_eq!(
                    enter(&context, source, KeyPhase::Up),
                    source == first,
                    "first={first:?}, release={source:?}",
                );
            }
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: SessionKey::Enter,
                    phase: EventPhase::Up,
                },
            );
            assert!(outbound.try_recv().is_err());
            assert_eq!(context.keyboard.lock().unwrap().captured_enter_source, None,);
        }
    }
}

#[test]
fn enter_source_tracking_survives_capture_and_config_transitions() {
    let (context, outbound, _terminal) = test_context(4);

    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Down,
        },
    );
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));

    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);
    apply_config(&context, ActivationConfig::default());
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
    assert!(enter(&context, EnterSource::Numpad, KeyPhase::Up));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Enter,
            phase: EventPhase::Up,
        },
    );
    assert!(outbound.try_recv().is_err());
}

#[test]
fn cancel_only_captures_escape_but_passes_enter_and_balances_after_off() {
    let (context, outbound, _terminal) = test_context(4);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);

    assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
    assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
    assert!(record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Down,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        },
    );

    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    assert!(record(
        &context,
        VK_ESCAPE,
        PhysicalKey::Escape,
        KeyPhase::Up,
    ));
    assert_eq!(
        receive_event(&outbound),
        KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Up,
        },
    );
}

#[test]
fn external_session_keys_are_input_equivalent_and_preserve_balancing() {
    let (context, outbound, _terminal) = test_context(8);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    for (virtual_key, scan_code) in [
        (VK_ESCAPE, 0x01),
        (VK_RETURN, 0x1C),
        (VK_ESCAPE, 0),
        (VK_RETURN, 0),
    ] {
        for phase in [KeyPhase::Down, KeyPhase::Up] {
            assert!(transactional_record(
                &context,
                virtual_key,
                scan_code,
                false,
                phase,
                InputSource::External,
                1,
            ));
        }
    }
    assert_eq!(outbound.try_iter().count(), 8);
}

#[test]
fn escape_and_enter_capture_remains_paired_and_modifier_independent() {
    let (context, outbound, _terminal) = test_context(8);
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
    modifier(&context, VK_LWIN, KeyPhase::Down);
    for (key, session_key) in [
        (PhysicalKey::Escape, SessionKey::Escape),
        (PhysicalKey::Enter, SessionKey::Enter),
    ] {
        assert!(record(&context, 0, key, KeyPhase::Down));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: session_key,
                phase: EventPhase::Down,
            }
        );
        assert!(record(&context, 0, key, KeyPhase::Down));
        assert!(record(&context, 0, key, KeyPhase::Up));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: session_key,
                phase: EventPhase::Up,
            }
        );
    }
}
