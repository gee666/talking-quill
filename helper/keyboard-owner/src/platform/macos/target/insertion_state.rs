//! Insertion claim, cancellation, and acceptance-safe terminal states.

use super::*;

pub(super) const INSERTION_PENDING: u8 = 0;
pub(super) const INSERTION_CLAIMED: u8 = 1;
pub(super) const INSERTION_SUCCEEDED: u8 = 2;
pub(super) const INSERTION_FAILED_UNAVAILABLE: u8 = 3;
pub(super) const INSERTION_FAILED_PERMISSION: u8 = 4;
pub(super) const INSERTION_FAILED_SECURE_INPUT: u8 = 5;
pub(super) const INSERTION_FAILED_OS_REJECTED: u8 = 6;
pub(super) const INSERTION_CANCELLED: u8 = 7;
pub(super) const INSERTION_AMBIGUOUS: u8 = 8;
pub(super) const INSERTION_TARGET_INVALID: u8 = 9;

pub(super) struct InsertionWork {
    pub(super) publication_id: u64,
    pub(super) notification_epoch: u64,
    pub(super) boundary_epoch: u64,
    pub(super) selected_range_epoch: u64,
    pub(super) expected_clipboard_sha256: ClipboardTextHash,
    pub(super) deadline: Instant,
    pub(super) state: Arc<AtomicU8>,
}

pub(in crate::platform::macos) struct InsertionRequest {
    pub(super) work: Option<InsertionWork>,
    pub(super) state: Arc<AtomicU8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::macos) enum InsertionStatus {
    Pending,
    Claimed,
    Succeeded,
    Failed(PasteFailure),
    TargetInvalid,
    Ambiguous,
}

pub(super) fn insertion_status(state: u8) -> InsertionStatus {
    match state {
        INSERTION_CLAIMED => InsertionStatus::Claimed,
        INSERTION_SUCCEEDED => InsertionStatus::Succeeded,
        INSERTION_FAILED_PERMISSION => InsertionStatus::Failed(PasteFailure::PermissionDenied),
        INSERTION_FAILED_SECURE_INPUT => InsertionStatus::Failed(PasteFailure::SecureInput),
        INSERTION_FAILED_OS_REJECTED => InsertionStatus::Failed(PasteFailure::OsRejected),
        INSERTION_FAILED_UNAVAILABLE | INSERTION_CANCELLED => {
            InsertionStatus::Failed(PasteFailure::Unavailable)
        }
        INSERTION_AMBIGUOUS => InsertionStatus::Ambiguous,
        INSERTION_TARGET_INVALID => InsertionStatus::TargetInvalid,
        _ => InsertionStatus::Pending,
    }
}

pub(super) fn insertion_failure_state(reason: PasteFailure) -> u8 {
    match reason {
        PasteFailure::PermissionDenied => INSERTION_FAILED_PERMISSION,
        PasteFailure::SecureInput => INSERTION_FAILED_SECURE_INPUT,
        PasteFailure::OsRejected => INSERTION_FAILED_OS_REJECTED,
        PasteFailure::ConflictingModifiers
        | PasteFailure::Unavailable
        | PasteFailure::Indeterminate => INSERTION_FAILED_UNAVAILABLE,
    }
}

pub(super) fn insertion_completion_budget_available(deadline: Instant, now: Instant) -> bool {
    deadline.saturating_duration_since(now) >= AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN
}

impl InsertionRequest {
    pub(in crate::platform::macos) fn status(&self) -> InsertionStatus {
        insertion_status(self.state.load(Ordering::Acquire))
    }

    pub(super) fn submit(&mut self, sender: &Sender<InsertionWork>) -> bool {
        let Some(work) = self.work.take() else {
            return false;
        };
        match sender.try_send(work) {
            Ok(()) => true,
            Err(error) => {
                self.work = Some(error.into_inner());
                false
            }
        }
    }

    pub(in crate::platform::macos) fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                INSERTION_PENDING,
                INSERTION_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Drop for InsertionRequest {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) fn fail_insertion(state: &AtomicU8, failure: InsertionFailure) {
    match failure {
        InsertionFailure::TargetInvalid => {
            let _ = state.compare_exchange(
                INSERTION_PENDING,
                INSERTION_TARGET_INVALID,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        InsertionFailure::Paste(reason) => fail_pending_insertion(state, reason),
    }
}

pub(super) fn fail_pending_insertion(state: &AtomicU8, reason: PasteFailure) {
    let _ = state.compare_exchange(
        INSERTION_PENDING,
        insertion_failure_state(reason),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

pub(super) fn mark_claimed_insertion_ambiguous(state: &AtomicU8) {
    let _ = state.compare_exchange(
        INSERTION_CLAIMED,
        INSERTION_AMBIGUOUS,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

pub(super) fn complete_claimed_insertion(state: &AtomicU8, inserted: bool) {
    let _ = state.compare_exchange(
        INSERTION_CLAIMED,
        if inserted {
            INSERTION_SUCCEEDED
        } else {
            INSERTION_FAILED_OS_REJECTED
        },
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AxSetOutcome {
    Succeeded,
    DefinitiveFailure,
    Ambiguous,
}

pub(super) fn classify_ax_set_error(error: i32) -> AxSetOutcome {
    match error {
        ffi::K_AX_ERROR_SUCCESS => AxSetOutcome::Succeeded,
        ffi::K_AX_ERROR_ILLEGAL_ARGUMENT
        | ffi::K_AX_ERROR_INVALID_UI_ELEMENT
        | ffi::K_AX_ERROR_ATTRIBUTE_UNSUPPORTED
        | ffi::K_AX_ERROR_API_DISABLED => AxSetOutcome::DefinitiveFailure,
        // kAXErrorCannotComplete is the documented result when messaging the
        // target times out. Unknown results also lack acceptance proof.
        ffi::K_AX_ERROR_CANNOT_COMPLETE => AxSetOutcome::Ambiguous,
        _ => AxSetOutcome::Ambiguous,
    }
}

pub(super) fn settle_claimed_ax_error(state: &AtomicU8, error: i32) -> bool {
    match classify_ax_set_error(error) {
        AxSetOutcome::Succeeded => true,
        AxSetOutcome::DefinitiveFailure => {
            complete_claimed_insertion(state, false);
            false
        }
        AxSetOutcome::Ambiguous => {
            mark_claimed_insertion_ambiguous(state);
            false
        }
    }
}
