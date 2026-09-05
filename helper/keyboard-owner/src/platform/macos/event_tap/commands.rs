//! Apply owner configuration and session policy in FIFO order.

use super::*;

pub(super) fn process_owner_commands(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    while let Ok(command) = context.owner_commands.try_recv() {
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

        let applied = match context.keyboard.try_lock() {
            Ok(mut keyboard) => match command.mutation.kind {
                OwnerMutationKind::Configure => {
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
                        let outcome = begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(compiled),
                        );
                        if !outcome.applied {
                            return false;
                        }
                        // Keep the legacy reducer fenced solely for independent
                        // Escape/Enter ownership and existing adapter tests.
                        keyboard.reducer.fence_activation_revision();
                        keyboard.fence_current_letters();
                        keyboard.merge_current_state_as_preheld(native_key_is_down);
                        keyboard.activation_revision_at = event_timestamp_now();
                        keyboard.activation = command.mutation.activation;
                        true
                    })
                }
                OwnerMutationKind::SetSessionCapture => {
                    let previous = SessionCaptureMode::from_u8(
                        context.state.session_capture_mode.load(Ordering::Acquire),
                    );
                    let next = command.mutation.session_capture_mode;
                    let enables_escape =
                        !previous.allows(SessionKey::Escape) && next.allows(SessionKey::Escape);
                    let enables_enter =
                        !previous.allows(SessionKey::Enter) && next.allows(SessionKey::Enter);
                    if enables_escape || enables_enter {
                        keyboard.merge_current_state_as_preheld(native_key_is_down);
                        let enabled_at = event_timestamp_now();
                        if enables_escape {
                            keyboard.escape_capture_enabled_at = enabled_at;
                        }
                        if enables_enter {
                            keyboard.enter_capture_enabled_at = enabled_at;
                        }
                    }
                    context
                        .state
                        .session_capture_mode
                        .store(next.as_u8(), Ordering::Release);
                    true
                }
                OwnerMutationKind::SuspendNativeInput => {
                    let disabled = keyboard
                        .transactional
                        .config()
                        .revision()
                        .checked_next()
                        .and_then(|revision| {
                            CompiledActivationConfig::compile(
                                revision,
                                false,
                                keyboard.transactional.config().bindings(),
                            )
                            .ok()
                        });
                    if disabled.is_none_or(|disabled| {
                        !begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(disabled),
                        )
                        .applied
                    }) {
                        false
                    } else {
                        keyboard.activation.enabled = false;
                        context
                            .state
                            .session_capture_mode
                            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
                        deliver_balancing_events(context, &mut keyboard.reducer);
                        keyboard.fence_current_letters();
                        true
                    }
                }
            },
            Err(_) => false,
        };

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
