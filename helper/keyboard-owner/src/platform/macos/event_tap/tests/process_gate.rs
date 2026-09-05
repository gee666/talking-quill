//! Process gate contracts.

use super::*;

#[test]
fn closed_process_gate_uses_listen_only_tap_and_bypasses_every_callback_path() {
    assert_eq!(
        event_tap_options(false),
        ffi::K_CG_EVENT_TAP_OPTION_LISTEN_ONLY
    );
    assert_eq!(event_tap_options(true), ffi::K_CG_EVENT_TAP_OPTION_DEFAULT);

    let (mut context, outbound, _commands) = test_context();
    context.suppression_enabled = false;
    let event = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
        ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE,
        TEST_RECOVERY_PHYSICAL_MARKER,
    );
    let returned = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_KEY_DOWN,
            event,
            (&raw mut context).cast(),
        )
    };
    assert_eq!(returned, event);
    assert_eq!(
        context
            .keyboard
            .lock()
            .unwrap()
            .transactional
            .owned_letters(),
        0
    );
    assert!(outbound.try_recv().is_err());
    unsafe { ffi::CFRelease(event.cast_const()) };
}
