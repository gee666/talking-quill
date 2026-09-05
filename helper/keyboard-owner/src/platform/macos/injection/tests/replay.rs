use super::*;

#[test]
fn replay_descriptor_preserves_key_phase_repeat_flags_and_marker() {
    let marker = OperationToken::for_test(7).marker;
    let down = replay_descriptor(record(PhysicalPhase::Down, 0x12_3400), marker);
    assert_eq!(down.key_code, 9);
    assert!(down.key_down);
    assert!(!down.repeat);
    assert_eq!(down.flags, 0x12_3400);
    assert_eq!(down.marker, marker);
    assert_eq!(
        event_mutation(down),
        EventMutation {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 9,
            repeat: 0,
            flags: 0x12_3400,
            marker,
        }
    );

    let repeat = replay_descriptor(record(PhysicalPhase::Repeat, 7), marker);
    assert_eq!(repeat.key_code, 9);
    assert!(repeat.key_down);
    assert!(repeat.repeat);
    assert_eq!(repeat.flags, 7);
    assert_eq!(repeat.marker, marker);
    assert_eq!(event_mutation(repeat).event_type, ffi::K_CG_EVENT_KEY_DOWN);
    assert_eq!(event_mutation(repeat).repeat, 1);

    let up = replay_descriptor(record(PhysicalPhase::Up, 9), marker);
    assert_eq!(up.key_code, 9);
    assert!(!up.key_down);
    assert!(!up.repeat);
    assert_eq!(up.flags, 9);
    assert_eq!(up.marker, marker);
    assert_eq!(event_mutation(up).event_type, ffi::K_CG_EVENT_KEY_UP);
    assert_eq!(event_mutation(up).repeat, 0);

    let modifier = ReplayRecord {
        key: KeyIdentity::Modifier(
            talking_quill_keyboard_core::transactional::ModifierSide::LeftAlt,
        ),
        native: NativeKey {
            virtual_key: 58,
            platform_flags: ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE,
            ..NativeKey::default()
        },
        phase: PhysicalPhase::Up,
        observed_at_ms: 18,
    };
    assert_eq!(
        replay_descriptor(modifier, marker).event_type,
        ffi::K_CG_EVENT_FLAGS_CHANGED
    );
}

#[test]
fn gap_barrier_is_an_ordered_operation_tagged_dummy_pair() {
    let token = OperationToken::for_test(8);
    let pair = gap_barrier_descriptors(token);
    assert_eq!(paste_barrier_descriptors(token), pair);
    assert!(pair[0].key_down);
    assert!(!pair[1].key_down);
    for event in pair {
        assert_eq!(event.key_code, 127);
        assert_eq!(event.marker, token.marker);
        assert_eq!(event.flags, 0);
        assert!(!event.repeat);
    }
}
