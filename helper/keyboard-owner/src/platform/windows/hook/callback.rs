//! Bounded low-level keyboard callback and deferred release accounting.
use super::*;

mod event;
pub(super) use event::*;

pub(super) unsafe extern "system" fn keyboard_hook(
    code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    // Callback-stack-local authority: nested helper SendInput callbacks cannot
    // overwrite the outer physical edge's panic disposition.
    let callback_disposition = Cell::new(CallbackDisposition::Pass);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if code != HC_ACTION as i32 {
            return None;
        }
        let context_ptr = CALLBACK_CONTEXT.load(Ordering::Acquire);
        if context_ptr.is_null() {
            return None;
        }
        // SAFETY: the owner stores this pointer before hook installation and
        // clears it only after unhooking on the same thread.
        let context = unsafe { &*context_ptr };
        context.observability.record_hc_action_callback();
        // SAFETY: HC_ACTION defines l_param as a valid KBDLLHOOKSTRUCT pointer
        // for this callback's duration.
        let native = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
        let phase = match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyPhase::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyPhase::Up,
            _ => return None,
        };
        let source =
            injection::classify(context.injection_markers, native.flags, native.dwExtraInfo);
        if source.is_physical() {
            context.observability.record_physical_callback();
        }
        let extended = native.flags & LLKHF_EXTENDED != 0;
        // When suppression is disabled, preserve every physical input record.
        // Owner-enabled builds opt into the reducer below.
        if !callback_may_process_source(context.suppression_enabled, source) {
            if source.is_physical() {
                context.observability.record_physical_callback_filtered();
            }
            return Some(false);
        }
        if matches!(
            source,
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
        ) {
            // Nested SendInput callbacks occur while the outer transaction owns
            // the keyboard mutex. Return before both reducer processing and the
            // pending-work snapshot below; attempting that snapshot here would
            // self-deadlock the hook and trigger LowLevelHooksTimeout removal.
            return Some(false);
        }
        let captured = process_transactional_hook_record_at(
            context,
            TransactionalHookRecord {
                virtual_key: native.vkCode as u16,
                scan_code: native.scanCode,
                extended,
                platform_flags: native.flags,
                phase,
                source,
            },
            HookObservation {
                observed_at_ms: context.owner_epoch.elapsed().as_millis() as u64,
                // LowLevelKeyboardProc runs before Windows updates asynchronous
                // key state. Callback-time GetAsyncKeyState is diagnostic only;
                // ordered LL edges are the transaction authority for every
                // physical-equivalent source. The owner timer repairs missed
                // native transitions outside the callback.
                native_modifiers: None,
            },
            &callback_disposition,
        );
        publish_pending_native_work(context);
        Some(captured)
    }));

    match result {
        Ok(Some(true)) => 1,
        Ok(Some(false)) | Ok(None) => {
            // SAFETY: the ignored hook handle may be null; original arguments
            // are forwarded unchanged.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
        Err(_) => {
            let context_ptr = CALLBACK_CONTEXT.load(Ordering::Acquire);
            if !context_ptr.is_null() {
                // SAFETY: context remains alive until owner-thread unhooking.
                let context = unsafe { &*context_ptr };
                handle_callback_panic(context);
                if callback_disposition.get() == CallbackDisposition::Capture {
                    return 1;
                }
            }
            // Untouched input fails open. A turn that already retained the
            // current edge keeps it suppressed while its authority recovers.
            // SAFETY: the original callback arguments are forwarded unchanged;
            // User32 ignores the null hook handle.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
    }
}

fn release_is_already_observed(keyboard: &CallbackKeyboard, identity: KeyIdentity) -> bool {
    if keyboard.transactional.pending_injected_cleanup().is_some()
        || keyboard.transactional.pending_menu_cleanup().is_some()
        || !keyboard.pending_paste_cleanup.is_empty()
    {
        return false;
    }
    match identity {
        KeyIdentity::Modifier(side) => {
            !keyboard.modifiers.transactional_sides().contains(side)
                && !keyboard.transactional.physical_modifiers().contains(side)
        }
        KeyIdentity::Letter(key) => {
            let bit = 1_u32 << key.index();
            (keyboard.physical.held_letter_bits()
                | keyboard.transactional.physical_letters()
                | keyboard.transactional.owned_letters())
                & bit
                == 0
        }
        _ => false,
    }
}

pub(super) fn menu_modifier_release_still_needed(keyboard: &CallbackKeyboard, slot: usize) -> bool {
    match slot {
        0 => !keyboard.modifiers.alt.left,
        1 => !keyboard.modifiers.alt.right,
        2 => !keyboard.modifiers.meta.left,
        3 => !keyboard.modifiers.meta.right,
        _ => false,
    }
}

pub(super) const fn menu_modifier_release_slot(identity: KeyIdentity) -> Option<usize> {
    match identity {
        KeyIdentity::Modifier(ModifierSide::LeftAlt) => Some(0),
        KeyIdentity::Modifier(ModifierSide::RightAlt) => Some(1),
        KeyIdentity::Modifier(ModifierSide::LeftMeta) => Some(2),
        KeyIdentity::Modifier(ModifierSide::RightMeta) => Some(3),
        _ => None,
    }
}

pub(super) fn track_deferred_replay_race(
    keyboard: &mut CallbackKeyboard,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
) {
    keyboard.deferred_replay_raced = true;
    let identity = map_key_identity(virtual_key, scan_code, extended);
    match identity {
        KeyIdentity::Modifier(_) => {
            keyboard
                .modifiers
                .observe(virtual_key, scan_code, extended, phase);
        }
        KeyIdentity::Letter(key) => {
            keyboard
                .physical
                .observe(PhysicalKey::Letter(key), None, phase);
        }
        KeyIdentity::Escape => {
            keyboard.physical.observe(PhysicalKey::Escape, None, phase);
        }
        KeyIdentity::Enter => {
            keyboard.physical.observe(
                PhysicalKey::Enter,
                record_enter_source(virtual_key, scan_code, extended),
                phase,
            );
        }
        KeyIdentity::Other(_) => {}
    }
    if virtual_key == VK_V {
        keyboard.logical_v_down = phase == KeyPhase::Down;
    }
    if matches!(identity, KeyIdentity::Modifier(ModifierSide::LeftCtrl)) && phase == KeyPhase::Up {
        keyboard.altgr_synthetic_ctrl = false;
    }
    if matches!(identity, KeyIdentity::Modifier(ModifierSide::RightAlt)) {
        keyboard.altgr_synthetic_ctrl = phase == KeyPhase::Down && keyboard.modifiers.ctrl.left;
        keyboard.altgr_active = phase == KeyPhase::Down
            && conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
    }
}

pub(super) fn transaction_gate(context: &CallbackContext) -> GateState {
    let initialized = context.state.protocol_initialized.load(Ordering::Acquire);
    if context.terminal.is_triggered()
        || context.state.stopping.load(Ordering::Acquire)
        || (initialized && !context.gate.is_open())
    {
        GateState::Closed
    } else {
        GateState::Open
    }
}

pub(super) fn complete_transaction_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    completion: Completion,
    record: CompletedHookRecord,
) -> bool {
    let CompletedHookRecord {
        identity,
        virtual_key,
        phase,
        repeat,
        enter_source,
        physical,
    } = record;
    let Completion::Event(outcome) = completion else {
        unreachable!("an event turn completes as event")
    };
    if outcome.terminal
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.terminal.is_triggered()
    {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    if physical && phase == KeyPhase::Up && outcome.disposition == EventDisposition::PassCurrent {
        if virtual_key == VK_V {
            keyboard.pending_paste_cleanup =
                keyboard.pending_paste_cleanup.without_virtual_key(VK_V);
        }
        if matches!(
            identity,
            KeyIdentity::Modifier(ModifierSide::LeftCtrl | ModifierSide::RightCtrl)
        ) && !keyboard.modifiers.ctrl.is_down()
        {
            keyboard.pending_paste_cleanup = keyboard
                .pending_paste_cleanup
                .without_virtual_key(VK_CONTROL);
        }
    }
    if keyboard.shutdown_requested && transaction_obligations_drained(keyboard) {
        // SAFETY: called on the keyboard owner thread after the final owned up.
        unsafe { PostQuitMessage(0) };
    }
    if outcome.disposition == EventDisposition::CaptureCurrent {
        return true;
    }
    match identity {
        KeyIdentity::Escape => {
            process_session_event(context, keyboard, PhysicalKey::Escape, phase, repeat, None)
        }
        KeyIdentity::Enter => process_session_event(
            context,
            keyboard,
            PhysicalKey::Enter,
            phase,
            repeat,
            enter_source,
        ),
        KeyIdentity::Letter(_) | KeyIdentity::Modifier(_) | KeyIdentity::Other(_) => false,
    }
}
