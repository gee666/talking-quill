//! Physical contracts.

use super::*;

#[test]
fn physical_snapshots_use_hid_system_state_not_logical_session_state() {
    assert_eq!(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, 1);
}

#[test]
fn mach_timebase_conversion_matches_cg_event_nanoseconds() {
    assert_eq!(mach_ticks_to_nanoseconds(3, 125, 3), Some(125));
    assert_eq!(mach_ticks_to_nanoseconds(1, 1, 0), None);
    assert_eq!(mach_ticks_to_nanoseconds(u64::MAX, u32::MAX, 1), None);
}

#[test]
fn every_ansi_letter_keycode_maps_to_its_dom_physical_position() {
    let mut unique = std::collections::BTreeSet::new();
    for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
        assert!(unique.insert(key_code));
        assert_eq!(
            map_key_code(key_code),
            PhysicalKey::Letter(ActivationKey::from_index(index as u8).unwrap())
        );
    }
    assert_eq!(map_key_code(ESCAPE_KEY_CODE), PhysicalKey::Escape);
    assert_eq!(map_key_code(RETURN_KEY_CODE), PhysicalKey::Enter);
    assert_eq!(map_key_code(KEYPAD_ENTER_KEY_CODE), PhysicalKey::Enter);
    assert_eq!(map_key_code(127), PhysicalKey::Other);
}

#[test]
fn modifier_keycodes_and_event_flags_project_to_every_exact_protocol_modifier() {
    for bits in 0_u8..16 {
        let flags = (if bits & 0b0001 != 0 {
            ffi::K_CG_EVENT_FLAG_MASK_CONTROL
        } else {
            0
        }) | (if bits & 0b0010 != 0 {
            ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE
        } else {
            0
        }) | (if bits & 0b0100 != 0 {
            ffi::K_CG_EVENT_FLAG_MASK_SHIFT
        } else {
            0
        }) | (if bits & 0b1000 != 0 {
            ffi::K_CG_EVENT_FLAG_MASK_COMMAND
        } else {
            0
        });
        assert_eq!(
            modifier_mask_from_flags(flags | 0x0000_0100),
            ModifierMask::new(
                bits & 0b0001 != 0,
                bits & 0b0010 != 0,
                bits & 0b0100 != 0,
                bits & 0b1000 != 0,
            ),
        );
    }

    for (key_code, expected) in [
        (
            LEFT_CONTROL_KEY_CODE,
            ModifierMask::new(true, false, false, false),
        ),
        (
            LEFT_OPTION_KEY_CODE,
            ModifierMask::new(false, true, false, false),
        ),
        (
            LEFT_SHIFT_KEY_CODE,
            ModifierMask::new(false, false, true, false),
        ),
        (
            LEFT_COMMAND_KEY_CODE,
            ModifierMask::new(false, false, false, true),
        ),
    ] {
        let mut tracker = MacModifierTracker::default();
        assert!(tracker.observe_flags_changed(key_code, true));
        assert_eq!(tracker.mask(), expected);
    }

    let mut tracker = MacModifierTracker::default();
    tracker.observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
    tracker.observe_flags_changed(RIGHT_SHIFT_KEY_CODE, true);
    tracker.observe_flags_changed(LEFT_SHIFT_KEY_CODE, false);
    assert!(tracker.mask().shift());
    tracker.observe_flags_changed(RIGHT_SHIFT_KEY_CODE, false);
    assert!(!tracker.mask().shift());
    assert!(!tracker.observe_flags_changed(57, true));
}

#[test]
fn all_eight_modifier_keycodes_preserve_side_specific_identity() {
    for (key_code, side) in [
        (LEFT_CONTROL_KEY_CODE, ModifierSide::LeftCtrl),
        (RIGHT_CONTROL_KEY_CODE, ModifierSide::RightCtrl),
        (LEFT_OPTION_KEY_CODE, ModifierSide::LeftAlt),
        (RIGHT_OPTION_KEY_CODE, ModifierSide::RightAlt),
        (LEFT_SHIFT_KEY_CODE, ModifierSide::LeftShift),
        (RIGHT_SHIFT_KEY_CODE, ModifierSide::RightShift),
        (LEFT_COMMAND_KEY_CODE, ModifierSide::LeftMeta),
        (RIGHT_COMMAND_KEY_CODE, ModifierSide::RightMeta),
    ] {
        assert_eq!(modifier_side_for_key_code(key_code), Some(side));
        let mut tracker = MacModifierTracker::default();
        assert!(tracker.observe_flags_changed(key_code, true));
        assert!(tracker.sides().contains(side));
        assert!(tracker.observe_flags_changed(key_code, false));
        assert!(!tracker.sides().contains(side));
    }
    assert_eq!(modifier_side_for_key_code(57), None);
}
