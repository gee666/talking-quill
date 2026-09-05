//! Apply configuration and session changes on the hook thread.
use super::*;

pub(super) fn process_owner_commands(context: &CallbackContext, receiver: &Receiver<OwnerCommand>) {
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
    while let Ok(command) = receiver.try_recv() {
        if context.state.stopping.load(Ordering::Acquire) {
            let _ = cancel_owner_command(&command.state);
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::ThreadStopped));
            continue;
        }
        if !claim_owner_command(&command.state) {
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::NativeFailure));
            continue;
        }

        let applied = match command.mutation.kind {
            OwnerMutationKind::Configure => match lock_keyboard_recovering(context) {
                Some(mut keyboard) => {
                    // Keep the session reducer's historical revision fence for
                    // Escape/Enter compatibility tests; global activation is
                    // replaced atomically in the transactional core.
                    keyboard.reducer.fence_activation_revision();
                    keyboard.activation_fenced_letters = keyboard.physical.held_letter_bits();
                    keyboard.modifiers_fenced =
                        keyboard.modifiers.mask() != ModifierMask::default();
                    keyboard.activation = command.mutation.activation;
                    let compiled = keyboard
                        .transactional
                        .config()
                        .revision()
                        .checked_next()
                        .and_then(|revision| {
                            CompiledActivationConfig::compile(
                                revision,
                                command.mutation.activation.enabled,
                                command.mutation.activation.bindings,
                            )
                            .ok()
                        });
                    compiled.is_some_and(|compiled| {
                        begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(compiled),
                        )
                        .is_some_and(|outcome| outcome.applied)
                    })
                }
                None => false,
            },
            OwnerMutationKind::CloseAdmission => {
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    begin_transaction_control(
                        context,
                        &mut keyboard,
                        Control::CloseAdmission(CancelReason::HelperDisconnected),
                    )
                    .is_some_and(|outcome| outcome.applied)
                })
            }
            OwnerMutationKind::CancelCandidate => {
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    begin_transaction_control(context, &mut keyboard, Control::Shutdown)
                        .is_some_and(|outcome| outcome.applied)
                })
            }
            OwnerMutationKind::SetSessionCapture => {
                context.state.session_capture_mode.store(
                    command.mutation.session_capture_mode.as_u8(),
                    Ordering::Release,
                );
                true
            }
            #[cfg(test)]
            OwnerMutationKind::InjectTestUnmatchedDown => {
                // Package-excluded seam: model one already-captured Escape down
                // without installing a global hook callback or calling SendInput.
                // Shutdown must wait for its physical up until the platform
                // deadline, then retire terminal-incomplete without fabricating it.
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    keyboard.session_escape_native_owned = true;
                    true
                })
            }
        };

        publish_pending_native_work(context);
        if applied {
            command
                .state
                .store(OwnerCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(Ok(()));
        } else {
            command
                .state
                .store(OwnerCommandState::Cancelled as u8, Ordering::Release);
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::NativeFailure));
        }
    }
}
