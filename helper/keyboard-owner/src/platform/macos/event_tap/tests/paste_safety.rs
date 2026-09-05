//! Paste safety contracts.

use super::*;

#[test]
fn paste_final_gate_rejects_every_mutable_safety_transition() {
    let valid = PastePostChecks {
        admission_open: true,
        before_deadline: true,
        before_injection_cutoff: true,
        modifiers_neutral: true,
        permissions_granted: true,
        secure_input_inactive: true,
        target_valid: true,
    };
    let waiting = AtomicU8::new(PasteCommandState::Waiting as u8);
    assert_eq!(paste_check_failure(&waiting, valid), None);
    assert!(!modifier_wait_timed_out(
        PasteFailure::ConflictingModifiers,
        true
    ));
    assert!(modifier_wait_timed_out(
        PasteFailure::ConflictingModifiers,
        false
    ));
    assert!(!modifier_wait_timed_out(PasteFailure::Unavailable, false));

    for (checks, expected) in [
        (
            PastePostChecks {
                admission_open: false,
                ..valid
            },
            PasteFailure::Unavailable,
        ),
        (
            PastePostChecks {
                before_deadline: false,
                ..valid
            },
            PasteFailure::Unavailable,
        ),
        (
            PastePostChecks {
                before_injection_cutoff: false,
                ..valid
            },
            PasteFailure::Unavailable,
        ),
        (
            PastePostChecks {
                modifiers_neutral: false,
                ..valid
            },
            PasteFailure::ConflictingModifiers,
        ),
        (
            PastePostChecks {
                permissions_granted: false,
                ..valid
            },
            PasteFailure::PermissionDenied,
        ),
        (
            PastePostChecks {
                secure_input_inactive: false,
                ..valid
            },
            PasteFailure::SecureInput,
        ),
        (
            PastePostChecks {
                target_valid: false,
                ..valid
            },
            PasteFailure::Unavailable,
        ),
    ] {
        assert_eq!(paste_check_failure(&waiting, checks), Some(expected));
    }
}

#[test]
fn cancellation_during_delayed_target_validation_prevents_late_injection() {
    let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
    let posted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (started_tx, started_rx) = bounded(1);
    let (release_tx, release_rx) = bounded(1);
    let worker_state = Arc::clone(&state);
    let worker_posted = Arc::clone(&posted);
    let validator = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let checks = PastePostChecks {
            admission_open: true,
            before_deadline: true,
            before_injection_cutoff: true,
            modifiers_neutral: true,
            permissions_granted: true,
            secure_input_inactive: true,
            target_valid: true,
        };
        if claim_paste_injection(&worker_state, checks).is_ok() {
            worker_posted.store(true, Ordering::Release);
        }
    });

    started_rx.recv().unwrap();
    assert_eq!(cancel_paste_command(&state), PasteCommandState::Cancelled);
    release_tx.send(()).unwrap();
    validator.join().unwrap();
    assert!(!posted.load(Ordering::Acquire));
    assert_eq!(paste_command_state(&state), PasteCommandState::Cancelled);
}

#[test]
fn gate_or_terminal_transition_during_validation_prevents_post() {
    let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
    let admission_open = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let posted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (started_tx, started_rx) = bounded(1);
    let (release_tx, release_rx) = bounded(1);
    let worker_state = Arc::clone(&state);
    let worker_admission = Arc::clone(&admission_open);
    let worker_posted = Arc::clone(&posted);
    let validator = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let checks = PastePostChecks {
            admission_open: worker_admission.load(Ordering::Acquire),
            before_deadline: true,
            before_injection_cutoff: true,
            modifiers_neutral: true,
            permissions_granted: true,
            secure_input_inactive: true,
            target_valid: true,
        };
        if claim_paste_injection(&worker_state, checks).is_ok() {
            worker_posted.store(true, Ordering::Release);
        }
    });

    started_rx.recv().unwrap();
    admission_open.store(false, Ordering::Release);
    release_tx.send(()).unwrap();
    validator.join().unwrap();
    assert!(!posted.load(Ordering::Acquire));
    assert_eq!(paste_command_state(&state), PasteCommandState::Waiting);
}
