//! Normalize physical transitions and drive transactional/session capture.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct CallbackEvent {
    pub(super) event_ref: ffi::CGEventRef,
    pub(super) marker: i64,
    pub(super) event_type: u32,
    pub(super) key_code: u16,
    pub(super) native_repeat: bool,
    pub(super) flags: u64,
    pub(super) event_timestamp: u64,
    pub(super) source: InputSource,
    pub(super) source_pid: i64,
    pub(super) test_physical_source: bool,
}

pub(super) fn process_transactional_event(
    context: &CallbackContext,
    event: CallbackEvent,
    activation_reservation: Option<ActivationReservation>,
) -> bool {
    let CallbackEvent {
        event_ref,
        marker,
        event_type,
        key_code,
        native_repeat,
        flags,
        event_timestamp,
        source,
        source_pid,
        test_physical_source: _,
    } = event;
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return false;
        }
    };

    if source.is_physical()
        && matches!(
            event_type,
            ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
        )
    {
        let gap_phase = if event_type == ffi::K_CG_EVENT_KEY_UP {
            KeyPhase::Up
        } else {
            KeyPhase::Down
        };
        if keyboard.handle_gap_tombstone(key_code, gap_phase, native_repeat) {
            // A native snapshot proved this edge belongs to the pre-gap held
            // key. Capture queued repeats without mutating native state, and
            // capture the delayed old up while keeping the tracker released.
            // Only a nonrepeat fresh down retires and continues normally.
            return true;
        }
    }

    let candidate_start = source.is_physical()
        && keyboard.transactional.event_starts_candidate(
            transactional_key_identity(key_code),
            if event_type == ffi::K_CG_EVENT_KEY_UP {
                PhysicalPhase::Up
            } else if native_repeat {
                PhysicalPhase::Repeat
            } else {
                PhysicalPhase::Down
            },
        );
    if candidate_start && let Some(reservation) = activation_reservation {
        keyboard.candidate_target_captured = true;
        keyboard.candidate_target = Some(reservation);
        keyboard.owned_down_records = [None; 26];
        if let KeyIdentity::Letter(letter) = transactional_key_identity(key_code) {
            keyboard.owned_down_records[usize::from(letter.index())] = Some(ReplayRecord {
                key: KeyIdentity::Letter(letter),
                native: NativeKey {
                    virtual_key: key_code,
                    scan_code: u32::from(key_code),
                    extended: false,
                    platform_flags: flags,
                },
                phase: PhysicalPhase::Down,
                observed_at_ms: event_timestamp / 1_000_000,
            });
        }
    }

    let secure_transition = source.is_physical() && secure_input_active();
    let event_modifiers = modifier_mask_from_flags(flags);
    let (key, phase, session_phase) = match normalize_transition(
        context,
        &mut keyboard,
        event,
        secure_transition,
        event_modifiers,
    ) {
        Ok(transition) => transition,
        Err(captured) => return captured,
    };

    let permission_transition = source.is_physical()
        && matches!(key, KeyIdentity::Letter(_))
        && phase == PhysicalPhase::Down
        && keyboard.transactional.config().enabled()
        && !native_permissions_available();
    let native_transition = secure_transition || permission_transition;

    let snapshot = physical_snapshot(&keyboard);
    let gate = if context.gate.is_open() && !native_transition {
        GateState::Open
    } else {
        GateState::Closed
    };
    let current_revision = keyboard.transactional.config().revision();
    let config_revision = if source.is_physical()
        && keyboard.activation_revision_at != 0
        && event_timestamp <= keyboard.activation_revision_at
    {
        ConfigRevision::new(current_revision.get().saturating_sub(1))
    } else {
        current_revision
    };
    let event = NormalizedEvent {
        key,
        phase,
        source,
        native: NativeKey {
            virtual_key: key_code,
            scan_code: u32::from(key_code),
            extended: false,
            platform_flags: flags,
        },
        observed_at_ms: event_timestamp / 1_000_000,
        config_revision,
        gate,
        snapshot,
    };
    // Preserve the authoritative engine across every effect. Only a completed
    // turn commits the cloned successor; callback unwind retains ownership.
    let turn = if candidate_start && activation_reservation.is_none() {
        keyboard
            .transactional
            .clone()
            .pass_uncapturable_event(event)
    } else {
        begin_transaction_snapshot(&keyboard.transactional, EngineInput::Event(event))
    };
    if turn.event_disposition_hint() == Some(EventDisposition::CaptureCurrent) {
        set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Owned);
        if source.is_physical()
            && phase == PhysicalPhase::Down
            && let KeyIdentity::Letter(letter) = key
        {
            keyboard.owned_down_records[usize::from(letter.index())] = Some(ReplayRecord {
                key,
                native: event.native,
                phase: PhysicalPhase::Down,
                observed_at_ms: event.observed_at_ms,
            });
        }
    }
    let completion = drive_transaction_turn(context, &mut keyboard, turn, activation_reservation);
    let outcome = match completion {
        DriveCompletion::Complete(Completion::Event(outcome)) => outcome,
        DriveCompletion::NativeObservationPending => {
            // No later physical/external callback may overtake the HID replay.
            // Physical current is already the reserved final replay record;
            // external current is copied into the fixed deferred scalar queue.
            drop(keyboard);
            context
                .state
                .recovery_deferred_mode
                .store(true, Ordering::Release);
            if !source.is_physical() && !event_ref.is_null() {
                let _ = defer_callback_edge(
                    context, event_type, event_ref, source, source_pid, marker, false,
                );
            }
            return true;
        }
        _ => {
            // Activation delivery and physical replay retain the current edge
            // through an installed continuation/replay record.
            return true;
        }
    };
    if native_transition {
        // Existing Escape/Enter ownership may still consume this exact up, but
        // no fresh session down may be admitted during the transition.
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    }
    let transaction_captured = outcome.disposition == EventDisposition::CaptureCurrent;
    if source.is_physical()
        && transaction_captured
        && let KeyIdentity::Letter(letter) = event.key
    {
        let slot = usize::from(letter.index());
        match event.phase {
            PhysicalPhase::Down => {
                keyboard.owned_down_records[slot] = Some(ReplayRecord {
                    key: event.key,
                    native: event.native,
                    phase: PhysicalPhase::Down,
                    observed_at_ms: event.observed_at_ms,
                });
            }
            PhysicalPhase::Up => keyboard.owned_down_records[slot] = None,
            PhysicalPhase::Repeat => {}
        }
    }
    set_current_edge_disposition(
        context,
        &mut keyboard,
        if transaction_captured {
            CurrentEdgeDisposition::Owned
        } else {
            CurrentEdgeDisposition::Pass
        },
    );
    let session_captured = !transaction_captured
        && source.is_physical()
        && session_phase.is_some_and(|(key_phase, repeat)| {
            process_session_event(
                context,
                &mut keyboard,
                key_code,
                key_phase,
                repeat,
                event_timestamp,
            )
        });
    if native_transition {
        finish_native_transition(context, &mut keyboard);
    } else if outcome.terminal && context.gate.is_open() && !context.terminal.is_triggered() {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }

    if keyboard.transactional.journal_len() != 0
        || keyboard.transactional.owned_letters() != 0
        || has_session_ownership(&keyboard)
    {
        arm_maintenance_timer(context);
    }

    let captured = transaction_captured || session_captured;
    set_current_edge_disposition(
        context,
        &mut keyboard,
        if captured {
            CurrentEdgeDisposition::Owned
        } else {
            CurrentEdgeDisposition::Pass
        },
    );
    let shutdown_may_drain =
        context.state.stopping.load(Ordering::Acquire) && keyboard.shutdown_requested;
    drop(keyboard);
    if shutdown_may_drain {
        let _ = stop_owner_run_loop_if_drained(context);
    }
    captured
}
