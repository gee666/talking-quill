//! Insertion safety contracts.

use super::*;

#[test]
fn secure_input_or_permission_gap_is_terminal_only_while_ownership_exists() {
    assert!(!owned_native_transition_is_terminal(false, true, true));
    assert!(!owned_native_transition_is_terminal(false, false, false));
    assert!(owned_native_transition_is_terminal(true, true, true));
    assert!(owned_native_transition_is_terminal(true, false, false));
    assert!(!owned_native_transition_is_terminal(true, false, true));
}

#[test]
fn insertion_safety_transition_is_clipboard_only_before_claim() {
    assert!(!insertion_has_irreversible_authority(
        InsertionStatus::Pending
    ));
    assert!(!insertion_has_irreversible_authority(
        InsertionStatus::Failed(PasteFailure::SecureInput)
    ));
    assert!(insertion_has_irreversible_authority(
        InsertionStatus::Claimed
    ));
    assert!(insertion_has_irreversible_authority(
        InsertionStatus::Ambiguous
    ));
}

#[test]
fn insertion_claim_timeout_is_terminal_while_exact_results_are_publishable() {
    assert_eq!(
        insertion_owner_action(InsertionStatus::Claimed, false),
        InsertionOwnerAction::Wait
    );
    assert_eq!(
        insertion_owner_action(InsertionStatus::Claimed, true),
        InsertionOwnerAction::TerminalAmbiguous
    );
    assert_eq!(
        insertion_owner_action(InsertionStatus::Ambiguous, false),
        InsertionOwnerAction::TerminalAmbiguous
    );
    assert_eq!(
        insertion_owner_action(InsertionStatus::Succeeded, true),
        InsertionOwnerAction::CompleteSuccess
    );
    assert_eq!(
        insertion_owner_action(InsertionStatus::Failed(PasteFailure::OsRejected), true,),
        InsertionOwnerAction::CompleteFailure(PasteFailure::OsRejected)
    );
    assert_eq!(
        insertion_owner_action(InsertionStatus::Pending, true),
        InsertionOwnerAction::CancelPending
    );
}
