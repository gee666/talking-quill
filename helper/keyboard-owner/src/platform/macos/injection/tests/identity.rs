use super::*;

#[test]
fn operation_tokens_require_own_pid_and_reject_delayed_aba_sequences() {
    let identity = identity();
    let old = OperationToken::for_test(1);
    let current = OperationToken::for_test(2);
    assert_ne!(old, current);
    assert!(token_matches(
        identity,
        Some(current),
        current.marker,
        identity.source_pid
    ));
    assert!(!token_matches(
        identity,
        Some(current),
        old.marker,
        identity.source_pid
    ));
    assert!(!token_matches(identity, Some(current), current.marker, 99));
    assert!(!token_matches(
        identity,
        None,
        current.marker,
        identity.source_pid
    ));
    assert_eq!(unmarked_source(identity, 42), InputSource::External);
    assert_eq!(unmarked_source(identity, 0), InputSource::Physical);
}

#[test]
fn replay_shape_rejects_forged_type_keycode_and_repeat_state() {
    assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 9, false));
    assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 9, true));
    assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_UP, 9, false));
    assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_UP, 9, true));
    assert!(!replay_shape_is_valid(99, 9, false));
    assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, -1, false));
    assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 128, false));
}
