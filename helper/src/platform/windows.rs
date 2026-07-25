use std::{
    path::Path,
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
    Foundation::{CloseHandle, LPARAM, LRESULT, RECT, WPARAM},
    System::Threading::{
        GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MOD_ALT,
            MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, SendInput, UnregisterHotKey, VK_CONTROL,
            VK_ESCAPE, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_V,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowRect,
            GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HC_ACTION,
            KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, LLKHF_INJECTED, MSG, PM_NOREMOVE, PM_REMOVE,
            PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
            WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use super::{
    CallbackGate, FrontApp, HookStatus, PasteFailure, PasteResult, PermissionState, Permissions,
    Platform, PlatformError, TerminalReason, TerminalSignal, WindowBounds, deliver_callback_event,
    deliver_callback_event_with_session_arm,
};
use crate::{
    keyboard::{
        ActivationBindings, ActivationKey, KeyInput, KeyPhase, KeyboardReducer, PhysicalKey,
        PhysicalKeyTracker,
    },
    protocol::Outbound,
};

const INJECTED_MARKER: usize = 0x4D45_4348_4F50_5354;
const MAX_WINDOW_TITLE_UNITS: usize = 4096;
const MAX_PROCESS_PATH_UNITS: usize = 32_768;
const WM_ACTIVATION_CONFIG: u32 = WM_APP + 0x45;
const ACTIVATION_CONFIG_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_WAKE_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
];
const HOTKEY_BANK_A_BASE: i32 = 0x4D00;
const HOTKEY_BANK_B_BASE: i32 = 0x4E00;
static DPI_AWARENESS_READY: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ActivationConfig {
    enabled: bool,
    bindings: ActivationBindings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationSlot {
    A,
    B,
}
impl RegistrationSlot {
    const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
    const fn id(self, key: ActivationKey, shift: bool) -> i32 {
        let base = match self {
            Self::A => HOTKEY_BANK_A_BASE,
            Self::B => HOTKEY_BANK_B_BASE,
        };
        base + key.index() as i32 * 2 + shift as i32 + 1
    }
}
fn registration_from_id(id: i32) -> Option<(RegistrationSlot, ActivationKey, bool)> {
    let (slot, offset) = if id > HOTKEY_BANK_A_BASE && id <= HOTKEY_BANK_A_BASE + 52 {
        (RegistrationSlot::A, id - HOTKEY_BANK_A_BASE - 1)
    } else if id > HOTKEY_BANK_B_BASE && id <= HOTKEY_BANK_B_BASE + 52 {
        (RegistrationSlot::B, id - HOTKEY_BANK_B_BASE - 1)
    } else {
        return None;
    };
    Some((
        slot,
        ActivationKey::from_index((offset / 2) as u8)?,
        offset % 2 == 1,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HotKeyRegistrationState {
    config: ActivationConfig,
    /// Bank preferred for the next newly-added binding. Retained bindings may
    /// remain in either bank so Windows never sees the same chord under two IDs.
    slot: RegistrationSlot,
    bank_b: ActivationBindings,
    generation: u64,
}
impl HotKeyRegistrationState {
    const fn active_bindings(self) -> ActivationBindings {
        if self.config.enabled {
            self.config.bindings
        } else {
            ActivationBindings::from_bits(0)
        }
    }

    const fn slot_for(self, key: ActivationKey, shift: bool) -> Option<RegistrationSlot> {
        if !self.config.enabled || !self.config.bindings.contains(key, shift) {
            None
        } else if self.bank_b.contains(key, shift) {
            Some(RegistrationSlot::B)
        } else {
            Some(RegistrationSlot::A)
        }
    }
}
impl Default for HotKeyRegistrationState {
    fn default() -> Self {
        Self {
            config: ActivationConfig::default(),
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HotKeyTransactionError {
    Conflict,
    Cancelled,
    RollbackFailed,
}
trait HotKeyRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool;
    fn unregister(&mut self, id: i32) -> bool;
}
struct WindowsHotKeyRegistrar;
impl HotKeyRegistrar for WindowsHotKeyRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool {
        unsafe { RegisterHotKey(null_mut(), id, modifiers, virtual_key) != 0 }
    }
    fn unregister(&mut self, id: i32) -> bool {
        unsafe { UnregisterHotKey(null_mut(), id) != 0 }
    }
}
const fn activation_virtual_key(key: ActivationKey) -> u32 {
    0x41 + key.index() as u32
}
const fn hotkey_modifiers(shift: bool) -> u32 {
    MOD_ALT | MOD_NOREPEAT | if shift { MOD_SHIFT } else { 0 }
}
fn unregister_set(
    registrar: &mut impl HotKeyRegistrar,
    slot: RegistrationSlot,
    bindings: ActivationBindings,
) -> bool {
    bindings.iter().fold(true, |ok, (key, shift)| {
        registrar.unregister(slot.id(key, shift)) && ok
    })
}
fn register_set(
    registrar: &mut impl HotKeyRegistrar,
    slot: RegistrationSlot,
    bindings: ActivationBindings,
) -> Result<(), HotKeyTransactionError> {
    let mut registered = ActivationBindings::default();
    for (key, shift) in bindings.iter() {
        if !registrar.register(
            slot.id(key, shift),
            hotkey_modifiers(shift),
            activation_virtual_key(key),
        ) {
            if !unregister_set(registrar, slot, registered) {
                return Err(HotKeyTransactionError::RollbackFailed);
            }
            return Err(HotKeyTransactionError::Conflict);
        }
        registered = ActivationBindings::from_exact(
            &registered.iter().chain([(key, shift)]).collect::<Vec<_>>(),
        )
        .expect("bounded set");
    }
    Ok(())
}
fn configure_hotkeys(
    current: HotKeyRegistrationState,
    requested: ActivationConfig,
    registrar: &mut impl HotKeyRegistrar,
    commit_allowed: impl FnOnce() -> bool,
) -> Result<HotKeyRegistrationState, HotKeyTransactionError> {
    if current.config == requested {
        return Ok(current);
    }
    let current_active = current.active_bindings();
    let requested_active = if requested.enabled {
        requested.bindings
    } else {
        ActivationBindings::default()
    };
    let retained = ActivationBindings::from_bits(current_active.bits() & requested_active.bits());
    let added = ActivationBindings::from_bits(requested_active.bits() & !current_active.bits());
    let removed = ActivationBindings::from_bits(current_active.bits() & !requested_active.bits());
    let candidate = current.slot.other();

    // Register only genuinely new chords. RegisterHotKey rejects a retained
    // chord if it is registered again under an ID in the other bank.
    register_set(registrar, candidate, added)?;
    if !commit_allowed() {
        if !unregister_set(registrar, candidate, added) {
            return Err(HotKeyTransactionError::RollbackFailed);
        }
        return Err(HotKeyTransactionError::Cancelled);
    }

    let mut removed_a = ActivationBindings::default();
    let mut removed_b = ActivationBindings::default();
    for (key, shift) in removed.iter() {
        let slot = current
            .slot_for(key, shift)
            .expect("removed binding was active");
        if !registrar.unregister(slot.id(key, shift)) {
            let additions_removed = unregister_set(registrar, candidate, added);
            let restored_a = register_set(registrar, RegistrationSlot::A, removed_a).is_ok();
            let restored_b = register_set(registrar, RegistrationSlot::B, removed_b).is_ok();
            return if additions_removed && restored_a && restored_b {
                Err(HotKeyTransactionError::Conflict)
            } else {
                Err(HotKeyTransactionError::RollbackFailed)
            };
        }
        let removed_from_slot = if slot == RegistrationSlot::B {
            &mut removed_b
        } else {
            &mut removed_a
        };
        *removed_from_slot = ActivationBindings::from_bits(
            removed_from_slot.bits() | bindings_bit(key, shift).bits(),
        );
    }

    let retained_b = ActivationBindings::from_bits(current.bank_b.bits() & retained.bits());
    let bank_b = if candidate == RegistrationSlot::B {
        ActivationBindings::from_bits(retained_b.bits() | added.bits())
    } else {
        retained_b
    };
    Ok(HotKeyRegistrationState {
        config: requested,
        slot: if added.bits() == 0 {
            current.slot
        } else {
            candidate
        },
        bank_b,
        generation: current.generation.wrapping_add(1),
    })
}

fn bindings_bit(key: ActivationKey, shift: bool) -> ActivationBindings {
    ActivationBindings::from_exact(&[(key, shift)]).expect("one binding is bounded")
}
fn unregister_all_hotkeys(registrar: &mut impl HotKeyRegistrar) {
    for slot in [RegistrationSlot::A, RegistrationSlot::B] {
        for index in 0_u8..26 {
            let key = ActivationKey::from_index(index).unwrap();
            let _ = registrar.unregister(slot.id(key, false));
            let _ = registrar.unregister(slot.id(key, true));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActivationCandidate {
    generation: u64,
    slot: RegistrationSlot,
    key: ActivationKey,
    shift: bool,
}
#[cfg(test)]
impl ActivationCandidate {
    const fn id(self) -> i32 {
        self.slot.id(self.key, self.shift)
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ActivationCandidateState {
    #[default]
    Idle,
    Pressed(ActivationCandidate),
    ReleasedBeforeMessage(ActivationCandidate),
    Accepted(ActivationCandidate),
    ConsumedDown(ActivationCandidate),
    Completed(ActivationCandidate),
}
impl ActivationCandidateState {
    const fn blocks_configuration(self) -> bool {
        matches!(
            self,
            Self::Pressed(_)
                | Self::ReleasedBeforeMessage(_)
                | Self::Accepted(_)
                | Self::ConsumedDown(_)
        )
    }
    const fn candidate(self) -> Option<ActivationCandidate> {
        match self {
            Self::Idle => None,
            Self::Pressed(v)
            | Self::ReleasedBeforeMessage(v)
            | Self::Accepted(v)
            | Self::ConsumedDown(v)
            | Self::Completed(v) => Some(v),
        }
    }
}
#[derive(Debug, Default)]
struct HotKeyRuntime {
    registrations: HotKeyRegistrationState,
    candidate: ActivationCandidateState,
}
fn candidate_for_exact_passive_down(
    registrations: HotKeyRegistrationState,
    current: ActivationCandidateState,
    key: ActivationKey,
    alt: bool,
    shift: bool,
    disallowed_modifiers: bool,
    repeat: bool,
) -> ActivationCandidateState {
    // The hook's tracked Shift state identifies the registered physical
    // candidate. WM_HOTKEY remains authoritative for the exact variant Windows
    // recognized, guarding the boundary if native modifier state ever differs.
    let observed_shift = registrations
        .config
        .bindings
        .contains(key, shift)
        .then_some(shift);
    if !registrations.config.enabled
        || observed_shift.is_none()
        || !alt
        || disallowed_modifiers
        || repeat
    {
        return current;
    }
    match current {
        ActivationCandidateState::Idle | ActivationCandidateState::Completed(_) => {
            let observed_shift = observed_shift.expect("candidate binding was configured");
            ActivationCandidateState::Pressed(ActivationCandidate {
                generation: registrations.generation,
                slot: registrations
                    .slot_for(key, observed_shift)
                    .expect("candidate binding was configured"),
                key,
                shift: observed_shift,
            })
        }
        _ => current,
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateUpAction {
    Pass,
    SwallowAwaitMessage(ActivationCandidate),
    SwallowConsumed(ActivationCandidate),
    BalanceAccepted(ActivationCandidate),
}
fn release_activation_candidate(
    state: ActivationCandidateState,
    key: ActivationKey,
) -> (ActivationCandidateState, CandidateUpAction) {
    match state {
        ActivationCandidateState::Pressed(c) if c.key == key => (
            ActivationCandidateState::ReleasedBeforeMessage(c),
            CandidateUpAction::SwallowAwaitMessage(c),
        ),
        ActivationCandidateState::Accepted(c) if c.key == key => {
            (state, CandidateUpAction::BalanceAccepted(c))
        }
        ActivationCandidateState::ConsumedDown(c) if c.key == key => (
            ActivationCandidateState::Completed(c),
            CandidateUpAction::SwallowConsumed(c),
        ),
        _ => (state, CandidateUpAction::Pass),
    }
}
fn resolve_activation_candidate_message(
    candidate: ActivationCandidate,
    registrations: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> Option<ActivationCandidate> {
    if candidate.generation != registrations.generation {
        return None;
    }
    let (key, shift) = hotkey_message_activation(registrations, id, l_param)?;
    if key != candidate.key {
        return None;
    }
    Some(ActivationCandidate {
        generation: candidate.generation,
        slot: registrations.slot_for(key, shift)?,
        key,
        shift,
    })
}
#[cfg(test)]
fn candidate_matches_message(
    candidate: ActivationCandidate,
    registrations: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> bool {
    resolve_activation_candidate_message(candidate, registrations, id, l_param) == Some(candidate)
}

fn hotkey_message_activation(
    state: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> Option<(ActivationKey, bool)> {
    if !state.config.enabled {
        return None;
    }
    let (slot, key, shift) = registration_from_id(id)?;
    if state.slot_for(key, shift) != Some(slot) {
        return None;
    }
    let value = l_param as u32;
    let expected = MOD_ALT | if shift { MOD_SHIFT } else { 0 };
    ((value & 0xFFFF) == expected && (value >> 16) == activation_virtual_key(key))
        .then_some((key, shift))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum ConfigurationHandoff {
    Pending,
    Cancelled,
    Committed,
    RollbackConfirmed,
    TerminalRequested,
    TerminalComplete,
}

fn configuration_handoff(value: &AtomicU8) -> ConfigurationHandoff {
    match value.load(Ordering::Acquire) {
        1 => ConfigurationHandoff::Cancelled,
        2 => ConfigurationHandoff::Committed,
        3 => ConfigurationHandoff::RollbackConfirmed,
        4 => ConfigurationHandoff::TerminalRequested,
        5 => ConfigurationHandoff::TerminalComplete,
        _ => ConfigurationHandoff::Pending,
    }
}

fn transition_configuration(
    value: &AtomicU8,
    from: ConfigurationHandoff,
    to: ConfigurationHandoff,
) -> ConfigurationHandoff {
    match value.compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => to,
        Err(_) => configuration_handoff(value),
    }
}

fn cancel_configuration(value: &AtomicU8) -> ConfigurationHandoff {
    transition_configuration(
        value,
        ConfigurationHandoff::Pending,
        ConfigurationHandoff::Cancelled,
    )
}

fn commit_configuration(value: &AtomicU8) -> bool {
    transition_configuration(
        value,
        ConfigurationHandoff::Pending,
        ConfigurationHandoff::Committed,
    ) == ConfigurationHandoff::Committed
}

fn confirm_configuration_rollback(value: &AtomicU8) -> ConfigurationHandoff {
    transition_configuration(
        value,
        ConfigurationHandoff::Cancelled,
        ConfigurationHandoff::RollbackConfirmed,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigurationTimeoutAction {
    Success,
    Failed,
    WakeTerminalCleanup,
}

fn configuration_ack_timeout(value: &AtomicU8) -> ConfigurationTimeoutAction {
    match cancel_configuration(value) {
        ConfigurationHandoff::Committed => ConfigurationTimeoutAction::Success,
        ConfigurationHandoff::RollbackConfirmed | ConfigurationHandoff::TerminalComplete => {
            ConfigurationTimeoutAction::Failed
        }
        ConfigurationHandoff::Cancelled => match transition_configuration(
            value,
            ConfigurationHandoff::Cancelled,
            ConfigurationHandoff::TerminalRequested,
        ) {
            ConfigurationHandoff::Committed => ConfigurationTimeoutAction::Success,
            ConfigurationHandoff::RollbackConfirmed | ConfigurationHandoff::TerminalComplete => {
                ConfigurationTimeoutAction::Failed
            }
            ConfigurationHandoff::TerminalRequested => {
                ConfigurationTimeoutAction::WakeTerminalCleanup
            }
            ConfigurationHandoff::Pending | ConfigurationHandoff::Cancelled => {
                ConfigurationTimeoutAction::Failed
            }
        },
        ConfigurationHandoff::Pending | ConfigurationHandoff::TerminalRequested => {
            ConfigurationTimeoutAction::Failed
        }
    }
}

struct ActivationCommand {
    requested: ActivationConfig,
    handoff: Arc<AtomicU8>,
    acknowledgement: Sender<Result<(), PlatformError>>,
}

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

    fn shutdown(&mut self) {
        self.gate.close();
        self.state.stopping.store(true, Ordering::Release);
        self.state.session_capture.store(false, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return;
        };

        let first_wake = post_owner_message(self.thread_id, WM_QUIT);
        let completed = first_wake
            && (owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)
                || (post_owner_message(self.thread_id, WM_QUIT)
                    && owner_completed(&self.owner_completion, OWNER_COMPLETION_TIMEOUT)));
        if completed {
            // OwnerCompletion is sent only as the final hook-thread guard drops,
            // after unregistration, unhooking, and callback-context release.
            let _ = thread.join();
            self.state
                .hook_status
                .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
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
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        self.shutdown();
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
        let key = map_virtual_key(native.vkCode);
        let injected = native.flags & LLKHF_INJECTED != 0 || native.dwExtraInfo == INJECTED_MARKER;
        // Modifier events map to Other and always pass through. These sampled
        // values never authorize a low-level-hook activation down; WM_HOTKEY
        // separately validates Ctrl/AltGr/Windows-key exclusions.
        let alt = native.flags & LLKHF_ALTDOWN != 0 || key_is_down(VK_MENU);
        let mut input = KeyInput {
            key,
            phase,
            alt,
            shift: false,
            disallowed_modifiers: activation_disallowed_modifiers(key_is_down),
            repeat: false,
            injected,
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
            if !injected {
                keyboard.shift.observe(native.vkCode as u16, phase);
                input.shift = keyboard.shift.is_down();
                input.repeat = keyboard.physical.observe(key, phase);
            }
        }

        // Record only a fresh, exact registered chord. The low-level down is
        // deliberately passed: RegisterHotKey performs the OS suppression and
        // WM_HOTKEY remains the reducer's establishment point.
        if context.gate.is_open()
            && !injected
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
        if !injected
            && phase == KeyPhase::Up
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

const ACTIVATION_ENABLED_MASK: u64 = 1_u64 << 63;
const fn activation_config_value(enabled: bool, bindings: ActivationBindings) -> u64 {
    bindings.bits() | if enabled { ACTIVATION_ENABLED_MASK } else { 0 }
}

const fn activation_config_from_value(value: u64) -> (bool, ActivationBindings) {
    (
        value & ACTIVATION_ENABLED_MASK != 0,
        ActivationBindings::from_bits(value),
    )
}

fn activation_disallowed_modifiers(mut is_down: impl FnMut(u16) -> bool) -> bool {
    let ctrl = is_down(VK_CONTROL);
    let left_win = is_down(VK_LWIN);
    let right_win = is_down(VK_RWIN);
    ctrl || left_win || right_win
}

fn key_is_down(key: u16) -> bool {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PasteModifiers {
    ctrl: bool,
    shift: bool,
    alt: bool,
    left_win: bool,
    right_win: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PasteKeyInput {
    key: u16,
    key_up: bool,
}

impl PasteKeyInput {
    const fn down(key: u16) -> Self {
        Self { key, key_up: false }
    }

    const fn up(key: u16) -> Self {
        Self { key, key_up: true }
    }
}

const PASTE_WITH_CTRL: [PasteKeyInput; 4] = [
    PasteKeyInput::down(VK_CONTROL),
    PasteKeyInput::down(VK_V),
    PasteKeyInput::up(VK_V),
    PasteKeyInput::up(VK_CONTROL),
];
const PASTE_WITH_PREHELD_CTRL: [PasteKeyInput; 2] =
    [PasteKeyInput::down(VK_V), PasteKeyInput::up(VK_V)];

fn paste_plan(modifiers: PasteModifiers) -> Result<&'static [PasteKeyInput], PasteFailure> {
    if modifiers.shift || modifiers.alt || modifiers.left_win || modifiers.right_win {
        Err(PasteFailure::ConflictingModifiers)
    } else if modifiers.ctrl {
        Ok(&PASTE_WITH_PREHELD_CTRL)
    } else {
        Ok(&PASTE_WITH_CTRL)
    }
}

fn paste_cleanup_plan(inputs: &[PasteKeyInput], accepted: usize) -> Vec<PasteKeyInput> {
    let mut ctrl_owned = false;
    let mut v_owned = false;
    for input in inputs.iter().take(accepted) {
        match (input.key, input.key_up) {
            (VK_CONTROL, false) => ctrl_owned = true,
            (VK_CONTROL, true) => ctrl_owned = false,
            (VK_V, false) => v_owned = true,
            (VK_V, true) => v_owned = false,
            _ => {}
        }
    }

    let mut cleanup = Vec::with_capacity(2);
    if v_owned {
        cleanup.push(PasteKeyInput::up(VK_V));
    }
    if ctrl_owned {
        cleanup.push(PasteKeyInput::up(VK_CONTROL));
    }
    cleanup
}

fn paste_outcome(inputs: &[PasteKeyInput], accepted: usize) -> (PasteResult, Vec<PasteKeyInput>) {
    if accepted == inputs.len() {
        (
            PasteResult {
                submitted: true,
                reason: None,
            },
            Vec::new(),
        )
    } else {
        (
            PasteResult {
                submitted: false,
                reason: Some(PasteFailure::OsRejected),
            },
            paste_cleanup_plan(inputs, accepted),
        )
    }
}

fn inject_paste() -> PasteResult {
    let modifiers = PasteModifiers {
        ctrl: key_is_down(VK_CONTROL),
        shift: key_is_down(VK_SHIFT),
        alt: key_is_down(VK_MENU),
        left_win: key_is_down(VK_LWIN),
        right_win: key_is_down(VK_RWIN),
    };
    let plan = match paste_plan(modifiers) {
        Ok(plan) => plan,
        Err(reason) => {
            return PasteResult {
                submitted: false,
                reason: Some(reason),
            };
        }
    };
    let inputs = plan
        .iter()
        .map(|input| keyboard_input(input.key, input.flags()))
        .collect::<Vec<_>>();
    // SAFETY: the slice contains initialized INPUT values and its byte size is
    // exactly the structure size expected by SendInput.
    let sent = unsafe {
        SendInput(
            u32::try_from(inputs.len()).expect("fixed input count fits u32"),
            inputs.as_ptr(),
            i32::try_from(size_of::<INPUT>()).expect("INPUT size fits i32"),
        )
    };
    let accepted = usize::try_from(sent).expect("SendInput count fits usize");
    let (result, cleanup) = paste_outcome(plan, accepted);
    for release in cleanup {
        let release = keyboard_input(release.key, release.flags());
        // Attempt each helper-owned release separately so one rejected cleanup
        // does not prevent the remaining release from being attempted.
        // SAFETY: `release` is one initialized INPUT value.
        unsafe {
            SendInput(
                1,
                &raw const release,
                i32::try_from(size_of::<INPUT>()).expect("INPUT size fits i32"),
            );
        }
    }

    // Cleanup cannot turn the original partial submission into success.
    result
}

impl PasteKeyInput {
    const fn flags(self) -> u32 {
        if self.key_up { KEYEVENTF_KEYUP } else { 0 }
    }
}

fn keyboard_input(key: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_MARKER,
            },
        },
    }
}

fn front_app() -> Result<FrontApp, PlatformError> {
    // SAFETY: GetForegroundWindow takes no pointers.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return Err(PlatformError::NativeFailure);
    }

    // SAFETY: `window` is a live HWND returned by the OS.
    let title_length = unsafe { GetWindowTextLengthW(window) };
    let title_capacity = usize::try_from(title_length.max(0))
        .unwrap_or(0)
        .saturating_add(1)
        .min(MAX_WINDOW_TITLE_UNITS);
    let mut title = vec![0_u16; title_capacity.max(1)];
    // SAFETY: the title buffer is writable for its declared length.
    let copied = unsafe {
        GetWindowTextW(
            window,
            title.as_mut_ptr(),
            i32::try_from(title.len()).map_err(|_| PlatformError::NativeFailure)?,
        )
    };
    let copied = usize::try_from(copied.max(0)).unwrap_or(0);
    let window_title = String::from_utf16_lossy(&title[..copied]);

    let mut process_id = 0_u32;
    // SAFETY: `process_id` is valid writable storage.
    unsafe { GetWindowThreadProcessId(window, &raw mut process_id) };
    if process_id == 0 {
        return Err(PlatformError::NativeFailure);
    }
    // SAFETY: requested rights are read-only and process_id came from Windows.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return Err(PlatformError::NativeFailure);
    }

    let mut path = vec![0_u16; MAX_PROCESS_PATH_UNITS];
    let mut path_length = u32::try_from(path.len()).expect("bounded path length fits u32");
    // SAFETY: `process` is valid and the path buffer length matches the supplied
    // mutable size value.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &raw mut path_length,
        )
    };
    // SAFETY: process is an owned handle opened above.
    unsafe { CloseHandle(process) };
    if queried == 0 {
        return Err(PlatformError::NativeFailure);
    }
    let path = String::from_utf16_lossy(
        &path[..usize::try_from(path_length).map_err(|_| PlatformError::NativeFailure)?],
    );
    let process_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path)
        .to_owned();

    let mut rectangle = RECT::default();
    // SAFETY: rectangle is writable and window is the foreground HWND retained for this query.
    let window_bounds = if unsafe { GetWindowRect(window, &raw mut rectangle) } != 0 {
        let width = rectangle.right.saturating_sub(rectangle.left);
        let height = rectangle.bottom.saturating_sub(rectangle.top);
        match (u32::try_from(width), u32::try_from(height)) {
            (Ok(width), Ok(height)) if width > 0 && height > 0 => Some(WindowBounds {
                x: rectangle.left,
                y: rectangle.top,
                width,
                height,
            }),
            _ => None,
        }
    } else {
        None
    };

    Ok(FrontApp {
        process_name,
        window_title,
        window_bounds,
    })
}

const fn hook_status_to_u8(status: HookStatus) -> u8 {
    match status {
        HookStatus::Ready => 0,
        HookStatus::PermissionRequired => 1,
        HookStatus::Unavailable => 2,
        HookStatus::Stopped => 3,
    }
}

const fn hook_status_from_u8(value: u8) -> HookStatus {
    match value {
        0 => HookStatus::Ready,
        1 => HookStatus::PermissionRequired,
        3 => HookStatus::Stopped,
        _ => HookStatus::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RegistrarOperation {
        Register(i32, u32, u32),
        Unregister(i32),
    }

    #[derive(Default)]
    struct FakeRegistrar {
        active: BTreeMap<i32, (u32, u32)>,
        fail_register_once: BTreeSet<i32>,
        fail_unregister_once: BTreeSet<i32>,
        operations: Vec<RegistrarOperation>,
    }

    impl HotKeyRegistrar for FakeRegistrar {
        fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool {
            self.operations
                .push(RegistrarOperation::Register(id, modifiers, virtual_key));
            if self.fail_register_once.remove(&id)
                || self.active.contains_key(&id)
                || self
                    .active
                    .values()
                    .any(|chord| *chord == (modifiers, virtual_key))
            {
                return false;
            }
            self.active.insert(id, (modifiers, virtual_key));
            true
        }

        fn unregister(&mut self, id: i32) -> bool {
            self.operations.push(RegistrarOperation::Unregister(id));
            if self.fail_unregister_once.remove(&id) {
                return false;
            }
            self.active.remove(&id).is_some()
        }
    }

    fn bindings(values: &[(ActivationKey, bool)]) -> ActivationBindings {
        ActivationBindings::from_exact(values).unwrap()
    }

    fn config(enabled: bool, key: ActivationKey) -> ActivationConfig {
        ActivationConfig {
            enabled,
            bindings: bindings(&[(key, false), (key, true)]),
        }
    }

    fn hotkey_l_param(modifiers: u32, key: ActivationKey) -> LPARAM {
        ((activation_virtual_key(key) << 16) | modifiers) as LPARAM
    }

    fn modifiers(mask: u8) -> PasteModifiers {
        PasteModifiers {
            ctrl: mask & 0b00001 != 0,
            shift: mask & 0b00010 != 0,
            alt: mask & 0b00100 != 0,
            left_win: mask & 0b01000 != 0,
            right_win: mask & 0b10000 != 0,
        }
    }

    #[test]
    fn hotkey_registration_transaction_covers_lifecycle_and_alternating_ids() {
        let mut registrar = FakeRegistrar::default();
        let disabled = HotKeyRegistrationState::default();
        assert_eq!(disabled.config, ActivationConfig::default());
        assert!(registrar.active.is_empty());

        let enabled = configure_hotkeys(
            disabled,
            config(true, ActivationKey::A),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(enabled.slot, RegistrationSlot::B);
        assert_eq!(enabled.config, config(true, ActivationKey::A));
        assert_eq!(
            registrar
                .active
                .get(&RegistrationSlot::B.id(ActivationKey::A, false)),
            Some(&(
                hotkey_modifiers(false),
                activation_virtual_key(ActivationKey::A)
            ))
        );
        assert_eq!(
            registrar
                .active
                .get(&RegistrationSlot::B.id(ActivationKey::A, true)),
            Some(&(
                hotkey_modifiers(true),
                activation_virtual_key(ActivationKey::A)
            ))
        );
        assert!(hotkey_modifiers(false) & MOD_NOREPEAT != 0);
        assert!(hotkey_modifiers(true) & MOD_NOREPEAT != 0);

        let operation_count = registrar.operations.len();
        assert_eq!(
            configure_hotkeys(
                enabled,
                config(true, ActivationKey::A),
                &mut registrar,
                || true,
            ),
            Ok(enabled)
        );
        assert_eq!(registrar.operations.len(), operation_count);

        let changed = configure_hotkeys(
            enabled,
            config(true, ActivationKey::B),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(changed.slot, RegistrationSlot::A);
        assert_eq!(changed.config, config(true, ActivationKey::B));
        assert_eq!(registrar.active.len(), 2);
        assert!(
            registrar
                .active
                .contains_key(&RegistrationSlot::A.id(ActivationKey::B, false))
        );
        assert!(
            registrar
                .active
                .contains_key(&RegistrationSlot::A.id(ActivationKey::B, true))
        );

        let disabled_again = configure_hotkeys(
            changed,
            config(false, ActivationKey::C),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(disabled_again.config, config(false, ActivationKey::C));
        assert!(registrar.active.is_empty());

        let reenabled = configure_hotkeys(
            disabled_again,
            config(true, ActivationKey::D),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(reenabled.slot, RegistrationSlot::B);
        unregister_all_hotkeys(&mut registrar);
        assert!(registrar.active.is_empty());
    }

    #[test]
    fn partial_registration_conflict_and_cancellation_keep_previous_pair() {
        let mut registrar = FakeRegistrar::default();
        let current = configure_hotkeys(
            HotKeyRegistrationState::default(),
            config(true, ActivationKey::A),
            &mut registrar,
            || true,
        )
        .unwrap();
        let previous_active = registrar.active.clone();

        registrar
            .fail_register_once
            .insert(RegistrationSlot::A.id(ActivationKey::B, true));
        assert_eq!(
            configure_hotkeys(
                current,
                config(true, ActivationKey::B),
                &mut registrar,
                || true,
            ),
            Err(HotKeyTransactionError::Conflict)
        );
        assert_eq!(registrar.active, previous_active);

        assert_eq!(
            configure_hotkeys(
                current,
                config(true, ActivationKey::C),
                &mut registrar,
                || false,
            ),
            Err(HotKeyTransactionError::Cancelled)
        );
        assert_eq!(registrar.active, previous_active);
    }

    #[test]
    fn hotkey_id_mapping_and_message_policy_are_exact() {
        for (id, expected) in [
            (
                RegistrationSlot::A.id(ActivationKey::A, false),
                Some((RegistrationSlot::A, ActivationKey::A, false)),
            ),
            (
                RegistrationSlot::A.id(ActivationKey::A, true),
                Some((RegistrationSlot::A, ActivationKey::A, true)),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                Some((RegistrationSlot::B, ActivationKey::A, false)),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, true),
                Some((RegistrationSlot::B, ActivationKey::A, true)),
            ),
            (0, None),
        ] {
            assert_eq!(registration_from_id(id), expected);
        }

        let state = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: config(true, ActivationKey::A).bindings,
            generation: 1,
        };
        assert_eq!(
            hotkey_message_activation(
                state,
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            Some((ActivationKey::A, false))
        );
        assert_eq!(
            hotkey_message_activation(
                state,
                RegistrationSlot::B.id(ActivationKey::A, true),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ),
            Some((ActivationKey::A, true))
        );

        for (id, l_param) in [
            (
                RegistrationSlot::A.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, true),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT | 2, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::B),
            ),
        ] {
            assert_eq!(hotkey_message_activation(state, id, l_param), None);
        }

        let disabled = HotKeyRegistrationState {
            config: config(false, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: ActivationBindings::default(),
            generation: 2,
        };
        assert_eq!(
            hotkey_message_activation(
                disabled,
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            None
        );
    }

    #[test]
    fn candidate_interleavings_preserve_down_then_up_exactly_once() {
        use crate::keyboard::{EventPhase, HelperEvent};

        let registrations = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: config(true, ActivationKey::A).bindings,
            generation: 7,
        };
        let pressed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::A,
            true,
            false,
            false,
            false,
        );
        let candidate = pressed.candidate().unwrap();
        let (released, action) = release_activation_candidate(pressed, ActivationKey::A);
        assert_eq!(action, CandidateUpAction::SwallowAwaitMessage(candidate));
        assert!(candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::B.id(ActivationKey::A, false),
            hotkey_l_param(MOD_ALT, ActivationKey::A),
        ));
        assert!(matches!(
            released,
            ActivationCandidateState::ReleasedBeforeMessage(value) if value == candidate
        ));

        let mut reducer = KeyboardReducer::default();
        let down = reducer.plan_registered_hotkey(ActivationKey::A, false, ActivationKey::A, true);
        assert_eq!(
            down.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Down,
                shift: false,
            })
        );
        assert!(reducer.apply(down, true));
        let up = reducer.plan_passive_hook(
            KeyInput {
                key: PhysicalKey::Letter(ActivationKey::A),
                phase: KeyPhase::Up,
                alt: false,
                shift: false,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            false,
        );
        assert_eq!(
            up.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift: false,
            })
        );
        assert!(reducer.apply(up, true));

        // A duplicate or delayed message cannot re-establish the completed
        // sequence; only Pressed/ReleasedBeforeMessage states are accepted.
        let completed = ActivationCandidateState::Completed(candidate);
        assert!(!matches!(
            completed,
            ActivationCandidateState::Pressed(_)
                | ActivationCandidateState::ReleasedBeforeMessage(_)
        ));
    }

    #[test]
    fn candidate_validation_rejects_nonexact_and_stale_sequences() {
        let registrations = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 11,
        };
        for (key, alt, disallowed, repeat) in [
            (ActivationKey::B, true, false, false),
            (ActivationKey::A, false, false, false),
            (ActivationKey::A, true, true, false),
            (ActivationKey::A, true, false, true),
        ] {
            assert_eq!(
                candidate_for_exact_passive_down(
                    registrations,
                    ActivationCandidateState::Idle,
                    key,
                    alt,
                    false,
                    disallowed,
                    repeat,
                ),
                ActivationCandidateState::Idle
            );
        }

        let pressed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::A,
            true,
            true,
            false,
            false,
        );
        let candidate = pressed.candidate().unwrap();
        assert!(candidate.shift);
        assert!(candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::A.id(ActivationKey::A, true),
            hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
        ));
        for stale in [
            HotKeyRegistrationState {
                generation: 12,
                ..registrations
            },
            HotKeyRegistrationState {
                bank_b: registrations.config.bindings,
                ..registrations
            },
            HotKeyRegistrationState {
                config: config(true, ActivationKey::B),
                ..registrations
            },
        ] {
            assert!(!candidate_matches_message(
                candidate,
                stale,
                candidate.id(),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ));
        }
        assert!(!candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::A.id(ActivationKey::A, false),
            hotkey_l_param(MOD_ALT, ActivationKey::A),
        ));
    }

    #[test]
    fn candidate_release_balances_consumed_sequences_but_not_other_input() {
        let candidate = ActivationCandidate {
            generation: 3,
            slot: RegistrationSlot::A,
            key: ActivationKey::A,
            shift: false,
        };
        for (state, expected_state, expected_action) in [
            (
                ActivationCandidateState::Accepted(candidate),
                ActivationCandidateState::Accepted(candidate),
                CandidateUpAction::BalanceAccepted(candidate),
            ),
            (
                ActivationCandidateState::ConsumedDown(candidate),
                ActivationCandidateState::Completed(candidate),
                CandidateUpAction::SwallowConsumed(candidate),
            ),
        ] {
            assert_eq!(
                release_activation_candidate(state, ActivationKey::A),
                (expected_state, expected_action)
            );
            assert_eq!(
                release_activation_candidate(state, ActivationKey::B),
                (state, CandidateUpAction::Pass)
            );
        }
    }

    #[test]
    fn configuration_handoff_has_linearizable_commit_and_rollback_paths() {
        let handoff = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert!(commit_configuration(&handoff));
        assert_eq!(
            cancel_configuration(&handoff),
            ConfigurationHandoff::Committed
        );

        let handoff = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert_eq!(
            cancel_configuration(&handoff),
            ConfigurationHandoff::Cancelled
        );
        assert!(!commit_configuration(&handoff));
        assert_eq!(
            confirm_configuration_rollback(&handoff),
            ConfigurationHandoff::RollbackConfirmed
        );

        let handoff = AtomicU8::new(ConfigurationHandoff::Cancelled as u8);
        assert_eq!(
            transition_configuration(
                &handoff,
                ConfigurationHandoff::Cancelled,
                ConfigurationHandoff::TerminalRequested,
            ),
            ConfigurationHandoff::TerminalRequested
        );
        handoff.store(
            ConfigurationHandoff::TerminalComplete as u8,
            Ordering::Release,
        );
        assert_eq!(
            configuration_handoff(&handoff),
            ConfigurationHandoff::TerminalComplete
        );
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
    fn configuration_ack_timeout_never_converts_failure_to_success() {
        let pending = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert_eq!(
            configuration_ack_timeout(&pending),
            ConfigurationTimeoutAction::WakeTerminalCleanup
        );
        assert_eq!(
            configuration_handoff(&pending),
            ConfigurationHandoff::TerminalRequested
        );

        let committed = AtomicU8::new(ConfigurationHandoff::Committed as u8);
        assert_eq!(
            configuration_ack_timeout(&committed),
            ConfigurationTimeoutAction::Success
        );

        let rolled_back = AtomicU8::new(ConfigurationHandoff::RollbackConfirmed as u8);
        assert_eq!(
            configuration_ack_timeout(&rolled_back),
            ConfigurationTimeoutAction::Failed
        );
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
    fn paste_plan_rejects_every_conflicting_modifier_combination() {
        for mask in 0_u8..0b100000 {
            let expected = if mask & 0b11110 != 0 {
                Err(PasteFailure::ConflictingModifiers)
            } else if mask & 0b00001 != 0 {
                Ok(PASTE_WITH_PREHELD_CTRL.as_slice())
            } else {
                Ok(PASTE_WITH_CTRL.as_slice())
            };
            assert_eq!(paste_plan(modifiers(mask)), expected, "mask {mask:05b}");
        }
    }

    #[test]
    fn paste_plan_only_owns_ctrl_when_it_was_not_preheld() {
        assert_eq!(
            paste_plan(modifiers(0)),
            Ok([
                PasteKeyInput::down(VK_CONTROL),
                PasteKeyInput::down(VK_V),
                PasteKeyInput::up(VK_V),
                PasteKeyInput::up(VK_CONTROL),
            ]
            .as_slice())
        );
        assert_eq!(
            paste_plan(modifiers(1)),
            Ok([PasteKeyInput::down(VK_V), PasteKeyInput::up(VK_V)].as_slice())
        );
    }

    #[test]
    fn cleanup_covers_every_partial_count_when_ctrl_is_helper_owned() {
        let expected = [
            vec![],
            vec![PasteKeyInput::up(VK_CONTROL)],
            vec![PasteKeyInput::up(VK_V), PasteKeyInput::up(VK_CONTROL)],
            vec![PasteKeyInput::up(VK_CONTROL)],
        ];

        for (accepted, expected) in expected.iter().enumerate() {
            let (result, cleanup) = paste_outcome(&PASTE_WITH_CTRL, accepted);
            assert_eq!(
                result,
                PasteResult {
                    submitted: false,
                    reason: Some(PasteFailure::OsRejected),
                },
                "accepted {accepted}"
            );
            assert_eq!(
                cleanup.as_slice(),
                expected.as_slice(),
                "accepted {accepted}"
            );
        }
    }

    #[test]
    fn cleanup_covers_every_partial_count_when_ctrl_was_preheld() {
        let expected = [vec![], vec![PasteKeyInput::up(VK_V)]];

        for (accepted, expected) in expected.iter().enumerate() {
            let (result, cleanup) = paste_outcome(&PASTE_WITH_PREHELD_CTRL, accepted);
            assert_eq!(
                result,
                PasteResult {
                    submitted: false,
                    reason: Some(PasteFailure::OsRejected),
                },
                "accepted {accepted}"
            );
            assert_eq!(
                cleanup.as_slice(),
                expected.as_slice(),
                "accepted {accepted}"
            );
        }
    }

    #[test]
    fn paste_and_cleanup_plans_never_emit_keys_other_than_ctrl_or_v() {
        for plan in [&PASTE_WITH_CTRL[..], &PASTE_WITH_PREHELD_CTRL[..]] {
            for accepted in 0..=plan.len() {
                for input in plan.iter().chain(&paste_cleanup_plan(plan, accepted)) {
                    assert!(matches!(input.key, VK_CONTROL | VK_V));
                }
            }
        }
    }

    #[test]
    fn multi_binding_bank_registers_and_maps_every_exact_chord() {
        let requested = ActivationConfig {
            enabled: true,
            bindings: bindings(&[
                (ActivationKey::A, false),
                (ActivationKey::A, true),
                (ActivationKey::Q, false),
            ]),
        };
        let mut registrar = FakeRegistrar::default();
        let state = configure_hotkeys(
            HotKeyRegistrationState::default(),
            requested,
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 3);
        for (key, shift) in requested.bindings.iter() {
            let id = state.slot.id(key, shift);
            assert_eq!(registration_from_id(id), Some((state.slot, key, shift)));
            assert_eq!(
                hotkey_message_activation(
                    state,
                    id,
                    hotkey_l_param(MOD_ALT | if shift { MOD_SHIFT } else { 0 }, key),
                ),
                Some((key, shift))
            );
        }
    }

    #[test]
    fn overlapping_profile_add_edit_delete_retain_chords_without_duplicate_registration() {
        let mut registrar = FakeRegistrar::default();
        let initial = configure_hotkeys(
            HotKeyRegistrationState::default(),
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();

        let added = configure_hotkeys(
            initial,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false), (ActivationKey::Q, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 2);
        assert_eq!(
            added.slot_for(ActivationKey::A, false),
            Some(RegistrationSlot::B)
        );
        assert_eq!(
            added.slot_for(ActivationKey::Q, true),
            Some(RegistrationSlot::A)
        );

        let edited = configure_hotkeys(
            added,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false), (ActivationKey::R, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 2);
        assert_eq!(
            edited.slot_for(ActivationKey::A, false),
            Some(RegistrationSlot::B)
        );
        assert_eq!(
            edited.slot_for(ActivationKey::R, true),
            Some(RegistrationSlot::B)
        );
        assert_eq!(edited.slot_for(ActivationKey::Q, true), None);

        let deleted = configure_hotkeys(
            edited,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::R, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 1);
        assert_eq!(deleted.slot_for(ActivationKey::A, false), None);
        assert_eq!(
            deleted.slot_for(ActivationKey::R, true),
            Some(RegistrationSlot::B)
        );
        let id = RegistrationSlot::B.id(ActivationKey::R, true);
        assert_eq!(
            hotkey_message_activation(
                deleted,
                id,
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::R),
            ),
            Some((ActivationKey::R, true)),
        );
    }

    #[test]
    fn candidate_bank_conflict_rolls_back_without_changing_active_bank() {
        let current_config = ActivationConfig {
            enabled: true,
            bindings: bindings(&[(ActivationKey::A, false)]),
        };
        let mut registrar = FakeRegistrar::default();
        let current = configure_hotkeys(
            HotKeyRegistrationState::default(),
            current_config,
            &mut registrar,
            || true,
        )
        .unwrap();
        registrar
            .fail_register_once
            .insert(RegistrationSlot::A.id(ActivationKey::Q, true));
        let requested = ActivationConfig {
            enabled: true,
            bindings: bindings(&[(ActivationKey::B, false), (ActivationKey::Q, true)]),
        };
        assert_eq!(
            configure_hotkeys(current, requested, &mut registrar, || true),
            Err(HotKeyTransactionError::Conflict)
        );
        assert_eq!(registrar.active.len(), 1);
        assert!(
            registrar
                .active
                .contains_key(&current.slot.id(ActivationKey::A, false))
        );
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

    #[test]
    fn shifted_hotkey_message_corrects_a_stale_hook_shift_sample() {
        let registrations = HotKeyRegistrationState {
            config: ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::Z, false), (ActivationKey::Z, true)]),
            },
            slot: RegistrationSlot::B,
            bank_b: bindings(&[(ActivationKey::Z, false), (ActivationKey::Z, true)]),
            generation: 7,
        };
        let observed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::Z,
            true,
            false,
            false,
            false,
        )
        .candidate()
        .expect("physical candidate");
        assert!(!observed.shift);

        let resolved = resolve_activation_candidate_message(
            observed,
            registrations,
            RegistrationSlot::B.id(ActivationKey::Z, true),
            hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::Z),
        )
        .expect("registered hotkey is authoritative");
        assert_eq!(resolved.key, ActivationKey::Z);
        assert!(resolved.shift);
    }

    #[test]
    fn physical_candidate_requires_the_sampled_exact_binding() {
        let state = HotKeyRegistrationState {
            config: ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::Z, true)]),
            },
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 7,
        };
        assert!(matches!(
            candidate_for_exact_passive_down(
                state,
                ActivationCandidateState::Idle,
                ActivationKey::Z,
                true,
                true,
                false,
                false,
            ),
            ActivationCandidateState::Pressed(_)
        ));
        for (key, sampled_shift) in [(ActivationKey::Z, false), (ActivationKey::Y, true)] {
            assert_eq!(
                candidate_for_exact_passive_down(
                    state,
                    ActivationCandidateState::Idle,
                    key,
                    true,
                    sampled_shift,
                    false,
                    false,
                ),
                ActivationCandidateState::Idle
            );
        }
    }
}
