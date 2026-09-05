//! Pure event-to-matcher and replay-record conversions.
use super::*;

pub(super) fn modifier_release_completes_pending_exact(
    event: NormalizedEvent,
    pending: Option<PendingExact>,
    expected: ModifierMask,
    current: ModifierMask,
) -> bool {
    pending.is_some()
        && event.phase == PhysicalPhase::Up
        && matches!(event.key, KeyIdentity::Modifier(_))
        && current != expected
        // A release may remove required modifiers, but an added modifier is an
        // unambiguous cancellation rather than a shortcut completion.
        && (!current.ctrl() || expected.ctrl())
        && (!current.alt() || expected.alt())
        && (!current.shift() || expected.shift())
        && (!current.meta() || expected.meta())
}

pub(super) fn pending_exact(
    result: MatchClass,
    trigger: ActivationKey,
    started_at_ms: u64,
) -> Option<PendingExact> {
    let MatchClass::ExactWithLonger { binding, .. } = result else {
        return None;
    };
    Some(PendingExact {
        binding,
        trigger,
        started_at_ms,
    })
}

pub(super) const fn letter_bit(key: ActivationKey) -> u32 {
    1_u32 << key.index()
}

pub(super) const fn replay_record(event: NormalizedEvent) -> ReplayRecord {
    ReplayRecord {
        key: event.key,
        native: event.native,
        phase: event.phase,
        observed_at_ms: event.observed_at_ms,
    }
}
