//! Paste command arbitration and first-result publication.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum PasteCommandState {
    Pending,
    Waiting,
    Injecting,
    Committed,
    Applied,
    Cancelled,
    ResultReady,
}

pub(super) struct PasteResultSlot {
    pub(super) value: AtomicU8,
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

    pub(super) const fn new() -> Self {
        Self {
            value: AtomicU8::new(Self::PENDING),
        }
    }

    pub(super) const fn encode(result: PasteResult) -> u8 {
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

    pub(super) const fn decode(value: u8) -> Option<PasteResult> {
        match value {
            1 => Some(PasteResult {
                submitted: true,
                reason: None,
            }),
            2 => Some(failed_paste(PasteFailure::PermissionDenied)),
            3 => Some(failed_paste(PasteFailure::ConflictingModifiers)),
            4 => Some(failed_paste(PasteFailure::SecureInput)),
            5 => Some(failed_paste(PasteFailure::OsRejected)),
            6 => Some(failed_paste(PasteFailure::Unavailable)),
            7 => Some(failed_paste(PasteFailure::Indeterminate)),
            _ => None,
        }
    }

    pub(super) fn publish(&self, result: PasteResult) -> PasteResult {
        let proposed = Self::encode(result);
        let value = self
            .value
            .compare_exchange(Self::PENDING, proposed, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|current| current, |_| proposed);
        Self::decode(value).expect("published paste result encoding")
    }

    pub(super) fn result(&self) -> Option<PasteResult> {
        Self::decode(self.value.load(Ordering::Acquire))
    }
}

pub(super) struct PasteCommand {
    pub(super) context: ActivationContext,
    pub(super) expected_clipboard_sha256: ClipboardTextHash,
    pub(super) injection_deadline: Instant,
    pub(super) state: Arc<AtomicU8>,
    pub(super) result: Arc<PasteResultSlot>,
    pub(super) acknowledgement: Sender<()>,
}

pub(super) struct PendingPaste {
    pub(super) state: Arc<AtomicU8>,
    pub(super) result: Arc<PasteResultSlot>,
    pub(super) expected_clipboard_sha256: ClipboardTextHash,
    pub(super) acknowledgement: Sender<()>,
    pub(super) evidence: TargetEvidence,
    pub(super) deadline: Instant,
    pub(super) timer_id: usize,
    pub(super) modifier_wait: ModifierNeutralWait,
}

pub(super) fn paste_command_state(state: &AtomicU8) -> PasteCommandState {
    match state.load(Ordering::Acquire) {
        1 => PasteCommandState::Waiting,
        2 => PasteCommandState::Injecting,
        3 => PasteCommandState::Committed,
        4 => PasteCommandState::Applied,
        5 => PasteCommandState::Cancelled,
        6 => PasteCommandState::ResultReady,
        _ => PasteCommandState::Pending,
    }
}

pub(super) fn claim_paste_injection(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            PasteCommandState::Waiting as u8,
            PasteCommandState::Injecting as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(super) fn cancel_paste_command(state: &AtomicU8) -> PasteCommandState {
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

pub(super) fn wait_for_claimed_paste_completion(
    response: &Receiver<()>,
    result: &PasteResultSlot,
    timeout: Duration,
) -> PasteResult {
    let received = response.recv_timeout(timeout).is_ok();
    if let Some(result) = result.result() {
        return result;
    }
    debug_assert!(
        !received,
        "SendInput result signal publishes its slot first"
    );
    failed_paste(PasteFailure::Indeterminate)
}

pub(super) const fn failed_paste(reason: PasteFailure) -> PasteResult {
    PasteResult {
        submitted: false,
        reason: Some(reason),
    }
}
