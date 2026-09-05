use std::{
    cell::Cell,
    ptr::null_mut,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{
        StationsAndDesktops::{
            CloseDesktop, DESKTOP_HOOKCONTROL, DESKTOP_JOURNALPLAYBACK, DESKTOP_READOBJECTS,
            DESKTOP_SWITCHDESKTOP, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop,
            SetThreadDesktop, UOI_NAME,
        },
        Threading::GetCurrentThreadId,
    },
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyboardLayout, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW, VK_CONTROL,
            VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN,
            VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_V, VkKeyScanExW,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_SYSTEM_FOREGROUND, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
            HC_ACTION, KBDLLHOOKSTRUCT, KillTimer, LLKHF_EXTENDED, MSG, PM_NOREMOVE, PM_REMOVE,
            PeekMessageW, PostQuitMessage, PostThreadMessageW, SetTimer, SetWindowsHookExW,
            TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WINEVENT_OUTOFCONTEXT, WM_APP,
            WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
        },
    },
};

use super::{
    audio_devices::AudioDeviceMonitor,
    clipboard,
    front_app::front_app,
    injection,
    target::{
        CandidateTargetEvidence, TargetEvidence, TargetRegistry, capture_candidate_target,
        revalidate_candidate_target, revalidate_target,
    },
};
use crate::{
    ActivationCaptureGate,
    platform::NativeEvent,
    platform::{
        CallbackGate, ClipboardTextHash, FrontApp, HookStatus, ModifierNeutralWait, PasteFailure,
        PasteResult, PermissionState, Permissions, Platform, PlatformError, PlatformShutdown,
        TerminalReason, TerminalSignal, TransactionObservability, TransactionObservabilitySnapshot,
        deliver_callback_event, hook_status_from_u8, hook_status_to_u8,
    },
};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, ActivationKey,
    EventPhase, KeyInput, KeyPhase, KeyboardEvent, KeyboardReducer, ModifierMask, PhysicalKey,
    PhysicalKeyTracker, SessionCaptureMode,
    transactional::{
        ActivationNotice, CancelReason, CompiledActivationConfig, Completion, ConfigRevision,
        Continuation, Control, EffectOutcome, EffectRequest, EngineInput, EventDisposition,
        GateState, InputSource, KeyIdentity, MAX_EFFECTS_PER_TURN, MatchClass, MatchCursor,
        ModifierSide, ModifierSides as TransactionalModifierSides, NativeKey, NormalizedEvent,
        PhysicalPhase, PhysicalSnapshot, ShutdownState, TransactionEngine, Turn,
    },
};

const WM_OWNER_COMMAND: u32 = WM_APP + 0x45;
const WM_OWNER_PASTE: u32 = WM_APP + 0x46;
const WM_OWNER_REPLAY: u32 = WM_APP + 0x47;
const PASTE_TIMER_INTERVAL_MS: u32 = 10;
const NATIVE_SNAPSHOT_INTERVAL_MS: u32 = 100;
const HOOK_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const PASTE_NEUTRAL_TIMEOUT: Duration = Duration::from_millis(1_500);
const PASTE_COMMAND_TIMEOUT: Duration = Duration::from_millis(1_800);
const PASTE_POST_CAS_COMPLETION_TIMEOUT: Duration = Duration::from_millis(250);
const OWNER_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_millis(1_500);
const OWNER_WAKE_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
];
static DPI_AWARENESS_READY: OnceLock<bool> = OnceLock::new();
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(null_mut());
static TARGET_CHANGE_EPOCH: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
std::thread_local! {
    static TEST_TRANSACTION_PANIC_POINT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[derive(Clone, Copy)]
#[repr(u8)]
enum TestTransactionPanicPoint {
    AfterTurn = 1,
    AfterEffect = 2,
}

#[cfg(all(test, feature = "windows-native-test-input"))]
fn arm_transaction_panic(point: TestTransactionPanicPoint) {
    TEST_TRANSACTION_PANIC_POINT.with(|armed| armed.set(point as u8));
}

#[cfg(test)]
fn run_reentrant_helper_callback_before_effect_panic(context: &CallbackContext) {
    let armed = TEST_TRANSACTION_PANIC_POINT
        .with(|point| point.get() == TestTransactionPanicPoint::AfterEffect as u8);
    if !armed {
        return;
    }
    let nested_disposition = Cell::new(CallbackDisposition::Capture);
    assert!(!process_transactional_hook_record_at(
        context,
        TransactionalHookRecord {
            virtual_key: 0xFF,
            scan_code: 0,
            extended: false,
            platform_flags: 0,
            phase: KeyPhase::Down,
            source: InputSource::HelperDummy,
        },
        HookObservation {
            observed_at_ms: 0,
            native_modifiers: None,
        },
        &nested_disposition,
    ));
    assert_eq!(nested_disposition.get(), CallbackDisposition::Pass);
}

#[cfg(test)]
fn panic_at_transaction_seam(point: TestTransactionPanicPoint) {
    TEST_TRANSACTION_PANIC_POINT.with(|armed| {
        if armed.get() == point as u8 {
            armed.set(0);
            panic!("deterministic Windows transaction panic seam");
        }
    });
}

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

fn post_owner_message_once(thread_id: u32, message: u32) -> bool {
    // SAFETY: owner messages are pointer-free and target the thread whose queue
    // is created before startup readiness is reported.
    unsafe { PostThreadMessageW(thread_id, message, 0, 0) != 0 }
}

fn post_owner_message(thread_id: u32, message: u32) -> bool {
    retry_owner_wake(
        || post_owner_message_once(thread_id, message),
        thread::sleep,
    )
}

fn owner_completed(receiver: &Receiver<()>, timeout: Duration) -> bool {
    receiver.recv_timeout(timeout).is_ok()
}

fn join_completed_owner(
    thread: JoinHandle<()>,
    completion: &Receiver<()>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    if !owner_completed(completion, timeout) {
        return false;
    }
    while !thread.is_finished() && Instant::now() < deadline {
        thread::yield_now();
    }
    thread.is_finished() && thread.join().is_ok()
}

struct OwnerHookInstallation {
    hook: windows_sys::Win32::UI::WindowsAndMessaging::HHOOK,
    win_events: [HWINEVENTHOOK; 3],
    timer: usize,
}

struct HookThreadDesktop {
    input: windows_sys::Win32::System::StationsAndDesktops::HDESK,
    previous: windows_sys::Win32::System::StationsAndDesktops::HDESK,
}

impl Drop for HookThreadDesktop {
    fn drop(&mut self) {
        // Restore the original assignment only after the hook has been removed.
        unsafe {
            let _ = SetThreadDesktop(self.previous);
            let _ = CloseDesktop(self.input);
        }
    }
}

impl Drop for OwnerHookInstallation {
    fn drop(&mut self) {
        if self.timer != 0 {
            // SAFETY: timer belongs to the current owner thread.
            unsafe { KillTimer(null_mut(), self.timer) };
        }
        for hook in self.win_events {
            if !hook.is_null() {
                // SAFETY: every handle was installed by this owner thread.
                unsafe { UnhookWinEvent(hook) };
            }
        }
        // The callback pointer remains live until the hook is uninstalled on
        // its owner thread.
        unsafe { UnhookWindowsHookEx(self.hook) };
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
    }
}

impl OwnerHookInstallation {
    fn refresh_keyboard_hook(&mut self) -> bool {
        // Windows silently removes timed-out low-level hooks. Reinstall on the
        // owning message thread, preserving the reducer and its key state.
        // Install first so a failed refresh leaves the previous hook intact.
        let replacement = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                low_level_hook_module(),
                0,
            )
        };
        if replacement.is_null() {
            return false;
        }
        let previous = std::mem::replace(&mut self.hook, replacement);
        // SAFETY: no message pump runs between installation and same-thread
        // removal. A stale handle from Windows' automatic removal is harmless.
        unsafe { UnhookWindowsHookEx(previous) };
        true
    }
}

unsafe extern "system" fn target_change_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    _window: windows_sys::Win32::Foundation::HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Candidate activation evidence intentionally models the foreground HWND,
    // not a virtual control inside it. Redundant focus/location notifications
    // from that same foreground must not cancel Alt+X between down and up.
    // Foreground transitions still provide the A->B->A epoch fence; concrete
    // HWND/process/thread identity is revalidated at every effect boundary.
    if event == EVENT_SYSTEM_FOREGROUND {
        let _ = TARGET_CHANGE_EPOCH.fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
            Some(epoch.saturating_add(1))
        });
    }
    let _ = object_id;
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
    session_capture_mode: AtomicU8,
    hook_status: AtomicU8,
    protocol_initialized: AtomicBool,
    stopping: AtomicBool,
    post_claim_timeout: AtomicBool,
    pending_native_work: AtomicBool,
    target_change_evidence_ready: AtomicBool,
    shutdown_deadline: Mutex<Option<Instant>>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            session_capture_mode: AtomicU8::new(SessionCaptureMode::Off.as_u8()),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            protocol_initialized: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            post_claim_timeout: AtomicBool::new(false),
            pending_native_work: AtomicBool::new(false),
            target_change_evidence_ready: AtomicBool::new(false),
            shutdown_deadline: Mutex::new(None),
        }
    }
}

mod tracking;
use tracking::*;

#[derive(Default)]
struct CallbackKeyboard {
    // Retained only for independent Escape/Enter capture and compatibility
    // tests. Global activation is exclusively transactional on Windows.
    reducer: KeyboardReducer,
    transactional: TransactionEngine,
    transaction_authority: Option<TransactionAuthority>,
    deferred_callback_replay: Option<DeferredCallbackReplay>,
    deferred_replay_raced: bool,
    deferred_menu_releases: [Option<NativeKey>; 4],
    deferred_observed_at_ms: u64,
    dispatcher: ActivationDispatcher,
    physical: WindowsPhysicalTracker,
    modifiers: ModifierTracker,
    activation_fenced_letters: u32,
    activation: ActivationConfig,
    captured_enter_source: Option<EnterSource>,
    session_escape_native_owned: bool,
    altgr_active: bool,
    altgr_synthetic_ctrl: bool,
    modifiers_fenced: bool,
    pending_paste_cleanup: injection::PasteCleanup,
    candidate_target: Option<CandidateTargetEvidence>,
    candidate_desktop: Option<DesktopIdentity>,
    candidate_target_epoch: Option<u64>,
    candidate_target_changed: bool,
    input_desktop: Option<DesktopIdentity>,
    logical_v_down: bool,
    shutdown_requested: bool,
    external_reconcile_after: Option<Instant>,
    external_held_letters: u32,
    registered_observation: RegisteredObservationShadow,
}

struct CallbackContext {
    state: Arc<SharedState>,
    keyboard: Mutex<CallbackKeyboard>,
    owner_epoch: Instant,
    suppression_enabled: bool,
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
    injection_markers: injection::InjectionMarkers,
    replay_sender: Option<Sender<ReplayWork>>,
    replay_accepted: Arc<AtomicU64>,
}

fn lock_keyboard_recovering(context: &CallbackContext) -> Option<MutexGuard<'_, CallbackKeyboard>> {
    match context.keyboard.try_lock() {
        Ok(keyboard) => Some(keyboard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            Some(poisoned.into_inner())
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerMutationKind {
    Configure,
    CloseAdmission,
    CancelCandidate,
    SetSessionCapture,
    #[cfg(test)]
    InjectTestUnmatchedDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnerMutation {
    kind: OwnerMutationKind,
    activation: ActivationConfig,
    session_capture_mode: SessionCaptureMode,
}

impl OwnerMutation {
    fn configure(activation: ActivationConfig) -> Self {
        Self {
            kind: OwnerMutationKind::Configure,
            activation,
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    fn close_admission() -> Self {
        Self {
            kind: OwnerMutationKind::CloseAdmission,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    fn cancel_candidate() -> Self {
        Self {
            kind: OwnerMutationKind::CancelCandidate,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
        }
    }

    fn set_session_capture(mode: SessionCaptureMode) -> Self {
        Self {
            kind: OwnerMutationKind::SetSessionCapture,
            activation: ActivationConfig::default(),
            session_capture_mode: mode,
        }
    }

    #[cfg(test)]
    fn inject_test_unmatched_down() -> Self {
        Self {
            kind: OwnerMutationKind::InjectTestUnmatchedDown,
            activation: ActivationConfig::default(),
            session_capture_mode: SessionCaptureMode::Off,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum PasteCommandState {
    Pending,
    Waiting,
    Injecting,
    Committed,
    Applied,
    Cancelled,
    ResultReady,
}

struct PasteResultSlot {
    value: AtomicU8,
}

impl PasteResultSlot {
    const PENDING: u8 = 0;
    const SUCCESS: u8 = 1;
    const PERMISSION_DENIED: u8 = 2;
    const CONFLICTING_MODIFIERS: u8 = 3;
    const SECURE_INPUT: u8 = 4;
    const OS_REJECTED: u8 = 5;
    const UNAVAILABLE: u8 = 6;
    const INDETERMINATE: u8 = 7;

    const fn new() -> Self {
        Self {
            value: AtomicU8::new(Self::PENDING),
        }
    }

    const fn encode(result: PasteResult) -> u8 {
        if result.submitted {
            Self::SUCCESS
        } else {
            match result.reason {
                Some(PasteFailure::PermissionDenied) => Self::PERMISSION_DENIED,
                Some(PasteFailure::ConflictingModifiers) => Self::CONFLICTING_MODIFIERS,
                Some(PasteFailure::SecureInput) => Self::SECURE_INPUT,
                Some(PasteFailure::OsRejected) => Self::OS_REJECTED,
                Some(PasteFailure::Unavailable) | None => Self::UNAVAILABLE,
                Some(PasteFailure::Indeterminate) => Self::INDETERMINATE,
            }
        }
    }

    const fn decode(value: u8) -> Option<PasteResult> {
        match value {
            1 => Some(PasteResult {
                submitted: true,
                reason: None,
            }),
            2 => Some(failed_paste(PasteFailure::PermissionDenied)),
            3 => Some(failed_paste(PasteFailure::ConflictingModifiers)),
            4 => Some(failed_paste(PasteFailure::SecureInput)),
            5 => Some(failed_paste(PasteFailure::OsRejected)),
            6 => Some(failed_paste(PasteFailure::Unavailable)),
            7 => Some(failed_paste(PasteFailure::Indeterminate)),
            _ => None,
        }
    }

    fn publish(&self, result: PasteResult) -> PasteResult {
        let proposed = Self::encode(result);
        let value = self
            .value
            .compare_exchange(Self::PENDING, proposed, Ordering::AcqRel, Ordering::Acquire)
            .map_or_else(|current| current, |_| proposed);
        Self::decode(value).expect("published paste result encoding")
    }

    fn result(&self) -> Option<PasteResult> {
        Self::decode(self.value.load(Ordering::Acquire))
    }
}

struct PasteCommand {
    context: ActivationContext,
    expected_clipboard_sha256: ClipboardTextHash,
    injection_deadline: Instant,
    state: Arc<AtomicU8>,
    result: Arc<PasteResultSlot>,
    acknowledgement: Sender<()>,
}

struct PendingPaste {
    state: Arc<AtomicU8>,
    result: Arc<PasteResultSlot>,
    expected_clipboard_sha256: ClipboardTextHash,
    acknowledgement: Sender<()>,
    evidence: TargetEvidence,
    deadline: Instant,
    timer_id: usize,
    modifier_wait: ModifierNeutralWait,
}

fn paste_command_state(state: &AtomicU8) -> PasteCommandState {
    match state.load(Ordering::Acquire) {
        1 => PasteCommandState::Waiting,
        2 => PasteCommandState::Injecting,
        3 => PasteCommandState::Committed,
        4 => PasteCommandState::Applied,
        5 => PasteCommandState::Cancelled,
        6 => PasteCommandState::ResultReady,
        _ => PasteCommandState::Pending,
    }
}

fn claim_paste_injection(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            PasteCommandState::Waiting as u8,
            PasteCommandState::Injecting as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn cancel_paste_command(state: &AtomicU8) -> PasteCommandState {
    loop {
        let current = paste_command_state(state);
        if !matches!(
            current,
            PasteCommandState::Pending | PasteCommandState::Waiting
        ) {
            return current;
        }
        if state
            .compare_exchange(
                current as u8,
                PasteCommandState::Cancelled as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return PasteCommandState::Cancelled;
        }
    }
}

fn wait_for_claimed_paste_completion(
    response: &Receiver<()>,
    result: &PasteResultSlot,
    timeout: Duration,
) -> PasteResult {
    let received = response.recv_timeout(timeout).is_ok();
    if let Some(result) = result.result() {
        return result;
    }
    debug_assert!(
        !received,
        "SendInput result signal publishes its slot first"
    );
    failed_paste(PasteFailure::Indeterminate)
}

const fn failed_paste(reason: PasteFailure) -> PasteResult {
    PasteResult {
        submitted: false,
        reason: Some(reason),
    }
}

mod native_platform;
pub use native_platform::NativePlatform;
#[cfg(all(test, feature = "windows-native-test-input"))]
use native_platform::isolated_audio_terminal;

mod transaction;
use transaction::*;

mod commands;
use commands::*;

mod paste;
use paste::*;

mod desktop;
use desktop::*;

mod owner_thread;
use owner_thread::*;

mod callback;
use callback::*;

mod session;
use session::*;

mod key_mapping;
use key_mapping::*;

#[cfg(all(test, feature = "windows-native-test-input"))]
mod tests;
