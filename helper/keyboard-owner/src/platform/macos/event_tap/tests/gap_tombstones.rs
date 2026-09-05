//! Gap tombstones contracts.

use super::*;

#[test]
fn tombstone_suppresses_all_queued_repeats_then_the_delayed_old_up() {
    let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
    let key_code = LETTER_KEY_CODES[15];
    let mut keyboard = CallbackKeyboard {
        gap_reconciled_letters: p_bit,
        gap_barrier_pending: true,
        ..CallbackKeyboard::default()
    };

    for _ in 0..3 {
        assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
        assert!(keyboard.gap_tombstones_pending());
        assert!(keyboard.gap_barrier_pending);
        assert!(!keyboard.physical.is_held(key_code));
    }
    assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
    assert!(!keyboard.gap_tombstones_pending());
    assert!(!keyboard.gap_barrier_pending);
    assert!(!keyboard.physical.is_held(key_code));
}

#[test]
fn only_nonrepeat_fresh_down_retires_tombstone_and_balances_its_fresh_up() {
    let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
    let key_code = LETTER_KEY_CODES[15];
    let mut keyboard = CallbackKeyboard {
        gap_reconciled_letters: p_bit,
        gap_barrier_pending: true,
        ..CallbackKeyboard::default()
    };
    assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
    // Stream order proves any old queued up precedes this genuine down.
    assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, false));
    assert!(!keyboard.gap_tombstones_pending());
    assert!(!keyboard.gap_barrier_pending);

    // Production processing now admits exactly this fresh pair.
    assert!(!keyboard.physical.observe(key_code, KeyPhase::Down));
    assert!(keyboard.physical.is_held(key_code));
    assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
    assert!(!keyboard.physical.observe(key_code, KeyPhase::Up));
    assert!(!keyboard.physical.is_held(key_code));
}

#[test]
fn barrier_between_repeat_and_later_edge_retires_owned_suppression() {
    let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
    let key_code = LETTER_KEY_CODES[15];
    let mut keyboard = CallbackKeyboard {
        gap_reconciled_letters: p_bit,
        gap_barrier_pending: true,
        ..CallbackKeyboard::default()
    };
    assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
    // The marked barrier down is swallowed but does not prove the older
    // pipeline drained; another queued repeat remains owned.
    assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
    assert!(keyboard.gap_barrier_pending);
    // Only the marked barrier up completes the proof.
    keyboard.clear_gap_tombstones();
    assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
    assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
}

#[test]
fn barrier_only_state_keeps_disabled_tap_recovery_admitted() {
    let keyboard = CallbackKeyboard {
        gap_barrier_pending: true,
        ..CallbackKeyboard::default()
    };
    assert!(keyboard.strict_drain_recovery_needed());
}

#[test]
fn ordered_barrier_retires_only_remaining_tombstones() {
    let mut keyboard = CallbackKeyboard {
        gap_reconciled_escape: true,
        gap_reconciled_enter_key_code: Some(KEYPAD_ENTER_KEY_CODE),
        gap_barrier_pending: true,
        ..CallbackKeyboard::default()
    };
    assert!(keyboard.gap_tombstones_pending());
    keyboard.clear_gap_tombstones();
    assert!(!keyboard.gap_tombstones_pending());
    assert!(!keyboard.gap_barrier_pending);
}
