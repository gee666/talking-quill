//! Activation dispatch contracts.

use super::*;

#[test]
fn transactional_dispatcher_pairs_context_and_advances_complete_generation() {
    let (context, outbound, _commands) = test_context();
    let binding = bindings().iter().next().unwrap();
    let first_context = {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(deliver_test_activation(
            &context,
            &mut keyboard,
            ActivationNotice::Down { binding },
        ));
        let KeyboardEvent::Activation {
            context: first,
            phase: EventPhase::Down,
            ..
        } = receive_event(&outbound)
        else {
            panic!("expected activation down");
        };
        assert!(deliver_test_activation(
            &context,
            &mut keyboard,
            ActivationNotice::Up {
                binding,
                held_ms: 15,
            },
        ));
        first
    };
    let KeyboardEvent::Activation {
        context: up_context,
        phase: EventPhase::Up,
        ..
    } = receive_event(&outbound)
    else {
        panic!("expected activation up");
    };
    assert_eq!(up_context, first_context);
    assert_eq!(
        first_context.activation_generation(),
        ActivationGeneration::FIRST
    );

    {
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(deliver_test_activation(
            &context,
            &mut keyboard,
            ActivationNotice::Complete {
                binding,
                held_ms: 20,
            },
        ));
    }
    let KeyboardEvent::ActivationComplete {
        context: complete,
        held_ms: 20,
        ..
    } = receive_event(&outbound)
    else {
        panic!("expected complete activation");
    };
    assert_eq!(complete.activation_generation().get(), 2);
}
