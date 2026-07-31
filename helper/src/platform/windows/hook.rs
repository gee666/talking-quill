use std::{
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering},
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
            GetAsyncKeyState, GetKeyboardLayout, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW, VK_CONTROL,
            VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN,
            VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW,
            GetWindowThreadProcessId, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED,
            MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
            TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP,
            WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use super::{front_app::front_app, paste::inject_paste};
use crate::{
    keyboard::{
        ActivationBindings, ActivationKey, HelperEvent, KeyInput, KeyPhase, KeyboardReducer,
        ModifierMask, PhysicalKey, PhysicalKeyTracker,
    },
    platform::{
        CallbackGate, FrontApp, HookStatus, PasteResult, PermissionState, Permissions, Platform,
        PlatformError, TerminalReason, TerminalSignal, deliver_callback_event, hook_status_from_u8,
        hook_status_to_u8,
    },
    protocol::Outbound,
};

#[cfg(target_pointer_width = "64")]
pub(super) const INJECTED_MARKER: usize = 0x4D45_4348_4F50_5354;
#[cfg(target_pointer_width = "32")]
pub(super) const INJECTED_MARKER: usize = 0x4F50_5354;

const WM_OWNER_COMMAND: u32 = WM_APP + 0x45;
const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_WAKE_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
];
static DPI_AWARENESS_READY: OnceLock<bool> = OnceLock::new();
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(null_mut());

/// Set-1 scan codes for the physical positions exposed by DOM `KeyboardEvent.code`.
/// The array index is the corresponding `ActivationKey` discriminant.
const LETTER_SCAN_CODES: [u32; 26] = [
    0x1E, // KeyA
    0x30, // KeyB
    0x2E, // KeyC
    0x20, // KeyD
    0x12, // KeyE
    0x21, // KeyF
    0x22, // KeyG
    0x23, // KeyH
    0x17, // KeyI
    0x24, // KeyJ
    0x25, // KeyK
    0x26, // KeyL
    0x32, // KeyM
    0x31, // KeyN
    0x18, // KeyO
    0x19, // KeyP
    0x10, // KeyQ
    0x13, // KeyR
    0x1F, // KeyS
    0x14, // KeyT
    0x16, // KeyU
    0x2F, // KeyV
    0x11, // KeyW
    0x2D, // KeyX
    0x15, // KeyY
    0x2C, // KeyZ
];

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
            // SAFETY: owner messages are pointer-free and target the thread
            // whose queue is created before startup readiness is reported.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum StartupState {
    Pending,
    Running,
    Cancelled,
}

fn claim_startup(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            StartupState::Pending as u8,
            StartupState::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn cancel_startup(state: &AtomicU8) -> StartupState {
    match state.compare_exchange(
        StartupState::Pending as u8,
        StartupState::Cancelled as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => StartupState::Cancelled,
        Err(value) if value == StartupState::Running as u8 => StartupState::Running,
        Err(_) => StartupState::Cancelled,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ActivationConfig {
    enabled: bool,
    bindings: ActivationBindings,
}

struct SharedState {
    session_capture: AtomicBool,
    hook_status: AtomicU8,
    stopping: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            session_capture: AtomicBool::new(false),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            stopping: AtomicBool::new(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ModifierSides {
    left: bool,
    right: bool,
    generic: bool,
}

impl ModifierSides {
    fn from_state(
        left: u16,
        right: u16,
        generic: Option<u16>,
        is_down: &mut impl FnMut(u16) -> bool,
    ) -> Self {
        let left_down = is_down(left);
        let right_down = is_down(right);
        Self {
            left: left_down,
            right: right_down,
            generic: !left_down && !right_down && generic.is_some_and(is_down),
        }
    }

    fn observe_left(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.left = phase == KeyPhase::Down;
    }

    fn observe_right(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.right = phase == KeyPhase::Down;
    }

    fn observe_generic(&mut self, phase: KeyPhase) {
        self.generic = phase == KeyPhase::Down;
    }

    const fn is_down(self) -> bool {
        self.left || self.right || self.generic
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ModifierTracker {
    ctrl: ModifierSides,
    alt: ModifierSides,
    shift: ModifierSides,
    meta: ModifierSides,
}

impl ModifierTracker {
    fn from_state(mut is_down: impl FnMut(u16) -> bool) -> Self {
        Self {
            ctrl: ModifierSides::from_state(
                VK_LCONTROL,
                VK_RCONTROL,
                Some(VK_CONTROL),
                &mut is_down,
            ),
            alt: ModifierSides::from_state(VK_LMENU, VK_RMENU, Some(VK_MENU), &mut is_down),
            shift: ModifierSides::from_state(VK_LSHIFT, VK_RSHIFT, Some(VK_SHIFT), &mut is_down),
            meta: ModifierSides::from_state(VK_LWIN, VK_RWIN, None, &mut is_down),
        }
    }

    fn observe(
        &mut self,
        virtual_key: u16,
        scan_code: u32,
        extended: bool,
        phase: KeyPhase,
    ) -> bool {
        match virtual_key {
            VK_LCONTROL => self.ctrl.observe_left(phase),
            VK_RCONTROL => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D && extended => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D => self.ctrl.observe_left(phase),
            VK_CONTROL => self.ctrl.observe_generic(phase),
            VK_LMENU => self.alt.observe_left(phase),
            VK_RMENU => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 && extended => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 => self.alt.observe_left(phase),
            VK_MENU => self.alt.observe_generic(phase),
            VK_LSHIFT => self.shift.observe_left(phase),
            VK_RSHIFT => self.shift.observe_right(phase),
            VK_SHIFT if scan_code == 0x2A => self.shift.observe_left(phase),
            VK_SHIFT if scan_code == 0x36 => self.shift.observe_right(phase),
            VK_SHIFT => self.shift.observe_generic(phase),
            VK_LWIN => self.meta.observe_left(phase),
            VK_RWIN => self.meta.observe_right(phase),
            _ => return false,
        }
        true
    }

    const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.ctrl.is_down(),
            self.alt.is_down(),
            self.shift.is_down(),
            self.meta.is_down(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InjectionKind {
    Physical,
    External,
    Helper,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HookObservation {
    observed_at_ms: u64,
    native_modifiers: Option<ModifierTracker>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnterSource {
    Main,
    Numpad,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct WindowsPhysicalTracker {
    common: PhysicalKeyTracker,
    main_enter_held: bool,
    numpad_enter_held: bool,
}

impl WindowsPhysicalTracker {
    fn observe(
        &mut self,
        key: PhysicalKey,
        enter_source: Option<EnterSource>,
        phase: KeyPhase,
    ) -> bool {
        if key != PhysicalKey::Enter {
            return self.common.observe(key, phase);
        }
        let held = match enter_source {
            Some(EnterSource::Main) => &mut self.main_enter_held,
            Some(EnterSource::Numpad) => &mut self.numpad_enter_held,
            None => return false,
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

    fn seed_enter_preheld(&mut self) {
        // GetAsyncKeyState exposes both physical Enter sources as VK_RETURN.
        // Conservatively fence each source until its own observed up.
        self.main_enter_held = true;
        self.numpad_enter_held = true;
    }

    fn held_letter_bits(&self) -> u32 {
        self.common.held_letter_bits()
    }
}

#[derive(Default)]
struct CallbackKeyboard {
    reducer: KeyboardReducer,
    physical: WindowsPhysicalTracker,
    modifiers: ModifierTracker,
    activation_fenced_letters: u32,
    activation: ActivationConfig,
    captured_enter_source: Option<EnterSource>,
    altgr_active: bool,
    modifiers_fenced: bool,
}

struct CallbackContext {
    state: Arc<SharedState>,
    keyboard: Mutex<CallbackKeyboard>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerMutationKind {
    Configure,
    SetSessionCapture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnerMutation {
    kind: OwnerMutationKind,
    activation: ActivationConfig,
    session_capture: bool,
}

impl OwnerMutation {
    fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture: false,
        }
    }

    fn set_session_capture(active: bool) -> Self {
        Self {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture: active,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum OwnerCommandState {
    Pending,
    Applying,
    Applied,
    Cancelled,
}

fn owner_command_state(state: &AtomicU8) -> OwnerCommandState {
    match state.load(Ordering::Acquire) {
        1 => OwnerCommandState::Applying,
        2 => OwnerCommandState::Applied,
        3 => OwnerCommandState::Cancelled,
        _ => OwnerCommandState::Pending,
    }
}

fn claim_owner_command(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            OwnerCommandState::Pending as u8,
            OwnerCommandState::Applying as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn cancel_owner_command(state: &AtomicU8) -> OwnerCommandState {
    match state.compare_exchange(
        OwnerCommandState::Pending as u8,
        OwnerCommandState::Cancelled as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => OwnerCommandState::Cancelled,
        Err(_) => owner_command_state(state),
    }
}

struct OwnerCommand {
    mutation: OwnerMutation,
    state: Arc<AtomicU8>,
    acknowledgement: Sender<Result<(), PlatformError>>,
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    owner_commands: Sender<OwnerCommand>,
    thread_id: u32,
    owner_completion: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl NativePlatform {
    fn submit_owner_mutation(
        &self,
        mutation: OwnerMutation,
        terminal_reason: TerminalReason,
    ) -> Result<(), PlatformError> {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return Err(PlatformError::ThreadStopped);
        }

        let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        self.owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .map_err(|_| PlatformError::NativeFailure)?;

        if !post_owner_message(self.thread_id, WM_OWNER_COMMAND)
            && cancel_owner_command(&command_state) == OwnerCommandState::Cancelled
        {
            self.mark_owner_failure(terminal_reason);
            return Err(PlatformError::NativeFailure);
        }

        if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
            return result;
        }

        match cancel_owner_command(&command_state) {
            OwnerCommandState::Applied => Ok(()),
            OwnerCommandState::Applying => {
                if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
                    return result;
                }
                if owner_command_state(&command_state) == OwnerCommandState::Applied {
                    Ok(())
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    Err(PlatformError::NativeFailure)
                }
            }
            OwnerCommandState::Pending | OwnerCommandState::Cancelled => {
                self.mark_owner_failure(terminal_reason);
                Err(PlatformError::NativeFailure)
            }
        }
    }

    fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        // Establish per-monitor-v2 awareness so front-app bounds are returned
        // in unvirtualized physical pixels.
        if !*DPI_AWARENESS_READY.get_or_init(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0
        }) {
            return Err(PlatformError::NativeFailure);
        }

        let state = Arc::new(SharedState::new());
        let context = CallbackContext {
            state: Arc::clone(&state),
            keyboard: Mutex::new(CallbackKeyboard::default()),
            outbound,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
        };
        let (ready_tx, ready_rx) = bounded(1);
        let startup_state = Arc::new(AtomicU8::new(StartupState::Pending as u8));
        let (owner_completion_tx, owner_completion) = bounded(1);
        let (owner_commands, owner_command_receiver) = bounded(8);
        let owner_startup_state = Arc::clone(&startup_state);
        let thread = thread::Builder::new()
            .name("talking-quill-helper-win-hook".into())
            .spawn(move || {
                hook_thread(
                    context,
                    ready_tx,
                    owner_startup_state,
                    owner_command_receiver,
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
                state.stopping.store(true, Ordering::Release);
                state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                if cancel_startup(&startup_state) == StartupState::Running
                    && let Ok(Ok(late_thread_id)) = ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT)
                {
                    let _ = post_owner_message(late_thread_id, WM_QUIT);
                }
                drop(ready_rx);
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
            owner_commands,
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
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(active),
            TerminalReason::OwnerThreadUnresponsive,
        )
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
        let Some(thread) = self.thread.take() else {
            return self.terminal.reason();
        };

        let _ = post_owner_message(self.thread_id, WM_QUIT);
        let completed = owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT) || {
            // Always re-check completion: the owner may exit before either
            // retry post, making that post fail after completion was queued.
            let _ = post_owner_message(self.thread_id, WM_QUIT);
            owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)
        };
        if completed {
            if thread.join().is_ok() {
                self.state
                    .hook_status
                    .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
            } else {
                self.mark_owner_failure(TerminalReason::HookStopped);
            }
        } else {
            // Never join an owner whose queue did not dispatch WM_QUIT. The
            // process owns the detached hook resources until imminent exit.
            drop(thread);
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
        }
        self.terminal.reason()
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn process_owner_commands(context: &CallbackContext, receiver: &Receiver<OwnerCommand>) {
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
            OwnerMutationKind::Configure => match context.keyboard.try_lock() {
                Ok(mut keyboard) => {
                    // Preserve accepted trigger ownership solely for balancing
                    // up, but fence every passive prefix across this binding
                    // revision until all letters held at the revision release.
                    keyboard.reducer.fence_activation_revision();
                    keyboard.activation_fenced_letters = keyboard.physical.held_letter_bits();
                    keyboard.modifiers_fenced =
                        keyboard.modifiers.mask() != ModifierMask::default();
                    keyboard.activation = command.mutation.activation;
                    true
                }
                Err(_) => false,
            },
            OwnerMutationKind::SetSessionCapture => {
                context
                    .state
                    .session_capture
                    .store(command.mutation.session_capture, Ordering::Release);
                true
            }
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

fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<u32, PlatformError>>,
    startup_state: Arc<AtomicU8>,
    owner_commands: Receiver<OwnerCommand>,
    owner_completion: Sender<()>,
) {
    // Declared first so it drops last, after all owner-thread resources.
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

    let mut message = MSG::default();
    // SAFETY: this no-remove peek creates the owner queue before hook
    // installation, so low-level callbacks always have a live message loop.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };

    // SAFETY: the callback has the system ABI, and the boxed context remains
    // alive and registered until this owner thread unhooks.
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), null_mut(), 0) };
    if hook.is_null() {
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }

    // Low-level callbacks are delivered on this message-loop thread. Seed all
    // tracked physical state after installation and before readiness so a key
    // already held cannot begin an activation or session sequence.
    let physical = physical_tracker_from_state(native_physical_key_is_down);
    let modifiers = ModifierTracker::from_state(key_is_down);
    let keyboard = match context.keyboard.get_mut() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            // SAFETY: `hook` is installed and still owned by this thread.
            unsafe { UnhookWindowsHookEx(hook) };
            CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
    };
    keyboard.physical = physical;
    keyboard.modifiers = modifiers;
    keyboard.modifiers_fenced = keyboard.modifiers.mask() != ModifierMask::default();

    if !claim_startup(&startup_state) {
        // SAFETY: `hook` is valid and owned by this thread.
        unsafe { UnhookWindowsHookEx(hook) };
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        return;
    }
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Ready), Ordering::Release);
    // SAFETY: reads the current native thread identifier.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() || context.state.stopping.load(Ordering::Acquire) {
        // SAFETY: `hook` is valid and owned by this thread.
        unsafe { UnhookWindowsHookEx(hook) };
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        return;
    }

    loop {
        // SAFETY: `message` is valid writable storage; null HWND selects all
        // messages for this hook owner thread.
        let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_OWNER_COMMAND {
            process_owner_commands(&context, &owner_commands);
        } else {
            // SAFETY: `message` was initialized by GetMessageW.
            unsafe {
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }

    context.gate.close();
    context
        .state
        .session_capture
        .store(false, Ordering::Release);
    while let Ok(command) = owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
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
    // SAFETY: `hook` remains owned by this thread and is unhooked once.
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
        // SAFETY: the owner stores this pointer before hook installation and
        // clears it only after unhooking on the same thread.
        let context = unsafe { &*context_ptr };
        // SAFETY: HC_ACTION defines l_param as a valid KBDLLHOOKSTRUCT pointer
        // for this callback's duration.
        let native = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
        let phase = match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyPhase::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyPhase::Up,
            _ => return None,
        };
        let injection = if native.dwExtraInfo == INJECTED_MARKER {
            InjectionKind::Helper
        } else if native.flags & LLKHF_INJECTED != 0 {
            InjectionKind::External
        } else {
            InjectionKind::Physical
        };
        let extended = native.flags & LLKHF_EXTENDED != 0;
        Some(process_hook_record_at(
            context,
            native.vkCode as u16,
            native.scanCode,
            extended,
            phase,
            injection,
            HookObservation {
                observed_at_ms: u64::from(native.time),
                native_modifiers: (injection == InjectionKind::Physical)
                    .then(|| ModifierTracker::from_state(key_is_down)),
            },
        ))
    });

    match result {
        Ok(Some(true)) => 1,
        Ok(Some(false)) | Ok(None) => {
            // SAFETY: the ignored hook handle may be null; original arguments
            // are forwarded unchanged.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
        Err(_) => {
            let context_ptr = CALLBACK_CONTEXT.load(Ordering::Acquire);
            if !context_ptr.is_null() {
                // SAFETY: context remains alive until owner-thread unhooking.
                let context = unsafe { &*context_ptr };
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::CallbackPanicked);
            }
            // SAFETY: panic handling always fails open with original arguments.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
    }
}

#[cfg(test)]
fn process_hook_record(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    injected: bool,
) -> bool {
    process_hook_record_at(
        context,
        virtual_key,
        scan_code,
        extended,
        phase,
        if injected {
            InjectionKind::External
        } else {
            InjectionKind::Physical
        },
        HookObservation::default(),
    )
}

fn process_hook_record_at(
    context: &CallbackContext,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
    injection: InjectionKind,
    observation: HookObservation,
) -> bool {
    // Injected input must never become physical hotkey state. In particular,
    // an unmatched injected modifier must not remain latched and combine with
    // ordinary typing to activate dictation.
    if injection == InjectionKind::Helper {
        return false;
    }

    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return false;
        }
    };

    if injection == InjectionKind::External {
        return false;
    }

    let right_alt =
        virtual_key == VK_RMENU || (virtual_key == VK_MENU && scan_code == 0x38 && extended);
    if right_alt && phase == KeyPhase::Down {
        // Right Alt is AltGr on many layouts, and Windows does not guarantee a
        // stable injected-Ctrl event shape. Never allow it to activate a
        // global shortcut; left Alt remains available for configured bindings.
        keyboard.altgr_active = true;
    }
    if right_alt && phase == KeyPhase::Up {
        keyboard.altgr_active = false;
    }

    if keyboard
        .modifiers
        .observe(virtual_key, scan_code, extended, phase)
    {
        let modifiers = keyboard.modifiers.mask();
        if keyboard.modifiers_fenced && modifiers == ModifierMask::default() {
            keyboard.modifiers_fenced = false;
        }
        keyboard.reducer.observe_modifiers(modifiers);
        // Modifier prefixes intentionally leak through to the foreground app.
        return false;
    }
    let key = map_scan_code(scan_code, extended);
    if key == PhysicalKey::Other {
        return false;
    }
    if let Some(native_modifiers) = observation.native_modifiers {
        // Snapshot recovery also repairs a missed Right-Alt release. Conversely,
        // a physically held Right Alt remains suppressed even when Windows did
        // not expose AltGr's synthetic Ctrl as injected.
        keyboard.altgr_active = native_modifiers.alt.right;
        if native_modifiers.mask() != keyboard.modifiers.mask() {
            // A modifier release can be lost across secure-desktop transitions
            // or helper startup. Resynchronize before considering ordinary
            // typing. When no modifier is physically down, fence this letter
            // through its up so stale state can never turn it into activation.
            keyboard.modifiers = native_modifiers;
            let modifiers = keyboard.modifiers.mask();
            // This event-time native snapshot is authoritative. Startup and
            // configuration fences remain intact when masks agree, while a
            // repaired mismatch can use a genuinely held left-side modifier.
            keyboard.modifiers_fenced = false;
            keyboard.reducer.observe_modifiers(modifiers);
            keyboard.reducer.fence_activation_revision();
            if modifiers == ModifierMask::default()
                && let PhysicalKey::Letter(letter) = key
            {
                keyboard.activation_fenced_letters |= 1_u32 << u32::from(letter.index());
            }
        }
    }
    let suppress_activation = keyboard.altgr_active;
    let enter_source = enter_source(scan_code, extended);
    let repeat = keyboard.physical.observe(key, enter_source, phase);
    let input = KeyInput {
        key,
        phase,
        modifiers: keyboard.modifiers.mask(),
        repeat,
        injected: false,
    };
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_source
            .is_some_and(|captured| Some(captured) != enter_source)
    {
        return false;
    }
    let accepting = context.gate.is_open();
    if !accepting && !keyboard.reducer.is_capturing(key) {
        // Keep native physical state current while closed, but do not retain
        // prefixes that could complete a chord after initialization/reopening.
        if let PhysicalKey::Letter(letter) = key
            && phase == KeyPhase::Down
        {
            keyboard.activation_fenced_letters |= 1_u32 << u32::from(letter.index());
        }
        return false;
    }
    let activation = keyboard.activation;
    let capture = context.state.session_capture.load(Ordering::Acquire);
    let plan = keyboard.reducer.plan_bindings_at(
        input,
        activation.bindings,
        accepting
            && activation.enabled
            && keyboard.activation_fenced_letters == 0
            && !keyboard.modifiers_fenced
            && !suppress_activation,
        accepting && capture,
        observation.observed_at_ms,
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
    if let PhysicalKey::Letter(letter) = key
        && phase == KeyPhase::Up
    {
        keyboard.activation_fenced_letters &= !(1_u32 << u32::from(letter.index()));
    }
    if let Some(HelperEvent::SessionKey {
        key: crate::keyboard::SessionKey::Enter,
        phase: crate::keyboard::EventPhase::Down,
    }) = planned_event
        && delivered
        && swallowed
    {
        keyboard.captured_enter_source = enter_source;
    } else if key == PhysicalKey::Enter
        && phase == KeyPhase::Up
        && keyboard.captured_enter_source == enter_source
    {
        keyboard.captured_enter_source = None;
    }
    swallowed
}

const fn enter_source(scan_code: u32, extended: bool) -> Option<EnterSource> {
    if scan_code != 0x1C {
        None
    } else if extended {
        Some(EnterSource::Numpad)
    } else {
        Some(EnterSource::Main)
    }
}

fn map_scan_code(scan_code: u32, extended: bool) -> PhysicalKey {
    if !extended
        && let Some(index) = LETTER_SCAN_CODES
            .iter()
            .position(|candidate| *candidate == scan_code)
    {
        return PhysicalKey::Letter(
            ActivationKey::from_index(index as u8).expect("scan table has exactly A-Z entries"),
        );
    }
    match scan_code {
        0x01 => PhysicalKey::Escape,
        // Preserve existing session behavior for both main and numpad Enter.
        0x1C => PhysicalKey::Enter,
        _ => PhysicalKey::Other,
    }
}

fn physical_tracker_from_state(
    mut is_down: impl FnMut(PhysicalKey) -> bool,
) -> WindowsPhysicalTracker {
    let mut tracker = WindowsPhysicalTracker::default();
    for index in 0_u8..26 {
        let key = PhysicalKey::Letter(ActivationKey::from_index(index).expect("A-Z index"));
        if is_down(key) {
            tracker.observe(key, None, KeyPhase::Down);
        }
    }
    if is_down(PhysicalKey::Escape) {
        tracker.observe(PhysicalKey::Escape, None, KeyPhase::Down);
    }
    if is_down(PhysicalKey::Enter) {
        tracker.seed_enter_preheld();
    }
    tracker
}

fn native_physical_key_is_down(key: PhysicalKey) -> bool {
    let virtual_key = match key {
        PhysicalKey::Letter(letter) => {
            let scan_code = LETTER_SCAN_CODES[usize::from(letter.index())];
            // Use the foreground layout to translate each physical scan
            // position into the virtual key queried by GetAsyncKeyState.
            // SAFETY: all calls take scalar values or a null optional pointer.
            let foreground_thread = unsafe {
                let window = GetForegroundWindow();
                if window.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(window, null_mut())
                }
            };
            // SAFETY: a zero thread ID requests the current thread's layout.
            let layout = unsafe { GetKeyboardLayout(foreground_thread) };
            // SAFETY: scan code, mapping mode, and layout are valid scalar inputs.
            let mapped = unsafe { MapVirtualKeyExW(scan_code, MAPVK_VSC_TO_VK_EX, layout) };
            let Ok(mapped) = u16::try_from(mapped & 0xFFFF) else {
                return false;
            };
            mapped
        }
        PhysicalKey::Escape => VK_ESCAPE,
        PhysicalKey::Enter => VK_RETURN,
        PhysicalKey::Other => return false,
    };
    virtual_key != 0 && key_is_down(virtual_key)
}

pub(super) fn key_is_down(key: u16) -> bool {
    // SAFETY: GetAsyncKeyState has no pointer preconditions.
    unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::keyboard::{
        ActivationBinding, EventPhase, ProfileId, SessionKey, Shortcut, ShortcutModifiers,
    };

    fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
        Shortcut::new(modifiers, keys).unwrap()
    }

    fn full_bindings() -> ActivationBindings {
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

    fn test_context(
        outbound_capacity: usize,
    ) -> (
        CallbackContext,
        Receiver<Outbound>,
        Receiver<TerminalReason>,
    ) {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound, outbound_rx) = bounded(outbound_capacity);
        let context = CallbackContext {
            state: Arc::new(SharedState::new()),
            keyboard: Mutex::new(CallbackKeyboard {
                activation: ActivationConfig {
                    enabled: true,
                    bindings: full_bindings(),
                },
                ..CallbackKeyboard::default()
            }),
            outbound,
            gate,
            terminal,
        };
        (context, outbound_rx, terminal_rx)
    }

    fn record(
        context: &CallbackContext,
        virtual_key: u16,
        key: PhysicalKey,
        phase: KeyPhase,
    ) -> bool {
        let (scan_code, extended) = match key {
            PhysicalKey::Letter(letter) => (LETTER_SCAN_CODES[usize::from(letter.index())], false),
            PhysicalKey::Escape => (0x01, false),
            PhysicalKey::Enter => (0x1C, false),
            PhysicalKey::Other => (0, false),
        };
        process_hook_record(context, virtual_key, scan_code, extended, phase, false)
    }

    fn enter(context: &CallbackContext, source: EnterSource, phase: KeyPhase) -> bool {
        process_hook_record(
            context,
            VK_RETURN,
            0x1C,
            source == EnterSource::Numpad,
            phase,
            false,
        )
    }

    fn modifier(context: &CallbackContext, virtual_key: u16, phase: KeyPhase) -> bool {
        process_hook_record(context, virtual_key, 0, false, phase, false)
    }

    fn receive_event(receiver: &Receiver<Outbound>) -> HelperEvent {
        match receiver.recv_timeout(Duration::from_millis(50)).unwrap() {
            Outbound::Event(event) => event,
            other => panic!("unexpected outbound: {other:?}"),
        }
    }

    fn apply_config(context: &CallbackContext, activation: ActivationConfig) {
        let (command_tx, command_rx) = bounded(1);
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        command_tx
            .send(OwnerCommand {
                mutation: OwnerMutation::configure(activation),
                state: Arc::clone(&state),
                acknowledgement,
            })
            .unwrap();
        process_owner_commands(context, &command_rx);
        assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
        assert!(response.recv().unwrap().is_ok());
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
    fn startup_handoff_has_exclusive_running_or_cancelled_outcomes() {
        let cancelled = AtomicU8::new(StartupState::Pending as u8);
        assert_eq!(cancel_startup(&cancelled), StartupState::Cancelled);
        assert!(!claim_startup(&cancelled));

        let running = AtomicU8::new(StartupState::Pending as u8);
        assert!(claim_startup(&running));
        assert_eq!(cancel_startup(&running), StartupState::Running);
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
    fn owner_commands_apply_full_config_and_capture_in_fifo_order() {
        let (context, _outbound, _terminal) = test_context(4);
        let (command_tx, command_rx) = bounded(4);
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
            OwnerMutation::configure(updated),
            OwnerMutation::set_session_capture(true),
        ] {
            let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
            let (ack, response) = bounded(1);
            command_tx
                .send(OwnerCommand {
                    mutation,
                    state: Arc::clone(&state),
                    acknowledgement: ack,
                })
                .unwrap();
            states.push(state);
            responses.push(response);
        }

        process_owner_commands(&context, &command_rx);

        assert_eq!(context.keyboard.lock().unwrap().activation, updated);
        assert!(context.state.session_capture.load(Ordering::Acquire));
        for state in states {
            assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
        }
        for response in responses {
            assert!(response.recv().unwrap().is_ok());
        }
    }

    #[test]
    fn cancelled_owner_command_never_applies_late() {
        let (context, _outbound, _terminal) = test_context(1);
        let previous = context.keyboard.lock().unwrap().activation;
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (command_tx, command_rx) = bounded(1);
        let (ack, response) = bounded(1);
        command_tx
            .send(OwnerCommand {
                mutation: OwnerMutation::configure(ActivationConfig::default()),
                state: Arc::clone(&state),
                acknowledgement: ack,
            })
            .unwrap();
        assert_eq!(cancel_owner_command(&state), OwnerCommandState::Cancelled);

        process_owner_commands(&context, &command_rx);

        assert_eq!(context.keyboard.lock().unwrap().activation, previous);
        assert!(response.recv().unwrap().is_err());
    }

    #[test]
    fn scan_codes_map_every_dom_letter_position_independent_of_virtual_key() {
        for (index, scan_code) in LETTER_SCAN_CODES.iter().copied().enumerate() {
            assert_eq!(
                map_scan_code(scan_code, false),
                PhysicalKey::Letter(ActivationKey::from_index(index as u8).unwrap())
            );
            assert_eq!(map_scan_code(scan_code, true), PhysicalKey::Other);
        }
        assert_eq!(map_scan_code(0x01, false), PhysicalKey::Escape);
        assert_eq!(map_scan_code(0x1C, false), PhysicalKey::Enter);
        assert_eq!(map_scan_code(0x1C, true), PhysicalKey::Enter);
        assert_eq!(enter_source(0x1C, false), Some(EnterSource::Main));
        assert_eq!(enter_source(0x1C, true), Some(EnterSource::Numpad));
        assert_eq!(enter_source(0x01, false), None);
        assert_eq!(map_scan_code(0, false), PhysicalKey::Other);
    }

    #[test]
    fn post_install_snapshot_seeds_every_tracked_key_without_seeding_reducer() {
        let held = [
            PhysicalKey::Letter(ActivationKey::X),
            PhysicalKey::Escape,
            PhysicalKey::Enter,
        ];
        let mut queried = Vec::new();
        let mut tracker = physical_tracker_from_state(|key| {
            queried.push(key);
            held.contains(&key)
        });
        assert_eq!(queried.len(), 28);
        for index in 0_u8..26 {
            let key = PhysicalKey::Letter(ActivationKey::from_index(index).unwrap());
            assert_eq!(
                tracker.observe(key, None, KeyPhase::Down),
                held.contains(&key),
                "physical letter index {index}"
            );
        }
        assert!(tracker.observe(PhysicalKey::Escape, None, KeyPhase::Down));
        assert!(tracker.observe(PhysicalKey::Enter, Some(EnterSource::Main), KeyPhase::Down,));
        assert!(tracker.observe(
            PhysicalKey::Enter,
            Some(EnterSource::Numpad),
            KeyPhase::Down,
        ));
    }

    #[test]
    fn modifier_tracker_is_exact_side_aware_and_generic_safe() {
        let mut tracker = ModifierTracker::from_state(|key| key == VK_LSHIFT);
        assert_eq!(tracker.mask(), ModifierMask::new(false, false, true, false));
        tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Down);
        tracker.observe(VK_LSHIFT, 0x2A, false, KeyPhase::Up);
        assert!(tracker.mask().shift());
        tracker.observe(VK_RSHIFT, 0x36, false, KeyPhase::Up);
        assert_eq!(tracker.mask(), ModifierMask::default());

        for (key, expected) in [
            (VK_CONTROL, ModifierMask::new(true, false, false, false)),
            (VK_MENU, ModifierMask::new(false, true, false, false)),
            (VK_SHIFT, ModifierMask::new(false, false, true, false)),
            (VK_LWIN, ModifierMask::new(false, false, false, true)),
        ] {
            let mut tracker = ModifierTracker::default();
            assert!(tracker.observe(key, 0, false, KeyPhase::Down));
            assert_eq!(tracker.mask(), expected);
            assert!(tracker.observe(key, 0, false, KeyPhase::Up));
            assert_eq!(tracker.mask(), ModifierMask::default());
        }

        // Generic virtual keys are resolved by scan code/extended state, so
        // releasing one side cannot clear the other. AltGr remains exact
        // Ctrl+Alt rather than an injected or implicit modifier.
        let mut tracker = ModifierTracker::default();
        tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
        tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Down);
        tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Up);
        assert!(tracker.mask().ctrl());
        tracker.observe(VK_CONTROL, 0x1D, true, KeyPhase::Up);
        assert!(!tracker.mask().ctrl());
        tracker.observe(VK_CONTROL, 0x1D, false, KeyPhase::Down);
        tracker.observe(VK_MENU, 0x38, true, KeyPhase::Down);
        assert_eq!(tracker.mask(), ModifierMask::new(true, true, false, false));
    }

    #[test]
    fn altgr_never_supplies_or_matches_activation_modifiers() {
        for modifiers in [
            ShortcutModifiers {
                ctrl: false,
                alt: true,
                shift: false,
                meta: false,
            },
            ShortcutModifiers {
                ctrl: true,
                alt: true,
                shift: false,
                meta: false,
            },
        ] {
            let (context, outbound, _terminal) = test_context(4);
            context.keyboard.lock().unwrap().activation = ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[ActivationBinding::new(
                    ProfileId::GENERAL,
                    shortcut(modifiers, &[ActivationKey::X]),
                )])
                .unwrap(),
            };

            // Windows synthesizes an injected left-Ctrl immediately before the
            // physical right-Alt record for AltGr. The synthetic record may
            // suppress activation, but it must never supply a modifier.
            assert!(!process_hook_record(
                &context,
                if modifiers.ctrl {
                    VK_LCONTROL
                } else {
                    VK_CONTROL
                },
                0x1D,
                false,
                KeyPhase::Down,
                true,
            ));
            assert!(!process_hook_record(
                &context,
                VK_RMENU,
                0x38,
                true,
                KeyPhase::Down,
                false,
            ));
            assert_eq!(
                context.keyboard.lock().unwrap().modifiers.mask(),
                ModifierMask::new(false, true, false, false),
            );
            assert!(!record(
                &context,
                0x58,
                PhysicalKey::Letter(ActivationKey::X),
                KeyPhase::Down,
            ));
            assert!(outbound.try_recv().is_err());

            record(
                &context,
                0x58,
                PhysicalKey::Letter(ActivationKey::X),
                KeyPhase::Up,
            );
            assert!(!process_hook_record(
                &context,
                VK_RMENU,
                0x38,
                true,
                KeyPhase::Up,
                false,
            ));
            assert!(!context.keyboard.lock().unwrap().altgr_active);
        }
    }

    #[test]
    fn physical_looking_altgr_and_missed_release_never_activate_plain_typing() {
        let binding = ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: true,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X],
            ),
        );
        let (context, outbound, _terminal) = test_context(4);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[binding]).unwrap(),
        };

        // Some layouts expose AltGr's synthetic Ctrl as a physical-looking
        // record. Right Alt suppression must still prevent Ctrl+Alt activation.
        assert!(!process_hook_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            false,
        ));
        assert!(!process_hook_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Down,
            false,
        ));
        let mut held_altgr = ModifierTracker::default();
        held_altgr.observe(VK_LCONTROL, 0x1D, false, KeyPhase::Down);
        held_altgr.observe(VK_RMENU, 0x38, true, KeyPhase::Down);
        assert!(!process_hook_record_at(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InjectionKind::Physical,
            HookObservation {
                observed_at_ms: 0,
                native_modifiers: Some(held_altgr),
            },
        ));
        assert!(outbound.try_recv().is_err());
        record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        );

        // If the desktop transition loses every AltGr release event, the next
        // native snapshot repairs both modifiers and suppression without using
        // the ordinary X as a shortcut.
        let no_modifiers = ModifierTracker::default();
        assert!(!process_hook_record_at(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InjectionKind::Physical,
            HookObservation {
                observed_at_ms: 0,
                native_modifiers: Some(no_modifiers),
            },
        ));
        assert!(!context.keyboard.lock().unwrap().altgr_active);
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn external_injected_modifiers_never_activate_or_clear_physical_modifiers() {
        let binding = ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X],
            ),
        );
        let (context, outbound, _terminal) = test_context(4);
        context.keyboard.lock().unwrap().activation = ActivationConfig {
            enabled: true,
            bindings: ActivationBindings::new(&[binding]).unwrap(),
        };

        assert!(!process_hook_record(
            &context,
            VK_LMENU,
            0,
            false,
            KeyPhase::Down,
            true,
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::default(),
        );
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());
        record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        );

        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!process_hook_record(
            &context,
            VK_LMENU,
            0,
            false,
            KeyPhase::Up,
            true,
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::new(false, true, false, false),
        );
        assert!(record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding,
                phase: EventPhase::Down,
            },
        );
    }

    #[test]
    fn stale_tracked_alt_is_resynchronized_without_activating_plain_typing() {
        let binding = ActivationBinding::new(
            ProfileId::GENERAL,
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::X],
            ),
        );
        let (context, outbound, _terminal) = test_context(4);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.activation = ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[binding]).unwrap(),
            };
            keyboard
                .modifiers
                .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
        }

        let no_modifiers = ModifierTracker::default();
        assert!(!process_hook_record_at(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InjectionKind::Physical,
            HookObservation {
                observed_at_ms: 0,
                native_modifiers: Some(no_modifiers),
            },
        ));
        assert!(!process_hook_record_at(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Up,
            InjectionKind::Physical,
            HookObservation {
                observed_at_ms: 0,
                native_modifiers: Some(no_modifiers),
            },
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::default(),
        );
        assert!(outbound.try_recv().is_err());

        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding,
                phase: EventPhase::Down,
            },
        );
    }

    #[test]
    fn every_nonempty_exact_modifier_mask_can_activate_in_the_native_path() {
        for bits in 1_u8..16 {
            let modifiers = ShortcutModifiers {
                ctrl: bits & 0b0001 != 0,
                alt: bits & 0b0010 != 0,
                shift: bits & 0b0100 != 0,
                meta: bits & 0b1000 != 0,
            };
            let expected = shortcut(modifiers, &[ActivationKey::P]);
            let expected_binding = ActivationBinding::new(ProfileId::GENERAL, expected);
            let (context, outbound, _terminal) = test_context(2);
            context.keyboard.lock().unwrap().activation = ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[ActivationBinding::new(
                    ProfileId::GENERAL,
                    expected,
                )])
                .unwrap(),
            };
            for (enabled, virtual_key) in [
                (modifiers.ctrl, VK_LCONTROL),
                (modifiers.alt, VK_LMENU),
                (modifiers.shift, VK_LSHIFT),
                (modifiers.meta, VK_LWIN),
            ] {
                if enabled {
                    modifier(&context, virtual_key, KeyPhase::Down);
                }
            }

            assert!(
                record(
                    &context,
                    0x50,
                    PhysicalKey::Letter(ActivationKey::P),
                    KeyPhase::Down,
                ),
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
    fn closed_gate_tracks_native_state_without_retaining_future_prefixes() {
        let (context, outbound, _terminal) = test_context(4);
        context.gate.close();
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
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
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());
        record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        );
        record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        );

        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
    }

    #[test]
    fn every_binding_revision_fences_physically_held_letters_until_release() {
        let (context, outbound, _terminal) = test_context(4);
        context.gate.close();
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
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
            ActivationConfig {
                enabled: true,
                bindings: ActivationBindings::new(&[one_key]).unwrap(),
            },
        );
        context.gate.open();
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());
        record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        );
        record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        );
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
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
    fn modifier_changes_fence_a_passive_native_prefix_until_all_letters_release() {
        let (context, outbound, _terminal) = test_context(4);
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());
        for (virtual_key, physical) in [
            (0x50, PhysicalKey::Letter(ActivationKey::P)),
            (0x58, PhysicalKey::Letter(ActivationKey::X)),
        ] {
            record(&context, virtual_key, physical, KeyPhase::Up);
        }
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
    }

    #[test]
    fn alt_x_p_passes_prefix_and_modifiers_but_swallows_trigger_sequence() {
        let (context, outbound, _terminal) = test_context(4);
        assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: full_bindings().iter().next().unwrap(),
                phase: EventPhase::Down,
            }
        );
        // Activation delivery alone must not globally capture Enter/Escape.
        // Electron explicitly enables that capture only after accepting and
        // visibly starting the session.
        assert!(!context.state.session_capture.load(Ordering::Acquire));
        assert!(!record(
            &context,
            VK_ESCAPE,
            PhysicalKey::Escape,
            KeyPhase::Down
        ));
        assert!(!record(
            &context,
            VK_ESCAPE,
            PhysicalKey::Escape,
            KeyPhase::Up
        ));
        assert!(outbound.try_recv().is_err());

        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());

        // Config, prefix, and modifier changes cannot alter the accepted up.
        apply_config(&context, ActivationConfig::default());
        assert!(!modifier(&context, VK_LMENU, KeyPhase::Up));
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Up,
        ));
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: full_bindings().iter().next().unwrap(),
                phase: EventPhase::Up,
            }
        );
    }

    #[test]
    fn ctrl_shift_p_matches_exactly_and_extra_or_missing_state_does_not() {
        let (context, outbound, _terminal) = test_context(4);
        assert!(!modifier(&context, VK_LCONTROL, KeyPhase::Down));
        assert!(!modifier(&context, VK_RSHIFT, KeyPhase::Down));
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        let expected = full_bindings().iter().nth(1).unwrap();
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: expected,
                phase: EventPhase::Down,
            }
        );
        assert!(record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        ));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::Activation {
                binding: expected,
                phase: EventPhase::Up,
            }
        );

        // Missing Shift prevents a separate fresh gesture.
        let (missing, missing_outbound, _terminal) = test_context(2);
        modifier(&missing, VK_LCONTROL, KeyPhase::Down);
        assert!(!record(
            &missing,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(missing_outbound.try_recv().is_err());

        // An extra modifier prevents the next fresh gesture.
        assert!(!modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        ));
        assert!(outbound.try_recv().is_err());

        // An extra held letter also prevents the otherwise exact chord.
        let (extra, extra_outbound, _terminal) = test_context(2);
        modifier(&extra, VK_LCONTROL, KeyPhase::Down);
        modifier(&extra, VK_LSHIFT, KeyPhase::Down);
        record(
            &extra,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        );
        assert!(!record(
            &extra,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(extra_outbound.try_recv().is_err());
    }

    #[test]
    fn wrong_order_extra_letters_and_injected_records_never_activate_or_mutate() {
        let (context, outbound, _terminal) = test_context(4);
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(outbound.try_recv().is_err());

        // Externally injected modifiers and letters cannot mutate physical
        // state or complete a sequence.
        assert!(!process_hook_record(
            &context,
            VK_LMENU,
            0,
            false,
            KeyPhase::Up,
            true,
        ));
        assert!(!process_hook_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Up,
            true,
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::new(false, true, false, false)
        );
        assert!(context.keyboard.lock().unwrap().physical.observe(
            PhysicalKey::Letter(ActivationKey::P),
            None,
            KeyPhase::Down
        ));

        // Talking Quill's own marked SendInput records remain entirely inert.
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!process_hook_record_at(
            &context,
            VK_LMENU,
            0,
            false,
            KeyPhase::Up,
            InjectionKind::Helper,
            HookObservation::default(),
        ));
        assert_eq!(
            context.keyboard.lock().unwrap().modifiers.mask(),
            ModifierMask::new(false, true, false, false)
        );
    }

    #[test]
    fn outbound_failure_passes_current_trigger_and_every_later_record() {
        let (context, _outbound, terminal) = test_context(0);
        modifier(&context, VK_LMENU, KeyPhase::Down);
        assert!(!record(
            &context,
            0x58,
            PhysicalKey::Letter(ActivationKey::X),
            KeyPhase::Down,
        ));
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert_eq!(
            terminal.recv_timeout(Duration::from_millis(50)).unwrap(),
            TerminalReason::OutboundQueueUnavailable
        );
        assert!(!context.gate.is_open());
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Down,
        ));
        assert!(!record(
            &context,
            0x50,
            PhysicalKey::Letter(ActivationKey::P),
            KeyPhase::Up,
        ));
    }

    #[test]
    fn simultaneous_enter_sources_latch_one_balanced_sequence_in_every_order() {
        for (first, second) in [
            (EnterSource::Main, EnterSource::Numpad),
            (EnterSource::Numpad, EnterSource::Main),
        ] {
            for release_accepted_first in [false, true] {
                let (context, outbound, _terminal) = test_context(4);
                context.state.session_capture.store(true, Ordering::Release);

                assert!(enter(&context, first, KeyPhase::Down));
                assert_eq!(
                    receive_event(&outbound),
                    HelperEvent::SessionKey {
                        key: SessionKey::Enter,
                        phase: EventPhase::Down,
                    },
                );
                assert!(enter(&context, first, KeyPhase::Down));
                assert!(!enter(&context, second, KeyPhase::Down));
                assert!(!enter(&context, second, KeyPhase::Down));
                assert!(outbound.try_recv().is_err());

                let releases = if release_accepted_first {
                    [first, second]
                } else {
                    [second, first]
                };
                for source in releases {
                    assert_eq!(
                        enter(&context, source, KeyPhase::Up),
                        source == first,
                        "first={first:?}, release={source:?}",
                    );
                }
                assert_eq!(
                    receive_event(&outbound),
                    HelperEvent::SessionKey {
                        key: SessionKey::Enter,
                        phase: EventPhase::Up,
                    },
                );
                assert!(outbound.try_recv().is_err());
                assert_eq!(context.keyboard.lock().unwrap().captured_enter_source, None,);
            }
        }
    }

    #[test]
    fn enter_source_tracking_survives_capture_and_config_transitions() {
        let (context, outbound, _terminal) = test_context(4);

        assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
        context.state.session_capture.store(true, Ordering::Release);
        assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Down,
            },
        );
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));

        context
            .state
            .session_capture
            .store(false, Ordering::Release);
        apply_config(&context, ActivationConfig::default());
        assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Up));
        assert_eq!(
            receive_event(&outbound),
            HelperEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            },
        );
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn escape_and_enter_capture_remains_paired_and_modifier_independent() {
        let (context, outbound, _terminal) = test_context(8);
        context.state.session_capture.store(true, Ordering::Release);
        modifier(&context, VK_LWIN, KeyPhase::Down);
        for (key, session_key) in [
            (PhysicalKey::Escape, SessionKey::Escape),
            (PhysicalKey::Enter, SessionKey::Enter),
        ] {
            assert!(record(&context, 0, key, KeyPhase::Down));
            assert_eq!(
                receive_event(&outbound),
                HelperEvent::SessionKey {
                    key: session_key,
                    phase: EventPhase::Down,
                }
            );
            assert!(record(&context, 0, key, KeyPhase::Down));
            assert!(record(&context, 0, key, KeyPhase::Up));
            assert_eq!(
                receive_event(&outbound),
                HelperEvent::SessionKey {
                    key: session_key,
                    phase: EventPhase::Up,
                }
            );
        }
    }
}
