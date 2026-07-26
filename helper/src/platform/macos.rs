use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
};

use crossbeam_channel::Sender;

use super::{
    CallbackGate, FrontApp, HookStatus, PasteResult, Permissions, Platform, PlatformError,
    TerminalReason, TerminalSignal, activation_config_from_value, activation_config_value,
    hook_status_from_u8, hook_status_to_u8, permissions_allow_native_input,
};
use crate::{
    keyboard::{ActivationBindings, KeyboardReducer},
    protocol::Outbound,
};

mod accessibility;
mod cf;
mod event_tap;
mod ffi;
mod paste;

const INJECTED_MARKER: i64 = 0x4D45_4348_4F50_5354;

struct SharedState {
    input_transaction: Mutex<()>,
    activation_config: AtomicU64,
    session_capture: AtomicBool,
    hook_status: AtomicU8,
    run_loop: AtomicUsize,
    event_tap: AtomicPtr<c_void>,
    tap_recovery: AtomicU8,
    stopping: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            input_transaction: Mutex::new(()),
            activation_config: AtomicU64::new(activation_config_value(
                false,
                ActivationBindings::default(),
            )),
            session_capture: AtomicBool::new(false),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            run_loop: AtomicUsize::new(0),
            event_tap: AtomicPtr::new(null_mut()),
            tap_recovery: AtomicU8::new(0),
            stopping: AtomicBool::new(false),
        }
    }
}

fn suspend_native_input(state: &SharedState, reducer: &Mutex<KeyboardReducer>) {
    let _transaction = state
        .input_transaction
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    suspend_native_input_locked(state, reducer);
}

fn suspend_native_input_locked(state: &SharedState, reducer: &Mutex<KeyboardReducer>) {
    let (_, bindings) =
        activation_config_from_value(state.activation_config.load(Ordering::Acquire));
    state
        .activation_config
        .store(activation_config_value(false, bindings), Ordering::Release);
    state.session_capture.store(false, Ordering::Release);
    *reducer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = KeyboardReducer::default();
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    reducer: Arc<Mutex<KeyboardReducer>>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    thread: Option<JoinHandle<()>>,
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(SharedState::new());
        let reducer = Arc::new(Mutex::new(KeyboardReducer::default()));
        let thread = event_tap::start_hook(
            Arc::clone(&state),
            Arc::clone(&reducer),
            outbound,
            Arc::clone(&gate),
            Arc::clone(&terminal),
        )?;
        Ok(Self {
            state,
            reducer,
            gate,
            terminal,
            thread: Some(thread),
        })
    }

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let _transaction = self
            .state
            .input_transaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enabled && self.hook_status() != HookStatus::Ready {
            suspend_native_input_locked(&self.state, &self.reducer);
            return Err(PlatformError::HookUnavailable);
        }
        if enabled && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            suspend_native_input_locked(&self.state, &self.reducer);
            return Err(PlatformError::PermissionDenied);
        }
        self.state.activation_config.store(
            activation_config_value(enabled, bindings),
            Ordering::Release,
        );
        Ok(())
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        let _transaction = self
            .state
            .input_transaction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active && self.hook_status() != HookStatus::Ready {
            suspend_native_input_locked(&self.state, &self.reducer);
            return Err(PlatformError::HookUnavailable);
        }
        if active && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            suspend_native_input_locked(&self.state, &self.reducer);
            return Err(PlatformError::PermissionDenied);
        }
        self.state.session_capture.store(active, Ordering::Release);
        Ok(())
    }

    fn inject_paste(&self) -> PasteResult {
        paste::inject_paste()
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        accessibility::front_app()
    }

    fn permissions(&self) -> Permissions {
        let permissions = accessibility::permission_snapshot();
        if !permissions_allow_native_input(permissions) {
            // The transaction waits for any accepted callback to finish and
            // makes concurrent callbacks pass through. Returning from this
            // method is therefore the native fail-open linearization point.
            suspend_native_input(&self.state, &self.reducer);
        }
        permissions
    }

    fn shutdown(&mut self) -> Option<TerminalReason> {
        self.gate.close();
        event_tap::request_stop(&self.state);
        let joined = self
            .thread
            .take()
            .is_none_or(|thread| thread.join().is_ok());
        if joined {
            self.state
                .hook_status
                .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
        } else {
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.terminal.trigger(TerminalReason::HookStopped);
        }
        self.terminal.reason()
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::ActivationKey;

    #[test]
    fn activation_config_is_atomic_and_defaults_disabled() {
        let state = SharedState::new();
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (false, ActivationBindings::default())
        );
        let bindings =
            ActivationBindings::from_exact(&[(ActivationKey::B, false), (ActivationKey::Q, true)])
                .unwrap();
        state
            .activation_config
            .store(activation_config_value(true, bindings), Ordering::Release);
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (true, bindings)
        );
    }

    #[test]
    fn permission_suspension_preserves_bindings_and_clears_all_capture() {
        let state = SharedState::new();
        let bindings =
            ActivationBindings::from_exact(&[(ActivationKey::B, false), (ActivationKey::Q, true)])
                .unwrap();
        state
            .activation_config
            .store(activation_config_value(true, bindings), Ordering::Release);
        state.session_capture.store(true, Ordering::Release);

        suspend_native_input(&state, &Mutex::new(KeyboardReducer::default()));

        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (false, bindings)
        );
        assert!(!state.session_capture.load(Ordering::Acquire));
    }
}
