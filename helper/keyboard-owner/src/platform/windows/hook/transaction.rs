//! Drive keyboard transactions and complete deferred native effects.
use super::*;

mod deferred;
pub(super) use deferred::*;

pub(super) fn candidate_target_is_current(keyboard: &CallbackKeyboard) -> bool {
    let (Some(expected), Some(expected_desktop), Some(expected_epoch)) = (
        keyboard.candidate_target,
        keyboard.candidate_desktop,
        keyboard.candidate_target_epoch,
    ) else {
        return false;
    };
    #[cfg(test)]
    if expected.is_test_only() {
        return true;
    }
    !keyboard.candidate_target_changed
        && current_input_desktop() == Some(expected_desktop)
        && TARGET_CHANGE_EPOCH.load(Ordering::Acquire) == expected_epoch
        && revalidate_candidate_target(expected)
}

pub(super) fn capture_coherent_candidate_target(
    trusted_desktop: Option<DesktopIdentity>,
) -> Option<(CandidateTargetEvidence, DesktopIdentity, u64)> {
    #[cfg(test)]
    if capture_candidate_target().is_none() {
        return Some((
            CandidateTargetEvidence::test_only(),
            trusted_desktop.unwrap_or(DesktopIdentity {
                name: [0; 64],
                len: 0,
            }),
            TARGET_CHANGE_EPOCH.load(Ordering::Acquire),
        ));
    }
    let desktop = trusted_desktop?;
    let before_epoch = TARGET_CHANGE_EPOCH.load(Ordering::Acquire);
    let target = capture_candidate_target()?;
    let after_epoch = TARGET_CHANGE_EPOCH.load(Ordering::Acquire);
    (before_epoch == after_epoch && revalidate_candidate_target(target)).then_some((
        target,
        desktop,
        before_epoch,
    ))
}

pub(super) fn validate_candidate_target(keyboard: &mut CallbackKeyboard) -> bool {
    let current = candidate_target_is_current(keyboard);
    if !current && keyboard.candidate_target.is_some() {
        keyboard.candidate_target_changed = true;
    }
    current
}

pub(super) fn drive_transaction_turn(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    mut turn: Turn,
    callback_disposition: Option<&Cell<CallbackDisposition>>,
) -> Option<Completion> {
    for _ in 0..=MAX_EFFECTS_PER_TURN {
        if transaction_gate(context) == GateState::Closed
            && matches!(&turn, Turn::NeedEffect { .. })
            && !turn.effect_is_cleanup()
        {
            turn = turn.close_before_unsubmitted_effect(CancelReason::GateClosed);
        }
        keyboard.transaction_authority = Some(TransactionAuthority::Turn(turn.clone()));
        if let Some(callback_disposition) = callback_disposition {
            callback_disposition.set(
                if turn.event_disposition_hint() == Some(EventDisposition::CaptureCurrent) {
                    CallbackDisposition::Capture
                } else {
                    CallbackDisposition::Pass
                },
            );
        }
        #[cfg(test)]
        panic_at_transaction_seam(TestTransactionPanicPoint::AfterTurn);
        match turn {
            Turn::Complete { engine, completion } => {
                context.observability.publish(engine.metrics());
                if engine.journal_len() == 0 && engine.owned_letters() == 0 {
                    keyboard.candidate_target = None;
                    keyboard.candidate_desktop = None;
                    keyboard.candidate_target_epoch = None;
                    keyboard.candidate_target_changed = false;
                }
                keyboard.transactional = engine;
                keyboard.transaction_authority = None;
                return Some(completion);
            }
            Turn::NeedEffect {
                effect,
                continuation,
            } => {
                let outcome = match effect {
                    EffectRequest::NeutralizeMenu(modifiers) if callback_disposition.is_some() => {
                        let target_valid_at_callback = validate_candidate_target(keyboard);
                        keyboard.deferred_callback_replay = Some(DeferredCallbackReplay {
                            continuation,
                            record: None,
                            outcome: DeferredEffectOutcome::MenuNeutralization,
                        });
                        keyboard.transaction_authority =
                            Some(TransactionAuthority::AwaitingDeferredReplay);
                        let submitted = target_valid_at_callback
                            && keyboard
                                .candidate_target
                                .zip(keyboard.candidate_desktop)
                                .is_some_and(|(target, desktop)| {
                                    context.replay_sender.as_ref().is_some_and(|sender| {
                                        sender
                                            .try_send(ReplayWork::NeutralizeMenu {
                                                modifiers,
                                                target,
                                                desktop,
                                            })
                                            .is_ok()
                                    })
                                });
                        if !submitted {
                            context.replay_accepted.store(1, Ordering::Release);
                            post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            );
                        }
                        return None;
                    }
                    EffectRequest::NeutralizeMenu(modifiers) => EffectOutcome::Neutralized {
                        accepted: if validate_candidate_target(keyboard) {
                            injection::neutralize_menu(context.injection_markers, modifiers)
                        } else {
                            0
                        },
                    },
                    EffectRequest::CleanupMenuNeutralization(modifiers) => {
                        EffectOutcome::MenuCleanupAccepted {
                            accepted: injection::cleanup_menu_neutralization(
                                context.injection_markers,
                                modifiers,
                            ),
                        }
                    }
                    EffectRequest::DeliverActivation(notice) => {
                        let target_valid = matches!(notice, ActivationNotice::Up { .. })
                            || validate_candidate_target(keyboard);
                        EffectOutcome::ActivationDelivered(
                            target_valid
                                && keyboard.dispatcher.deliver(
                                    &context.outbound,
                                    &context.terminal,
                                    &context.observability,
                                    notice,
                                    keyboard.candidate_target,
                                ),
                        )
                    }
                    EffectRequest::Replay(batch) if callback_disposition.is_some() => {
                        // SendInput from inside WH_KEYBOARD_LL can strand the hook even
                        // though our private marker bypasses reducer recursion. Retain
                        // the continuation and current-edge suppression authority, then
                        // submit replay from the windowless owner's message pump after
                        // this callback returns.
                        let target_valid_at_callback = validate_candidate_target(keyboard);
                        keyboard.deferred_callback_replay = Some(DeferredCallbackReplay {
                            continuation,
                            record: None,
                            outcome: DeferredEffectOutcome::Replay,
                        });
                        keyboard.transaction_authority =
                            Some(TransactionAuthority::AwaitingDeferredReplay);
                        let submitted = if target_valid_at_callback {
                            keyboard
                                .candidate_target
                                .zip(keyboard.candidate_desktop)
                                .is_some_and(|(target, desktop)| {
                                    context.replay_sender.as_ref().is_some_and(|sender| {
                                        sender
                                            .try_send(ReplayWork::Replay {
                                                batch,
                                                target,
                                                desktop,
                                            })
                                            .is_ok()
                                    })
                                })
                        } else {
                            context.replay_accepted.store(1, Ordering::Release);
                            post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            )
                        };
                        if !submitted {
                            // No worker can publish this replay. Resolve the
                            // retained continuation as suppressed; the timer
                            // polls this result even if the wake post fails.
                            context.replay_accepted.store(1, Ordering::Release);
                            let _ = post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            );
                            context
                                .terminal
                                .trigger(TerminalReason::OwnerThreadUnresponsive);
                        }
                        return None;
                    }
                    EffectRequest::Replay(batch) => {
                        if validate_candidate_target(keyboard) {
                            EffectOutcome::ReplayAccepted {
                                accepted: injection::inject_replay(
                                    context.injection_markers,
                                    batch,
                                ),
                            }
                        } else {
                            EffectOutcome::ReplaySuppressedTargetChanged
                        }
                    }
                    EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                        accepted: injection::inject_cleanup(context.injection_markers, batch),
                    },
                };
                keyboard.transaction_authority = Some(TransactionAuthority::Resume {
                    continuation: continuation.clone(),
                    outcome,
                });
                #[cfg(test)]
                run_reentrant_helper_callback_before_effect_panic(context);
                #[cfg(test)]
                panic_at_transaction_seam(TestTransactionPanicPoint::AfterEffect);
                turn = continuation.resume(outcome);
            }
        }
    }
    panic!("transaction core exceeded MAX_EFFECTS_PER_TURN");
}

pub(super) fn recover_transaction_authority(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> bool {
    let Some(authority) = keyboard.transaction_authority.clone() else {
        return true;
    };
    let turn = match authority {
        TransactionAuthority::Turn(turn) => turn,
        // The worker owns the only continuation while replay is in flight.
        // A racing callback must pass through without mutating the transactional
        // engine; the replay completion message resumes that exact turn.
        TransactionAuthority::AwaitingDeferredReplay => return false,
        TransactionAuthority::Resume {
            continuation,
            outcome,
        } => continuation.resume(outcome),
    };
    let completion = drive_transaction_turn(context, keyboard, turn, None)
        .expect("owner-side transaction effects cannot defer");
    let terminal = match completion {
        Completion::Event(outcome) => outcome.terminal,
        Completion::Control(outcome) => outcome.shutdown == ShutdownState::Terminal,
    };
    if terminal && !context.terminal.is_triggered() {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    true
}

pub(super) fn begin_transaction_control(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    control: Control,
) -> Option<talking_quill_keyboard_core::transactional::ControlOutcome> {
    if !recover_transaction_authority(context, keyboard) {
        return None;
    }
    let turn = keyboard
        .transactional
        .clone()
        .begin(EngineInput::Control(control));
    let completion = drive_transaction_turn(context, keyboard, turn, None)
        .expect("control effects cannot defer outside a callback event");
    let Completion::Control(outcome) = completion else {
        unreachable!("a control turn completes as control")
    };
    Some(outcome)
}
