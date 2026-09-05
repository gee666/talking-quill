//! Legacy reducer adapter retained only for independent model tests.

use super::*;

#[cfg(test)]
pub(super) fn process_key_event_with_modifiers(
    context: &CallbackContext,
    key_code: u16,
    phase: KeyPhase,
    native_repeat: bool,
    event_modifiers: Option<ModifierMask>,
    event_timestamp: u64,
    reconcile_held_state: bool,
) -> bool {
    let key = map_key_code(key_code);
    if key == PhysicalKey::Other {
        return false;
    }
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return false;
        }
    };
    let was_held = keyboard.physical.is_held(key_code);
    let discontinuity = match phase {
        KeyPhase::Down => was_held && !native_repeat,
        KeyPhase::Up => !was_held,
    };
    let tracked_modifiers = keyboard.modifiers.mask();
    let observed_modifiers = event_modifiers.unwrap_or(tracked_modifiers);
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let relevant_to_capture = keyboard.reducer.has_captured_sequence()
        || !keyboard.preheld_letters.is_empty()
        || match key {
            PhysicalKey::Letter(_) => {
                keyboard.activation.enabled && observed_modifiers != ModifierMask::default()
            }
            PhysicalKey::Escape => capture_mode.allows(SessionKey::Escape),
            PhysicalKey::Enter => capture_mode.allows(SessionKey::Enter),
            PhysicalKey::Other => false,
        };
    let tracked_state_mismatch = reconcile_held_state
        && relevant_to_capture
        && phase == KeyPhase::Down
        && !keyboard.tracked_native_state_is_consistent(key_code);
    if discontinuity
        || tracked_state_mismatch
        || event_modifiers.is_some_and(|observed| observed != tracked_modifiers)
    {
        // A non-repeat discontinuity or event-time modifier mismatch indicates
        // a missed, synthetic, or Secure-Input-hidden transition. Fail open and
        // conservatively resync only after rejecting the current event.
        deliver_balancing_events(context, &mut keyboard.reducer);
        keyboard.captured_enter_key_code = None;
        keyboard.seed_from_state(native_key_is_down);
        return false;
    }

    let tracked_repeat = keyboard.physical.observe(key_code, phase);
    let repeat = tracked_repeat || native_repeat;
    let policy_cutoff = match key {
        PhysicalKey::Letter(_) if keyboard.activation.enabled => keyboard.activation_revision_at,
        PhysicalKey::Escape if capture_mode.allows(SessionKey::Escape) => {
            keyboard.escape_capture_enabled_at
        }
        PhysicalKey::Enter if capture_mode.allows(SessionKey::Enter) => {
            keyboard.enter_capture_enabled_at
        }
        _ => 0,
    };
    let predates_policy = policy_cutoff != 0 && event_timestamp <= policy_cutoff;
    if let PhysicalKey::Letter(letter) = key {
        if phase == KeyPhase::Down
            && (!context.gate.is_open() || predates_policy || (native_repeat && !tracked_repeat))
        {
            keyboard.preheld_letters.insert(letter);
        } else if phase == KeyPhase::Up {
            keyboard.preheld_letters.remove(letter);
        }
    }
    let accepting = context.gate.is_open();
    let releases_passive_letter = matches!(key, PhysicalKey::Letter(_)) && phase == KeyPhase::Up;
    if (!accepting || predates_policy)
        && !keyboard.reducer.is_capturing(key)
        && !releases_passive_letter
    {
        return false;
    }
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_key_code
            .is_some_and(|captured| captured != key_code)
    {
        return false;
    }
    let activation = keyboard.activation;
    let input = KeyInput {
        key,
        phase,
        modifiers: observed_modifiers,
        repeat,
        injected: false,
    };
    let plan = keyboard.reducer.plan_bindings_at(
        input,
        activation.bindings,
        accepting && activation.enabled && keyboard.preheld_letters.is_empty(),
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
        event_timestamp / 1_000_000,
    );
    let planned_event = plan.event();
    let delivered = planned_event.is_none()
        || (accepting
            && deliver_callback_event(
                &context.outbound,
                &context.terminal,
                planned_event.expect("event presence checked above"),
            ));
    if !delivered {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
    }
    let swallowed = keyboard.reducer.apply(plan, delivered);
    if let Some(KeyboardEvent::SessionKey {
        key: talking_quill_keyboard_core::SessionKey::Enter,
        phase: talking_quill_keyboard_core::EventPhase::Down,
    }) = planned_event
        && delivered
        && swallowed
    {
        keyboard.captured_enter_key_code = Some(key_code);
    } else if key == PhysicalKey::Enter
        && phase == KeyPhase::Up
        && keyboard.captured_enter_key_code == Some(key_code)
    {
        keyboard.captured_enter_key_code = None;
    }
    swallowed
}

#[cfg(test)]
pub(super) fn process_key_event(
    context: &CallbackContext,
    key_code: u16,
    phase: KeyPhase,
    native_repeat: bool,
    _reconcile_native_state: bool,
) -> bool {
    process_key_event_with_modifiers(
        context,
        key_code,
        phase,
        native_repeat,
        None,
        u64::MAX,
        false,
    )
}

#[cfg(test)]
pub(super) const fn is_synthetic_event(
    identity: injection::InjectionIdentity,
    _marker: i64,
    source_pid: i64,
) -> bool {
    !matches!(
        injection::unmarked_source(identity, source_pid),
        InputSource::Physical
    )
}
