use std::{
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_ESCAPE, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RETURN,
            VK_RSHIFT, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT,
            LLKHF_ALTDOWN, LLKHF_INJECTED, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
            PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
            WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
            WM_SYSKEYUP,
        },
    },
};

use super::{
    front_app::front_app,
    hotkeys::{
        ActivationCandidate, ActivationCandidateState, ActivationCommand, ActivationConfig,
        CandidateUpAction, ConfigurationHandoff, ConfigurationTimeoutAction, HotKeyRegistrar,
        HotKeyRuntime, HotKeyTransactionError, WindowsHotKeyRegistrar, cancel_configuration,
        candidate_for_exact_passive_down, commit_configuration, configuration_ack_timeout,
        configuration_handoff, configure_hotkeys, confirm_configuration_rollback,
        release_activation_candidate, resolve_activation_candidate_message, unregister_all_hotkeys,
    },
    paste::inject_paste,
};
use crate::{
    keyboard::{
        ActivationBindings, ActivationKey, KeyInput, KeyPhase, KeyboardReducer, PhysicalKey,
        PhysicalKeyTracker,
    },
    platform::{
        CallbackGate, FrontApp, HookStatus, PasteResult, PermissionState, Permissions, Platform,
        PlatformError, TerminalReason, TerminalSignal, activation_config_from_value,
        activation_config_value, deliver_callback_event, deliver_callback_event_with_session_arm,
        hook_status_from_u8, hook_status_to_u8,
    },
    protocol::Outbound,
};

#[cfg(target_pointer_width = "64")]
pub(super) const INJECTED_MARKER: usize = 0x4D45_4348_4F50_5354;
#[cfg(target_pointer_width = "32")]
pub(super) const INJECTED_MARKER: usize = 0x4F50_5354;
const WM_ACTIVATION_CONFIG: u32 = WM_APP + 0x45;
const ACTIVATION_CONFIG_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_WAKE_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
];
static DPI_AWARENESS_READY: OnceLock<bool> = OnceLock::new();

fn retry_owner_wake(mut post: impl FnMut() -> bool, mut backoff: impl FnMut(Duration)) -> bool {
    if post() {
        return true;
    }
    for delay in OWNER_WAKE_RETRY_DELAYS {
        backoff(delay);
        if post() {
            return true;
        }
    }
    false
}

fn post_owner_message(thread_id: u32, message: u32) -> bool {
    retry_owner_wake(
        || {
            // SAFETY: all owner messages are pointer-free and target the thread
            // whose queue was created before readiness was reported.
            unsafe { PostThreadMessageW(thread_id, message, 0, 0) != 0 }
        },
        thread::sleep,
    )
}

fn owner_completed(receiver: &Receiver<()>, timeout: Duration) -> bool {
    receiver.recv_timeout(timeout).is_ok()
}

struct OwnerCompletion(Sender<()>);

impl Drop for OwnerCompletion {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(null_mut());

struct SharedState {
    activation_config: AtomicU64,
    session_capture: AtomicBool,
    hook_status: AtomicU8,
    stopping: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            activation_config: AtomicU64::new(activation_config_value(
                false,
                ActivationBindings::default(),
            )),
            session_capture: AtomicBool::new(false),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            stopping: AtomicBool::new(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ShiftKeyTracker {
    left: bool,
    right: bool,
    generic: bool,
}

impl ShiftKeyTracker {
    fn from_state(mut is_down: impl FnMut(u16) -> bool) -> Self {
        let left = is_down(VK_LSHIFT);
        let right = is_down(VK_RSHIFT);
        Self {
            left,
            right,
            // Some keyboards expose only the aggregate virtual key. Keep that
            // fallback separate so normal side-specific key-up events cannot
            // clear the other held Shift key.
            generic: !left && !right && is_down(VK_SHIFT),
        }
    }

    fn observe(&mut self, code: u16, phase: KeyPhase) {
        let down = phase == KeyPhase::Down;
        match code {
            VK_LSHIFT => self.left = down,
            VK_RSHIFT => self.right = down,
            VK_SHIFT => self.generic = down,
            _ => {}
        }
    }

    const fn is_down(self) -> bool {
        self.left || self.right || self.generic
    }
}

#[derive(Default)]
struct CallbackKeyboard {
    reducer: KeyboardReducer,
    physical: PhysicalKeyTracker,
    shift: ShiftKeyTracker,
}

struct CallbackContext {
    state: Arc<SharedState>,
    keyboard: Mutex<CallbackKeyboard>,
    hotkeys: Mutex<HotKeyRuntime>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    activation_commands: Sender<ActivationCommand>,
    thread_id: u32,
    owner_completion: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        // Establish per-monitor-v2 awareness so GetWindowRect returns
        // unvirtualized physical pixels. Electron owns the monitor-aware
        // physical-to-DIP conversion before display matching.
        if !*DPI_AWARENESS_READY.get_or_init(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0
        }) {
            return Err(PlatformError::NativeFailure);
        }
        let state = Arc::new(SharedState::new());
        let context = CallbackContext {
            state: Arc::clone(&state),
            keyboard: Mutex::new(CallbackKeyboard::default()),
            hotkeys: Mutex::new(HotKeyRuntime::default()),
            outbound,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
        };
        let (ready_tx, ready_rx) = bounded(1);
        let (owner_completion_tx, owner_completion) = bounded(1);
        let (activation_commands, activation_command_receiver) = bounded(4);
        let thread = thread::Builder::new()
            .name("talking-quill-helper-win-hook".into())
            .spawn(move || {
                hook_thread(
                    context,
                    ready_tx,
                    activation_command_receiver,
                    owner_completion_tx,
                );
            })
            .map_err(|_| PlatformError::ThreadStopped)?;

        let thread_id = match ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) {
                    let _ = thread.join();
                }
                return Err(error);
            }
            Err(_) => {
                gate.close();
                state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) {
                    let _ = thread.join();
                }
                return Err(PlatformError::ThreadStopped);
            }
        };

        Ok(Self {
            state,
            gate,
            terminal,
            activation_commands,
            thread_id,
            owner_completion,
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
        if self.state.stopping.load(Ordering::Acquire) || self.thread.is_none() {
            return Err(PlatformError::ThreadStopped);
        }
        let handoff = Arc::new(AtomicU8::new(ConfigurationHandoff::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        self.activation_commands
            .send_timeout(
                ActivationCommand {
                    requested: ActivationConfig { enabled, bindings },
                    handoff: Arc::clone(&handoff),
                    acknowledgement,
                },
                ACTIVATION_CONFIG_TIMEOUT,
            )
            .map_err(|_| PlatformError::NativeFailure)?;

        if !post_owner_message(self.thread_id, WM_ACTIVATION_CONFIG) {
            let committed = cancel_configuration(&handoff) == ConfigurationHandoff::Committed;
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.terminal
                .trigger(TerminalReason::ActivationConfigurationUnavailable);
            return if committed {
                Ok(())
            } else {
                Err(PlatformError::NativeFailure)
            };
        }

        if let Ok(result) = response.recv_timeout(ACTIVATION_CONFIG_TIMEOUT) {
            return result;
        }

        match configuration_ack_timeout(&handoff) {
            ConfigurationTimeoutAction::Success => return Ok(()),
            ConfigurationTimeoutAction::Failed => {
                self.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                self.terminal
                    .trigger(TerminalReason::ActivationConfigurationUnavailable);
                return Err(PlatformError::NativeFailure);
            }
            ConfigurationTimeoutAction::WakeTerminalCleanup => {}
        }

        // The owner may still complete rollback/terminal cleanup, but neither a
        // permanently failed post nor an expired acknowledgement can block a
        // protocol thread. TerminalRequested prevents a later commit. If the
        // queue never recovers, process exit releases the thread registrations.
        if !post_owner_message(self.thread_id, WM_ACTIVATION_CONFIG)
            || response.recv_timeout(ACTIVATION_CONFIG_TIMEOUT).is_err()
        {
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.terminal
                .trigger(TerminalReason::ActivationConfigurationUnavailable);
            return Err(PlatformError::NativeFailure);
        }

        if configuration_handoff(&handoff) == ConfigurationHandoff::Committed {
            Ok(())
        } else {
            Err(PlatformError::NativeFailure)
        }
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        self.state.session_capture.store(active, Ordering::Release);
        Ok(())
    }

    fn inject_paste(&self) -> PasteResult {
        inject_paste()
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        front_app()
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            accessibility: PermissionState::NotApplicable,
            input_monitoring: PermissionState::NotApplicable,
            event_post: PermissionState::NotApplicable,
        }
    }

    fn shutdown(&mut self) -> Option<TerminalReason> {
        self.gate.close();
        self.state.stopping.store(true, Ordering::Release);
        self.state.session_capture.store(false, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return self.terminal.reason();
        };

        let first_wake = post_owner_message(self.thread_id, WM_QUIT);
        let completed = first_wake
            && (owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)
                || (post_owner_message(self.thread_id, WM_QUIT)
                    && owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)));
        if completed {
            // OwnerCompletion is sent only as the final hook-thread guard drops,
            // after unregistration, unhooking, and callback-context release.
            if thread.join().is_ok() {
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
        } else {
            // Never join an owner whose queue did not accept/dispatch WM_QUIT.
            // Dropping JoinHandle detaches safely: the thread owns its context
            // and Arc resources. If Windows never recovers the queue, process
            // exit performs the unavoidable hook/registration cleanup.
            drop(thread);
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.terminal
                .trigger(TerminalReason::OwnerThreadUnresponsive);
        }
        self.terminal.reason()
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn mark_hotkey_configuration_terminal(
    context: &CallbackContext,
    runtime: &mut HotKeyRuntime,
    registrar: &mut impl HotKeyRegistrar,
) {
    unregister_all_hotkeys(registrar);
    runtime.registrations.config.enabled = false;
    runtime.candidate = ActivationCandidateState::Idle;
    context.state.activation_config.store(
        activation_config_value(false, runtime.registrations.config.bindings),
        Ordering::Release,
    );
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::ActivationConfigurationUnavailable);
}

fn finish_failed_configuration(
    context: &CallbackContext,
    runtime: &mut HotKeyRuntime,
    registrar: &mut impl HotKeyRegistrar,
    handoff: &AtomicU8,
    rollback_failed: bool,
) {
    let mut state = configuration_handoff(handoff);
    if state == ConfigurationHandoff::Pending {
        state = cancel_configuration(handoff);
    }
    if state == ConfigurationHandoff::Cancelled && !rollback_failed {
        state = confirm_configuration_rollback(handoff);
    }
    if rollback_failed || state == ConfigurationHandoff::TerminalRequested {
        mark_hotkey_configuration_terminal(context, runtime, registrar);
        handoff.store(
            ConfigurationHandoff::TerminalComplete as u8,
            Ordering::Release,
        );
    }
}

fn process_activation_commands(
    context: &CallbackContext,
    receiver: &Receiver<ActivationCommand>,
    registrar: &mut impl HotKeyRegistrar,
) {
    while let Ok(command) = receiver.try_recv() {
        let initial_handoff = configuration_handoff(&command.handoff);
        if initial_handoff != ConfigurationHandoff::Pending {
            if let Ok(mut runtime) = context.hotkeys.try_lock() {
                finish_failed_configuration(
                    context,
                    &mut runtime,
                    registrar,
                    &command.handoff,
                    false,
                );
            } else {
                unregister_all_hotkeys(registrar);
                command.handoff.store(
                    ConfigurationHandoff::TerminalComplete as u8,
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
            }
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::NativeFailure));
            continue;
        }

        let mut runtime = match context.hotkeys.try_lock() {
            Ok(runtime) => runtime,
            Err(_) => {
                unregister_all_hotkeys(registrar);
                command.handoff.store(
                    ConfigurationHandoff::TerminalComplete as u8,
                    Ordering::Release,
                );
                let _ = command
                    .acknowledgement
                    .try_send(Err(PlatformError::NativeFailure));
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                continue;
            }
        };
        if runtime.registrations.config != command.requested
            && runtime.candidate.blocks_configuration()
        {
            let _ = cancel_configuration(&command.handoff);
            finish_failed_configuration(context, &mut runtime, registrar, &command.handoff, false);
            let _ = command
                .acknowledgement
                .try_send(Err(PlatformError::NativeFailure));
            continue;
        }

        let previous = runtime.registrations;
        match configure_hotkeys(previous, command.requested, registrar, || {
            configuration_handoff(&command.handoff) == ConfigurationHandoff::Pending
        }) {
            Ok(next) if commit_configuration(&command.handoff) => {
                runtime.registrations = next;
                if next != previous {
                    runtime.candidate = ActivationCandidateState::Idle;
                }
                context.state.activation_config.store(
                    activation_config_value(next.config.enabled, next.config.bindings),
                    Ordering::Release,
                );
                let _ = command.acknowledgement.try_send(Ok(()));
            }
            Ok(next) => {
                let rollback_failed =
                    match configure_hotkeys(next, previous.config, registrar, || true) {
                        Ok(restored) => {
                            runtime.registrations = restored;
                            runtime.candidate = ActivationCandidateState::Idle;
                            context.state.activation_config.store(
                                activation_config_value(
                                    restored.config.enabled,
                                    restored.config.bindings,
                                ),
                                Ordering::Release,
                            );
                            false
                        }
                        Err(_) => true,
                    };
                finish_failed_configuration(
                    context,
                    &mut runtime,
                    registrar,
                    &command.handoff,
                    rollback_failed,
                );
                let _ = command
                    .acknowledgement
                    .try_send(Err(PlatformError::NativeFailure));
            }
            Err(HotKeyTransactionError::RollbackFailed) => {
                finish_failed_configuration(
                    context,
                    &mut runtime,
                    registrar,
                    &command.handoff,
                    true,
                );
                let _ = command
                    .acknowledgement
                    .try_send(Err(PlatformError::NativeFailure));
            }
            Err(HotKeyTransactionError::Conflict | HotKeyTransactionError::Cancelled) => {
                finish_failed_configuration(
                    context,
                    &mut runtime,
                    registrar,
                    &command.handoff,
                    false,
                );
                let _ = command
                    .acknowledgement
                    .try_send(Err(PlatformError::NativeFailure));
            }
        }
    }
}

fn disable_hotkeys_after_delivery_failure(context: &CallbackContext) {
    // This callback runs on the RegisterHotKey owner thread. The current chord
    // cannot be replayed safely after Windows consumed it, but unregistering
    // synchronously makes every subsequent chord fail open.
    let mut registrar = WindowsHotKeyRegistrar;
    unregister_all_hotkeys(&mut registrar);
    if let Ok(mut runtime) = context.hotkeys.try_lock() {
        runtime.registrations.config.enabled = false;
        runtime.registrations.generation = runtime.registrations.generation.wrapping_add(1);
        context.state.activation_config.store(
            activation_config_value(false, runtime.registrations.config.bindings),
            Ordering::Release,
        );
    }
}

fn mark_activation_candidate_consumed(context: &CallbackContext, candidate: ActivationCandidate) {
    if let Ok(mut runtime) = context.hotkeys.try_lock()
        && runtime.candidate.candidate() == Some(candidate)
    {
        runtime.candidate = ActivationCandidateState::ConsumedDown(candidate);
    }
}

fn complete_activation_candidate(context: &CallbackContext, candidate: ActivationCandidate) {
    if let Ok(mut runtime) = context.hotkeys.try_lock()
        && runtime.candidate.candidate() == Some(candidate)
    {
        runtime.candidate = ActivationCandidateState::Completed(candidate);
    }
}

fn handle_hotkey_message(context: &CallbackContext, w_param: WPARAM, l_param: LPARAM) {
    let Ok(id) = i32::try_from(w_param) else {
        return;
    };

    let (registrations, candidate, released_before_message) = {
        let mut runtime = match context.hotkeys.try_lock() {
            Ok(runtime) => runtime,
            Err(_) => {
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                return;
            }
        };
        let registrations = runtime.registrations;
        if !matches!(
            runtime.candidate,
            ActivationCandidateState::Pressed(_)
                | ActivationCandidateState::ReleasedBeforeMessage(_)
        ) {
            return;
        }
        let Some(candidate) = runtime.candidate.candidate().and_then(|candidate| {
            resolve_activation_candidate_message(candidate, registrations, id, l_param)
        }) else {
            return;
        };
        let released_before_message = matches!(
            runtime.candidate,
            ActivationCandidateState::ReleasedBeforeMessage(_)
        );
        runtime.candidate = ActivationCandidateState::Accepted(candidate);
        (registrations, candidate, released_before_message)
    };

    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            disable_hotkeys_after_delivery_failure(context);
            if released_before_message {
                complete_activation_candidate(context, candidate);
            } else {
                mark_activation_candidate_consumed(context, candidate);
            }
            return;
        }
    };
    let down = keyboard.reducer.plan_registered_binding(
        candidate.key,
        candidate.shift,
        registrations.config.bindings,
        registrations.config.enabled,
    );
    let down_delivered = down.event().is_none()
        || deliver_callback_event_with_session_arm(
            &context.outbound,
            &context.terminal,
            &context.state.session_capture,
            down.event().expect("event checked above"),
        );
    if !down_delivered {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
    }
    let _ = keyboard.reducer.apply(down, down_delivered);
    if !down_delivered {
        drop(keyboard);
        disable_hotkeys_after_delivery_failure(context);
        if released_before_message {
            complete_activation_candidate(context, candidate);
        } else {
            // RegisterHotKey already consumed the OS chord. Replaying is unsafe;
            // retain only a balancing marker so the physical up is swallowed.
            mark_activation_candidate_consumed(context, candidate);
        }
        return;
    }

    if released_before_message {
        let up = keyboard.reducer.plan_passive_bindings(
            KeyInput {
                key: PhysicalKey::Letter(candidate.key),
                phase: KeyPhase::Up,
                alt: false,
                shift: false,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            registrations.config.bindings,
            registrations.config.enabled,
            false,
        );
        let up_delivered = up.event().is_none()
            || deliver_callback_event(
                &context.outbound,
                &context.terminal,
                up.event().expect("event checked above"),
            );
        if !up_delivered {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
        }
        let _ = keyboard.reducer.apply(up, up_delivered);
        drop(keyboard);
        if !up_delivered {
            disable_hotkeys_after_delivery_failure(context);
        }
        complete_activation_candidate(context, candidate);
    }
}

fn drain_queued_hotkey_messages(context: &CallbackContext) {
    let mut message = MSG::default();
    loop {
        // SAFETY: called only on the message-queue owner thread. PM_REMOVE
        // transfers one complete thread WM_HOTKEY message into local storage.
        let found = unsafe {
            PeekMessageW(
                &raw mut message,
                null_mut(),
                WM_HOTKEY,
                WM_HOTKEY,
                PM_REMOVE,
            )
        };
        if found == 0 {
            return;
        }
        handle_hotkey_message(context, message.wParam, message.lParam);
    }
}

fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<u32, PlatformError>>,
    activation_commands: Receiver<ActivationCommand>,
    owner_completion: Sender<()>,
) {
    // Declared first so it drops last, after every owner-thread resource.
    let _owner_completion = OwnerCompletion(owner_completion);
    let mut context = Box::new(context);
    let context_ptr = (&raw mut *context).cast::<CallbackContext>();
    if CALLBACK_CONTEXT
        .compare_exchange(null_mut(), context_ptr, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }

    // SAFETY: the callback has the required system ABI, the context pointer is
    // kept alive for the complete hook lifetime, and this thread owns the
    // Windows message loop which dispatches low-level hook callbacks.
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), null_mut(), 0) };
    if hook.is_null() {
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }

    // Low-level callbacks are delivered through this owning thread's message
    // loop. Snapshot after installation but before dispatch or readiness, so a
    // key already held when the hook was installed is seeded before its first
    // queued repeat can be reduced.
    let physical = physical_tracker_from_state(key_is_down);
    let shift = ShiftKeyTracker::from_state(key_is_down);
    let keyboard = match context.keyboard.get_mut() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            // SAFETY: `hook` was installed successfully above and remains owned
            // by this thread; no callback has been dispatched.
            unsafe { UnhookWindowsHookEx(hook) };
            CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
    };
    keyboard.physical = physical;
    keyboard.shift = shift;

    let mut message = MSG::default();
    // SAFETY: a no-remove peek creates this owner thread's message queue before
    // readiness, so later PostThreadMessageW configuration commands are valid.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };
    let mut registrar = WindowsHotKeyRegistrar;

    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Ready), Ordering::Release);
    // SAFETY: this call only reads the current native thread identifier.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        unregister_all_hotkeys(&mut registrar);
        // SAFETY: `hook` is valid and owned by this thread.
        unsafe { UnhookWindowsHookEx(hook) };
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        return;
    }

    loop {
        // SAFETY: `message` is valid writable storage. A null HWND requests
        // all messages for the current hook thread.
        let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        match message.message {
            WM_ACTIVATION_CONFIG => {
                // Resolve an activation whose up raced its queued WM_HOTKEY
                // before allowing a configuration generation to change.
                drain_queued_hotkey_messages(&context);
                process_activation_commands(&context, &activation_commands, &mut registrar);
            }
            WM_HOTKEY => handle_hotkey_message(&context, message.wParam, message.lParam),
            _ => {
                // SAFETY: `message` was initialized by GetMessageW.
                unsafe {
                    TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
        }
    }

    context.gate.close();
    unregister_all_hotkeys(&mut registrar);
    while let Ok(command) = activation_commands.try_recv() {
        command.handoff.store(
            ConfigurationHandoff::TerminalComplete as u8,
            Ordering::Release,
        );
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // SAFETY: `hook` remains owned by this thread and is unhooked exactly once.
    unsafe { UnhookWindowsHookEx(hook) };
    CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
}

unsafe extern "system" fn keyboard_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    let result = std::panic::catch_unwind(|| {
        if code != HC_ACTION as i32 {
            return None;
        }
        let context_ptr = CALLBACK_CONTEXT.load(Ordering::Acquire);
        if context_ptr.is_null() {
            return None;
        }
        // SAFETY: the hook thread stores this pointer before installation and
        // clears it only after unhooking. Windows invokes this callback on that
        // same thread.
        let context = unsafe { &*context_ptr };

        // SAFETY: for HC_ACTION Windows documents l_param as a valid pointer to
        // KBDLLHOOKSTRUCT for the duration of this callback.
        let native = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
        let phase = match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyPhase::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyPhase::Up,
            _ => return None,
        };
        let injected = native.flags & LLKHF_INJECTED != 0 || native.dwExtraInfo == INJECTED_MARKER;
        if injected {
            return None;
        }
        let key = map_virtual_key(native.vkCode);
        if key == PhysicalKey::Other {
            if matches!(native.vkCode as u16, VK_SHIFT | VK_LSHIFT | VK_RSHIFT) {
                let mut keyboard = match context.keyboard.try_lock() {
                    Ok(keyboard) => keyboard,
                    Err(_) => {
                        context.state.hook_status.store(
                            hook_status_to_u8(HookStatus::Unavailable),
                            Ordering::Release,
                        );
                        context.terminal.trigger(TerminalReason::ReducerPoisoned);
                        return None;
                    }
                };
                keyboard.shift.observe(native.vkCode as u16, phase);
            }
            return None;
        }

        // Native modifier queries are only relevant to activation letters.
        // Enter and Escape capture depends solely on the active session gate.
        let (alt, disallowed_modifiers) = if matches!(key, PhysicalKey::Letter(_)) {
            (
                native.flags & LLKHF_ALTDOWN != 0 || key_is_down(VK_MENU),
                activation_disallowed_modifiers(key_is_down),
            )
        } else {
            (false, false)
        };
        let mut input = KeyInput {
            key,
            phase,
            alt,
            shift: false,
            disallowed_modifiers,
            repeat: false,
            injected: false,
        };

        {
            let mut keyboard = match context.keyboard.try_lock() {
                Ok(keyboard) => keyboard,
                Err(_) => {
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context.terminal.trigger(TerminalReason::ReducerPoisoned);
                    return None;
                }
            };
            input.shift = keyboard.shift.is_down();
            input.repeat = keyboard.physical.observe(key, phase);
        }

        // Record only a fresh, exact registered chord. The low-level down is
        // deliberately passed: RegisterHotKey performs the OS suppression and
        // WM_HOTKEY remains the reducer's establishment point.
        if context.gate.is_open()
            && phase == KeyPhase::Down
            && let PhysicalKey::Letter(letter) = key
        {
            let mut runtime = match context.hotkeys.try_lock() {
                Ok(runtime) => runtime,
                Err(_) => {
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context.terminal.trigger(TerminalReason::ReducerPoisoned);
                    return None;
                }
            };
            runtime.candidate = candidate_for_exact_passive_down(
                runtime.registrations,
                runtime.candidate,
                letter,
                alt,
                input.shift,
                input.disallowed_modifiers,
                input.repeat,
            );
        }

        // An up may run before its posted WM_HOTKEY is dispatched. Drain first
        // so accepted downs are always reduced before their balancing ups.
        let mut candidate_up = CandidateUpAction::Pass;
        if phase == KeyPhase::Up
            && let PhysicalKey::Letter(letter) = key
        {
            drain_queued_hotkey_messages(context);
            let mut runtime = match context.hotkeys.try_lock() {
                Ok(runtime) => runtime,
                Err(_) => {
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context.terminal.trigger(TerminalReason::ReducerPoisoned);
                    return None;
                }
            };
            (runtime.candidate, candidate_up) =
                release_activation_candidate(runtime.candidate, letter);
        }

        if matches!(candidate_up, CandidateUpAction::SwallowAwaitMessage(_)) {
            return Some(true);
        }

        // Physical state is updated even while the gate is closed, but a gate
        // transition must not bypass the balancing up of an OS-consumed down.
        let must_balance = matches!(
            candidate_up,
            CandidateUpAction::BalanceAccepted(_) | CandidateUpAction::SwallowConsumed(_)
        );
        if !context.gate.is_open() && !must_balance {
            return None;
        }

        let (activation_enabled, configured) =
            activation_config_from_value(context.state.activation_config.load(Ordering::Acquire));
        let capture = context.state.session_capture.load(Ordering::Acquire);
        let mut keyboard = match context.keyboard.try_lock() {
            Ok(keyboard) => keyboard,
            Err(_) => {
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                return if must_balance { Some(true) } else { None };
            }
        };
        let plan =
            keyboard
                .reducer
                .plan_passive_bindings(input, configured, activation_enabled, capture);
        let delivered = plan.event().is_none()
            || (context.gate.is_open()
                && deliver_callback_event(
                    &context.outbound,
                    &context.terminal,
                    plan.event().expect("event checked above"),
                ));
        if !delivered {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
        }
        let reducer_swallow = keyboard.reducer.apply(plan, delivered);
        drop(keyboard);

        if let CandidateUpAction::BalanceAccepted(candidate)
        | CandidateUpAction::SwallowConsumed(candidate) = candidate_up
        {
            complete_activation_candidate(context, candidate);
            if !delivered {
                disable_hotkeys_after_delivery_failure(context);
            }
            Some(true)
        } else {
            Some(reducer_swallow)
        }
    });

    match result {
        Ok(Some(true)) => 1,
        Ok(Some(false)) | Ok(None) => {
            // SAFETY: passing null for the ignored hook handle is documented for
            // low-level hooks; the original callback arguments are unchanged.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
        Err(_) => {
            let context_ptr = CALLBACK_CONTEXT.load(Ordering::Acquire);
            if !context_ptr.is_null() {
                // SAFETY: the callback context remains alive until unhooking.
                let context = unsafe { &*context_ptr };
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::CallbackPanicked);
            }
            // SAFETY: fail open with the original callback arguments.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
    }
}

fn activation_disallowed_modifiers(mut is_down: impl FnMut(u16) -> bool) -> bool {
    let ctrl = is_down(VK_CONTROL);
    let left_win = is_down(VK_LWIN);
    let right_win = is_down(VK_RWIN);
    ctrl || left_win || right_win
}

pub(super) fn key_is_down(key: u16) -> bool {
    // SAFETY: GetAsyncKeyState has no pointer preconditions.
    unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
}

fn physical_tracker_from_state(mut is_down: impl FnMut(u16) -> bool) -> PhysicalKeyTracker {
    let mut tracker = PhysicalKeyTracker::default();
    for code in 0x41_u16..=0x5A {
        if is_down(code) {
            tracker.observe(map_virtual_key(u32::from(code)), KeyPhase::Down);
        }
    }
    for (code, key) in [
        (VK_ESCAPE, PhysicalKey::Escape),
        (VK_RETURN, PhysicalKey::Enter),
    ] {
        if is_down(code) {
            tracker.observe(key, KeyPhase::Down);
        }
    }
    tracker
}

fn map_virtual_key(code: u32) -> PhysicalKey {
    if (0x41..=0x5A).contains(&code) {
        let index = u8::try_from(code - 0x41).expect("A-Z range fits in u8");
        return PhysicalKey::Letter(
            ActivationKey::from_index(index).expect("A-Z maps to activation key"),
        );
    }
    if code == u32::from(VK_ESCAPE) {
        PhysicalKey::Escape
    } else if code == u32::from(VK_RETURN) {
        PhysicalKey::Enter
    } else {
        PhysicalKey::Other
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    fn bindings(values: &[(ActivationKey, bool)]) -> ActivationBindings {
        ActivationBindings::from_exact(values).unwrap()
    }

    #[test]
    fn owner_wake_retries_post_failures_and_stops_after_exhaustion() {
        let mut outcomes = VecDeque::from([false, false, true]);
        let mut delays = Vec::new();
        assert!(retry_owner_wake(
            || outcomes.pop_front().unwrap_or(false),
            |delay| delays.push(delay),
        ));
        assert_eq!(delays, OWNER_WAKE_RETRY_DELAYS[..2]);

        let mut attempts = 0;
        let mut delays = Vec::new();
        assert!(!retry_owner_wake(
            || {
                attempts += 1;
                false
            },
            |delay| delays.push(delay),
        ));
        assert_eq!(attempts, OWNER_WAKE_RETRY_DELAYS.len() + 1);
        assert_eq!(delays, OWNER_WAKE_RETRY_DELAYS);
    }

    #[test]
    fn owner_completion_wait_is_bounded_and_accepts_normal_completion() {
        let (completed_tx, completed_rx) = bounded(1);
        completed_tx.send(()).unwrap();
        assert!(owner_completed(&completed_rx, Duration::from_millis(1)));

        let (_pending_tx, pending_rx) = bounded(1);
        assert!(!owner_completed(&pending_rx, Duration::from_millis(1)));
    }

    #[test]
    fn activation_config_is_atomic_and_defaults_disabled() {
        let state = SharedState::new();
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (false, ActivationBindings::default())
        );
        state.activation_config.store(
            activation_config_value(
                true,
                bindings(&[(ActivationKey::B, false), (ActivationKey::B, true)]),
            ),
            Ordering::Release,
        );
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (
                true,
                bindings(&[(ActivationKey::B, false), (ActivationKey::B, true)])
            )
        );

        for enabled in [false, true] {
            for index in 0..26 {
                let key = ActivationKey::from_index(index).unwrap();
                assert_eq!(
                    activation_config_from_value(activation_config_value(
                        enabled,
                        bindings(&[(key, false), (key, true)])
                    )),
                    (enabled, bindings(&[(key, false), (key, true)]))
                );
            }
        }
    }

    #[test]
    fn ctrl_and_windows_keys_exhaustively_disallow_activation() {
        for mask in 0_u8..8 {
            let mut queried = Vec::new();
            let disallowed = activation_disallowed_modifiers(|key| {
                queried.push(key);
                match key {
                    VK_CONTROL => mask & 0b001 != 0,
                    VK_LWIN => mask & 0b010 != 0,
                    VK_RWIN => mask & 0b100 != 0,
                    _ => false,
                }
            });
            assert_eq!(disallowed, mask != 0, "mask {mask:03b}");
            assert_eq!(queried, [VK_CONTROL, VK_LWIN, VK_RWIN]);
        }

        for modifier in [VK_CONTROL, VK_LWIN, VK_RWIN, VK_MENU, VK_SHIFT] {
            assert_eq!(map_virtual_key(u32::from(modifier)), PhysicalKey::Other);
        }
    }

    #[test]
    fn post_install_snapshot_queries_and_seeds_every_tracked_physical_key() {
        let held = [0x41, 0x5A, VK_ESCAPE, VK_RETURN];
        let mut queried = Vec::new();
        let mut tracker = physical_tracker_from_state(|code| {
            queried.push(code);
            held.contains(&code)
        });

        let mut expected_queries = (0x41_u16..=0x5A).collect::<Vec<_>>();
        expected_queries.extend([VK_ESCAPE, VK_RETURN]);
        assert_eq!(queried, expected_queries);

        for code in 0x41_u16..=0x5A {
            assert_eq!(
                tracker.observe(map_virtual_key(u32::from(code)), KeyPhase::Down),
                held.contains(&code),
                "virtual key {code:#x}"
            );
        }
        assert!(tracker.observe(PhysicalKey::Escape, KeyPhase::Down));
        assert!(tracker.observe(PhysicalKey::Enter, KeyPhase::Down));
    }

    #[test]
    fn post_install_held_keys_cannot_begin_activation_or_session_capture() {
        let mut tracker =
            physical_tracker_from_state(|code| matches!(code, 0x41 | VK_ESCAPE | VK_RETURN));
        let mut reducer = KeyboardReducer::default();

        for (key, alt, capture) in [
            (PhysicalKey::Letter(ActivationKey::A), true, false),
            (PhysicalKey::Escape, false, true),
            (PhysicalKey::Enter, false, true),
        ] {
            let repeat = tracker.observe(key, KeyPhase::Down);
            assert!(repeat);
            let plan = reducer.plan(
                KeyInput {
                    key,
                    phase: KeyPhase::Down,
                    alt,
                    shift: false,
                    disallowed_modifiers: false,
                    repeat,
                    injected: false,
                },
                ActivationKey::A,
                true,
                capture,
            );
            assert!(plan.event().is_none());
            assert!(!reducer.apply(plan, true));
        }
    }

    #[test]
    fn shift_tracker_preserves_both_sides_and_aggregate_only_keyboards() {
        let mut tracker = ShiftKeyTracker::from_state(|code| code == VK_LSHIFT);
        assert!(tracker.is_down());
        tracker.observe(VK_RSHIFT, KeyPhase::Down);
        tracker.observe(VK_LSHIFT, KeyPhase::Up);
        assert!(tracker.is_down());
        tracker.observe(VK_RSHIFT, KeyPhase::Up);
        assert!(!tracker.is_down());

        let mut aggregate = ShiftKeyTracker::from_state(|code| code == VK_SHIFT);
        assert!(aggregate.is_down());
        aggregate.observe(VK_SHIFT, KeyPhase::Up);
        assert!(!aggregate.is_down());
    }
}
