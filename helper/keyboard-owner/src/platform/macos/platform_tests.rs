use super::*;

#[test]
fn startup_handoff_is_cancel_or_run_exclusive() {
    let cancelled = AtomicU8::new(StartupState::Pending as u8);
    assert_eq!(cancel_startup(&cancelled), StartupState::Cancelled);
    assert!(!claim_startup(&cancelled));

    let running = AtomicU8::new(StartupState::Pending as u8);
    assert!(claim_startup(&running));
    assert_eq!(cancel_startup(&running), StartupState::Running);
}

#[test]
fn permission_required_poll_never_requests_owner_suspension() {
    let denied = Permissions {
        accessibility: crate::platform::PermissionState::Denied,
        input_monitoring: crate::platform::PermissionState::Denied,
        event_post: crate::platform::PermissionState::Denied,
    };
    assert!(!permission_poll_requires_suspension(
        HookStatus::PermissionRequired,
        denied,
    ));
    assert!(permission_poll_requires_suspension(
        HookStatus::InstalledUnobserved,
        denied,
    ));
}

#[test]
fn permission_required_no_tap_completion_is_cleanly_detected() {
    let (completed_tx, completed_rx) = bounded(1);
    let thread = std::thread::spawn(move || {
        let _ = completed_tx.try_send(());
    });
    while !thread.is_finished() {
        std::thread::yield_now();
    }
    assert!(owner_is_already_quiescent(&completed_rx, &thread));
    assert!(thread.join().is_ok());
}

#[test]
fn owner_completion_wait_is_bounded_and_accepts_cleanup() {
    let (completed_tx, completed_rx) = bounded(1);
    drop(OwnerCompletion(completed_tx));
    assert!(owner_completed(&completed_rx, Duration::from_millis(1)));

    let (_pending_tx, pending_rx) = bounded(1);
    assert!(!owner_completed(&pending_rx, Duration::from_millis(1)));
}

#[test]
fn stop_request_is_nonblocking_when_startup_resources_are_unpublished() {
    let state = Arc::new(SharedState::new());
    event_tap::request_stop(&state);
    assert!(state.quiescing.load(Ordering::Acquire));
    assert!(state.stopping.load(Ordering::Acquire));
    assert_eq!(
        SessionCaptureMode::from_u8(state.session_capture_mode.load(Ordering::Acquire)),
        SessionCaptureMode::Off,
    );
    assert_eq!(
        signal_owner_endpoint(&state),
        OwnerSignalOutcome::Unavailable
    );
}

#[test]
fn owner_signal_has_no_queue_or_lock_full_state() {
    let state = SharedState::new();
    for _ in 0..32 {
        assert_eq!(
            signal_owner_endpoint(&state),
            OwnerSignalOutcome::Unavailable
        );
    }
    assert!(!state.stopping.load(Ordering::Acquire));
}

#[test]
fn full_owner_queue_does_not_prevent_nonblocking_stop_fallback() {
    let state = Arc::new(SharedState::new());
    let (commands, receiver) = bounded(1);
    let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
    let (acknowledgement, _response) = bounded(1);
    commands
        .try_send(OwnerCommand {
            mutation: OwnerMutation::suspend_native_input(),
            state: Arc::clone(&command_state),
            acknowledgement,
        })
        .unwrap();
    let (acknowledgement, _response) = bounded(1);
    assert!(
        commands
            .try_send(OwnerCommand {
                mutation: OwnerMutation::suspend_native_input(),
                state: Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8)),
                acknowledgement,
            })
            .is_err()
    );
    event_tap::request_stop(&state);
    assert!(state.stopping.load(Ordering::Acquire));
    assert_eq!(
        signal_owner_endpoint(&state),
        OwnerSignalOutcome::Unavailable
    );
    drop(receiver);
}

#[test]
fn suspension_enqueue_failure_is_terminal_and_closes_gate() {
    let state = Arc::new(SharedState::new());
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (terminal_tx, _terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (owner_commands, owner_receiver) = bounded(1);
    drop(owner_receiver);
    let (paste_commands, _paste_receiver) = bounded(1);
    let (_completion_tx, owner_completion) = bounded(1);
    let thread = std::thread::spawn(|| {});
    let mut platform = NativePlatform {
        state,
        gate: Arc::clone(&gate),
        terminal: Arc::clone(&terminal),
        observability: Arc::new(TransactionObservability::new()),
        owner_commands,
        paste_commands,
        owner_completion,
        thread: Some(thread),
        shutdown_observability_quiescent: true,
        capture_gate: ActivationCaptureGate::open_for_test_harness(),
    };

    assert!(platform.suspend_native_input().is_err());
    assert!(!gate.is_open());
    assert_eq!(
        terminal.reason(),
        Some(TerminalReason::OwnerThreadUnresponsive)
    );
    if let Some(thread) = platform.thread.take() {
        thread.join().unwrap();
    }
}

#[test]
fn owner_command_handoff_is_cancel_or_apply_exclusive() {
    let cancelled = AtomicU8::new(OwnerCommandState::Pending as u8);
    assert_eq!(
        cancel_owner_command(&cancelled),
        OwnerCommandState::Cancelled
    );
    assert!(!claim_owner_command(&cancelled));

    let applying = AtomicU8::new(OwnerCommandState::Pending as u8);
    assert!(claim_owner_command(&applying));
    assert_eq!(cancel_owner_command(&applying), OwnerCommandState::Applying);
}

#[test]
fn one_absolute_paste_deadline_covers_every_delayed_stage_without_reset() {
    let start = Instant::now();
    let deadline = start + PASTE_COMMAND_TIMEOUT;
    for cumulative_delay in [100, 450, 900, 1_400, 1_999] {
        assert!(paste_before_deadline(
            deadline,
            start + Duration::from_millis(cumulative_delay)
        ));
    }
    assert!(!paste_before_deadline(
        deadline,
        start + PASTE_COMMAND_TIMEOUT
    ));
    assert!(!paste_before_deadline(
        deadline,
        start + PASTE_COMMAND_TIMEOUT + Duration::from_millis(1)
    ));
}

#[test]
fn final_paste_cutoff_reserves_acknowledgement_and_writer_margin() {
    let start = Instant::now();
    let deadline = start + PASTE_COMMAND_TIMEOUT;
    let cutoff = paste_injection_cutoff(deadline);
    assert_eq!(deadline.duration_since(cutoff), PASTE_FINAL_ACK_MARGIN);
    assert!(paste_before_deadline(
        cutoff,
        cutoff - Duration::from_nanos(1)
    ));
    assert!(!paste_before_deadline(cutoff, cutoff));
    assert!(paste_before_deadline(deadline, cutoff));
}

#[test]
fn paste_result_slot_linearizes_only_authoritative_preclaim_or_completion_results() {
    let success_first = PasteResultSlot::new();
    assert!(
        success_first
            .publish(PasteResult {
                submitted: true,
                reason: None,
            })
            .submitted
    );
    assert!(
        success_first
            .publish(failed_paste(PasteFailure::OsRejected))
            .submitted
    );

    let rejected_before_claim = PasteResultSlot::new();
    assert_eq!(
        rejected_before_claim.publish(failed_paste(PasteFailure::OsRejected)),
        failed_paste(PasteFailure::OsRejected)
    );
    assert_eq!(
        rejected_before_claim.publish(PasteResult {
            submitted: true,
            reason: None,
        }),
        failed_paste(PasteFailure::OsRejected)
    );
}

#[test]
fn committed_paste_cannot_be_cancelled_while_matching_up_drains() {
    let committed = AtomicU8::new(PasteCommandState::Committed as u8);
    assert_eq!(
        cancel_paste_command(&committed),
        PasteCommandState::Committed
    );
    assert_eq!(
        paste_command_state(&committed),
        PasteCommandState::Committed
    );
}

#[test]
fn paste_command_handoff_cancels_only_before_owner_injection() {
    let pending = AtomicU8::new(PasteCommandState::Pending as u8);
    assert_eq!(cancel_paste_command(&pending), PasteCommandState::Cancelled);
    assert_eq!(paste_command_state(&pending), PasteCommandState::Cancelled);

    let waiting = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert_eq!(cancel_paste_command(&waiting), PasteCommandState::Cancelled);

    for state in [
        PasteCommandState::Injecting,
        PasteCommandState::Committed,
        PasteCommandState::Applied,
    ] {
        let value = AtomicU8::new(state as u8);
        assert_eq!(cancel_paste_command(&value), state);
    }
}
