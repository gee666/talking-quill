//! Normal-callback paste observation; recovery classification stays separate.

use super::*;

pub(super) fn paste_observation(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    _paste_barrier_token: Option<injection::OperationToken>,
) -> bool {
    let key_code =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let repeat = unsafe {
        ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
    };
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let mut pending = match context.pending_paste.try_lock() {
        Ok(pending) => pending,
        Err(_) => {
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        }
    };
    let observation = pending
        .as_mut()
        .and_then(|command| {
            Some(observe_exact_pair(
                &mut command.neutral_barrier_state,
                event_type,
                u16::try_from(key_code).ok()?,
                127,
                repeat,
                flags,
                0,
            ))
        })
        .unwrap_or(GapBarrierObservation::Forged);
    #[cfg(feature = "transactional-shortcuts-dev")]
    if observation != GapBarrierObservation::Forged
        && context.test_physical_seam_enabled
        && let Some(token) = _paste_barrier_token
    {
        crate::platform::macos::record_test_marker_acknowledgement(
            crate::platform::macos::MacosTestOperationClass::PasteBarrier,
            token,
        );
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if observation == GapBarrierObservation::Down
        && context
            .test_paste_barrier_split_active
            .load(Ordering::Acquire)
    {
        // Callback work remains one release-store. Owner maintenance
        // performs all file I/O before releasing the prebuilt up.
        context
            .test_paste_barrier_down_observed
            .store(true, Ordering::Release);
        arm_maintenance_timer(context);
    }
    if observation == GapBarrierObservation::Complete {
        let post_result = (|| -> Result<(), PasteFailure> {
            let command = pending.as_mut().ok_or(PasteFailure::Unavailable)?;
            let validated_epoch = command
                .validated_target_epoch
                .ok_or(PasteFailure::Unavailable)?;
            let validated_boundary_epoch = command
                .validated_target_boundary_epoch
                .ok_or(PasteFailure::Unavailable)?;
            let validated_selected_range_epoch = command
                .validated_selected_range_epoch
                .ok_or(PasteFailure::Unavailable)?;
            let cache = context
                .target_cache
                .as_ref()
                .ok_or(PasteFailure::Unavailable)?;
            if !cache.handle_is_current(
                command.evidence,
                validated_epoch,
                validated_boundary_epoch,
                validated_selected_range_epoch,
            ) {
                context.observability.record_target_validation_fallback();
                return Err(PasteFailure::Unavailable);
            }
            let expected_modifier_epoch = command
                .neutral_modifier_epoch
                .ok_or(PasteFailure::ConflictingModifiers)?;
            if !context.keyboard.try_lock().is_ok_and(|keyboard| {
                keyboard.modifier_epoch == expected_modifier_epoch
                    && keyboard.modifiers.sides().bits() == 0
            }) || !native_modifiers_neutral()
            {
                return Err(PasteFailure::ConflictingModifiers);
            }
            let _gate_lease = context
                .gate
                .try_acquire_delivery()
                .ok_or(PasteFailure::Unavailable)?;
            let now = Instant::now();
            claim_paste_injection(
                &command.state,
                PastePostChecks {
                    admission_open: paste_admission_open(context),
                    before_deadline: paste_before_deadline(command.deadline, now),
                    before_injection_cutoff: paste_before_deadline(command.injection_cutoff, now),
                    modifiers_neutral: true,
                    // Owner validation immediately before posting the
                    // barrier proved TCC grants. The callback performs
                    // no AX/permission messaging.
                    permissions_granted: true,
                    secure_input_inactive: !secure_input_active(),
                    target_valid: true,
                },
            )?;
            let insertion = command
                .insertion_request
                .as_mut()
                .ok_or(PasteFailure::Unavailable)?;
            if cache.submit_insertion(insertion) {
                Ok(())
            } else {
                Err(PasteFailure::Unavailable)
            }
        })();
        if let Err(reason) = post_result {
            finish_pending_paste(&mut pending, failed_paste(reason));
        }
        arm_maintenance_timer(context);
    } else if observation == GapBarrierObservation::Forged {
        finish_pending_paste(&mut pending, failed_paste(PasteFailure::OsRejected));
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    return true;
}
