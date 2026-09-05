//! Consume durable worker results, resume authority, and discharge raced releases.
use super::*;

pub(in crate::platform::windows::hook) fn process_deferred_callback_replay(
    context: &CallbackContext,
) {
    // A posted message is only a wake hint. Ignore premature, duplicate, or
    // stale messages until the sole replay worker publishes a nonzero result.
    let encoded_accepted = context.replay_accepted.swap(0, Ordering::AcqRel);
    if encoded_accepted == 0 {
        return;
    }
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        context
            .replay_accepted
            .store(encoded_accepted, Ordering::Release);
        return;
    };
    let Some(pending) = keyboard.deferred_callback_replay.take() else {
        return;
    };
    let Some(record) = pending.record else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return;
    };
    let outcome = match pending.outcome {
        DeferredEffectOutcome::Replay if encoded_accepted >= 2 => EffectOutcome::ReplayAccepted {
            accepted: (encoded_accepted - 2) as usize,
        },
        DeferredEffectOutcome::Replay => EffectOutcome::ReplaySuppressedTargetChanged,
        DeferredEffectOutcome::MenuNeutralization => EffectOutcome::Neutralized {
            accepted: if encoded_accepted >= 2 {
                (encoded_accepted - 2) as usize
            } else {
                0
            },
        },
    };
    let mut menu_releases = [NativeKey::default(); 4];
    let mut menu_release_count = 0_usize;
    if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization) {
        for slot in 0..keyboard.deferred_menu_releases.len() {
            let release = keyboard.deferred_menu_releases[slot].take();
            if menu_modifier_release_still_needed(&keyboard, slot)
                && let Some(native) = release
            {
                menu_releases[menu_release_count] = native;
                menu_release_count += 1;
            }
        }
    }
    keyboard.transaction_authority = Some(TransactionAuthority::Resume {
        continuation: pending.continuation.clone(),
        outcome,
    });
    let turn = pending.continuation.resume(outcome);
    let Some(completion) = drive_transaction_turn(context, &mut keyboard, turn, None) else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return;
    };
    let _ = complete_transaction_event(context, &mut keyboard, completion, record);
    if std::mem::take(&mut keyboard.deferred_replay_raced) {
        let snapshot = PhysicalSnapshot::new(
            keyboard.physical.held_letter_bits(),
            keyboard.modifiers.transactional_sides(),
            keyboard.altgr_active,
        );
        let control = if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization) {
            Control::ReconcileObserved {
                snapshot,
                observed_at_ms: keyboard.deferred_observed_at_ms,
            }
        } else {
            Control::Reconcile(snapshot)
        };
        let _ = begin_transaction_control(context, &mut keyboard, control);
    }
    // Finish activation delivery before releasing retained Alt/Win keys. Never
    // hold the keyboard mutex across SendInput: User32 can deliver pending
    // physical callbacks reentrantly while the native call is in progress.
    drop(keyboard);
    if injection::inject_modifier_releases(
        context.injection_markers,
        &menu_releases[..menu_release_count],
    ) != menu_release_count
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    publish_pending_native_work(context);
}
