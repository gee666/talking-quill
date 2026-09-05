//! Normal-callback gap observation; recovery classification stays separate.

use super::*;

pub(super) fn gap_observation(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    _gap_token: Option<injection::OperationToken>,
) -> bool {
    let key_code =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let repeat = unsafe {
        ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
    };
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let observation = u16::try_from(key_code)
        .ok()
        .and_then(|key_code| {
            context.keyboard.try_lock().ok().map(|mut keyboard| {
                keyboard.observe_gap_barrier_event(event_type, key_code, repeat, flags)
            })
        })
        .unwrap_or(GapBarrierObservation::Forged);
    if observation == GapBarrierObservation::Complete {
        complete_gap_barrier(context);
    }
    if observation != GapBarrierObservation::Forged {
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled
            && let Some(token) = _gap_token
        {
            crate::platform::macos::record_test_marker_acknowledgement(
                crate::platform::macos::MacosTestOperationClass::GapBarrier,
                token,
            );
        }
        return true;
    }
    // A correct nonce/PID with the wrong shape/order is not an
    // acknowledgement and must not mutate recovery state.
    context
        .terminal
        .trigger(TerminalReason::InputInjectionUnavailable);
    return true;
}
