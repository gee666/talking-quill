use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
};

use crossbeam_channel::{Receiver, Sender, bounded};

use super::{
    ActivationConfig, INJECTED_MARKER, OWNER_COMPLETION_TIMEOUT, OwnerCommand, OwnerCommandState,
    OwnerCompletion, OwnerMutationKind, SharedState, StartupState,
    accessibility::permission_snapshot, cancel_owner_command, cancel_startup, claim_owner_command,
    claim_startup, ffi, join_owner_bounded, owner_completed,
};
use crate::{
    keyboard::{
        ActivationKey, HelperEvent, KeyInput, KeyPhase, KeyboardReducer, ModifierMask, PhysicalKey,
        SessionCaptureMode, SessionKey,
    },
    platform::{
        CallbackGate, HookStatus, PermissionState, PlatformError, TapRecoveryDecision,
        TapRecoveryEvent, TapRecoveryPolicy, TerminalReason, TerminalSignal,
        deliver_callback_event, hook_status_to_u8,
    },
    protocol::Outbound,
};

const LETTER_KEY_CODES: [u16; 26] = [
    0,  // KeyA
    11, // KeyB
    8,  // KeyC
    2,  // KeyD
    14, // KeyE
    3,  // KeyF
    5,  // KeyG
    4,  // KeyH
    34, // KeyI
    38, // KeyJ
    40, // KeyK
    37, // KeyL
    46, // KeyM
    45, // KeyN
    31, // KeyO
    35, // KeyP
    12, // KeyQ
    15, // KeyR
    1,  // KeyS
    17, // KeyT
    32, // KeyU
    9,  // KeyV
    13, // KeyW
    7,  // KeyX
    16, // KeyY
    6,  // KeyZ
];
const ESCAPE_KEY_CODE: u16 = 53;
const RETURN_KEY_CODE: u16 = 36;
const KEYPAD_ENTER_KEY_CODE: u16 = 76;
const LEFT_COMMAND_KEY_CODE: u16 = 55;
const RIGHT_COMMAND_KEY_CODE: u16 = 54;
const LEFT_SHIFT_KEY_CODE: u16 = 56;
const RIGHT_SHIFT_KEY_CODE: u16 = 60;
const LEFT_OPTION_KEY_CODE: u16 = 58;
const RIGHT_OPTION_KEY_CODE: u16 = 61;
const LEFT_CONTROL_KEY_CODE: u16 = 59;
const RIGHT_CONTROL_KEY_CODE: u16 = 62;
const MODIFIER_KEY_CODES: [u16; 8] = [
    LEFT_CONTROL_KEY_CODE,
    RIGHT_CONTROL_KEY_CODE,
    LEFT_OPTION_KEY_CODE,
    RIGHT_OPTION_KEY_CODE,
    LEFT_SHIFT_KEY_CODE,
    RIGHT_SHIFT_KEY_CODE,
    LEFT_COMMAND_KEY_CODE,
    RIGHT_COMMAND_KEY_CODE,
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct MacPhysicalTracker {
    held: [bool; 128],
}

impl MacPhysicalTracker {
    fn observe(&mut self, key_code: u16, phase: KeyPhase) -> bool {
        let Some(held) = self.held.get_mut(usize::from(key_code)) else {
            return false;
        };
        match phase {
            KeyPhase::Down => {
                let repeat = *held;
                *held = true;
                repeat
            }
            KeyPhase::Up => {
                *held = false;
                false
            }
        }
    }

    fn seed(&mut self, key_code: u16) {
        if let Some(held) = self.held.get_mut(usize::from(key_code)) {
            *held = true;
        }
    }

    fn is_held(&self, key_code: u16) -> bool {
        self.held
            .get(usize::from(key_code))
            .copied()
            .unwrap_or(false)
    }

    fn native_state_is_consistent_except(
        &self,
        excluded_key_code: u16,
        mut is_down: impl FnMut(u16) -> bool,
    ) -> bool {
        LETTER_KEY_CODES
            .into_iter()
            .chain([ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE])
            .filter(|key_code| *key_code != excluded_key_code)
            .all(|key_code| self.is_held(key_code) == is_down(key_code))
    }
}

impl Default for MacPhysicalTracker {
    fn default() -> Self {
        Self { held: [false; 128] }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PreheldLetters(u32);

impl PreheldLetters {
    fn insert(&mut self, key: ActivationKey) {
        self.0 |= 1_u32 << u32::from(key.index());
    }

    fn remove(&mut self, key: ActivationKey) {
        self.0 &= !(1_u32 << u32::from(key.index()));
    }

    const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct MacModifierTracker(u8);

impl MacModifierTracker {
    fn observe_flags_changed(&mut self, key_code: u16, is_down: bool) -> bool {
        let Some(index) = MODIFIER_KEY_CODES
            .iter()
            .position(|candidate| *candidate == key_code)
        else {
            return false;
        };
        let bit = 1_u8 << index;
        if is_down {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
        true
    }

    fn seed(&mut self, key_code: u16) {
        let _ = self.observe_flags_changed(key_code, true);
    }

    fn set_mask(&mut self, mask: ModifierMask) {
        self.0 = if mask.ctrl() { 0b0000_0001 } else { 0 }
            | if mask.alt() { 0b0000_0100 } else { 0 }
            | if mask.shift() { 0b0001_0000 } else { 0 }
            | if mask.meta() { 0b0100_0000 } else { 0 };
    }

    const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.0 & 0b0000_0011 != 0,
            self.0 & 0b0000_1100 != 0,
            self.0 & 0b0011_0000 != 0,
            self.0 & 0b1100_0000 != 0,
        )
    }
}

#[derive(Default)]
struct CallbackKeyboard {
    reducer: KeyboardReducer,
    physical: MacPhysicalTracker,
    modifiers: MacModifierTracker,
    preheld_letters: PreheldLetters,
    activation: ActivationConfig,
    activation_revision_at: u64,
    escape_capture_enabled_at: u64,
    enter_capture_enabled_at: u64,
    captured_enter_key_code: Option<u16>,
}

impl CallbackKeyboard {
    fn seed_from_state(&mut self, mut is_down: impl FnMut(u16) -> bool) {
        self.physical = MacPhysicalTracker::default();
        self.modifiers = MacModifierTracker::default();
        self.preheld_letters = PreheldLetters::default();
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if is_down(key_code) {
                self.physical.seed(key_code);
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
        for key_code in [ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE] {
            if is_down(key_code) {
                self.physical.seed(key_code);
            }
        }
        for key_code in MODIFIER_KEY_CODES {
            if is_down(key_code) {
                self.modifiers.seed(key_code);
            }
        }
    }

    fn merge_current_state_as_preheld(&mut self, mut is_down: impl FnMut(u16) -> bool) {
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if is_down(key_code) {
                self.physical.seed(key_code);
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
        for key_code in [ESCAPE_KEY_CODE, RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE] {
            if is_down(key_code) {
                self.physical.seed(key_code);
            }
        }
        for key_code in MODIFIER_KEY_CODES {
            if is_down(key_code) {
                self.modifiers.seed(key_code);
            }
        }
    }

    fn tracked_native_state_is_consistent(&self, excluded_key_code: u16) -> bool {
        self.physical
            .native_state_is_consistent_except(excluded_key_code, native_key_is_down)
    }

    fn fence_current_letters(&mut self) {
        self.preheld_letters = PreheldLetters::default();
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            if self.physical.is_held(key_code) {
                self.preheld_letters
                    .insert(ActivationKey::from_index(index as u8).expect("A-Z key table"));
            }
        }
    }
}

struct CallbackContext {
    state: Arc<SharedState>,
    keyboard: Mutex<CallbackKeyboard>,
    owner_commands: Receiver<OwnerCommand>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
}

pub(super) fn start_hook(
    state: Arc<SharedState>,
    owner_commands: Receiver<OwnerCommand>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
) -> Result<(JoinHandle<()>, Receiver<()>), PlatformError> {
    let stop_state = Arc::clone(&state);
    let stop_gate = Arc::clone(&gate);
    let context = CallbackContext {
        state,
        keyboard: Mutex::new(CallbackKeyboard::default()),
        owner_commands,
        outbound,
        gate,
        terminal,
    };
    let (ready_tx, ready_rx) = bounded(1);
    let startup_state = Arc::new(AtomicU8::new(StartupState::Pending as u8));
    let owner_startup_state = Arc::clone(&startup_state);
    let (owner_completion_tx, owner_completion) = bounded(1);
    let thread = thread::Builder::new()
        .name("talking-quill-helper-macos-hook".into())
        .spawn(move || {
            hook_thread(context, ready_tx, owner_startup_state, owner_completion_tx);
        })
        .map_err(|_| PlatformError::ThreadStopped)?;
    match ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT) {
        Ok(()) => Ok((thread, owner_completion)),
        Err(_) => {
            stop_gate.close();
            stop_state.quiescing.store(true, Ordering::Release);
            stop_state.stopping.store(true, Ordering::Release);
            if cancel_startup(&startup_state) == StartupState::Running {
                let _ = ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT);
            }
            request_stop(&stop_state);
            if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) {
                let _ = join_owner_bounded(thread, OWNER_COMPLETION_TIMEOUT);
            } else {
                drop(thread);
            }
            Err(PlatformError::ThreadStopped)
        }
    }
}

pub(super) fn request_stop(state: &Arc<SharedState>) {
    state.quiescing.store(true, Ordering::Release);
    state.stopping.store(true, Ordering::Release);
    state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);

    // Core Foundation wake-up must not make the coordinator's bounded shutdown
    // path block. The endpoint lock keeps owner cleanup from releasing either
    // object if this detached request itself wedges inside Core Foundation.
    let stop_state = Arc::clone(state);
    let _ = thread::Builder::new()
        .name("talking-quill-helper-macos-stop".into())
        .spawn(move || signal_stop(&stop_state));
}

fn signal_stop(state: &SharedState) {
    let endpoint = state
        .owner_endpoint
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let source = endpoint.command_source as ffi::CFRunLoopSourceRef;
    let run_loop = endpoint.run_loop as ffi::CFRunLoopRef;
    if run_loop.is_null() {
        return;
    }
    // SAFETY: the endpoint lock prevents owner teardown until these
    // thread-safe Core Foundation calls return. A signalled source makes the
    // stop durable if this races the first CFRunLoopRun activation.
    unsafe {
        if !source.is_null() {
            ffi::CFRunLoopSourceSignal(source);
        }
        ffi::CFRunLoopStop(run_loop);
        ffi::CFRunLoopWakeUp(run_loop);
    }
}

fn hook_thread(
    context: CallbackContext,
    ready: Sender<()>,
    startup_state: Arc<AtomicU8>,
    owner_completion: Sender<()>,
) {
    // Declared first so cleanup completion is published only after all owner
    // resources and callback context have been dropped.
    let _owner_completion = OwnerCompletion(owner_completion);
    let mut context = Box::new(context);
    let callback_context = (&raw mut *context).cast::<c_void>();
    let mask = (1_u64 << ffi::K_CG_EVENT_KEY_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_KEY_UP)
        | (1_u64 << ffi::K_CG_EVENT_FLAGS_CHANGED);
    // SAFETY: callback context remains boxed until every source and tap has
    // been removed, invalidated, and released below.
    let tap = unsafe {
        ffi::CGEventTapCreate(
            ffi::K_CG_SESSION_EVENT_TAP,
            ffi::K_CG_HEAD_INSERT_EVENT_TAP,
            ffi::K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            Some(event_tap_callback),
            callback_context,
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
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(());
        return;
    }
    context.state.event_tap.store(tap, Ordering::Release);

    // SAFETY: `tap` is a valid CFMachPort returned above.
    let tap_source = unsafe { ffi::CFMachPortCreateRunLoopSource(null(), tap, 0) };
    if tap_source.is_null() {
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(());
        return;
    }

    let mut source_context = ffi::CFRunLoopSourceContext {
        version: 0,
        info: callback_context,
        retain: None,
        release: None,
        copy_description: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(owner_command_perform),
    };
    // SAFETY: Core Foundation copies the version-0 context. Its info pointer
    // remains valid for the source's complete lifetime.
    let command_source = unsafe { ffi::CFRunLoopSourceCreate(null(), 0, &raw mut source_context) };
    if command_source.is_null() {
        // SAFETY: tap_source and tap are owned and no run loop references them.
        unsafe {
            ffi::CFRelease(tap_source.cast_const());
        }
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(());
        return;
    }

    // SAFETY: called on the future run-loop owner thread.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    // SAFETY: all objects are valid and remain alive through CFRunLoopRun.
    unsafe {
        ffi::CFRunLoopAddSource(run_loop, tap_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopAddSource(run_loop, command_source, ffi::kCFRunLoopCommonModes);
        ffi::CGEventTapEnable(tap, true);
    }
    if let Ok(keyboard) = context.keyboard.get_mut() {
        keyboard.seed_from_state(native_key_is_down);
    } else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
    }
    {
        let mut endpoint = context
            .state
            .owner_endpoint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        endpoint.run_loop = run_loop as usize;
        endpoint.command_source = command_source as usize;
    }
    if claim_startup(&startup_state) {
        context
            .state
            .hook_status
            .store(hook_status_to_u8(HookStatus::Ready), Ordering::Release);
        if ready.send(()).is_ok() && !context.state.stopping.load(Ordering::Acquire) {
            // SAFETY: runs until shutdown calls CFRunLoopStop.
            unsafe { ffi::CFRunLoopRun() };
        } else {
            context.gate.close();
        }
    } else {
        context.gate.close();
    }

    context.gate.close();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    {
        let mut endpoint = context
            .state
            .owner_endpoint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        endpoint.run_loop = 0;
        endpoint.command_source = 0;
    }
    while let Ok(command) = context.owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    context.state.event_tap.store(null_mut(), Ordering::Release);
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // SAFETY: sources and tap are owned by this function. Endpoint publication
    // was cleared under its lock, so no other thread can signal released data.
    unsafe {
        ffi::CGEventTapEnable(tap, false);
        ffi::CFRunLoopRemoveSource(run_loop, tap_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopRemoveSource(run_loop, command_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopSourceInvalidate(command_source);
        ffi::CFMachPortInvalidate(tap);
        ffi::CFRelease(command_source.cast_const());
        ffi::CFRelease(tap_source.cast_const());
        ffi::CFRelease(tap.cast_const());
    }
}

fn cleanup_tap_without_sources(context: &CallbackContext, tap: ffi::CFMachPortRef) {
    context.state.event_tap.store(null_mut(), Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    // SAFETY: tap is owned, has no live run-loop source, and is released once.
    unsafe {
        ffi::CGEventTapEnable(tap, false);
        ffi::CFMachPortInvalidate(tap);
        ffi::CFRelease(tap.cast_const());
    }
}

unsafe extern "C" fn owner_command_perform(info: *mut c_void) {
    let result = std::panic::catch_unwind(|| {
        if info.is_null() {
            return;
        }
        // SAFETY: info is the boxed callback context retained through source cleanup.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        if context.state.stopping.load(Ordering::Acquire) {
            // SAFETY: this callback runs on the owner thread's active run loop.
            unsafe { ffi::CFRunLoopStop(ffi::CFRunLoopGetCurrent()) };
        } else {
            process_owner_commands(context);
        }
    });
    if result.is_err() && !info.is_null() {
        // SAFETY: context remains alive through source invalidation.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context.terminal.trigger(TerminalReason::CallbackPanicked);
    }
}

fn process_owner_commands(context: &CallbackContext) {
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
            Ok(mut keyboard) => {
                match command.mutation.kind {
                    OwnerMutationKind::Configure => {
                        // Every owner-linearized binding revision fences passive
                        // letters already tracked or physically held. Accepted
                        // trigger ownership remains in the reducer for balancing up.
                        keyboard.reducer.fence_activation_revision();
                        keyboard.fence_current_letters();
                        keyboard.merge_current_state_as_preheld(native_key_is_down);
                        keyboard.activation_revision_at = event_timestamp_now();
                        keyboard.activation = command.mutation.activation;
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
                    }
                    OwnerMutationKind::SuspendNativeInput => {
                        keyboard.activation.enabled = false;
                        context
                            .state
                            .session_capture_mode
                            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
                        deliver_balancing_events(context, &mut keyboard.reducer);
                        keyboard.captured_enter_key_code = None;
                        keyboard.fence_current_letters();
                    }
                }
                true
            }
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

fn deliver_balancing_events(context: &CallbackContext, reducer: &mut KeyboardReducer) {
    for event in reducer.fail_open_balancing_events().into_iter().flatten() {
        if !context.gate.is_open()
            || !deliver_callback_event(&context.outbound, &context.terminal, event)
        {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            break;
        }
    }
}

fn apply_tap_recovery(context: &CallbackContext, event: TapRecoveryEvent) -> TapRecoveryDecision {
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
    decision
}

fn resynchronize_after_gap(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return;
        }
    };
    deliver_balancing_events(context, &mut keyboard.reducer);
    keyboard.captured_enter_key_code = None;
    keyboard.seed_from_state(native_key_is_down);
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
        // SAFETY: user_info points to the boxed context retained for the tap lifetime.
        let context = unsafe { &*user_info.cast::<CallbackContext>() };
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            if context.state.quiescing.load(Ordering::Acquire) {
                return false;
            }
            apply_tap_recovery(context, TapRecoveryEvent::DisabledByUserInput);
            return false;
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
            if context.state.quiescing.load(Ordering::Acquire) {
                return false;
            }
            let tap = context.state.event_tap.load(Ordering::Acquire);
            let recovered = if tap.is_null() {
                false
            } else {
                // SAFETY: tap is owned by the active hook thread.
                unsafe {
                    ffi::CGEventTapEnable(tap, true);
                    ffi::CGEventTapIsEnabled(tap)
                }
            };
            let decision = apply_tap_recovery(
                context,
                if recovered {
                    TapRecoveryEvent::TimeoutRecovered
                } else {
                    TapRecoveryEvent::TimeoutRecoveryFailed
                },
            );
            if recovered && decision == TapRecoveryDecision::Continue {
                resynchronize_after_gap(context);
            }
            return false;
        }
        if event.is_null() {
            return false;
        }
        // SAFETY: Core Graphics guarantees a valid event for ordinary callbacks.
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        if is_synthetic_event(marker, source_pid) {
            return false;
        }
        apply_tap_recovery(context, TapRecoveryEvent::Activity);
        // SAFETY: keycode exists for key and flags-changed event records.
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let Ok(key_code) = u16::try_from(key_code) else {
            return false;
        };

        if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
            // The event's aggregate flags are ordered with the tap stream and
            // correctly preserve a still-held opposite-side modifier.
            let modifiers = modifier_mask_from_flags(unsafe { ffi::CGEventGetFlags(event) });
            if let Ok(mut keyboard) = context.keyboard.try_lock() {
                keyboard.modifiers.set_mask(modifiers);
                keyboard.reducer.observe_modifiers(modifiers);
            } else {
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
            }
            return false;
        }

        let phase = match event_type {
            ffi::K_CG_EVENT_KEY_DOWN => KeyPhase::Down,
            ffi::K_CG_EVENT_KEY_UP => KeyPhase::Up,
            _ => return false,
        };
        // SAFETY: timestamp, flags, and autorepeat are defined for keyboard events.
        let event_timestamp = unsafe { ffi::CGEventGetTimestamp(event) };
        let event_modifiers = modifier_mask_from_flags(unsafe { ffi::CGEventGetFlags(event) });
        let native_repeat = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
        };
        process_key_event_with_modifiers(
            context,
            key_code,
            phase,
            native_repeat,
            Some(event_modifiers),
            event_timestamp,
            true,
        )
    });

    match handled {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            if !user_info.is_null() {
                // SAFETY: context remains alive until owner-thread tap cleanup.
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

fn process_key_event_with_modifiers(
    context: &CallbackContext,
    key_code: u16,
    phase: KeyPhase,
    native_repeat: bool,
    event_modifiers: Option<ModifierMask>,
    event_timestamp: u64,
    reconcile_held_state: bool,
) -> bool {
    let key = map_key_code(key_code);
    if key == PhysicalKey::Other {
        return false;
    }
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return false;
        }
    };
    let was_held = keyboard.physical.is_held(key_code);
    let discontinuity = match phase {
        KeyPhase::Down => was_held && !native_repeat,
        KeyPhase::Up => !was_held,
    };
    let tracked_modifiers = keyboard.modifiers.mask();
    let observed_modifiers = event_modifiers.unwrap_or(tracked_modifiers);
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let relevant_to_capture = keyboard.reducer.has_captured_sequence()
        || !keyboard.preheld_letters.is_empty()
        || match key {
            PhysicalKey::Letter(_) => {
                keyboard.activation.enabled && observed_modifiers != ModifierMask::default()
            }
            PhysicalKey::Escape => capture_mode.allows(SessionKey::Escape),
            PhysicalKey::Enter => capture_mode.allows(SessionKey::Enter),
            PhysicalKey::Other => false,
        };
    let tracked_state_mismatch = reconcile_held_state
        && relevant_to_capture
        && phase == KeyPhase::Down
        && !keyboard.tracked_native_state_is_consistent(key_code);
    if discontinuity
        || tracked_state_mismatch
        || event_modifiers.is_some_and(|observed| observed != tracked_modifiers)
    {
        // A non-repeat discontinuity or event-time modifier mismatch indicates
        // a missed, synthetic, or Secure-Input-hidden transition. Fail open and
        // conservatively resync only after rejecting the current event.
        deliver_balancing_events(context, &mut keyboard.reducer);
        keyboard.captured_enter_key_code = None;
        keyboard.seed_from_state(native_key_is_down);
        return false;
    }

    let tracked_repeat = keyboard.physical.observe(key_code, phase);
    let repeat = tracked_repeat || native_repeat;
    let policy_cutoff = match key {
        PhysicalKey::Letter(_) if keyboard.activation.enabled => keyboard.activation_revision_at,
        PhysicalKey::Escape if capture_mode.allows(SessionKey::Escape) => {
            keyboard.escape_capture_enabled_at
        }
        PhysicalKey::Enter if capture_mode.allows(SessionKey::Enter) => {
            keyboard.enter_capture_enabled_at
        }
        _ => 0,
    };
    let predates_policy = policy_cutoff != 0 && event_timestamp <= policy_cutoff;
    if let PhysicalKey::Letter(letter) = key {
        if phase == KeyPhase::Down
            && (!context.gate.is_open() || predates_policy || (native_repeat && !tracked_repeat))
        {
            keyboard.preheld_letters.insert(letter);
        } else if phase == KeyPhase::Up {
            keyboard.preheld_letters.remove(letter);
        }
    }
    let accepting = context.gate.is_open();
    let releases_passive_letter = matches!(key, PhysicalKey::Letter(_)) && phase == KeyPhase::Up;
    if (!accepting || predates_policy)
        && !keyboard.reducer.is_capturing(key)
        && !releases_passive_letter
    {
        return false;
    }
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_key_code
            .is_some_and(|captured| captured != key_code)
    {
        return false;
    }
    let activation = keyboard.activation;
    let input = KeyInput {
        key,
        phase,
        modifiers: observed_modifiers,
        repeat,
        injected: false,
    };
    let plan = keyboard.reducer.plan_bindings_at(
        input,
        activation.bindings,
        accepting && activation.enabled && keyboard.preheld_letters.is_empty(),
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
        event_timestamp / 1_000_000,
    );
    let planned_event = plan.event();
    let delivered = planned_event.is_none()
        || (accepting
            && deliver_callback_event(
                &context.outbound,
                &context.terminal,
                planned_event.expect("event presence checked above"),
            ));
    if !delivered {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
    }
    let swallowed = keyboard.reducer.apply(plan, delivered);
    if let Some(HelperEvent::SessionKey {
        key: crate::keyboard::SessionKey::Enter,
        phase: crate::keyboard::EventPhase::Down,
    }) = planned_event
        && delivered
        && swallowed
    {
        keyboard.captured_enter_key_code = Some(key_code);
    } else if key == PhysicalKey::Enter
        && phase == KeyPhase::Up
        && keyboard.captured_enter_key_code == Some(key_code)
    {
        keyboard.captured_enter_key_code = None;
    }
    swallowed
}

#[cfg(test)]
fn process_key_event(
    context: &CallbackContext,
    key_code: u16,
    phase: KeyPhase,
    native_repeat: bool,
    _reconcile_native_state: bool,
) -> bool {
    process_key_event_with_modifiers(
        context,
        key_code,
        phase,
        native_repeat,
        None,
        u64::MAX,
        false,
    )
}

const fn is_synthetic_event(marker: i64, source_pid: i64) -> bool {
    marker == INJECTED_MARKER || source_pid > 0
}

const fn modifier_mask_from_flags(flags: u64) -> ModifierMask {
    ModifierMask::new(
        flags & ffi::K_CG_EVENT_FLAG_MASK_CONTROL != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_SHIFT != 0,
        flags & ffi::K_CG_EVENT_FLAG_MASK_COMMAND != 0,
    )
}

fn mach_ticks_to_nanoseconds(ticks: u64, numer: u32, denom: u32) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    let value = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    u64::try_from(value).ok()
}

fn event_timestamp_now() -> u64 {
    let mut timebase = ffi::MachTimebaseInfo::default();
    // SAFETY: timebase is valid writable storage and mach_absolute_time has no
    // pointer preconditions. CGEvent timestamps use nanoseconds since startup.
    let status = unsafe { ffi::mach_timebase_info(&raw mut timebase) };
    if status != 0 {
        return u64::MAX;
    }
    let ticks = unsafe { ffi::mach_absolute_time() };
    mach_ticks_to_nanoseconds(ticks, timebase.numer, timebase.denom).unwrap_or(u64::MAX)
}

fn native_key_is_down(key_code: u16) -> bool {
    // SAFETY: combined-session key state accepts every bounded CGKeyCode.
    unsafe { ffi::CGEventSourceKeyState(ffi::K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION, key_code) }
}

fn map_key_code(code: u16) -> PhysicalKey {
    if let Some(index) = LETTER_KEY_CODES
        .iter()
        .position(|candidate| *candidate == code)
    {
        return PhysicalKey::Letter(
            ActivationKey::from_index(index as u8).expect("key table has exactly A-Z entries"),
        );
    }
    match code {
        ESCAPE_KEY_CODE => PhysicalKey::Escape,
        RETURN_KEY_CODE | KEYPAD_ENTER_KEY_CODE => PhysicalKey::Enter,
        _ => PhysicalKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{OwnerMutation, owner_command_state};
    use super::*;
    use crate::keyboard::{
        ActivationBinding, ActivationBindings, EventPhase, ProfileId, Shortcut, ShortcutModifiers,
    };

    fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
        Shortcut::new(modifiers, keys).unwrap()
    }

    fn bindings() -> ActivationBindings {
        ActivationBindings::new(&[
            ActivationBinding::new(
                ProfileId::PROMPT,
                shortcut(
                    ShortcutModifiers {
                        ctrl: false,
                        alt: true,
                        shift: false,
                        meta: false,
                    },
                    &[ActivationKey::X, ActivationKey::P],
                ),
            ),
            ActivationBinding::new(
                ProfileId::GENERAL,
                shortcut(
                    ShortcutModifiers {
                        ctrl: true,
                        alt: false,
                        shift: true,
                        meta: false,
                    },
                    &[ActivationKey::P],
                ),
            ),
        ])
        .unwrap()
    }

    fn test_context_with_capacity(
        outbound_capacity: usize,
    ) -> (CallbackContext, Receiver<Outbound>, Sender<OwnerCommand>) {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound, outbound_rx) = bounded(outbound_capacity);
        let (command_tx, owner_commands) = bounded(4);
        (
            CallbackContext {
                state: Arc::new(SharedState::new()),
                keyboard: Mutex::new(CallbackKeyboard {
                    activation: ActivationConfig {
                        enabled: true,
                        bindings: bindings(),
                    },
                    ..CallbackKeyboard::default()
                }),
                owner_commands,
                outbound,
                gate,
                terminal,
            },
            outbound_rx,
            command_tx,
        )
    }

    fn test_context() -> (CallbackContext, Receiver<Outbound>, Sender<OwnerCommand>) {
        test_context_with_capacity(8)
    }

    fn receive_event(receiver: &Receiver<Outbound>) -> HelperEvent {
        match receiver.try_recv().unwrap() {
            Outbound::Event(event) => event,
            other => panic!("unexpected outbound: {other:?}"),
        }
    }

    fn apply_config(
        context: &CallbackContext,
        commands: &Sender<OwnerCommand>,
        activation: ActivationConfig,
    ) {
        let state = Arc::new(std::sync::atomic::AtomicU8::new(
            OwnerCommandState::Pending as u8,
        ));
        let (acknowledgement, response) = bounded(1);
        commands
            .send(OwnerCommand {
                mutation: OwnerMutation {
                    kind: OwnerMutationKind::Configure,
                    activation,
                    session_capture_mode: SessionCaptureMode::Off,
                },
                state: Arc::clone(&state),
                acknowledgement,
            })
            .unwrap();
        process_owner_commands(context);
        assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
        assert!(response.recv().unwrap().is_ok());
    }

    #[test]
    fn mach_timebase_conversion_matches_cg_event_nanoseconds() {
        assert_eq!(mach_ticks_to_nanoseconds(3, 125, 3), Some(125));
        assert_eq!(mach_ticks_to_nanoseconds(1, 1, 0), None);
        assert_eq!(mach_ticks_to_nanoseconds(u64::MAX, u32::MAX, 1), None);
    }

    #[test]
    fn every_ansi_letter_keycode_maps_to_its_dom_physical_position() {
        let mut unique = std::collections::BTreeSet::new();
        for (index, key_code) in LETTER_KEY_CODES.iter().copied().enumerate() {
            assert!(unique.insert(key_code));
            assert_eq!(
                map_key_code(key_code),
                PhysicalKey::Letter(ActivationKey::from_index(index as u8).unwrap())
            );
        }
        assert_eq!(map_key_code(ESCAPE_KEY_CODE), PhysicalKey::Escape);
        assert_eq!(map_key_code(RETURN_KEY_CODE), PhysicalKey::Enter);
        assert_eq!(map_key_code(KEYPAD_ENTER_KEY_CODE), PhysicalKey::Enter);
        assert_eq!(map_key_code(127), PhysicalKey::Other);
    }

    #[test]
    fn modifier_keycodes_and_event_flags_project_to_every_exact_protocol_modifier() {
        for bits in 0_u8..16 {
            let flags = (if bits & 0b0001 != 0 {
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL
            } else {
                0
            }) | (if bits & 0b0010 != 0 {
                ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE
            } else {
                0
            }) | (if bits & 0b0100 != 0 {
                ffi::K_CG_EVENT_FLAG_MASK_SHIFT
            } else {
                0
            }) | (if bits & 0b1000 != 0 {
                ffi::K_CG_EVENT_FLAG_MASK_COMMAND
            } else {
                0
            });
            assert_eq!(
                modifier_mask_from_flags(flags | 0x0000_0100),
                ModifierMask::new(
                    bits & 0b0001 != 0,
                    bits & 0b0010 != 0,
                    bits & 0b0100 != 0,
                    bits & 0b1000 != 0,
                ),
            );
        }

        for (key_code, expected) in [
            (
                LEFT_CONTROL_KEY_CODE,
                ModifierMask::new(true, false, false, false),
            ),
            (
                LEFT_OPTION_KEY_CODE,
                ModifierMask::new(false, true, false, false),
            ),
            (
                LEFT_SHIFT_KEY_CODE,
                ModifierMask::new(false, false, true, false),
            ),
            (
                LEFT_COMMAND_KEY_CODE,
                ModifierMask::new(false, false, false, true),
            ),
        ] {
            let mut tracker = MacModifierTracker::default();
            assert!(tracker.observe_flags_changed(key_code, true));
            assert_eq!(tracker.mask(), expected);
        }

        let mut tracker = MacModifierTracker::default();
        tracker.observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
        tracker.observe_flags_changed(RIGHT_SHIFT_KEY_CODE, true);
        tracker.observe_flags_changed(LEFT_SHIFT_KEY_CODE, false);
        assert!(tracker.mask().shift());
        tracker.observe_flags_changed(RIGHT_SHIFT_KEY_CODE, false);
        assert!(!tracker.mask().shift());
        assert!(!tracker.observe_flags_changed(57, true));
    }

    #[test]
    fn preheld_snapshot_fences_letters_and_distinguishes_both_enter_keys() {
        let held = [LETTER_KEY_CODES[23], RETURN_KEY_CODE, KEYPAD_ENTER_KEY_CODE];
        let mut queried = Vec::new();
        let mut keyboard = CallbackKeyboard::default();
        keyboard.seed_from_state(|key_code| {
            queried.push(key_code);
            held.contains(&key_code)
        });
        assert_eq!(queried.len(), 37);
        assert!(!keyboard.preheld_letters.is_empty());
        assert!(keyboard.physical.observe(RETURN_KEY_CODE, KeyPhase::Down));
        assert!(
            keyboard
                .physical
                .observe(KEYPAD_ENTER_KEY_CODE, KeyPhase::Down)
        );
        keyboard.physical.observe(RETURN_KEY_CODE, KeyPhase::Up);
        assert!(
            keyboard
                .physical
                .observe(KEYPAD_ENTER_KEY_CODE, KeyPhase::Down)
        );
    }

    #[test]
    fn events_queued_before_policy_enable_are_tracked_but_never_captured() {
        let (context, outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.activation_revision_at = 100;
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        }
        assert!(!process_key_event_with_modifiers(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            None,
            99,
            false,
        ));
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .reducer
                .held_letters()
                .is_empty()
        );
        assert!(outbound.try_recv().is_err());

        let (capture_context, capture_outbound, _commands) = test_context();
        capture_context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        {
            let mut keyboard = capture_context.keyboard.lock().unwrap();
            keyboard.escape_capture_enabled_at = 100;
            keyboard.enter_capture_enabled_at = 100;
        }
        assert!(!process_key_event_with_modifiers(
            &capture_context,
            ESCAPE_KEY_CODE,
            KeyPhase::Down,
            false,
            None,
            99,
            false,
        ));
        assert!(capture_outbound.try_recv().is_err());
    }

    #[test]
    fn pre_revision_passive_up_releases_the_reducer_fence() {
        let (context, outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        }
        assert!(!process_key_event_with_modifiers(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            None,
            50,
            false,
        ));
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.reducer.fence_activation_revision();
            keyboard.fence_current_letters();
            keyboard.activation_revision_at = 100;
        }
        assert!(!process_key_event_with_modifiers(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Up,
            false,
            None,
            99,
            false,
        ));
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .reducer
                .held_letters()
                .is_empty()
        );

        assert!(!process_key_event_with_modifiers(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            None,
            101,
            false,
        ));
        assert!(process_key_event_with_modifiers(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            None,
            102,
            false,
        ));
        assert!(matches!(
            receive_event(&outbound),
            HelperEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
    }

    #[test]
    fn simultaneous_main_and_keypad_enter_keep_the_captured_source_balanced() {
        let (context, outbound, _commands) = test_context();
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        assert!(process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!process_key_event(
            &context,
            KEYPAD_ENTER_KEY_CODE,
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!process_key_event(
            &context,
            KEYPAD_ENTER_KEY_CODE,
            KeyPhase::Up,
            false,
            false,
        ));
        assert!(process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Up,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: crate::keyboard::SessionKey::Enter,
                phase: EventPhase::Down,
            }
        );
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: crate::keyboard::SessionKey::Enter,
                phase: EventPhase::Up,
            }
        );
    }

    #[test]
    fn capture_mode_changes_preserve_balancing_and_cancel_only_passes_fresh_enter() {
        let (context, outbound, _commands) = test_context();
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        assert!(process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Down,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Down,
            },
        );

        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);
        assert!(process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Up,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            },
        );
        assert!(!process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!process_key_event(
            &context,
            RETURN_KEY_CODE,
            KeyPhase::Up,
            false,
            false,
        ));

        assert!(process_key_event(
            &context,
            ESCAPE_KEY_CODE,
            KeyPhase::Down,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            },
        );
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        assert!(process_key_event(
            &context,
            ESCAPE_KEY_CODE,
            KeyPhase::Up,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            },
        );
    }

    #[test]
    fn outbound_failure_passes_trigger_and_closes_the_callback_gate() {
        let (context, _outbound, _commands) = test_context_with_capacity(0);
        context
            .keyboard
            .lock()
            .unwrap()
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(!context.gate.is_open());
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Up,
            false,
            false,
        ));
    }

    #[test]
    fn injected_marker_and_unknown_repeat_are_conservatively_ignored_or_fenced() {
        assert!(is_synthetic_event(INJECTED_MARKER, 0));
        assert!(is_synthetic_event(0, 42));
        assert!(!is_synthetic_event(0, 0));

        let (context, outbound, _commands) = test_context();
        context
            .keyboard
            .lock()
            .unwrap()
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            true,
            false,
        ));
        assert!(!context.keyboard.lock().unwrap().preheld_letters.is_empty());
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn cancelled_owner_command_never_applies_after_wakeup() {
        let (context, _outbound, commands) = test_context();
        let previous = context.keyboard.lock().unwrap().activation;
        let state = Arc::new(std::sync::atomic::AtomicU8::new(
            OwnerCommandState::Pending as u8,
        ));
        let (acknowledgement, response) = bounded(1);
        commands
            .send(OwnerCommand {
                mutation: OwnerMutation {
                    kind: OwnerMutationKind::Configure,
                    activation: ActivationConfig::default(),
                    session_capture_mode: SessionCaptureMode::Off,
                },
                state: Arc::clone(&state),
                acknowledgement,
            })
            .unwrap();
        assert_eq!(cancel_owner_command(&state), OwnerCommandState::Cancelled);

        process_owner_commands(&context);

        assert_eq!(context.keyboard.lock().unwrap().activation, previous);
        assert!(response.recv().unwrap().is_err());
    }

    #[test]
    fn owner_commands_apply_full_bindings_and_capture_in_fifo_order() {
        let (context, _outbound, commands) = test_context();
        let updated = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[ActivationBinding::new(
                ProfileId::GENERAL,
                shortcut(
                    ShortcutModifiers {
                        ctrl: false,
                        alt: false,
                        shift: false,
                        meta: true,
                    },
                    &[ActivationKey::Q, ActivationKey::P],
                ),
            )])
            .unwrap(),
        };
        let mut states = Vec::new();
        let mut responses = Vec::new();
        for mutation in [
            OwnerMutation {
                kind: OwnerMutationKind::Configure,
                activation: updated,
                session_capture_mode: SessionCaptureMode::Off,
            },
            OwnerMutation {
                kind: OwnerMutationKind::SetSessionCapture,
                activation: ActivationConfig::default(),
                session_capture_mode: SessionCaptureMode::Recording,
            },
        ] {
            let state = Arc::new(std::sync::atomic::AtomicU8::new(
                OwnerCommandState::Pending as u8,
            ));
            let (acknowledgement, response) = bounded(1);
            commands
                .send(OwnerCommand {
                    mutation,
                    state: Arc::clone(&state),
                    acknowledgement,
                })
                .unwrap();
            states.push(state);
            responses.push(response);
        }

        process_owner_commands(&context);

        assert_eq!(context.keyboard.lock().unwrap().activation, updated);
        assert_eq!(
            SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
            SessionCaptureMode::Recording,
        );
        for state in states {
            assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
        }
        for response in responses {
            assert!(response.recv().unwrap().is_ok());
        }
    }

    #[test]
    fn every_nonempty_modifier_mask_can_activate_in_the_macos_model() {
        for bits in 1_u8..16 {
            let modifiers = ShortcutModifiers {
                ctrl: bits & 0b0001 != 0,
                alt: bits & 0b0010 != 0,
                shift: bits & 0b0100 != 0,
                meta: bits & 0b1000 != 0,
            };
            let expected = shortcut(modifiers, &[ActivationKey::P]);
            let expected_binding = ActivationBinding::new(ProfileId::GENERAL, expected);
            let (context, outbound, _commands) = test_context();
            {
                let mut keyboard = context.keyboard.lock().unwrap();
                keyboard.activation = ActivationConfig {
                    enabled: true,
                    bindings: ActivationBindings::new(&[ActivationBinding::new(
                        ProfileId::GENERAL,
                        expected,
                    )])
                    .unwrap(),
                };
                for (enabled, key_code) in [
                    (modifiers.ctrl, LEFT_CONTROL_KEY_CODE),
                    (modifiers.alt, LEFT_OPTION_KEY_CODE),
                    (modifiers.shift, LEFT_SHIFT_KEY_CODE),
                    (modifiers.meta, LEFT_COMMAND_KEY_CODE),
                ] {
                    if enabled {
                        keyboard.modifiers.observe_flags_changed(key_code, true);
                    }
                }
            }
            assert!(
                process_key_event(&context, LETTER_KEY_CODES[15], KeyPhase::Down, false, false,),
                "modifier bits {bits:04b}",
            );
            assert_eq!(
                receive_event(&outbound),
                HelperEvent::Activation {
                    binding: expected_binding,
                    phase: EventPhase::Down,
                }
            );
        }
    }

    #[test]
    fn every_binding_revision_fences_held_letters_until_release() {
        let (context, outbound, commands) = test_context();
        context.gate.close();
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));

        let one_key = ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::P],
            ),
        );
        apply_config(
            &context,
            &commands,
            ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[one_key]).unwrap(),
            },
        );
        context.gate.open();
        context
            .keyboard
            .lock()
            .unwrap()
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(outbound.try_recv().is_err());
        process_key_event(&context, LETTER_KEY_CODES[15], KeyPhase::Up, false, false);
        process_key_event(&context, LETTER_KEY_CODES[23], KeyPhase::Up, false, false);
        assert!(process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: one_key,
                phase: EventPhase::Down,
            },
        );
    }

    #[test]
    fn modifier_changes_fence_a_passive_macos_prefix_until_release() {
        let (context, outbound, _commands) = test_context();
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));
        context
            .keyboard
            .lock()
            .unwrap()
            .modifiers
            .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(outbound.try_recv().is_err());
        for key_code in [LETTER_KEY_CODES[15], LETTER_KEY_CODES[23]] {
            process_key_event(&context, key_code, KeyPhase::Up, false, false);
        }
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
    }

    #[test]
    fn alt_x_p_and_ctrl_shift_p_use_ordered_native_tracking_and_snapshots() {
        let (context, outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        }
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        let accepted = bindings().iter().next().unwrap();
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: accepted,
                phase: EventPhase::Down,
            }
        );
        apply_config(&context, &_commands, ActivationConfig::default());
        assert!(process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Up,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: accepted,
                phase: EventPhase::Up,
            }
        );

        let (context, outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
        }
        assert!(process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: bindings().iter().nth(1).unwrap(),
                phase: EventPhase::Down,
            }
        );
    }

    #[test]
    fn closed_gate_inputs_cannot_invent_order() {
        let (context, outbound, _commands) = test_context();
        context.gate.close();
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[23],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .reducer
                .held_letters()
                .is_empty()
        );
        context.gate.open();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard
                .modifiers
                .observe_flags_changed(LEFT_OPTION_KEY_CODE, true);
        }
        assert!(!process_key_event(
            &context,
            LETTER_KEY_CODES[15],
            KeyPhase::Down,
            false,
            false,
        ));
        assert!(outbound.try_recv().is_err());
    }
}
