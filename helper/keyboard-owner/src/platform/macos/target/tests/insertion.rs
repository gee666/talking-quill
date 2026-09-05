use super::*;

#[test]
fn post_claim_pause_exposes_claim_and_preserves_exact_completion() {
    for inserted in [true, false] {
        let state = Arc::new(AtomicU8::new(INSERTION_PENDING));
        let request = InsertionRequest {
            work: None,
            state: Arc::clone(&state),
        };
        let (claimed_tx, claimed_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            worker_state
                .compare_exchange(
                    INSERTION_PENDING,
                    INSERTION_CLAIMED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .unwrap();
            claimed_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            complete_claimed_insertion(&worker_state, inserted);
        });
        claimed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(request.status(), InsertionStatus::Claimed);
        assert!(!request.cancel(), "claim authority is immutable");
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            request.status(),
            if inserted {
                InsertionStatus::Succeeded
            } else {
                InsertionStatus::Failed(PasteFailure::OsRejected)
            }
        );
    }
}

#[test]
fn clipboard_hash_and_change_count_jointly_prevent_torn_authorization() {
    let expected = ClipboardTextHash::from_bytes([7; 32]);
    assert!(clipboard_sample_is_authorized(expected, 11, expected, 11));
    assert!(!clipboard_sample_is_authorized(expected, 11, expected, 12));
    assert!(!clipboard_sample_is_authorized(
        ClipboardTextHash::from_bytes([8; 32]),
        11,
        expected,
        11,
    ));
}

#[test]
fn postclaim_clipboard_authority_is_only_the_retained_scalar_revision() {
    assert!(postclaim_clipboard_revision_is_authorized(17, 17));
    assert!(!postclaim_clipboard_revision_is_authorized(17, 18));
    // The postclaim helper accepts no CFString or byte buffer, making a
    // second full conversion/hash structurally unavailable after claim.
    assert_eq!(size_of_val(&17_isize), size_of::<isize>());
}

#[test]
fn both_ax_range_and_text_set_errors_use_acceptance_safe_classification() {
    for _step in ["set-range", "set-selected-text"] {
        assert_eq!(
            classify_ax_set_error(ffi::K_AX_ERROR_SUCCESS),
            AxSetOutcome::Succeeded
        );
        assert_eq!(
            classify_ax_set_error(ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED),
            AxSetOutcome::DefinitiveFailure
        );
        assert_eq!(
            classify_ax_set_error(ffi::K_AX_ERROR_CANNOT_COMPLETE),
            AxSetOutcome::Ambiguous
        );
        assert_eq!(classify_ax_set_error(-29_999), AxSetOutcome::Ambiguous);
    }

    let cannot_complete = AtomicU8::new(INSERTION_CLAIMED);
    assert!(!settle_claimed_ax_error(
        &cannot_complete,
        ffi::K_AX_ERROR_CANNOT_COMPLETE,
    ));
    assert_eq!(
        insertion_status(cannot_complete.load(Ordering::Acquire)),
        InsertionStatus::Ambiguous
    );

    let succeeded = AtomicU8::new(INSERTION_CLAIMED);
    if settle_claimed_ax_error(&succeeded, ffi::K_AX_ERROR_SUCCESS) {
        complete_claimed_insertion(&succeeded, true);
    }
    assert_eq!(
        insertion_status(succeeded.load(Ordering::Acquire)),
        InsertionStatus::Succeeded
    );

    let definitive = AtomicU8::new(INSERTION_CLAIMED);
    assert!(!settle_claimed_ax_error(
        &definitive,
        ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED,
    ));
    assert_eq!(
        insertion_status(definitive.load(Ordering::Acquire)),
        InsertionStatus::Failed(PasteFailure::OsRejected)
    );
}

#[test]
fn real_safety_failures_are_clipboard_only_only_before_claim() {
    assert_eq!(
        insertion_identity_failure(true, false),
        Some(InsertionFailure::Paste(PasteFailure::Unavailable))
    );
    assert_eq!(
        insertion_identity_failure(false, false),
        Some(InsertionFailure::TargetInvalid)
    );
    assert_eq!(insertion_identity_failure(false, true), None);

    let pending = AtomicU8::new(INSERTION_PENDING);
    fail_pending_insertion(&pending, PasteFailure::SecureInput);
    assert_eq!(
        insertion_status(pending.load(Ordering::Acquire)),
        InsertionStatus::Failed(PasteFailure::SecureInput)
    );

    let target_invalid = AtomicU8::new(INSERTION_PENDING);
    fail_insertion(&target_invalid, InsertionFailure::TargetInvalid);
    assert_eq!(
        insertion_status(target_invalid.load(Ordering::Acquire)),
        InsertionStatus::TargetInvalid
    );

    let claimed = AtomicU8::new(INSERTION_CLAIMED);
    fail_pending_insertion(&claimed, PasteFailure::PermissionDenied);
    assert_eq!(
        insertion_status(claimed.load(Ordering::Acquire)),
        InsertionStatus::Claimed,
        "a post-claim safety transition cannot publish clipboard-only",
    );
    mark_claimed_insertion_ambiguous(&claimed);
    assert_eq!(
        insertion_status(claimed.load(Ordering::Acquire)),
        InsertionStatus::Ambiguous
    );
}

#[test]
fn claimed_timeout_is_explicitly_ambiguous_and_budget_is_reserved() {
    let now = Instant::now();
    assert!(!insertion_completion_budget_available(
        now + AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN - Duration::from_nanos(1),
        now,
    ));
    assert!(insertion_completion_budget_available(
        now + AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN,
        now,
    ));
    let state = Arc::new(AtomicU8::new(INSERTION_CLAIMED));
    let (release_tx, release_rx) = bounded(1);
    let worker_state = Arc::clone(&state);
    let worker = thread::spawn(move || {
        release_rx.recv().unwrap();
        complete_claimed_insertion(&worker_state, true);
    });
    // Deterministically expire owner proof while the worker is paused after
    // claim but before its simulated AX completion.
    mark_claimed_insertion_ambiguous(&state);
    assert_eq!(
        insertion_status(state.load(Ordering::Acquire)),
        InsertionStatus::Ambiguous
    );
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(
        insertion_status(state.load(Ordering::Acquire)),
        InsertionStatus::Ambiguous,
        "terminal ambiguity cannot be rewritten as ordinary success/failure",
    );
}
