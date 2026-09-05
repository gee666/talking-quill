//! Lossless repost contracts.

use super::*;

#[test]
fn deferred_observation_rejects_every_mutated_lossless_scalar() {
    let keyboard = tagged_keyboard_event(
        ffi::K_CG_EVENT_KEY_DOWN,
        LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
        0x8000_0000_0010_0000,
        0,
    );
    unsafe {
        ffi::CGEventSetIntegerValueField(keyboard, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE, 41);
    }
    let mut expected = injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_KEY_DOWN,
        key_code: LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
        flags: unsafe { ffi::CGEventGetFlags(keyboard) },
        keyboard_type: unsafe {
            ffi::CGEventGetIntegerValueField(keyboard, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        },
        ..injection::DeferredEvent::EMPTY
    };
    assert!(deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_KEY_DOWN,
        keyboard
    ));
    unsafe { ffi::CGEventSetFlags(keyboard, expected.flags ^ ffi::K_CG_EVENT_FLAG_MASK_SHIFT) };
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_KEY_DOWN,
        keyboard
    ));
    unsafe {
        ffi::CGEventSetFlags(keyboard, expected.flags);
        ffi::CGEventSetIntegerValueField(
            keyboard,
            ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE,
            expected.keyboard_type + 1,
        );
    }
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_KEY_DOWN,
        keyboard
    ));

    let mouse = tagged_mouse_event(
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        ffi::CGPoint { x: 12.5, y: 33.25 },
        2,
        3,
        0,
    );
    unsafe {
        for (field, value) in [
            (ffi::K_CG_MOUSE_EVENT_NUMBER, 4),
            (ffi::K_CG_MOUSE_EVENT_DELTA_X, -7),
            (ffi::K_CG_MOUSE_EVENT_DELTA_Y, 9),
            (ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER, 1),
            (ffi::K_CG_MOUSE_EVENT_SUBTYPE, 2),
        ] {
            ffi::CGEventSetIntegerValueField(mouse, field, value);
        }
        ffi::CGEventSetDoubleValueField(mouse, ffi::K_CG_MOUSE_EVENT_PRESSURE, 0.625);
    }
    expected = injection::DeferredEvent {
        event_type: ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        flags: unsafe { ffi::CGEventGetFlags(mouse) },
        location: unsafe { ffi::CGEventGetLocation(mouse) },
        mouse_number: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_NUMBER)
        },
        mouse_click_state: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_CLICK_STATE)
        },
        mouse_pressure: unsafe {
            ffi::CGEventGetDoubleValueField(mouse, ffi::K_CG_MOUSE_EVENT_PRESSURE)
        },
        mouse_button: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER)
        },
        mouse_delta_x: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_DELTA_X)
        },
        mouse_delta_y: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_DELTA_Y)
        },
        mouse_instant_mouser: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
        },
        mouse_subtype: unsafe {
            ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_SUBTYPE)
        },
        ..injection::DeferredEvent::EMPTY
    };
    assert!(deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        mouse
    ));
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_OTHER_MOUSE_UP,
        mouse
    ));
    unsafe { ffi::CGEventSetFlags(mouse, expected.flags ^ ffi::K_CG_EVENT_FLAG_MASK_SHIFT) };
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        mouse
    ));
    unsafe { ffi::CGEventSetFlags(mouse, expected.flags) };
    for (field, original, mutation) in [
        (
            ffi::K_CG_MOUSE_EVENT_NUMBER,
            expected.mouse_number,
            expected.mouse_number + 1,
        ),
        (
            ffi::K_CG_MOUSE_EVENT_CLICK_STATE,
            expected.mouse_click_state,
            expected.mouse_click_state + 1,
        ),
        (
            ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER,
            expected.mouse_button,
            expected.mouse_button + 1,
        ),
        (
            ffi::K_CG_MOUSE_EVENT_DELTA_X,
            expected.mouse_delta_x,
            expected.mouse_delta_x + 1,
        ),
        (
            ffi::K_CG_MOUSE_EVENT_DELTA_Y,
            expected.mouse_delta_y,
            expected.mouse_delta_y + 1,
        ),
        (
            ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER,
            expected.mouse_instant_mouser,
            i64::from(expected.mouse_instant_mouser == 0),
        ),
        (
            ffi::K_CG_MOUSE_EVENT_SUBTYPE,
            expected.mouse_subtype,
            i64::from(expected.mouse_subtype == 0),
        ),
    ] {
        unsafe { ffi::CGEventSetIntegerValueField(mouse, field, mutation) };
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            mouse
        ));
        unsafe { ffi::CGEventSetIntegerValueField(mouse, field, original) };
    }
    unsafe {
        ffi::CGEventSetDoubleValueField(
            mouse,
            ffi::K_CG_MOUSE_EVENT_PRESSURE,
            if expected.mouse_pressure <= 0.5 {
                expected.mouse_pressure + 0.25
            } else {
                expected.mouse_pressure - 0.25
            },
        );
    }
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        mouse
    ));
    unsafe {
        ffi::CGEventSetDoubleValueField(
            mouse,
            ffi::K_CG_MOUSE_EVENT_PRESSURE,
            expected.mouse_pressure,
        );
        ffi::CGEventSetLocation(
            mouse,
            ffi::CGPoint {
                x: expected.location.x + 1.0,
                y: expected.location.y,
            },
        );
    }
    assert!(!deferred_edge_matches_observed(
        expected,
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
        mouse
    ));
    unsafe {
        ffi::CFRelease(mouse.cast_const());
        ffi::CFRelease(keyboard.cast_const());
    }
}
