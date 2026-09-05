use super::*;

#[test]
fn owner_commands_apply_full_config_and_capture_in_fifo_order() {
    let (context, _outbound, _terminal) = test_context(4);
    let (command_tx, command_rx) = bounded(4);
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
        OwnerMutation::configure(updated),
        OwnerMutation::set_session_capture(SessionCaptureMode::Recording),
    ] {
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (ack, response) = bounded(1);
        command_tx
            .send(OwnerCommand {
                mutation,
                state: Arc::clone(&state),
                acknowledgement: ack,
            })
            .unwrap();
        states.push(state);
        responses.push(response);
    }

    process_owner_commands(&context, &command_rx);

    assert_eq!(context.keyboard.lock().unwrap().activation, updated);
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

#[test]
fn cancelled_owner_command_never_applies_late() {
    let (context, _outbound, _terminal) = test_context(1);
    let previous = context.keyboard.lock().unwrap().activation;
    let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
    let (command_tx, command_rx) = bounded(1);
    let (ack, response) = bounded(1);
    command_tx
        .send(OwnerCommand {
            mutation: OwnerMutation::configure(ActivationConfig::default()),
            state: Arc::clone(&state),
            acknowledgement: ack,
        })
        .unwrap();
    assert_eq!(cancel_owner_command(&state), OwnerCommandState::Cancelled);

    process_owner_commands(&context, &command_rx);

    assert_eq!(context.keyboard.lock().unwrap().activation, previous);
    assert!(response.recv().unwrap().is_err());
}
