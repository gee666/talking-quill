//! Independent Escape/Enter ownership and UI balancing.

use super::*;

pub(super) fn has_session_ownership(keyboard: &CallbackKeyboard) -> bool {
    keyboard.session_escape_native_owned || keyboard.session_enter_native_owned.is_some()
}

pub(super) fn native_modifiers_neutral() -> bool {
    MODIFIER_KEY_CODES
        .into_iter()
        .all(|key_code| !native_key_is_down(key_code))
}

pub(super) fn deliver_balancing_events(context: &CallbackContext, reducer: &mut KeyboardReducer) {
    for event in reducer.fail_open_balancing_events().into_iter().flatten() {
        if !context.gate.is_open()
            || !deliver_callback_event(&context.outbound, &context.terminal, event)
        {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            break;
        }
    }
}

pub(super) fn reconcile_hidden_session_native_ownership(
    keyboard: &mut CallbackKeyboard,
    escape_is_down: bool,
    enter_is_down: bool,
) -> (bool, bool, Option<u16>) {
    let captured_enter = keyboard.session_enter_native_owned;
    let escape_cleared = keyboard.session_escape_native_owned && !escape_is_down;
    let enter_cleared = keyboard.session_enter_native_owned.is_some() && !enter_is_down;
    let _ = keyboard
        .reducer
        .reconcile_hidden_session_releases(escape_is_down, enter_is_down);
    if escape_cleared {
        keyboard.session_escape_native_owned = false;
    }
    if enter_cleared {
        keyboard.session_enter_native_owned = None;
        keyboard.captured_enter_key_code = None;
    }
    (escape_cleared, enter_cleared, captured_enter)
}
pub(super) fn process_session_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    key_code: u16,
    phase: KeyPhase,
    repeat: bool,
    event_timestamp: u64,
) -> bool {
    let key = map_key_code(key_code);
    let session_key = match key {
        PhysicalKey::Escape => SessionKey::Escape,
        PhysicalKey::Enter => SessionKey::Enter,
        PhysicalKey::Letter(_) | PhysicalKey::Other => return false,
    };
    if key == PhysicalKey::Enter
        && keyboard
            .session_enter_native_owned
            .is_some_and(|captured| captured != key_code)
    {
        return false;
    }
    let native_owned = match key {
        PhysicalKey::Escape => keyboard.session_escape_native_owned,
        PhysicalKey::Enter => keyboard.session_enter_native_owned == Some(key_code),
        PhysicalKey::Letter(_) | PhysicalKey::Other => false,
    };
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let accepting = context.gate.is_open();
    let cutoff = match session_key {
        SessionKey::Escape => keyboard.escape_capture_enabled_at,
        SessionKey::Enter => keyboard.enter_capture_enabled_at,
    };
    let predates_policy = cutoff != 0 && event_timestamp <= cutoff;
    if (!accepting || predates_policy) && !native_owned {
        return false;
    }
    let plan = keyboard.reducer.plan_bindings_at(
        KeyInput {
            key,
            phase,
            modifiers: keyboard.modifiers.mask(),
            repeat,
            injected: false,
        },
        talking_quill_keyboard_core::ActivationBindings::default(),
        false,
        if accepting && !predates_policy {
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
    let swallowed = keyboard.reducer.apply(plan, delivered);
    if delivered && swallowed && phase == KeyPhase::Down && !repeat {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = true,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = Some(key_code);
                keyboard.captured_enter_key_code = Some(key_code);
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
    }
    if native_owned && phase == KeyPhase::Up {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = None;
                keyboard.captured_enter_key_code = None;
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
        // UI balancing may already have reset reducer protocol state, but the
        // matching native up remains owned and must still be suppressed.
        return true;
    }
    swallowed || native_owned
}
pub(super) fn finish_native_transition(context: &CallbackContext, keyboard: &mut CallbackKeyboard) {
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    deliver_balancing_events(context, &mut keyboard.reducer);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    if !context.terminal.is_triggered() {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}
