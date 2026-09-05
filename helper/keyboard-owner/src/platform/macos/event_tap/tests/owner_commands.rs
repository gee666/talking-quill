//! Owner commands contracts.

use super::*;

#[test]
fn cancelled_owner_command_never_applies_after_wakeup() {
    let (context, _outbound, commands) = test_context();
    let previous = context.keyboard.lock().unwrap().activation;
    let state = Arc::new(std::sync::atomic::AtomicU8::new(
        OwnerCommandState::Pending as u8,
    ));
    let (acknowledgement, response) = bounded(1);
    commands
        .send(OwnerCommand {
            mutation: OwnerMutation {
                kind: OwnerMutationKind::Configure,
                activation: ActivationConfig::default(),
                session_capture_mode: SessionCaptureMode::Off,
            },
            state: Arc::clone(&state),
            acknowledgement,
        })
        .unwrap();
    assert_eq!(cancel_owner_command(&state), OwnerCommandState::Cancelled);

    process_owner_commands(&context);

    assert_eq!(context.keyboard.lock().unwrap().activation, previous);
    assert!(response.recv().unwrap().is_err());
}

#[test]
fn owner_commands_apply_full_bindings_and_capture_in_fifo_order() {
    let (context, _outbound, commands) = test_context();
    let updated = ActivationConfig {
        enabled: true,
        bindings: ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: false,
                    shift: false,
                    meta: true,
                },
                &[ActivationKey::Q, ActivationKey::P],
            ),
        )])
        .unwrap(),
    };
    let mut states = Vec::new();
    let mut responses = Vec::new();
    for mutation in [
        OwnerMutation {
            kind: OwnerMutationKind::Configure,
            activation: updated,
            session_capture_mode: SessionCaptureMode::Off,
        },
        OwnerMutation {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Recording,
        },
    ] {
        let state = Arc::new(std::sync::atomic::AtomicU8::new(
            OwnerCommandState::Pending as u8,
        ));
        let (acknowledgement, response) = bounded(1);
        commands
            .send(OwnerCommand {
                mutation,
                state: Arc::clone(&state),
                acknowledgement,
            })
            .unwrap();
        states.push(state);
        responses.push(response);
    }

    process_owner_commands(&context);

    {
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.activation, updated);
        assert!(keyboard.transactional.config().enabled());
        assert_eq!(keyboard.transactional.config().bindings(), updated.bindings);
        assert_eq!(keyboard.transactional.config().revision().get(), 1);
    }
    assert_eq!(
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
        SessionCaptureMode::Recording,
    );
    for state in states {
        assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
    }
    for response in responses {
        assert!(response.recv().unwrap().is_ok());
    }
}
