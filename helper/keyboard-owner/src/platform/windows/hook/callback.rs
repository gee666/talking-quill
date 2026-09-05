//! Bounded low-level keyboard callback and deferred release accounting.
use super::*;

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
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
    }
}

pub(super) fn process_transactional_hook_record_at(
    context: &CallbackContext,
    record: TransactionalHookRecord,
    observation: HookObservation,
    callback_disposition: &Cell<CallbackDisposition>,
) -> bool {
    let TransactionalHookRecord {
        virtual_key,
        scan_code,
        extended,
        platform_flags,
        phase,
        source,
    } = record;
    callback_disposition.set(CallbackDisposition::Pass);
    // Helper-owned traffic is a strict bypass. In particular a SendInput call
    // made while the callback mutex is held can never recursively poison it.
    if matches!(
        source,
        InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
    ) {
        return false;
    }

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return false;
    };
    if source == InputSource::External {
        keyboard.external_reconcile_after = Some(Instant::now() + Duration::from_millis(500));
        if let KeyIdentity::Letter(key) = map_key_identity(virtual_key, scan_code, extended) {
            let bit = 1_u32 << key.index();
            match phase {
                KeyPhase::Down => keyboard.external_held_letters |= bit,
                KeyPhase::Up => keyboard.external_held_letters &= !bit,
            }
        }
    }
    if !recover_transaction_authority(context, &mut keyboard) {
        let identity = map_key_identity(virtual_key, scan_code, extended);
        let menu_neutralization_pending =
            keyboard
                .deferred_callback_replay
                .as_ref()
                .is_some_and(|pending| {
                    matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization)
                });
        let suppress_menu_release = menu_neutralization_pending
            && phase == KeyPhase::Up
            && menu_modifier_release_slot(identity).is_some();
        let suppress_owned_letter = menu_neutralization_pending
            && matches!(identity, KeyIdentity::Letter(key)
                if keyboard.transactional.owned_letters() & (1_u32 << key.index()) != 0);
        track_deferred_replay_race(&mut keyboard, virtual_key, scan_code, extended, phase);
        keyboard.deferred_observed_at_ms = observation.observed_at_ms;
        if suppress_menu_release && let Some(slot) = menu_modifier_release_slot(identity) {
            keyboard.deferred_menu_releases[slot] = Some(NativeKey {
                virtual_key,
                scan_code,
                extended,
                platform_flags: u64::from(platform_flags),
            });
        }
        if suppress_menu_release || suppress_owned_letter {
            callback_disposition.set(CallbackDisposition::Capture);
            return true;
        }
        return false;
    }
    let identity = map_key_identity(virtual_key, scan_code, extended);
    // Windows can deliver a release after polling has already observed it,
    // and external input senders may release an already-neutral modifier.
    // Such an unowned edge is harmless. Do not feed it to the reducer as a
    // second physical transition and permanently retire keyboard capture.
    if phase == KeyPhase::Up && release_is_already_observed(&keyboard, identity) {
        return false;
    }
    let native = NativeKey {
        virtual_key,
        scan_code,
        extended,
        // Shared replay storage is lossless u64; widening preserves the exact
        // Windows u32 LL-hook flags without changing injection behavior.
        platform_flags: u64::from(platform_flags),
    };

    // GetAsyncKeyState can lag a serialized external SendInput edge. The callback
    // edge is authoritative during that bounded publication window; periodic
    // reconciliation runs after the deadline.
    if source != InputSource::External
        && !matches!(identity, KeyIdentity::Modifier(_))
        && let Some(native_modifiers) = observation.native_modifiers
        && native_modifiers.transactional_sides() != keyboard.modifiers.transactional_sides()
    {
        keyboard.modifiers = native_modifiers;
        keyboard.altgr_active =
            conservative_altgr(&native_modifiers, keyboard.altgr_synthetic_ctrl);
        let snapshot = PhysicalSnapshot::new(
            keyboard.physical.held_letter_bits(),
            keyboard.modifiers.transactional_sides(),
            keyboard.altgr_active,
        );
        let outcome =
            begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
        if outcome.is_some_and(|outcome| outcome.shutdown == ShutdownState::Terminal)
            && !context.terminal.is_triggered()
        {
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
        }
    }

    let repeat = match identity {
        KeyIdentity::Modifier(side) => {
            let repeat =
                phase == KeyPhase::Down && keyboard.modifiers.transactional_sides().contains(side);
            keyboard
                .modifiers
                .observe(virtual_key, scan_code, extended, phase);
            let modifiers = keyboard.modifiers.mask();
            keyboard.reducer.observe_modifiers(modifiers);
            repeat
        }
        KeyIdentity::Letter(key) => {
            keyboard
                .physical
                .observe(PhysicalKey::Letter(key), None, phase)
        }
        KeyIdentity::Escape => keyboard.physical.observe(PhysicalKey::Escape, None, phase),
        KeyIdentity::Enter => keyboard.physical.observe(
            PhysicalKey::Enter,
            record_enter_source(virtual_key, scan_code, extended),
            phase,
        ),
        KeyIdentity::Other(_) => false,
    };
    if virtual_key == VK_V {
        keyboard.logical_v_down = phase == KeyPhase::Down;
    }
    let left_control = matches!(identity, KeyIdentity::Modifier(ModifierSide::LeftCtrl));
    let right_alt = matches!(identity, KeyIdentity::Modifier(ModifierSide::RightAlt));
    if left_control && phase == KeyPhase::Up {
        keyboard.altgr_synthetic_ctrl = false;
    }
    if right_alt {
        keyboard.altgr_synthetic_ctrl = phase == KeyPhase::Down && keyboard.modifiers.ctrl.left;
        keyboard.altgr_active = phase == KeyPhase::Down
            && conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
    } else if let Some(native_modifiers) = observation.native_modifiers {
        keyboard.altgr_active =
            conservative_altgr(&native_modifiers, keyboard.altgr_synthetic_ctrl);
    }
    let snapshot = PhysicalSnapshot::new(
        keyboard.physical.held_letter_bits(),
        keyboard.modifiers.transactional_sides(),
        keyboard.altgr_active,
    );
    let physical_phase = match (phase, repeat) {
        (KeyPhase::Down, true) => PhysicalPhase::Repeat,
        (KeyPhase::Down, false) => PhysicalPhase::Down,
        (KeyPhase::Up, _) => PhysicalPhase::Up,
    };

    // Disabled Windows configurations retain a faithful non-owning shadow of
    // the compiled production matcher. A proof is emitted only after an exact
    // match and matching trigger release in one opaque observation generation.
    if !keyboard.activation.enabled {
        let observation_config = keyboard.transactional.config();
        if let Some(generation) = keyboard.registered_observation.observe(
            &context.observability,
            observation_config,
            snapshot,
            identity,
            physical_phase,
        ) {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::PhysicalObserved),
                Ordering::Release,
            );
            if context
                .outbound
                .try_send(NativeEvent::RegisteredObservation { generation })
                .is_ok()
            {
                context.observability.record_callback_channel_accepted();
            } else {
                context.observability.record_callback_channel_rejected();
            }
        }
    } else {
        keyboard.registered_observation.reset();
    }

    if keyboard.transactional.journal_len() != 0 && !validate_candidate_target(&mut keyboard) {
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::Cancel(CancelReason::TargetChanged),
        );
    }

    let starts_candidate = keyboard
        .transactional
        .event_starts_candidate(identity, physical_phase);
    if starts_candidate {
        context.observability.record_registered_candidate_callback();
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::PhysicalObserved),
            Ordering::Release,
        );
    }
    let candidate_preflight = starts_candidate
        .then(|| capture_coherent_candidate_target(keyboard.input_desktop))
        .flatten();
    if let Some((target, desktop, epoch)) = candidate_preflight {
        keyboard.candidate_target = Some(target);
        keyboard.candidate_desktop = Some(desktop);
        keyboard.candidate_target_epoch = Some(epoch);
        keyboard.candidate_target_changed = false;
    }
    let event = NormalizedEvent {
        key: identity,
        phase: physical_phase,
        source,
        native,
        observed_at_ms: observation.observed_at_ms,
        config_revision: keyboard.transactional.config().revision(),
        gate: transaction_gate(context),
        snapshot,
    };
    let turn = if starts_candidate && candidate_preflight.is_none() {
        keyboard
            .transactional
            .clone()
            .pass_uncapturable_event(event)
    } else {
        keyboard
            .transactional
            .clone()
            .begin(EngineInput::Event(event))
    };
    let record = CompletedHookRecord {
        identity,
        virtual_key,
        phase,
        repeat,
        enter_source: record_enter_source(virtual_key, scan_code, extended),
        physical: true,
    };
    let completion =
        drive_transaction_turn(context, &mut keyboard, turn, Some(callback_disposition));
    if let Some(completion) = completion {
        return complete_transaction_event(context, &mut keyboard, completion, record);
    }
    let Some(pending) = keyboard.deferred_callback_replay.as_mut() else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return callback_disposition.get() == CallbackDisposition::Capture;
    };
    pending.record = Some(record);
    if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization)
        && phase == KeyPhase::Up
        && let Some(slot) = menu_modifier_release_slot(identity)
    {
        // Every physical-equivalent Alt/Win release waits for neutralization.
        // Passing an external sender's release here opens the foreground menu
        // before the worker can finish a quick prefix-only shortcut.
        callback_disposition.set(CallbackDisposition::Capture);
        keyboard.deferred_menu_releases[slot] = Some(native);
    }
    // The replay worker is the sole completion-message publisher. Posting here
    // could consume an empty result before SendInput completes.
    callback_disposition.get() == CallbackDisposition::Capture
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
