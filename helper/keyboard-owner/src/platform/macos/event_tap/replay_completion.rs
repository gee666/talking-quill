//! Resume observed effects and balance replay after target changes.

use super::*;

pub(super) fn observed_native_effect_outcome(effect: EffectRequest) -> Option<EffectOutcome> {
    match effect {
        EffectRequest::Replay(batch) => Some(EffectOutcome::ReplaySubmitted {
            submitted: batch.len(),
        }),
        EffectRequest::CleanupInjected(batch) => Some(EffectOutcome::CleanupSubmitted {
            submitted: batch.len(),
        }),
        _ => None,
    }
}

pub(super) fn mark_observed_native_effect(keyboard: &mut CallbackKeyboard) -> bool {
    let Some(inflight) = keyboard.inflight_effect.as_mut() else {
        return false;
    };
    let Some(outcome) = observed_native_effect_outcome(inflight.effect) else {
        return false;
    };
    inflight.outcome = Some(outcome);
    true
}

pub(super) fn resume_observed_native_effect(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    let Some(inflight) = keyboard.inflight_effect.take() else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return DriveCompletion::Failed;
    };
    let Some(outcome) = observed_native_effect_outcome(inflight.effect) else {
        keyboard.inflight_effect = Some(inflight);
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    };
    drive_transaction_turn(
        context,
        keyboard,
        inflight.continuation.resume(outcome),
        None,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TargetChangedReplayRecovery {
    AwaitObservation,
    ReconcileVisibleDowns,
    FinishAfterCleanup,
}

pub(super) fn target_changed_replay_recovery(
    keyboard: &CallbackKeyboard,
    inflight: &InflightEffect,
) -> Option<TargetChangedReplayRecovery> {
    (inflight.outcome.is_none()
        && matches!(inflight.effect, EffectRequest::Replay(_))
        && keyboard.replay_target_changed)
        .then(|| {
            if keyboard.replay_observation.is_some() {
                TargetChangedReplayRecovery::AwaitObservation
            } else if keyboard.replay_target_cleanup_pending {
                TargetChangedReplayRecovery::FinishAfterCleanup
            } else {
                TargetChangedReplayRecovery::ReconcileVisibleDowns
            }
        })
}

pub(super) fn reconcile_target_changed_replay(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    let cleanup = keyboard.visible_replay_cleanup();
    if cleanup.is_empty() {
        return finish_target_changed_replay(context, keyboard);
    }
    let submission = context.native_events.try_lock().ok().and_then(|mut pool| {
        let pool = pool.as_mut()?;
        let prepared = injection::prepare_cleanup(pool, cleanup)?;
        let submission = prepared.submission();
        let token = submission.token?;
        if submission.count != cleanup.len()
            || !keyboard.begin_replay_observation(ExpectedReplayBatch::Cleanup(cleanup), token)
        {
            return None;
        }
        keyboard.replay_target_cleanup_pending = true;
        prepared.post(pool, None);
        Some(submission)
    });
    if submission.is_some_and(|submitted| submitted.count == cleanup.len()) {
        arm_maintenance_timer(context);
        DriveCompletion::NativeObservationPending
    } else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        DriveCompletion::Failed
    }
}

pub(super) fn finish_target_changed_replay(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    keyboard.replay_target_cleanup_pending = false;
    let Some(inflight) = keyboard.inflight_effect.take() else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    };
    if !matches!(inflight.effect, EffectRequest::Replay(_)) {
        keyboard.inflight_effect = Some(inflight);
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    }
    drive_transaction_turn(
        context,
        keyboard,
        inflight
            .continuation
            .resume(EffectOutcome::ReplaySuppressedTargetChanged),
        None,
    )
}

pub(super) const fn failed_effect_outcome(effect: EffectRequest) -> EffectOutcome {
    match effect {
        EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 0 },
        EffectRequest::CleanupMenuNeutralization(_) => {
            EffectOutcome::MenuCleanupAccepted { accepted: 0 }
        }
        EffectRequest::DeliverActivation(_) => EffectOutcome::ActivationDelivered(false),
        EffectRequest::Replay(_) => EffectOutcome::ReplaySubmitted { submitted: 0 },
        EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupSubmitted { submitted: 0 },
    }
}
