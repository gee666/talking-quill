use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{Arc, Mutex, TryLockError, atomic::Ordering},
    thread::{self, JoinHandle},
};

use crossbeam_channel::{Sender, bounded};

use super::{INJECTED_MARKER, SharedState, accessibility::permission_snapshot, ffi};
use crate::{
    keyboard::{ActivationKey, KeyInput, KeyPhase, KeyboardReducer, PhysicalKey},
    platform::{
        CallbackGate, HookStatus, PermissionState, PlatformError, TapRecoveryDecision,
        TapRecoveryEvent, TapRecoveryPolicy, TerminalReason, TerminalSignal,
        activation_config_from_value, deliver_callback_event,
        deliver_callback_event_with_session_arm, hook_status_to_u8,
    },
    protocol::Outbound,
};

struct CallbackContext {
    state: Arc<SharedState>,
    reducer: Arc<Mutex<KeyboardReducer>>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
}

pub(super) fn start_hook(
    state: Arc<SharedState>,
    reducer: Arc<Mutex<KeyboardReducer>>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
) -> Result<JoinHandle<()>, PlatformError> {
    let context = CallbackContext {
        state,
        reducer,
        outbound,
        gate,
        terminal,
    };
    let (ready_tx, ready_rx) = bounded(1);
    let thread = thread::Builder::new()
        .name("talking-quill-helper-macos-hook".into())
        .spawn(move || hook_thread(context, ready_tx))
        .map_err(|_| PlatformError::ThreadStopped)?;
    if ready_rx.recv().is_err() {
        let _ = thread.join();
        Err(PlatformError::ThreadStopped)
    } else {
        Ok(thread)
    }
}

pub(super) fn request_stop(state: &SharedState) {
    state.stopping.store(true, Ordering::Release);
    state.session_capture.store(false, Ordering::Release);
    let run_loop = state.run_loop.load(Ordering::Acquire) as ffi::CFRunLoopRef;
    if !run_loop.is_null() {
        // SAFETY: CFRunLoopStop is thread-safe and the pointer remains owned
        // by the hook thread until that thread has joined.
        unsafe { ffi::CFRunLoopStop(run_loop) };
    }
}

fn hook_thread(context: CallbackContext, ready: Sender<()>) {
    let mut context = Box::new(context);
    let context_ptr = (&raw mut *context).cast::<c_void>();
    let mask = (1_u64 << ffi::K_CG_EVENT_KEY_DOWN) | (1_u64 << ffi::K_CG_EVENT_KEY_UP);
    // SAFETY: callback context remains boxed until the event tap and run-loop
    // source are disabled and released below.
    let tap = unsafe {
        ffi::CGEventTapCreate(
            ffi::K_CG_SESSION_EVENT_TAP,
            ffi::K_CG_HEAD_INSERT_EVENT_TAP,
            ffi::K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            Some(event_tap_callback),
            context_ptr,
        )
    };
    if tap.is_null() {
        let permissions = permission_snapshot();
        let status = if permissions.input_monitoring == PermissionState::Denied
            || permissions.accessibility == PermissionState::Denied
        {
            HookStatus::PermissionRequired
        } else {
            HookStatus::Unavailable
        };
        context
            .state
            .hook_status
            .store(hook_status_to_u8(status), Ordering::Release);
        let _ = ready.send(());
        return;
    }
    context.state.event_tap.store(tap, Ordering::Release);

    // SAFETY: `tap` is a valid CFMachPort returned above.
    let source = unsafe { ffi::CFMachPortCreateRunLoopSource(null(), tap, 0) };
    if source.is_null() {
        // SAFETY: `tap` is an owned CFMachPort. Invalidating before release
        // guarantees it cannot schedule callbacks with the boxed context.
        unsafe {
            ffi::CGEventTapEnable(tap, false);
            ffi::CFMachPortInvalidate(tap);
            ffi::CFRelease(tap.cast_const());
        }
        context.state.event_tap.store(null_mut(), Ordering::Release);
        let _ = ready.send(());
        return;
    }

    // SAFETY: called on the run-loop owner thread.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    context
        .state
        .run_loop
        .store(run_loop as usize, Ordering::Release);
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Ready), Ordering::Release);
    // SAFETY: all CF objects are valid and remain alive through CFRunLoopRun.
    unsafe {
        ffi::CFRunLoopAddSource(run_loop, source, ffi::kCFRunLoopCommonModes);
        ffi::CGEventTapEnable(tap, true);
    }
    if ready.send(()).is_err() {
        context.gate.close();
    } else {
        // SAFETY: runs until shutdown calls CFRunLoopStop.
        unsafe { ffi::CFRunLoopRun() };
    }

    context.gate.close();
    context.state.run_loop.store(0, Ordering::Release);
    context.state.event_tap.store(null_mut(), Ordering::Release);
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // SAFETY: source and tap are owned by this function. Disabling,
    // unregistering, and invalidating them prevents callbacks before the boxed
    // callback context is dropped.
    unsafe {
        ffi::CGEventTapEnable(tap, false);
        ffi::CFRunLoopRemoveSource(run_loop, source, ffi::kCFRunLoopCommonModes);
        ffi::CFMachPortInvalidate(tap);
        ffi::CFRelease(source.cast_const());
        ffi::CFRelease(tap.cast_const());
    }
}

fn apply_tap_recovery(context: &CallbackContext, event: TapRecoveryEvent) {
    let policy = TapRecoveryPolicy::from_consecutive_timeouts(
        context.state.tap_recovery.load(Ordering::Acquire),
    );
    let (next, decision) = policy.observe(event);
    context
        .state
        .tap_recovery
        .store(next.consecutive_timeouts(), Ordering::Release);
    if let TapRecoveryDecision::Terminal(reason) = decision {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context.terminal.trigger(reason);
    }
}

unsafe extern "C" fn event_tap_callback(
    _proxy: ffi::CGEventTapProxy,
    event_type: u32,
    event: ffi::CGEventRef,
    user_info: *mut c_void,
) -> ffi::CGEventRef {
    let handled = std::panic::catch_unwind(|| {
        if user_info.is_null() {
            return false;
        }
        // SAFETY: user_info points to the boxed context retained for the event
        // tap's full lifetime.
        let context = unsafe { &*user_info.cast::<CallbackContext>() };
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            apply_tap_recovery(context, TapRecoveryEvent::DisabledByUserInput);
            return false;
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
            if !context.gate.is_open() {
                return false;
            }
            let tap = context.state.event_tap.load(Ordering::Acquire);
            let recovered = if tap.is_null() {
                false
            } else {
                // Exactly one nonblocking re-enable attempt is allowed. Core
                // Graphics confirmation, not the void enable call, determines
                // whether recovery succeeded.
                // SAFETY: tap is owned by the active hook thread.
                unsafe {
                    ffi::CGEventTapEnable(tap, true);
                    ffi::CGEventTapIsEnabled(tap)
                }
            };
            apply_tap_recovery(
                context,
                if recovered {
                    TapRecoveryEvent::TimeoutRecovered
                } else {
                    TapRecoveryEvent::TimeoutRecoveryFailed
                },
            );
            return false;
        }
        if event.is_null() || !context.gate.is_open() {
            return false;
        }
        let phase = match event_type {
            ffi::K_CG_EVENT_KEY_DOWN => KeyPhase::Down,
            ffi::K_CG_EVENT_KEY_UP => KeyPhase::Up,
            _ => return false,
        };
        apply_tap_recovery(context, TapRecoveryEvent::Activity);
        // SAFETY: Core Graphics guarantees the event for this callback.
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let key = map_key_code(key_code);
        if key == PhysicalKey::Other {
            return false;
        }
        let repeat = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
        };
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        // Option/Shift are allowed activation context; Control/Command disallow
        // activation. Modifier flag-change events are not in the tap mask and
        // remain untouched; only the configured letter sequence may be swallowed.
        let input = KeyInput {
            key,
            phase,
            alt: flags & ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE != 0,
            shift: flags & ffi::K_CG_EVENT_FLAG_MASK_SHIFT != 0,
            disallowed_modifiers: activation_disallowed_modifiers(flags),
            repeat,
            injected: marker == INJECTED_MARKER,
        };
        let _input_transaction = match context.state.input_transaction.try_lock() {
            Ok(transaction) => transaction,
            Err(TryLockError::WouldBlock) => {
                // The server is changing or suspending native input policy.
                // Pass this event through, but preserve the balancing up
                // notification for a sequence whose down was already accepted.
                if let Ok(mut reducer) = context.reducer.try_lock() {
                    for event in reducer.fail_open_balancing_events().into_iter().flatten() {
                        if !deliver_callback_event(&context.outbound, &context.terminal, event) {
                            context.state.hook_status.store(
                                hook_status_to_u8(HookStatus::Unavailable),
                                Ordering::Release,
                            );
                            break;
                        }
                    }
                }
                return false;
            }
            Err(TryLockError::Poisoned(_)) => {
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                return false;
            }
        };
        let (activation_enabled, configured) =
            activation_config_from_value(context.state.activation_config.load(Ordering::Acquire));
        let capture = context.state.session_capture.load(Ordering::Acquire);
        let mut reducer = match context.reducer.try_lock() {
            Ok(reducer) => reducer,
            Err(_) => {
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                return false;
            }
        };
        let plan = reducer.plan_bindings(input, configured, activation_enabled, capture);
        let delivered = plan.event().is_none()
            || deliver_callback_event_with_session_arm(
                &context.outbound,
                &context.terminal,
                &context.state.session_capture,
                plan.event().expect("event checked above"),
            );
        if !delivered {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
        }
        reducer.apply(plan, delivered)
    });

    match handled {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            if !user_info.is_null() {
                // SAFETY: the context remains alive until the event tap is
                // disabled and invalidated on its owner thread.
                let context = unsafe { &*user_info.cast::<CallbackContext>() };
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::CallbackPanicked);
            }
            event
        }
    }
}

const fn activation_disallowed_modifiers(flags: u64) -> bool {
    flags & (ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_COMMAND) != 0
}

fn map_key_code(code: i64) -> PhysicalKey {
    let letter = match code {
        0 => Some(ActivationKey::A),
        11 => Some(ActivationKey::B),
        8 => Some(ActivationKey::C),
        2 => Some(ActivationKey::D),
        14 => Some(ActivationKey::E),
        3 => Some(ActivationKey::F),
        5 => Some(ActivationKey::G),
        4 => Some(ActivationKey::H),
        34 => Some(ActivationKey::I),
        38 => Some(ActivationKey::J),
        40 => Some(ActivationKey::K),
        37 => Some(ActivationKey::L),
        46 => Some(ActivationKey::M),
        45 => Some(ActivationKey::N),
        31 => Some(ActivationKey::O),
        35 => Some(ActivationKey::P),
        12 => Some(ActivationKey::Q),
        15 => Some(ActivationKey::R),
        1 => Some(ActivationKey::S),
        17 => Some(ActivationKey::T),
        32 => Some(ActivationKey::U),
        9 => Some(ActivationKey::V),
        13 => Some(ActivationKey::W),
        7 => Some(ActivationKey::X),
        16 => Some(ActivationKey::Y),
        6 => Some(ActivationKey::Z),
        _ => None,
    };
    if let Some(letter) = letter {
        PhysicalKey::Letter(letter)
    } else if code == 53 {
        PhysicalKey::Escape
    } else if code == 36 || code == 76 {
        PhysicalKey::Enter
    } else {
        PhysicalKey::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_command_flags_exhaustively_disallow_activation() {
        for mask in 0_u8..16 {
            let mut flags = 0;
            if mask & 0b0001 != 0 {
                flags |= ffi::K_CG_EVENT_FLAG_MASK_SHIFT;
            }
            if mask & 0b0010 != 0 {
                flags |= ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE;
            }
            if mask & 0b0100 != 0 {
                flags |= ffi::K_CG_EVENT_FLAG_MASK_CONTROL;
            }
            if mask & 0b1000 != 0 {
                flags |= ffi::K_CG_EVENT_FLAG_MASK_COMMAND;
            }
            assert_eq!(
                activation_disallowed_modifiers(flags),
                mask & 0b1100 != 0,
                "mask {mask:04b}"
            );
        }
    }

    #[test]
    fn mac_key_codes_map_to_supported_physical_keys() {
        assert_eq!(map_key_code(0), PhysicalKey::Letter(ActivationKey::A));
        assert_eq!(map_key_code(6), PhysicalKey::Letter(ActivationKey::Z));
        assert_eq!(map_key_code(53), PhysicalKey::Escape);
        assert_eq!(map_key_code(36), PhysicalKey::Enter);
        assert_eq!(map_key_code(76), PhysicalKey::Enter);
        assert_eq!(map_key_code(999), PhysicalKey::Other);
    }
}
