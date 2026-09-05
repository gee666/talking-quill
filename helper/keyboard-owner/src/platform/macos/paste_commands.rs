//! Paste authority, result publication, and absolute deadlines.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(in crate::platform) enum PasteCommandState {
    Pending,
    Waiting,
    Injecting,
    Committed,
    Applied,
    Cancelled,
}

pub(in crate::platform) struct PasteResultSlot {
    value: AtomicU8,
}

impl PasteResultSlot {
    const PENDING: u8 = 0;
    const SUCCESS: u8 = 1;
    const PERMISSION_DENIED: u8 = 2;
    const CONFLICTING_MODIFIERS: u8 = 3;
    const SECURE_INPUT: u8 = 4;
    const OS_REJECTED: u8 = 5;
    const UNAVAILABLE: u8 = 6;
    const INDETERMINATE: u8 = 7;

    pub(in crate::platform) const fn new() -> Self {
        Self {
            value: AtomicU8::new(Self::PENDING),
        }
    }

    const fn encode(result: PasteResult) -> u8 {
        if result.submitted {
            Self::SUCCESS
        } else {
            match result.reason {
                Some(PasteFailure::PermissionDenied) => Self::PERMISSION_DENIED,
                Some(PasteFailure::ConflictingModifiers) => Self::CONFLICTING_MODIFIERS,
                Some(PasteFailure::SecureInput) => Self::SECURE_INPUT,
                Some(PasteFailure::OsRejected) => Self::OS_REJECTED,
                Some(PasteFailure::Unavailable) | None => Self::UNAVAILABLE,
                Some(PasteFailure::Indeterminate) => Self::INDETERMINATE,
            }
        }
    }

    const fn decode(value: u8) -> Option<PasteResult> {
        match value {
            Self::SUCCESS => Some(PasteResult {
                submitted: true,
                reason: None,
            }),
            Self::PERMISSION_DENIED => Some(failed_paste(PasteFailure::PermissionDenied)),
            Self::CONFLICTING_MODIFIERS => Some(failed_paste(PasteFailure::ConflictingModifiers)),
            Self::SECURE_INPUT => Some(failed_paste(PasteFailure::SecureInput)),
            Self::OS_REJECTED => Some(failed_paste(PasteFailure::OsRejected)),
            Self::UNAVAILABLE => Some(failed_paste(PasteFailure::Unavailable)),
            Self::INDETERMINATE => Some(failed_paste(PasteFailure::Indeterminate)),
            _ => None,
        }
    }

    pub(in crate::platform) fn publish(&self, result: PasteResult) -> PasteResult {
        let proposed = Self::encode(result);
        let value = self
            .value
            .compare_exchange(Self::PENDING, proposed, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|current| current, |_| proposed);
        Self::decode(value).expect("published paste result encoding")
    }

    pub(in crate::platform) fn result(&self) -> Option<PasteResult> {
        Self::decode(self.value.load(Ordering::Acquire))
    }
}

pub(in crate::platform) struct PasteCommand {
    pub(in crate::platform) context: ActivationContext,
    pub(in crate::platform) expected_clipboard_sha256: ClipboardTextHash,
    pub(in crate::platform) state: Arc<AtomicU8>,
    pub(in crate::platform) result: Arc<PasteResultSlot>,
    pub(in crate::platform) acknowledgement: Sender<()>,
    pub(in crate::platform) deadline: Instant,
}

pub(in crate::platform::macos) struct PendingPaste {
    pub(in crate::platform::macos) state: Arc<AtomicU8>,
    pub(in crate::platform::macos) result: Arc<PasteResultSlot>,
    pub(in crate::platform::macos) acknowledgement: Sender<()>,
    pub(in crate::platform::macos) evidence: target::TargetHandle,
    pub(in crate::platform::macos) expected_clipboard_sha256: ClipboardTextHash,
    pub(in crate::platform::macos) validation_request: Option<target::ValidationRequest>,
    pub(in crate::platform::macos) validated_target_epoch: Option<u64>,
    pub(in crate::platform::macos) validated_target_boundary_epoch: Option<u64>,
    pub(in crate::platform::macos) validated_selected_range_epoch: Option<u64>,
    pub(in crate::platform::macos) insertion_request: Option<target::InsertionRequest>,
    pub(in crate::platform::macos) neutral_modifier_epoch: Option<u64>,
    pub(in crate::platform::macos) neutral_barrier_state: u8,
    pub(in crate::platform::macos) neutral_barrier_token: Option<injection::OperationToken>,
    pub(in crate::platform::macos) deadline: Instant,
    pub(in crate::platform::macos) injection_cutoff: Instant,
    pub(in crate::platform::macos) modifier_wait: ModifierNeutralWait,
}

pub(in crate::platform) fn paste_command_state(state: &AtomicU8) -> PasteCommandState {
    match state.load(Ordering::Acquire) {
        1 => PasteCommandState::Waiting,
        2 => PasteCommandState::Injecting,
        3 => PasteCommandState::Committed,
        4 => PasteCommandState::Applied,
        5 => PasteCommandState::Cancelled,
        _ => PasteCommandState::Pending,
    }
}

pub(in crate::platform) fn cancel_paste_command(state: &AtomicU8) -> PasteCommandState {
    loop {
        let current = paste_command_state(state);
        if !matches!(
            current,
            PasteCommandState::Pending | PasteCommandState::Waiting
        ) {
            return current;
        }
        if state
            .compare_exchange(
                current as u8,
                PasteCommandState::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return PasteCommandState::Cancelled;
        }
    }
}

pub(in crate::platform) fn paste_before_deadline(deadline: Instant, now: Instant) -> bool {
    now < deadline
}

pub(in crate::platform) fn paste_injection_cutoff(deadline: Instant) -> Instant {
    deadline - PASTE_FINAL_ACK_MARGIN
}

pub(super) fn await_paste_result(
    result: &PasteResultSlot,
    acknowledgement: &Receiver<()>,
    deadline: Instant,
) -> Option<PasteResult> {
    if let Some(result) = result.result() {
        return Some(result);
    }
    let _ = acknowledgement.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    result.result()
}

pub(in crate::platform) const fn failed_paste(reason: PasteFailure) -> PasteResult {
    PasteResult {
        submitted: false,
        reason: Some(reason),
    }
}
