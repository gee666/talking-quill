//! Normalize side-specific native transitions before starting a transaction turn.

use super::*;

pub(super) fn normalize_transition(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    event: CallbackEvent,
    secure_transition: bool,
    event_modifiers: ModifierMask,
) -> Result<(KeyIdentity, PhysicalPhase, Option<(KeyPhase, bool)>), bool> {
    let CallbackEvent {
        event_type,
        key_code,
        native_repeat,
        source,
        source_pid,
        test_physical_source,
        ..
    } = event;
    Ok(if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        if let Some(side) = modifier_side_for_key_code(key_code) {
            let modeled_is_down = if source.is_physical() {
                let was_down = keyboard.modifiers.sides().contains(side);
                Some(if test_physical_source {
                    !was_down
                } else {
                    native_key_is_down(key_code)
                })
            } else {
                context
                    .recovery_edges
                    .try_lock()
                    .ok()
                    .and_then(|mut journal| {
                        journal.observe_normal_external_modifier(source_pid, key_code)
                    })
            };
            if let Some(is_down) = modeled_is_down {
                if source.is_physical() {
                    let was_down = keyboard.modifiers.sides().contains(side);
                    if was_down == is_down && !secure_transition {
                        reconcile_locked(context, keyboard);
                        return Err(false);
                    }
                    let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
                    let Some(next_epoch) = keyboard.modifier_epoch.checked_add(1) else {
                        recover_callback_unwind(context);
                        return Err(true);
                    };
                    keyboard.modifier_epoch = next_epoch;
                    if keyboard.modifiers.mask() != event_modifiers {
                        reconcile_locked(context, keyboard);
                        return Err(false);
                    }
                    keyboard.reducer.observe_modifiers(event_modifiers);
                }
                #[cfg(feature = "transactional-shortcuts-dev")]
                if test_physical_source {
                    crate::platform::macos::observe_macos_test_physical_key(key_code, is_down);
                }
                (
                    KeyIdentity::Modifier(side),
                    if is_down {
                        PhysicalPhase::Down
                    } else {
                        PhysicalPhase::Up
                    },
                    None,
                )
            } else {
                // Every bounded external-source slot is pinned. Preserve the
                // event as an external cancellation boundary without inventing
                // a side transition from aggregate family flags.
                (KeyIdentity::Other(key_code), PhysicalPhase::Down, None)
            }
        } else {
            // Caps Lock and other flags-only keys are invalid continuations but
            // do not alter the side-specific modifier snapshot.
            (KeyIdentity::Other(key_code), PhysicalPhase::Down, None)
        }
    } else {
        let key_phase = match event_type {
            ffi::K_CG_EVENT_KEY_DOWN => KeyPhase::Down,
            ffi::K_CG_EVENT_KEY_UP => KeyPhase::Up,
            _ => return Err(false),
        };
        let key = transactional_key_identity(key_code);
        #[cfg(feature = "transactional-shortcuts-dev")]
        if test_physical_source {
            crate::platform::macos::observe_macos_test_physical_key(
                key_code,
                event_type == ffi::K_CG_EVENT_KEY_DOWN,
            );
        }
        if source.is_physical() {
            let was_held = keyboard.physical.is_held(key_code);
            let discontinuity = match key_phase {
                KeyPhase::Down => was_held != native_repeat,
                KeyPhase::Up => !was_held,
            };
            let relevant = keyboard.transactional.config().enabled()
                || keyboard.transactional.owned_letters() != 0
                || keyboard.transactional.journal_len() != 0
                || SessionCaptureMode::from_u8(
                    context.state.session_capture_mode.load(Ordering::Acquire),
                ) != SessionCaptureMode::Off;
            let native_mismatch = !test_physical_source
                && relevant
                && !keyboard
                    .physical
                    .native_state_is_consistent_except(key_code, native_key_is_down);
            if !secure_transition
                && (discontinuity
                    || native_mismatch
                    || event_modifiers != keyboard.modifiers.mask())
            {
                reconcile_locked(context, keyboard);
                return Err(false);
            }
        }
        let repeat = source.is_physical()
            && key_phase == KeyPhase::Down
            && keyboard.physical.observe(key_code, key_phase);
        if source.is_physical() && key_phase == KeyPhase::Up {
            let _ = keyboard.physical.observe(key_code, key_phase);
        }
        let repeat = repeat || native_repeat;
        (
            key,
            if repeat {
                PhysicalPhase::Repeat
            } else {
                key_phase.into()
            },
            Some((key_phase, repeat)),
        )
    })
}
