//! Session controls and native event normalization.
use super::*;

pub(super) fn process_session_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    key: PhysicalKey,
    phase: KeyPhase,
    repeat: bool,
    enter_source: Option<EnterSource>,
) -> bool {
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_source
            .is_some_and(|captured| Some(captured) != enter_source)
    {
        return false;
    }
    let accepting = context.gate.is_open();
    if !accepting && !keyboard.reducer.is_capturing(key) {
        return false;
    }
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let plan = keyboard.reducer.plan_bindings_at(
        KeyInput {
            key,
            phase,
            modifiers: keyboard.modifiers.mask(),
            repeat,
            injected: false,
        },
        ActivationBindings::default(),
        false,
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
        0,
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
            PhysicalKey::Enter => keyboard.captured_enter_source = enter_source,
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
    }
    if phase == KeyPhase::Up {
        match key {
            PhysicalKey::Escape if keyboard.session_escape_native_owned => {
                keyboard.session_escape_native_owned = false;
            }
            PhysicalKey::Enter if keyboard.captured_enter_source == enter_source => {
                keyboard.captured_enter_source = None;
            }
            PhysicalKey::Letter(_)
            | PhysicalKey::Enter
            | PhysicalKey::Escape
            | PhysicalKey::Other => {}
        }
    }
    swallowed
}

pub(super) fn map_key_identity(virtual_key: u16, scan_code: u32, extended: bool) -> KeyIdentity {
    if let Some(side) = modifier_side(virtual_key, scan_code, extended) {
        return KeyIdentity::Modifier(side);
    }
    match map_scan_code(scan_code, extended) {
        PhysicalKey::Letter(key) => KeyIdentity::Letter(key),
        PhysicalKey::Escape => KeyIdentity::Escape,
        PhysicalKey::Enter => KeyIdentity::Enter,
        PhysicalKey::Other if scan_code == 0 && virtual_key == VK_ESCAPE => KeyIdentity::Escape,
        PhysicalKey::Other if scan_code == 0 && virtual_key == VK_RETURN => KeyIdentity::Enter,
        PhysicalKey::Other if (0x41..=0x5A).contains(&virtual_key) => {
            // SendInput/UIAutomation may provide a virtual-key record without
            // KEYEVENTF_SCANCODE. The low-level hook then has no physical
            // position to normalize, so use the canonical A-Z virtual key.
            KeyIdentity::Letter(
                ActivationKey::from_index((virtual_key - 0x41) as u8)
                    .expect("validated A-Z virtual key has an activation index"),
            )
        }
        PhysicalKey::Other => KeyIdentity::Other(virtual_key),
    }
}

pub(super) const fn modifier_side(
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
) -> Option<ModifierSide> {
    match virtual_key {
        VK_LCONTROL => Some(ModifierSide::LeftCtrl),
        VK_RCONTROL => Some(ModifierSide::RightCtrl),
        VK_CONTROL if scan_code == 0x1D && extended => Some(ModifierSide::RightCtrl),
        VK_CONTROL => Some(ModifierSide::LeftCtrl),
        VK_LMENU => Some(ModifierSide::LeftAlt),
        VK_RMENU => Some(ModifierSide::RightAlt),
        VK_MENU if scan_code == 0x38 && extended => Some(ModifierSide::RightAlt),
        VK_MENU => Some(ModifierSide::LeftAlt),
        VK_LSHIFT => Some(ModifierSide::LeftShift),
        VK_RSHIFT => Some(ModifierSide::RightShift),
        VK_SHIFT if scan_code == 0x36 => Some(ModifierSide::RightShift),
        VK_SHIFT => Some(ModifierSide::LeftShift),
        VK_LWIN => Some(ModifierSide::LeftMeta),
        VK_RWIN => Some(ModifierSide::RightMeta),
        _ => None,
    }
}

#[cfg(all(test, feature = "windows-native-test-input"))]
pub(super) fn process_hook_record(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    injected: bool,
) -> bool {
    process_hook_record_at(
        context,
        virtual_key,
        scan_code,
        extended,
        phase,
        if injected {
            InjectionKind::External
        } else {
            InjectionKind::Physical
        },
        HookObservation::default(),
    )
}

#[cfg(all(test, feature = "windows-native-test-input"))]
pub(super) fn process_hook_record_at(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    injection: InjectionKind,
    observation: HookObservation,
) -> bool {
    // Injected input must never become physical hotkey state. In particular,
    // an unmatched injected modifier must not remain latched and combine with
    // ordinary typing to activate dictation.
    if injection == InjectionKind::Helper {
        return false;
    }

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return false;
    };

    if injection == InjectionKind::External {
        return false;
    }

    let right_alt =
        virtual_key == VK_RMENU || (virtual_key == VK_MENU && scan_code == 0x38 && extended);
    if right_alt && phase == KeyPhase::Down {
        // Right Alt is AltGr on many layouts, and Windows does not guarantee a
        // stable injected-Ctrl event shape. Never allow it to activate a
        // global shortcut; left Alt remains available for configured bindings.
        keyboard.altgr_active = true;
    }
    if right_alt && phase == KeyPhase::Up {
        keyboard.altgr_active = false;
    }

    if keyboard
        .modifiers
        .observe(virtual_key, scan_code, extended, phase)
    {
        let modifiers = keyboard.modifiers.mask();
        if keyboard.modifiers_fenced && modifiers == ModifierMask::default() {
            keyboard.modifiers_fenced = false;
        }
        keyboard.reducer.observe_modifiers(modifiers);
        // Modifier prefixes intentionally leak through to the foreground app.
        return false;
    }
    let key = map_scan_code(scan_code, extended);
    if key == PhysicalKey::Other {
        return false;
    }
    if let Some(native_modifiers) = observation.native_modifiers {
        // Snapshot recovery also repairs a missed Right-Alt release. Conversely,
        // a physically held Right Alt remains suppressed even when Windows did
        // not expose AltGr's synthetic Ctrl as injected.
        keyboard.altgr_active = native_modifiers.alt.right;
        if native_modifiers.mask() != keyboard.modifiers.mask() {
            // A modifier release can be lost across secure-desktop transitions
            // or helper startup. Resynchronize before considering ordinary
            // typing. When no modifier is physically down, fence this letter
            // through its up so stale state can never turn it into activation.
            keyboard.modifiers = native_modifiers;
            let modifiers = keyboard.modifiers.mask();
            // This event-time native snapshot is authoritative. Startup and
            // configuration fences remain intact when masks agree, while a
            // repaired mismatch can use a genuinely held left-side modifier.
            keyboard.modifiers_fenced = false;
            keyboard.reducer.observe_modifiers(modifiers);
            keyboard.reducer.fence_activation_revision();
            if modifiers == ModifierMask::default()
                && let PhysicalKey::Letter(letter) = key
            {
                keyboard.activation_fenced_letters |= 1_u32 << u32::from(letter.index());
            }
        }
    }
    let suppress_activation = keyboard.altgr_active;
    let enter_source = enter_source(scan_code, extended);
    let repeat = keyboard.physical.observe(key, enter_source, phase);
    let input = KeyInput {
        key,
        phase,
        modifiers: keyboard.modifiers.mask(),
        repeat,
        injected: false,
    };
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_source
            .is_some_and(|captured| Some(captured) != enter_source)
    {
        return false;
    }
    let accepting = context.gate.is_open();
    if !accepting && !keyboard.reducer.is_capturing(key) {
        // Keep native physical state current while closed, but do not retain
        // prefixes that could complete a chord after initialization/reopening.
        if let PhysicalKey::Letter(letter) = key
            && phase == KeyPhase::Down
        {
            keyboard.activation_fenced_letters |= 1_u32 << u32::from(letter.index());
        }
        return false;
    }
    let activation = keyboard.activation;
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let plan = keyboard.reducer.plan_bindings_at(
        input,
        activation.bindings,
        accepting
            && activation.enabled
            && keyboard.activation_fenced_letters == 0
            && !keyboard.modifiers_fenced
            && !suppress_activation,
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
        observation.observed_at_ms,
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
    if let PhysicalKey::Letter(letter) = key
        && phase == KeyPhase::Up
    {
        keyboard.activation_fenced_letters &= !(1_u32 << u32::from(letter.index()));
    }
    if let Some(KeyboardEvent::SessionKey {
        key: talking_quill_keyboard_core::SessionKey::Enter,
        phase: talking_quill_keyboard_core::EventPhase::Down,
    }) = planned_event
        && delivered
        && swallowed
    {
        keyboard.captured_enter_source = enter_source;
    } else if key == PhysicalKey::Enter
        && phase == KeyPhase::Up
        && keyboard.captured_enter_source == enter_source
    {
        keyboard.captured_enter_source = None;
    }
    swallowed
}

pub(super) const fn record_enter_source(
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
) -> Option<EnterSource> {
    // Virtual-key input can omit the scan code. Keep main/numpad balancing
    // equivalent to the corresponding physical Enter key.
    let scan_code = if virtual_key == VK_RETURN && scan_code == 0 {
        0x1C
    } else {
        scan_code
    };
    enter_source(scan_code, extended)
}

pub(super) const fn enter_source(scan_code: u32, extended: bool) -> Option<EnterSource> {
    if scan_code != 0x1C {
        None
    } else if extended {
        Some(EnterSource::Numpad)
    } else {
        Some(EnterSource::Main)
    }
}
