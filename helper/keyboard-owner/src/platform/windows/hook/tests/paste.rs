use super::*;

#[test]
fn paste_target_evidence_starts_closed_until_all_native_hooks_install() {
    let state = SharedState::new();
    assert!(!state.target_change_evidence_ready.load(Ordering::Acquire));
}

#[test]
fn paste_result_slot_is_the_only_irreversible_commit_authority() {
    let success = PasteResultSlot::new();
    assert!(success.result().is_none());
    assert!(
        success
            .publish(PasteResult {
                submitted: true,
                reason: None,
            })
            .submitted,
    );
    assert!(
        success
            .publish(failed_paste(PasteFailure::Unavailable))
            .submitted,
        "a later disconnect/failure cannot revoke accepted SendInput",
    );

    let failure = PasteResultSlot::new();
    assert!(
        !failure
            .publish(failed_paste(PasteFailure::OsRejected))
            .submitted
    );
    assert!(
        !failure
            .publish(PasteResult {
                submitted: true,
                reason: None,
            })
            .submitted,
        "state alone cannot fabricate submitted:true after rejection",
    );
}

#[test]
fn caller_timeout_wins_the_final_waiting_to_injecting_race() {
    let state = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert_eq!(cancel_paste_command(&state), PasteCommandState::Cancelled);
    assert!(!claim_paste_injection(&state));
    assert_eq!(paste_command_state(&state), PasteCommandState::Cancelled);
}

#[test]
fn post_cas_completion_wait_is_bounded_and_reports_indeterminate() {
    let (_sender, response) = bounded::<()>(1);
    let result = PasteResultSlot::new();
    let started = Instant::now();
    assert_eq!(
        wait_for_claimed_paste_completion(&response, &result, Duration::from_millis(5)),
        failed_paste(PasteFailure::Indeterminate)
    );
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn final_injection_claim_wins_the_cancellation_race_authoritatively() {
    let state = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert!(claim_paste_injection(&state));
    assert_eq!(cancel_paste_command(&state), PasteCommandState::Injecting);
    assert_eq!(paste_command_state(&state), PasteCommandState::Injecting);
}

#[test]
fn exact_send_input_rejection_leaves_injecting_before_cleanup() {
    let state = AtomicU8::new(PasteCommandState::Injecting as u8);
    let result = PasteResultSlot::new();
    let mut keyboard = CallbackKeyboard::default();
    let published = publish_initial_paste_acceptance(
        &state,
        &result,
        &mut keyboard,
        injection::test_paste_initial_outcome(injection::InjectionMarkers::generate().unwrap(), 0),
        || {},
    );
    assert!(!published.submitted);
    assert_eq!(paste_command_state(&state), PasteCommandState::ResultReady);
    assert!(keyboard.pending_paste_cleanup.is_empty());
}

#[test]
fn paste_commit_is_published_before_cleanup_pause() {
    let state = AtomicU8::new(PasteCommandState::Injecting as u8);
    let result = PasteResultSlot::new();
    let mut keyboard = CallbackKeyboard::default();
    let mut paused = false;

    let published = publish_initial_paste_acceptance(
        &state,
        &result,
        &mut keyboard,
        injection::test_paste_initial_outcome(injection::InjectionMarkers::generate().unwrap(), 2),
        || {
            paused = true;
            assert!(result.result().is_some_and(|result| result.submitted));
            assert_eq!(paste_command_state(&state), PasteCommandState::Committed);
        },
    );

    assert!(paused);
    assert!(published.submitted);
    assert!(!keyboard.pending_paste_cleanup.is_empty());
    assert!(
        result
            .publish(failed_paste(PasteFailure::Unavailable))
            .submitted,
        "cleanup failure after the pause cannot revoke commitment",
    );
}

#[test]
fn paste_deadline_distinguishes_modifier_wait_from_other_native_work() {
    assert_eq!(
        paste_deadline_reason(false),
        PasteFailure::ConflictingModifiers
    );
    assert_eq!(paste_deadline_reason(true), PasteFailure::Unavailable);
}

#[test]
fn stale_paste_context_counts_target_fallback_without_exposing_evidence() {
    let (context, _outbound, _terminal) = test_context(8);
    let (sender, receiver) = bounded(1);
    let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
    let result = Arc::new(PasteResultSlot::new());
    let (acknowledgement, _acknowledged) = bounded(1);
    sender
        .send(PasteCommand {
            context: activation_context(),
            expected_clipboard_sha256: ClipboardTextHash::from_bytes([0; 32]),
            injection_deadline: Instant::now() + Duration::from_secs(1),
            state,
            result: Arc::clone(&result),
            acknowledgement,
        })
        .unwrap();
    let mut pending = None;

    process_paste_commands(&context, &receiver, &mut pending);

    assert_eq!(
        result.result(),
        Some(failed_paste(PasteFailure::Unavailable))
    );
    assert_eq!(
        context
            .observability
            .snapshot()
            .native_paste
            .target_validation_fallbacks,
        1
    );
}
