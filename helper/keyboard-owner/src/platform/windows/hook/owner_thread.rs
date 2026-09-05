//! Own the Windows hook message loop and drain it during shutdown.
use super::*;

pub(super) fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<u32, PlatformError>>,
    startup_state: Arc<AtomicU8>,
    owner_commands: Receiver<OwnerCommand>,
    paste_commands: Receiver<PasteCommand>,
    owner_completion: Sender<()>,
) {
    // Declared first so it drops last, after all owner-thread resources.
    let _owner_completion = OwnerCompletion(owner_completion);
    let mut context = Box::new(context);
    let context_ptr = (&raw mut *context).cast::<CallbackContext>();
    if CALLBACK_CONTEXT
        .compare_exchange(null_mut(), context_ptr, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }

    // Bind this newly created, window-free owner thread to the interactive
    // input desktop before creating its queue or installing any hook. This is
    // explicit rather than relying on desktop inheritance across the
    // service-mediated CreateProcessAsUser launch.
    let thread_id = unsafe { GetCurrentThreadId() };
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let replay_accepted = Arc::clone(&context.replay_accepted);
    let replay_markers = context.injection_markers;
    let replay_worker = match thread::Builder::new()
        .name("talking-quill-win-replay".into())
        .spawn(move || {
            while let Ok(work) = replay_receiver.recv() {
                let accepted = match work {
                    ReplayWork::Replay {
                        batch,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::inject_replay(replay_markers, batch)),
                    ReplayWork::NeutralizeMenu {
                        modifiers,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::neutralize_menu(replay_markers, modifiers)),
                };
                replay_accepted.store(
                    accepted.map_or(1, |accepted| {
                        u64::try_from(accepted)
                            .unwrap_or(u64::MAX)
                            .saturating_add(2)
                    }),
                    Ordering::Release,
                );
                // Completion is durable in replay_accepted. The owner timer
                // polls it, so a failed wake cannot strand replay authority.
                let _ = post_owner_message_once(thread_id, WM_OWNER_REPLAY);
            }
        }) {
        Ok(worker) => worker,
        Err(_) => {
            CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
    };
    let previous_desktop = unsafe { GetThreadDesktop(thread_id) };
    let input_desktop = unsafe {
        OpenInputDesktop(
            0,
            0,
            // SendInput requires input playback access on the assigned desktop.
            // Hook access alone installs successfully but injection fails with
            // ERROR_ACCESS_DENIED, including replay and menu neutralization.
            DESKTOP_READOBJECTS
                | DESKTOP_HOOKCONTROL
                | DESKTOP_SWITCHDESKTOP
                | DESKTOP_JOURNALPLAYBACK,
        )
    };
    if previous_desktop.is_null()
        || input_desktop.is_null()
        || unsafe { SetThreadDesktop(input_desktop) } == 0
    {
        if !input_desktop.is_null() {
            unsafe { CloseDesktop(input_desktop) };
        }
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }
    let _thread_desktop = HookThreadDesktop {
        input: input_desktop,
        previous: previous_desktop,
    };

    let mut message = MSG::default();
    let _target_monitor = match super::super::target::TargetMonitor::start() {
        Ok(monitor) => monitor,
        Err(_) => {
            CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
    };
    // SAFETY: this no-remove peek creates the owner queue before hook
    // installation, so low-level callbacks always have a live message loop.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: the callback has the system ABI and remains live until same-thread
    // unhooking. WH_KEYBOARD_LL executes in this owner process, so no injectable
    // callback DLL/module handle is involved.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            low_level_hook_module(),
            0,
        )
    };
    if hook.is_null() {
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        let category = match error {
            windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED => "access_denied",
            _ => "native_unavailable",
        };
        eprintln!("keyboard-owner hook install unavailable: {category}");
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }
    context.observability.record_hook_installed();
    // Focus/foreground/caret epochs make A->B->A transitions observable even
    // when the point-in-time HWND tuple returns to its original values.
    let win_events = unsafe {
        [
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_FOCUS,
                EVENT_OBJECT_FOCUS,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
        ]
    };
    let target_change_evidence_ready = win_events.iter().all(|hook| !hook.is_null());
    context
        .state
        .target_change_evidence_ready
        .store(target_change_evidence_ready, Ordering::Release);
    // These hooks are optional for activation. Paste checks the readiness bit
    // above and fails closed unless the complete target-change evidence set is
    // present; partial hooks only conservatively invalidate epochs.
    let mut installation = OwnerHookInstallation {
        hook,
        win_events,
        timer: 0,
    };
    let owner_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Low-level callbacks are delivered on this message-loop thread. Seed all
        // tracked physical state after installation and before readiness so a key
        // already held cannot begin an activation or session sequence.
        let physical = physical_tracker_from_state(native_physical_key_is_down);
        let modifiers = ModifierTracker::from_state(key_is_down);
        let keyboard = match context.keyboard.get_mut() {
            Ok(keyboard) => keyboard,
            Err(poisoned) => {
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                poisoned.into_inner()
            }
        };
        keyboard.physical = physical;
        keyboard.modifiers = modifiers;
        // Bind candidate admission to the exact desktop handle assigned to
        // this windowless hook thread. Reopening the input desktop here can
        // race a switch and previously made a valid owner start targetless.
        keyboard.input_desktop = desktop_identity(input_desktop);
        keyboard.logical_v_down = key_is_down(VK_V);
        keyboard.altgr_active =
            conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
        keyboard.modifiers_fenced = keyboard.modifiers.mask() != ModifierMask::default();
        keyboard.transactional = TransactionEngine::with_physical_snapshot(
            CompiledActivationConfig::default(),
            PhysicalSnapshot::new(
                keyboard.physical.held_letter_bits(),
                keyboard.modifiers.transactional_sides(),
                keyboard.altgr_active,
            ),
        );

        if !claim_startup(&startup_state) {
            context.gate.close();
            return;
        }
        // SAFETY: null HWND creates a periodic current-thread recovery timer.
        let snapshot_timer = unsafe { SetTimer(null_mut(), 0, NATIVE_SNAPSHOT_INTERVAL_MS, None) };
        if snapshot_timer == 0 {
            context.gate.close();
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
        installation.timer = snapshot_timer;
        let mut startup_ready = Some(ready);
        let mut pending_paste = None;
        let mut next_hook_refresh = Instant::now() + HOOK_REFRESH_INTERVAL;
        loop {
            // SAFETY: null HWND selects this hook owner's complete thread queue.
            let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            if message.message == WM_OWNER_COMMAND {
                process_owner_commands(&context, &owner_commands);
            } else if message.message == WM_OWNER_PASTE {
                process_paste_commands(&context, &paste_commands, &mut pending_paste);
                poll_pending_paste(&context, &mut pending_paste);
            } else if message.message == WM_OWNER_REPLAY {
                process_deferred_callback_replay(&context);
            } else if message.message == WM_TIMER {
                if message.wParam == snapshot_timer {
                    process_deferred_callback_replay(&context);
                    process_owner_commands(&context, &owner_commands);
                    process_paste_commands(&context, &paste_commands, &mut pending_paste);
                    reconcile_native_state(&context);
                    if Instant::now() >= next_hook_refresh
                        && pending_paste.is_none()
                        && !context.state.stopping.load(Ordering::Acquire)
                        && context.keyboard.lock().is_ok_and(|keyboard| {
                            transaction_obligations_drained(&keyboard)
                                && keyboard.physical.held_letter_bits() == 0
                                && keyboard.modifiers.is_neutral()
                        })
                    {
                        if !installation.refresh_keyboard_hook() {
                            context.gate.close();
                            context
                                .terminal
                                .trigger(TerminalReason::InputInjectionUnavailable);
                            break;
                        }
                        next_hook_refresh = Instant::now() + HOOK_REFRESH_INTERVAL;
                    }
                    if let Some(ready) = startup_ready.take() {
                        context.observability.record_pump_alive();
                        context.state.hook_status.store(
                            hook_status_to_u8(HookStatus::InstalledUnobserved),
                            Ordering::Release,
                        );
                        if ready.send(Ok(thread_id)).is_err()
                            || context.state.stopping.load(Ordering::Acquire)
                        {
                            context.gate.close();
                            break;
                        }
                    }
                }
                poll_pending_paste(&context, &mut pending_paste);
            } else {
                // SAFETY: `message` was initialized by GetMessageW.
                unsafe {
                    TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
        }

        if let Some(ready) = startup_ready.take() {
            let _ = ready.send(Err(PlatformError::HookUnavailable));
        }
        context.gate.close();
        if pending_paste.is_some() {
            finish_pending_paste(&mut pending_paste, failed_paste(PasteFailure::Unavailable));
        }
        let shutdown_deadline = context
            .state
            .shutdown_deadline
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |installed| *installed)
            .unwrap_or_else(|| Instant::now() + SHUTDOWN_DRAIN_TIMEOUT);
        let mut needs_drain = false;
        if let Some(mut keyboard) = lock_keyboard_recovering(&context) {
            let authority_ready = recover_transaction_authority(&context, &mut keyboard);
            if authority_ready {
                if keyboard.transactional.pending_menu_cleanup().is_some()
                    || keyboard.transactional.pending_injected_cleanup().is_some()
                {
                    let _ =
                        begin_transaction_control(&context, &mut keyboard, Control::RetryCleanup);
                }
                retry_pending_paste_cleanup(&mut keyboard);
                let _ = begin_transaction_control(&context, &mut keyboard, Control::Shutdown);
                keyboard.shutdown_requested = true;
            }
            needs_drain = !transaction_obligations_drained(&keyboard);
        }
        if needs_drain {
            match drain_owned_transaction(&context, &mut message, shutdown_deadline) {
                ShutdownDrainOutcome::Drained => {}
                ShutdownDrainOutcome::TerminalIncomplete => {
                    context.observability.record_shutdown_ownership_deadline();
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context
                        .terminal
                        .trigger(TerminalReason::InputInjectionUnavailable);
                }
            }
        }
        while let Ok(command) = paste_commands.try_recv() {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
        }
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        while let Ok(command) = owner_commands.try_recv() {
            let _ = cancel_owner_command(&command.state);
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::ThreadStopped));
        }
        context
            .state
            .hook_status
            .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
        if !context.state.stopping.load(Ordering::Acquire) {
            context.terminal.trigger(TerminalReason::HookStopped);
        }
    }));
    if owner_result.is_err() {
        context.gate.close();
        context.state.stopping.store(true, Ordering::Release);
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
    drop(installation);
    drop(context.replay_sender.take());
    let replay_deadline = Instant::now() + Duration::from_millis(500);
    while !replay_worker.is_finished() && Instant::now() < replay_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if replay_worker.is_finished() {
        let _ = replay_worker.join();
    } else {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
}

pub(super) fn retry_pending_paste_cleanup(keyboard: &mut CallbackKeyboard) {
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

pub(super) fn transaction_obligations_drained(keyboard: &CallbackKeyboard) -> bool {
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

pub(super) fn publish_pending_native_work(context: &CallbackContext) {
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
pub(super) enum ShutdownDrainOutcome {
    Drained,
    TerminalIncomplete,
}

pub(super) fn drain_owned_transaction(
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

pub(super) fn pump_owner_messages(context: &CallbackContext, message: &mut MSG, deadline: Instant) {
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

pub(super) fn handle_callback_panic(context: &CallbackContext) {
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

pub(super) const fn callback_may_process_source(
    suppression_enabled: bool,
    source: InputSource,
) -> bool {
    suppression_enabled || !source.is_physical()
}
