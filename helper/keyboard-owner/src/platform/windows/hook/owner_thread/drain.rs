//! Retained native ownership, bounded shutdown pumping, and callback panic recovery.
use super::*;

pub(in crate::platform::windows::hook) fn retry_pending_paste_cleanup(
    keyboard: &mut CallbackKeyboard,
) {
    if keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    let physical_ctrl_down = keyboard.modifiers.ctrl.is_down();
    let physical_v_down = keyboard.logical_v_down;
    let (_, remaining) = injection::retry_paste_cleanup(
        keyboard.pending_paste_cleanup,
        physical_ctrl_down,
        physical_v_down,
    );
    keyboard.pending_paste_cleanup = remaining;
}

pub(in crate::platform::windows::hook) fn transaction_obligations_drained(
    keyboard: &CallbackKeyboard,
) -> bool {
    keyboard.transactional.journal_len() == 0
        && keyboard.transactional.owned_letters() == 0
        && keyboard.transactional.pending_injected_cleanup().is_none()
        && keyboard.transactional.pending_menu_cleanup().is_none()
        && keyboard.pending_paste_cleanup.is_empty()
        && keyboard.deferred_callback_replay.is_none()
        && keyboard.transaction_authority.is_none()
        && !keyboard.session_escape_native_owned
        && keyboard.captured_enter_source.is_none()
}

pub(in crate::platform::windows::hook) fn publish_pending_native_work(context: &CallbackContext) {
    let pending = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| !transaction_obligations_drained(&keyboard));
    context
        .state
        .pending_native_work
        .store(pending, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::windows::hook) enum ShutdownDrainOutcome {
    Drained,
    TerminalIncomplete,
}

pub(in crate::platform::windows::hook) fn drain_owned_transaction(
    context: &CallbackContext,
    message: &mut MSG,
    deadline: Instant,
) -> ShutdownDrainOutcome {
    // Retained replay/menu/paste cleanup suffixes continue through their
    // accepted-count authorities. Native downs are never exposed at shutdown:
    // an owned physical up must be observed before ownership can retire.
    let mut retry = 0_usize;
    loop {
        // Poll the durable worker result independently of its best-effort wake.
        // This also resolves replay if shutdown began before the normal timer.
        process_deferred_callback_replay(context);
        if let Some(mut keyboard) = lock_keyboard_recovering(context) {
            let authority_ready = recover_transaction_authority(context, &mut keyboard);
            if authority_ready {
                if keyboard.transactional.pending_menu_cleanup().is_some()
                    || keyboard.transactional.pending_injected_cleanup().is_some()
                {
                    let _ =
                        begin_transaction_control(context, &mut keyboard, Control::RetryCleanup);
                }
                retry_pending_paste_cleanup(&mut keyboard);
            }
            if transaction_obligations_drained(&keyboard) {
                return ShutdownDrainOutcome::Drained;
            }
            let now = Instant::now();
            if now >= deadline {
                return ShutdownDrainOutcome::TerminalIncomplete;
            }
        } else if Instant::now() >= deadline {
            return ShutdownDrainOutcome::TerminalIncomplete;
        }
        pump_owner_messages(context, message, deadline);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            continue;
        }
        let delay =
            OWNER_WAKE_RETRY_DELAYS[retry.min(OWNER_WAKE_RETRY_DELAYS.len() - 1)].min(remaining);
        retry = retry.saturating_add(1);
        thread::sleep(delay);
    }
}

pub(in crate::platform::windows::hook) fn pump_owner_messages(
    context: &CallbackContext,
    message: &mut MSG,
    deadline: Instant,
) {
    // SAFETY: message is owner-local writable storage. Bound each pass so a
    // producer that continuously posts messages cannot extend shutdown.
    let mut remaining = 64_usize;
    while remaining != 0
        && Instant::now() < deadline
        && unsafe { PeekMessageW(message, null_mut(), 0, 0, PM_REMOVE) } != 0
    {
        remaining -= 1;
        if message.message == WM_QUIT {
            continue;
        }
        if message.message == WM_OWNER_REPLAY {
            process_deferred_callback_replay(context);
            continue;
        }
        // SAFETY: PeekMessageW initialized this non-quit message.
        unsafe {
            TranslateMessage(message);
            DispatchMessageW(message);
        }
    }
}

pub(in crate::platform::windows::hook) fn handle_callback_panic(context: &CallbackContext) {
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    // TerminalSignal closes callback admission synchronously. The poisoned
    // keyboard contents and transaction authority remain intact for owner-side
    // recovery and exact-up drain. No terminal path exposes an owned down.
    context.terminal.trigger(TerminalReason::CallbackPanicked);
}

pub(in crate::platform::windows::hook) const fn callback_may_process_source(
    suppression_enabled: bool,
    source: InputSource,
) -> bool {
    suppression_enabled || !source.is_physical()
}
