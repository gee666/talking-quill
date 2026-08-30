#![cfg_attr(not(feature = "windows-native-test-input"), allow(dead_code))]

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
            CloseDesktop, DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP,
            GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop,
            UOI_NAME,
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

#[cfg(test)]
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

    const fn transactional_sides(self) -> TransactionalModifierSides {
        let mut bits = 0_u8;
        if self.ctrl.left || self.ctrl.generic {
            bits |= ModifierSide::LeftCtrl.bit();
        }
        if self.ctrl.right {
            bits |= ModifierSide::RightCtrl.bit();
        }
        if self.alt.left || self.alt.generic {
            bits |= ModifierSide::LeftAlt.bit();
        }
        if self.alt.right {
            bits |= ModifierSide::RightAlt.bit();
        }
        if self.shift.left || self.shift.generic {
            bits |= ModifierSide::LeftShift.bit();
        }
        if self.shift.right {
            bits |= ModifierSide::RightShift.bit();
        }
        if self.meta.left {
            bits |= ModifierSide::LeftMeta.bit();
        }
        if self.meta.right {
            bits |= ModifierSide::RightMeta.bit();
        }
        TransactionalModifierSides::from_bits(bits)
    }

    const fn is_neutral(self) -> bool {
        !self.mask().any()
    }
}

#[cfg(test)]
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
struct DesktopIdentity {
    name: [u16; 64],
    len: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TransactionalHookRecord {
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    platform_flags: u32,
    phase: KeyPhase,
    source: InputSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletedHookRecord {
    identity: KeyIdentity,
    virtual_key: u16,
    phase: KeyPhase,
    repeat: bool,
    enter_source: Option<EnterSource>,
    physical: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveActivationContext {
    binding: ActivationBinding,
    context: ActivationContext,
}

#[derive(Debug)]
struct ActivationDispatcher {
    next_generation: Option<ActivationGeneration>,
    active: Option<ActiveActivationContext>,
    targets: TargetRegistry,
}

impl ActivationDispatcher {
    fn deliver(
        &mut self,
        outbound: &Sender<NativeEvent>,
        terminal: &TerminalSignal,
        context_observability: &TransactionObservability,
        notice: ActivationNotice,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> bool {
        match notice {
            ActivationNotice::Down { binding } => {
                context_observability.record_registered_match_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: EventPhase::Down,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    self.active = Some(ActiveActivationContext { binding, context });
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
            ActivationNotice::Up { binding, .. } => {
                context_observability.record_registered_release_callback();
                let Some(active) = self.active.take() else {
                    return false;
                };
                if active.binding != binding {
                    self.targets.remove(active.context);
                    return false;
                }
                let delivered = deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    },
                );
                if delivered {
                    context_observability.record_callback_channel_accepted();
                } else {
                    context_observability.record_callback_channel_rejected();
                }
                delivered
            }
            ActivationNotice::Complete { binding, held_ms } => {
                context_observability.record_registered_match_callback();
                context_observability.record_registered_release_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::ActivationComplete {
                    binding,
                    context,
                    held_ms,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
        }
    }

    fn take_context(
        &mut self,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> Option<ActivationContext> {
        let generation = self.next_generation?;
        self.next_generation = if generation == ActivationGeneration::MAX {
            None
        } else {
            ActivationGeneration::new(generation.get() + 1)
        };
        Some(self.targets.capture_context(
            generation,
            candidate_target.and_then(CandidateTargetEvidence::paste_evidence),
        ))
    }
}

impl Default for ActivationDispatcher {
    fn default() -> Self {
        Self {
            next_generation: Some(ActivationGeneration::FIRST),
            active: None,
            targets: TargetRegistry::new(),
        }
    }
}

// Both variants are fixed-capacity reducer authority. Boxing would allocate in
// the low-level callback and would weaken panic recovery.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
enum TransactionAuthority {
    Turn(Turn),
    AwaitingDeferredReplay,
    Resume {
        continuation: Continuation,
        outcome: EffectOutcome,
    },
}

#[derive(Clone, Debug)]
struct DeferredCallbackReplay {
    continuation: Continuation,
    record: Option<CompletedHookRecord>,
    outcome: DeferredEffectOutcome,
}

#[derive(Clone, Copy, Debug)]
enum DeferredEffectOutcome {
    Replay,
    MenuNeutralization,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
enum ReplayWork {
    Replay {
        batch: talking_quill_keyboard_core::transactional::ReplayBatch,
        target: CandidateTargetEvidence,
        desktop: DesktopIdentity,
    },
    NeutralizeMenu {
        modifiers: talking_quill_keyboard_core::transactional::MenuModifiers,
        target: CandidateTargetEvidence,
        desktop: DesktopIdentity,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum CallbackDisposition {
    Pass = 0,
    Capture = 1,
}

#[derive(Clone, Copy, Debug)]
struct ShadowCandidate {
    generation: u64,
    revision: ConfigRevision,
    expected_modifiers: TransactionalModifierSides,
    cursor: MatchCursor,
    held_letters: u32,
    pending_exact: Option<(ActivationBinding, ActivationKey)>,
}

/// Non-owning mirror of the production matcher. It consumes the same compiled
/// prefix family and physical snapshots, but has no disposition/effect API.
#[derive(Clone, Copy, Debug)]
struct RegisteredObservationShadow {
    candidate: Option<ShadowCandidate>,
    next_generation: u64,
}

impl Default for RegisteredObservationShadow {
    fn default() -> Self {
        Self {
            candidate: None,
            next_generation: 1,
        }
    }
}

impl RegisteredObservationShadow {
    fn reset(&mut self) {
        self.candidate = None;
    }

    fn cancel(&mut self) {
        self.candidate = None;
    }

    fn observe(
        &mut self,
        observability: &TransactionObservability,
        config: CompiledActivationConfig,
        snapshot: PhysicalSnapshot,
        identity: KeyIdentity,
        phase: PhysicalPhase,
    ) -> Option<u64> {
        let Some(mut candidate) = self.candidate else {
            let KeyIdentity::Letter(letter) = identity else {
                return None;
            };
            let bit = 1_u32 << letter.index();
            if phase != PhysicalPhase::Down
                || snapshot.alt_gr_active
                || snapshot.held_letters != bit
            {
                return None;
            }
            let result = config
                .matcher()
                .start(snapshot.modifiers.combined(), letter);
            let cursor = result.cursor()?;
            let generation = self.next_generation;
            self.next_generation = self
                .next_generation
                .saturating_add(1)
                .clamp(1, crate::platform::observability::MAX_OBSERVABILITY_COUNTER);
            observability.record_registered_candidate_callback();
            let pending_exact = shadow_pending_exact(result, letter);
            if pending_exact.is_some() {
                observability.record_registered_match_callback();
            }
            self.candidate = Some(ShadowCandidate {
                generation,
                revision: config.revision(),
                expected_modifiers: snapshot.modifiers,
                cursor,
                held_letters: bit,
                pending_exact,
            });
            return None;
        };

        if candidate.revision != config.revision()
            || snapshot.alt_gr_active
            || snapshot.modifiers != candidate.expected_modifiers
        {
            self.cancel();
            return None;
        }
        let KeyIdentity::Letter(letter) = identity else {
            if !matches!(identity, KeyIdentity::Modifier(_)) {
                self.cancel();
            }
            return None;
        };
        let bit = 1_u32 << letter.index();
        match phase {
            PhysicalPhase::Repeat => {
                if candidate.held_letters & bit == 0 {
                    self.cancel();
                }
            }
            PhysicalPhase::Down => {
                if candidate.held_letters & bit != 0 {
                    self.cancel();
                    return None;
                }
                let result = config.matcher().advance(candidate.cursor, letter);
                let Some(cursor) = result.cursor() else {
                    self.cancel();
                    return None;
                };
                candidate.cursor = cursor;
                candidate.held_letters |= bit;
                candidate.pending_exact = shadow_pending_exact(result, letter);
                if candidate.pending_exact.is_some() {
                    observability.record_registered_match_callback();
                }
                self.candidate = Some(candidate);
            }
            PhysicalPhase::Up => {
                if candidate.held_letters & bit == 0 {
                    self.cancel();
                    return None;
                }
                candidate.held_letters &= !bit;
                if candidate
                    .pending_exact
                    .is_some_and(|(_, trigger)| trigger == letter)
                {
                    observability.record_registered_release_callback();
                    self.reset();
                    return Some(candidate.generation);
                }
                self.cancel();
            }
        }
        None
    }
}

fn shadow_pending_exact(
    result: MatchClass,
    trigger: ActivationKey,
) -> Option<(ActivationBinding, ActivationKey)> {
    match result {
        MatchClass::Exact { binding, .. } | MatchClass::ExactWithLonger { binding, .. } => {
            Some((binding, trigger))
        }
        MatchClass::Prefix(_) | MatchClass::NoCandidate => None,
    }
}

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

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
    audio_monitor: Option<AudioDeviceMonitor>,
    owner_commands: Sender<OwnerCommand>,
    paste_commands: Sender<PasteCommand>,
    thread_id: u32,
    owner_completion: Receiver<()>,
    thread: Option<JoinHandle<()>>,
    shutdown_completed: bool,
    shutdown_observability_quiescent: bool,
    capture_gate: ActivationCaptureGate,
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

    fn submit_paste(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
            || !self
                .state
                .target_change_evidence_ready
                .load(Ordering::Acquire)
        {
            return failed_paste(PasteFailure::Unavailable);
        }
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, response) = bounded(1);
        let submitted_at = Instant::now();
        let injection_deadline = submitted_at + PASTE_NEUTRAL_TIMEOUT;
        let response_deadline = submitted_at + PASTE_COMMAND_TIMEOUT;
        if self
            .paste_commands
            .send_timeout(
                PasteCommand {
                    context,
                    expected_clipboard_sha256,
                    injection_deadline,
                    state: Arc::clone(&state),
                    result: Arc::clone(&result),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .is_err()
            || !post_owner_message(self.thread_id, WM_OWNER_PASTE)
        {
            let _ = cancel_paste_command(&state);
            return failed_paste(PasteFailure::Unavailable);
        }
        let _ = response.recv_timeout(response_deadline.saturating_duration_since(Instant::now()));
        if let Some(result) = result.result() {
            return result;
        }
        match cancel_paste_command(&state) {
            PasteCommandState::Injecting => {
                // Once Waiting->Injecting wins, duplicate fallback is unsafe:
                // SendInput has irreversible authority. Wait only for the
                // bounded post-CAS completion budget. If the native call does
                // not return, fail the owner closed and conservatively report
                // submitted so the host cannot issue another insertion.
                let completion = wait_for_claimed_paste_completion(
                    &response,
                    &result,
                    PASTE_POST_CAS_COMPLETION_TIMEOUT,
                );
                if completion.reason == Some(PasteFailure::Indeterminate) {
                    self.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    self.state.post_claim_timeout.store(true, Ordering::Release);
                    self.gate.close();
                }
                completion
            }
            PasteCommandState::Committed
            | PasteCommandState::Applied
            | PasteCommandState::ResultReady => {
                if let Some(result) = result.result() {
                    result
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    failed_paste(PasteFailure::Unavailable)
                }
            }
            PasteCommandState::Pending
            | PasteCommandState::Waiting
            | PasteCommandState::Cancelled => result
                .result()
                .unwrap_or_else(|| failed_paste(PasteFailure::Unavailable)),
        }
    }

    #[cfg(test)]
    fn inject_test_unmatched_down(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::inject_test_unmatched_down(),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }
}

fn isolated_audio_terminal() -> (Arc<CallbackGate>, Arc<TerminalSignal>) {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (sender, _events) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), sender));
    (gate, terminal)
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        // DPI awareness improves target geometry but is not keyboard authority.
        // Windows can reject this when process policy established awareness
        // earlier; registered keybindings must remain available regardless.
        let _ = DPI_AWARENESS_READY.get_or_init(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0
        });

        let state = Arc::new(SharedState::new());
        let observability = Arc::new(TransactionObservability::new());
        // Core Audio is auxiliary. A missing endpoint service or COM failure
        // must not prevent installation of the keyboard hook.
        let (_audio_terminal_gate, audio_terminal) = isolated_audio_terminal();
        // Publication shares the global gate so shutdown closes every callback
        // class atomically. Audio failures use the separate terminal above and
        // therefore cannot close keyboard admission.
        let mut audio_monitor =
            AudioDeviceMonitor::start(outbound.clone(), Arc::clone(&gate), audio_terminal).ok();
        let Some(injection_markers) = injection::InjectionMarkers::generate() else {
            if let Some(monitor) = audio_monitor.as_mut() {
                let _ = monitor.shutdown();
            }
            return Err(PlatformError::HookUnavailable);
        };
        let context = CallbackContext {
            state: Arc::clone(&state),
            keyboard: Mutex::new(CallbackKeyboard::default()),
            owner_epoch: Instant::now(),
            suppression_enabled: capture_gate.is_open(),
            outbound,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
            observability: Arc::clone(&observability),
            injection_markers,
            replay_sender: None,
            replay_accepted: Arc::new(AtomicU64::new(0)),
        };
        let (ready_tx, ready_rx) = bounded(1);
        let startup_state = Arc::new(AtomicU8::new(StartupState::Pending as u8));
        let (owner_completion_tx, owner_completion) = bounded(1);
        let (owner_commands, owner_command_receiver) = bounded(8);
        let (paste_commands, paste_command_receiver) = bounded(1);
        let owner_startup_state = Arc::clone(&startup_state);
        let thread = match thread::Builder::new()
            .name("talking-quill-helper-win-hook".into())
            .spawn(move || {
                hook_thread(
                    context,
                    ready_tx,
                    owner_startup_state,
                    owner_command_receiver,
                    paste_command_receiver,
                    owner_completion_tx,
                );
            }) {
            Ok(thread) => thread,
            Err(_) => {
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(PlatformError::ThreadStopped);
            }
        };

        let thread_id = match ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                if let Some(monitor) = audio_monitor.as_mut() {
                    monitor.begin_shutdown();
                }
                let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(error);
            }
            Err(_) => {
                gate.close();
                if let Some(monitor) = audio_monitor.as_mut() {
                    monitor.begin_shutdown();
                }
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
                let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(PlatformError::ThreadStopped);
            }
        };

        if terminal.is_triggered() {
            gate.close();
            if let Some(monitor) = audio_monitor.as_mut() {
                monitor.begin_shutdown();
            }
            let _ = post_owner_message(thread_id, WM_QUIT);
            let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
            if let Some(monitor) = audio_monitor.as_mut() {
                let _ = monitor.shutdown();
            }
            return Err(PlatformError::NativeFailure);
        }

        let platform = Self {
            state,
            gate,
            terminal,
            observability,
            audio_monitor,
            owner_commands,
            paste_commands,
            thread_id,
            owner_completion,
            thread: Some(thread),
            shutdown_completed: false,
            shutdown_observability_quiescent: true,
            capture_gate,
        };
        #[cfg(test)]
        if std::env::var_os("TALKING_QUILL_TEST_UNMATCHED_DOWN").is_some() {
            platform.inject_test_unmatched_down()?;
        }
        Ok(platform)
    }

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn protocol_initialized(&self) {
        self.state
            .protocol_initialized
            .store(true, Ordering::Release);
        if let Some(monitor) = self.audio_monitor.as_ref() {
            monitor.protocol_initialized();
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let enabled = self.capture_gate.filter_enabled(enabled);
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn close_activation_admission(
        &self,
        _bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::close_admission(),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn cancel_activation_candidate(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::cancel_candidate(),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError> {
        let mode = self.capture_gate.filter_session_mode(mode);
        if mode == SessionCaptureMode::Off {
            // Fail open at the caller's commit boundary. The owner command acknowledges the
            // mutation without clearing reducer ownership needed to balance an already-held key.
            self.state
                .session_capture_mode
                .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        }
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(mode),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn inject_paste(&self) -> PasteResult {
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation(&self, _context: ActivationContext) -> PasteResult {
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation_with_clipboard_hash(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        self.submit_paste(context, expected_clipboard_sha256)
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

    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        self.observability.snapshot()
    }

    fn record_adapter_dequeued(&self) {
        self.observability.record_adapter_dequeued();
    }

    fn native_work_pending(&self) -> bool {
        self.state.pending_native_work.load(Ordering::Acquire)
    }

    fn shutdown(&mut self) -> PlatformShutdown {
        if self.shutdown_completed {
            return PlatformShutdown {
                terminal_reason: self.terminal.reason(),
                observability_quiescent: self.shutdown_observability_quiescent,
                terminal_incomplete: self
                    .observability
                    .snapshot()
                    .native_paste
                    .shutdown_ownership_deadlines
                    != 0,
            };
        }
        self.gate.close();
        self.state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        let deadline = Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        match self.state.shutdown_deadline.lock() {
            Ok(mut installed) => {
                let _ = installed.get_or_insert(deadline);
            }
            Err(poisoned) => {
                let _ = poisoned.into_inner().get_or_insert(deadline);
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            }
        }
        self.state.stopping.store(true, Ordering::Release);
        if let Some(monitor) = self.audio_monitor.as_mut() {
            monitor.begin_shutdown();
        }
        if self.thread.is_some() && !post_owner_message(self.thread_id, WM_QUIT) {
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
        }

        if let Some(thread) = self.thread.take() {
            let wait = deadline.saturating_duration_since(Instant::now());
            let completed = owner_completed(&self.owner_completion, wait) || thread.is_finished();
            while completed && !thread.is_finished() && Instant::now() < deadline {
                thread::yield_now();
            }
            if completed && thread.is_finished() {
                if thread.join().is_ok() {
                    self.state
                        .hook_status
                        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                }
            } else {
                // The owner has exhausted the single native deadline. Detach
                // rather than blocking process shutdown or inventing recovery.
                self.shutdown_observability_quiescent = false;
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                drop(thread);
            }
        }
        if let Some(monitor) = self.audio_monitor.as_mut() {
            // Audio shutdown cannot revoke otherwise healthy keyboard capture.
            let _ =
                monitor.shutdown_with_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        self.shutdown_completed = true;
        PlatformShutdown {
            terminal_reason: self.terminal.reason(),
            observability_quiescent: self.shutdown_observability_quiescent,
            terminal_incomplete: self
                .observability
                .snapshot()
                .native_paste
                .shutdown_ownership_deadlines
                != 0,
        }
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn candidate_target_is_current(keyboard: &CallbackKeyboard) -> bool {
    let (Some(expected), Some(expected_desktop), Some(expected_epoch)) = (
        keyboard.candidate_target,
        keyboard.candidate_desktop,
        keyboard.candidate_target_epoch,
    ) else {
        return false;
    };
    #[cfg(test)]
    if expected.is_test_only() {
        return true;
    }
    !keyboard.candidate_target_changed
        && current_input_desktop() == Some(expected_desktop)
        && TARGET_CHANGE_EPOCH.load(Ordering::Acquire) == expected_epoch
        && revalidate_candidate_target(expected)
}

fn capture_coherent_candidate_target(
    trusted_desktop: Option<DesktopIdentity>,
) -> Option<(CandidateTargetEvidence, DesktopIdentity, u64)> {
    #[cfg(test)]
    if capture_candidate_target().is_none() {
        return Some((
            CandidateTargetEvidence::test_only(),
            trusted_desktop.unwrap_or(DesktopIdentity {
                name: [0; 64],
                len: 0,
            }),
            TARGET_CHANGE_EPOCH.load(Ordering::Acquire),
        ));
    }
    let desktop = trusted_desktop?;
    let before_epoch = TARGET_CHANGE_EPOCH.load(Ordering::Acquire);
    let target = capture_candidate_target()?;
    let after_epoch = TARGET_CHANGE_EPOCH.load(Ordering::Acquire);
    (before_epoch == after_epoch && revalidate_candidate_target(target)).then_some((
        target,
        desktop,
        before_epoch,
    ))
}

fn validate_candidate_target(keyboard: &mut CallbackKeyboard) -> bool {
    let current = candidate_target_is_current(keyboard);
    if !current && keyboard.candidate_target.is_some() {
        keyboard.candidate_target_changed = true;
    }
    current
}

fn drive_transaction_turn(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    mut turn: Turn,
    callback_disposition: Option<&Cell<CallbackDisposition>>,
) -> Option<Completion> {
    for _ in 0..=MAX_EFFECTS_PER_TURN {
        if transaction_gate(context) == GateState::Closed
            && matches!(&turn, Turn::NeedEffect { .. })
            && !turn.effect_is_cleanup()
        {
            turn = turn.close_before_unsubmitted_effect(CancelReason::GateClosed);
        }
        keyboard.transaction_authority = Some(TransactionAuthority::Turn(turn.clone()));
        if let Some(callback_disposition) = callback_disposition {
            callback_disposition.set(
                if turn.event_disposition_hint() == Some(EventDisposition::CaptureCurrent) {
                    CallbackDisposition::Capture
                } else {
                    CallbackDisposition::Pass
                },
            );
        }
        #[cfg(test)]
        panic_at_transaction_seam(TestTransactionPanicPoint::AfterTurn);
        match turn {
            Turn::Complete { engine, completion } => {
                context.observability.publish(engine.metrics());
                if engine.journal_len() == 0 && engine.owned_letters() == 0 {
                    keyboard.candidate_target = None;
                    keyboard.candidate_desktop = None;
                    keyboard.candidate_target_epoch = None;
                    keyboard.candidate_target_changed = false;
                }
                keyboard.transactional = engine;
                keyboard.transaction_authority = None;
                return Some(completion);
            }
            Turn::NeedEffect {
                effect,
                continuation,
            } => {
                let outcome = match effect {
                    EffectRequest::NeutralizeMenu(modifiers) if callback_disposition.is_some() => {
                        let target_valid_at_callback = validate_candidate_target(keyboard);
                        keyboard.deferred_callback_replay = Some(DeferredCallbackReplay {
                            continuation,
                            record: None,
                            outcome: DeferredEffectOutcome::MenuNeutralization,
                        });
                        keyboard.transaction_authority =
                            Some(TransactionAuthority::AwaitingDeferredReplay);
                        let submitted = target_valid_at_callback
                            && keyboard
                                .candidate_target
                                .zip(keyboard.candidate_desktop)
                                .is_some_and(|(target, desktop)| {
                                    context.replay_sender.as_ref().is_some_and(|sender| {
                                        sender
                                            .try_send(ReplayWork::NeutralizeMenu {
                                                modifiers,
                                                target,
                                                desktop,
                                            })
                                            .is_ok()
                                    })
                                });
                        if !submitted {
                            context.replay_accepted.store(1, Ordering::Release);
                            post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            );
                        }
                        return None;
                    }
                    EffectRequest::NeutralizeMenu(modifiers) => EffectOutcome::Neutralized {
                        accepted: if validate_candidate_target(keyboard) {
                            injection::neutralize_menu(context.injection_markers, modifiers)
                        } else {
                            0
                        },
                    },
                    EffectRequest::CleanupMenuNeutralization(modifiers) => {
                        EffectOutcome::MenuCleanupAccepted {
                            accepted: injection::cleanup_menu_neutralization(
                                context.injection_markers,
                                modifiers,
                            ),
                        }
                    }
                    EffectRequest::DeliverActivation(notice) => {
                        let target_valid = matches!(notice, ActivationNotice::Up { .. })
                            || validate_candidate_target(keyboard);
                        EffectOutcome::ActivationDelivered(
                            target_valid
                                && keyboard.dispatcher.deliver(
                                    &context.outbound,
                                    &context.terminal,
                                    &context.observability,
                                    notice,
                                    keyboard.candidate_target,
                                ),
                        )
                    }
                    EffectRequest::Replay(batch) if callback_disposition.is_some() => {
                        // SendInput from inside WH_KEYBOARD_LL can strand the hook even
                        // though our private marker bypasses reducer recursion. Retain
                        // the continuation and current-edge suppression authority, then
                        // submit replay from the windowless owner's message pump after
                        // this callback returns.
                        let target_valid_at_callback = validate_candidate_target(keyboard);
                        keyboard.deferred_callback_replay = Some(DeferredCallbackReplay {
                            continuation,
                            record: None,
                            outcome: DeferredEffectOutcome::Replay,
                        });
                        keyboard.transaction_authority =
                            Some(TransactionAuthority::AwaitingDeferredReplay);
                        let submitted = if target_valid_at_callback {
                            keyboard
                                .candidate_target
                                .zip(keyboard.candidate_desktop)
                                .is_some_and(|(target, desktop)| {
                                    context.replay_sender.as_ref().is_some_and(|sender| {
                                        sender
                                            .try_send(ReplayWork::Replay {
                                                batch,
                                                target,
                                                desktop,
                                            })
                                            .is_ok()
                                    })
                                })
                        } else {
                            context.replay_accepted.store(1, Ordering::Release);
                            post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            )
                        };
                        if !submitted {
                            // No worker can publish this replay. Resolve the
                            // retained continuation as suppressed; the timer
                            // polls this result even if the wake post fails.
                            context.replay_accepted.store(1, Ordering::Release);
                            let _ = post_owner_message_once(
                                unsafe { GetCurrentThreadId() },
                                WM_OWNER_REPLAY,
                            );
                            context
                                .terminal
                                .trigger(TerminalReason::OwnerThreadUnresponsive);
                        }
                        return None;
                    }
                    EffectRequest::Replay(batch) => {
                        if validate_candidate_target(keyboard) {
                            EffectOutcome::ReplayAccepted {
                                accepted: injection::inject_replay(
                                    context.injection_markers,
                                    batch,
                                ),
                            }
                        } else {
                            EffectOutcome::ReplaySuppressedTargetChanged
                        }
                    }
                    EffectRequest::CleanupInjected(batch) => EffectOutcome::CleanupAccepted {
                        accepted: injection::inject_cleanup(context.injection_markers, batch),
                    },
                };
                keyboard.transaction_authority = Some(TransactionAuthority::Resume {
                    continuation: continuation.clone(),
                    outcome,
                });
                #[cfg(test)]
                run_reentrant_helper_callback_before_effect_panic(context);
                #[cfg(test)]
                panic_at_transaction_seam(TestTransactionPanicPoint::AfterEffect);
                turn = continuation.resume(outcome);
            }
        }
    }
    panic!("transaction core exceeded MAX_EFFECTS_PER_TURN");
}

fn recover_transaction_authority(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> bool {
    let Some(authority) = keyboard.transaction_authority.clone() else {
        return true;
    };
    let turn = match authority {
        TransactionAuthority::Turn(turn) => turn,
        // The worker owns the only continuation while replay is in flight.
        // A racing callback must pass through without mutating the transactional
        // engine; the replay completion message resumes that exact turn.
        TransactionAuthority::AwaitingDeferredReplay => return false,
        TransactionAuthority::Resume {
            continuation,
            outcome,
        } => continuation.resume(outcome),
    };
    let completion = drive_transaction_turn(context, keyboard, turn, None)
        .expect("owner-side transaction effects cannot defer");
    let terminal = match completion {
        Completion::Event(outcome) => outcome.terminal,
        Completion::Control(outcome) => outcome.shutdown == ShutdownState::Terminal,
    };
    if terminal && !context.terminal.is_triggered() {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    true
}

fn begin_transaction_control(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    control: Control,
) -> Option<talking_quill_keyboard_core::transactional::ControlOutcome> {
    if !recover_transaction_authority(context, keyboard) {
        return None;
    }
    let turn = keyboard
        .transactional
        .clone()
        .begin(EngineInput::Control(control));
    let completion = drive_transaction_turn(context, keyboard, turn, None)
        .expect("control effects cannot defer outside a callback event");
    let Completion::Control(outcome) = completion else {
        unreachable!("a control turn completes as control")
    };
    Some(outcome)
}

fn process_deferred_callback_replay(context: &CallbackContext) {
    // A posted message is only a wake hint. Ignore premature, duplicate, or
    // stale messages until the sole replay worker publishes a nonzero result.
    let encoded_accepted = context.replay_accepted.swap(0, Ordering::AcqRel);
    if encoded_accepted == 0 {
        return;
    }
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        context
            .replay_accepted
            .store(encoded_accepted, Ordering::Release);
        return;
    };
    let Some(pending) = keyboard.deferred_callback_replay.take() else {
        return;
    };
    let Some(record) = pending.record else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return;
    };
    let outcome = match pending.outcome {
        DeferredEffectOutcome::Replay if encoded_accepted >= 2 => EffectOutcome::ReplayAccepted {
            accepted: (encoded_accepted - 2) as usize,
        },
        DeferredEffectOutcome::Replay => EffectOutcome::ReplaySuppressedTargetChanged,
        DeferredEffectOutcome::MenuNeutralization => EffectOutcome::Neutralized {
            accepted: if encoded_accepted >= 2 {
                (encoded_accepted - 2) as usize
            } else {
                0
            },
        },
    };
    let mut menu_releases = [NativeKey::default(); 4];
    let mut menu_release_count = 0_usize;
    if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization) {
        for slot in 0..keyboard.deferred_menu_releases.len() {
            let release = keyboard.deferred_menu_releases[slot].take();
            if menu_modifier_release_still_needed(&keyboard, slot)
                && let Some(native) = release
            {
                menu_releases[menu_release_count] = native;
                menu_release_count += 1;
            }
        }
    }
    keyboard.transaction_authority = Some(TransactionAuthority::Resume {
        continuation: pending.continuation.clone(),
        outcome,
    });
    let turn = pending.continuation.resume(outcome);
    let Some(completion) = drive_transaction_turn(context, &mut keyboard, turn, None) else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return;
    };
    let _ = complete_transaction_event(context, &mut keyboard, completion, record);
    // The retained physical Alt/Win release must reach the foreground only
    // after activation delivery has committed. Reposting it first can change
    // the foreground menu/focus identity and make the immediately following
    // candidate-target check reject an otherwise valid physical shortcut.
    if injection::inject_modifier_releases(
        context.injection_markers,
        &menu_releases[..menu_release_count],
    ) != menu_release_count
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    if std::mem::take(&mut keyboard.deferred_replay_raced) {
        let snapshot = PhysicalSnapshot::new(
            keyboard.physical.held_letter_bits(),
            keyboard.modifiers.transactional_sides(),
            keyboard.altgr_active,
        );
        let control = if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization) {
            Control::ReconcileObserved(snapshot)
        } else {
            Control::Reconcile(snapshot)
        };
        let _ = begin_transaction_control(context, &mut keyboard, control);
    }
    publish_pending_native_work(context);
}

fn process_owner_commands(context: &CallbackContext, receiver: &Receiver<OwnerCommand>) {
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
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
            OwnerMutationKind::Configure => match lock_keyboard_recovering(context) {
                Some(mut keyboard) => {
                    // Keep the session reducer's historical revision fence for
                    // Escape/Enter compatibility tests; global activation is
                    // replaced atomically in the transactional core.
                    keyboard.reducer.fence_activation_revision();
                    keyboard.activation_fenced_letters = keyboard.physical.held_letter_bits();
                    keyboard.modifiers_fenced =
                        keyboard.modifiers.mask() != ModifierMask::default();
                    keyboard.activation = command.mutation.activation;
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
                        begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(compiled),
                        )
                        .is_some_and(|outcome| outcome.applied)
                    })
                }
                None => false,
            },
            OwnerMutationKind::CloseAdmission => {
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    begin_transaction_control(
                        context,
                        &mut keyboard,
                        Control::CloseAdmission(CancelReason::HelperDisconnected),
                    )
                    .is_some_and(|outcome| outcome.applied)
                })
            }
            OwnerMutationKind::CancelCandidate => {
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    begin_transaction_control(context, &mut keyboard, Control::Shutdown)
                        .is_some_and(|outcome| outcome.applied)
                })
            }
            OwnerMutationKind::SetSessionCapture => {
                context.state.session_capture_mode.store(
                    command.mutation.session_capture_mode.as_u8(),
                    Ordering::Release,
                );
                true
            }
            #[cfg(test)]
            OwnerMutationKind::InjectTestUnmatchedDown => {
                // Package-excluded seam: model one already-captured Escape down
                // without installing a global hook callback or calling SendInput.
                // Shutdown must wait for its physical up until the platform
                // deadline, then retire terminal-incomplete without fabricating it.
                lock_keyboard_recovering(context).is_some_and(|mut keyboard| {
                    keyboard.session_escape_native_owned = true;
                    true
                })
            }
        };

        publish_pending_native_work(context);
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

fn process_paste_commands(
    context: &CallbackContext,
    receiver: &Receiver<PasteCommand>,
    pending: &mut Option<PendingPaste>,
) {
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
    while let Ok(command) = receiver.try_recv() {
        if pending.is_some()
            || command
                .state
                .compare_exchange(
                    PasteCommandState::Pending as u8,
                    PasteCommandState::Waiting as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        let evidence = if let Some(mut keyboard) = lock_keyboard_recovering(context) {
            let evidence = keyboard.dispatcher.targets.take(command.context);
            if evidence.is_none() {
                context.observability.record_target_validation_fallback();
            }
            evidence
        } else {
            None
        };
        let Some(evidence) = evidence else {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(());
            continue;
        };
        // SAFETY: a null HWND creates a thread timer delivered to this owner's
        // message queue. It is killed on every completion/cancellation path.
        let timer_id = unsafe { SetTimer(null_mut(), 0, PASTE_TIMER_INTERVAL_MS, None) };
        if timer_id == 0 {
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        *pending = Some(PendingPaste {
            state: command.state,
            result: command.result,
            expected_clipboard_sha256: command.expected_clipboard_sha256,
            acknowledgement: command.acknowledgement,
            evidence,
            deadline: command.injection_deadline,
            timer_id,
            modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
        });
        #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
        pause_after_paste_admission(pending.as_ref().expect("installed paste").deadline);
    }
}

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
fn pause_after_paste_admission(deadline: Instant) {
    let (Some(arm), Some(admitted), Some(release)) = (
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_ARM"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_ADMITTED"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_PASTE_RELEASE"),
    ) else {
        return;
    };
    let arm = std::path::PathBuf::from(arm);
    if !arm.try_exists().unwrap_or(false) {
        return;
    }
    let admitted = std::path::PathBuf::from(admitted);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::remove_file(arm);
    let _ = std::fs::write(&admitted, b"paste admitted\n");
    while Instant::now() < deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

#[cfg(all(feature = "windows-native-test-input", debug_assertions))]
fn pause_after_valid_clipboard_sample() {
    let (Some(arm), Some(sampled), Some(release)) = (
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_ARM"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_SAMPLED"),
        std::env::var_os("TALKING_QUILL_WINDOWS_TEST_HASH_RELEASE"),
    ) else {
        return;
    };
    let arm = std::path::PathBuf::from(arm);
    if !arm.try_exists().unwrap_or(false) {
        return;
    }
    let sampled = std::path::PathBuf::from(sampled);
    let release = std::path::PathBuf::from(release);
    let _ = std::fs::remove_file(arm);
    let _ = std::fs::write(&sampled, b"valid clipboard hash sampled\n");
    let seam_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < seam_deadline && !release.try_exists().unwrap_or(false) {
        thread::sleep(Duration::from_millis(1));
    }
    let _ = std::fs::remove_file(release);
}

fn native_paste_modifiers_neutral() -> bool {
    ModifierTracker::from_state(key_is_down).is_neutral() && !key_is_down(VK_V)
}

const fn paste_deadline_reason(modifiers_neutral: bool) -> PasteFailure {
    if modifiers_neutral {
        PasteFailure::Unavailable
    } else {
        PasteFailure::ConflictingModifiers
    }
}

fn paste_deadline_failure(context: &CallbackContext, command: &mut PendingPaste) -> PasteResult {
    if native_paste_modifiers_neutral() {
        command.modifier_wait.finish();
        failed_paste(paste_deadline_reason(true))
    } else {
        command.modifier_wait.start();
        context.observability.record_modifier_timeout();
        failed_paste(paste_deadline_reason(false))
    }
}

fn poll_pending_paste(context: &CallbackContext, pending: &mut Option<PendingPaste>) {
    let Some(command) = pending.as_mut() else {
        return;
    };
    if context.keyboard.lock().map_or(true, |keyboard| {
        matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        )
    }) {
        return;
    }
    let timed_out = Instant::now() >= command.deadline;
    let modifiers_neutral = native_paste_modifiers_neutral();
    let unavailable = paste_command_state(&command.state) == PasteCommandState::Cancelled
        || context.state.stopping.load(Ordering::Acquire)
        || context.terminal.is_triggered()
        || !context.gate.is_open();
    if unavailable || timed_out {
        let result = if timed_out {
            paste_deadline_failure(context, command)
        } else {
            failed_paste(PasteFailure::Unavailable)
        };
        finish_pending_paste(pending, result);
        return;
    }
    if !revalidate_target(command.evidence) {
        context.observability.record_target_validation_fallback();
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    }
    if !modifiers_neutral {
        command.modifier_wait.start();
        return;
    }
    command.modifier_wait.finish();

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    // A previous accepted-prefix cleanup is retired while this command remains
    // Waiting and cancellable. No new injection authority exists yet.
    retry_pending_paste_cleanup(&mut keyboard);
    if !keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    drop(keyboard);

    // Clipboard Open/Get, canonical UTF-16 hashing, and stable sequence
    // sampling all remain on Waiting authority and share the immutable
    // submission-time deadline.
    let Some(clipboard_sequence) =
        clipboard::matching_text_sequence(command.expected_clipboard_sha256, command.deadline)
    else {
        let result = if Instant::now() >= command.deadline {
            paste_deadline_failure(context, command)
        } else {
            failed_paste(PasteFailure::Unavailable)
        };
        finish_pending_paste(pending, result);
        return;
    };
    if Instant::now() >= command.deadline {
        let result = paste_deadline_failure(context, command);
        finish_pending_paste(pending, result);
        return;
    }

    #[cfg(all(feature = "windows-native-test-input", debug_assertions))]
    pause_after_valid_clipboard_sample();

    if Instant::now() >= command.deadline {
        let result = paste_deadline_failure(context, command);
        finish_pending_paste(pending, result);
        return;
    }
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    if !keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    let target_valid = revalidate_target(command.evidence);
    if !target_valid {
        context.observability.record_target_validation_fallback();
    }
    let final_safe = paste_command_state(&command.state) == PasteCommandState::Waiting
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.terminal.is_triggered()
        && context.gate.is_open()
        && target_valid
        && ModifierTracker::from_state(key_is_down).is_neutral()
        && !key_is_down(VK_V);
    if !final_safe {
        drop(keyboard);
        finish_pending_paste(pending, failed_paste(PasteFailure::Unavailable));
        return;
    }

    let Some(outcome) = injection::inject_paste_initial_if(context.injection_markers, || {
        // These scalar checks and the single CAS are the complete final
        // closure. If cancellation wins Waiting->Cancelled, or the immutable
        // deadline/sequence changed, SendInput is never called. After the CAS
        // wins there is no blocking work before SendInput.
        Instant::now() < command.deadline
            && clipboard::sequence_is_current(clipboard_sequence)
            && Instant::now() < command.deadline
            && claim_paste_injection(&command.state)
    }) else {
        drop(keyboard);
        let result = if Instant::now() >= command.deadline {
            paste_deadline_failure(context, command)
        } else {
            failed_paste(PasteFailure::Unavailable)
        };
        finish_pending_paste(pending, result);
        return;
    };
    let result = publish_initial_paste_acceptance(
        &command.state,
        &command.result,
        &mut keyboard,
        outcome,
        || {},
    );
    // Wake a caller whose timeout lost the final CAS. The slot was published
    // first, so it consumes the authoritative SendInput result without waiting
    // for cleanup or receiving a contradictory ordinary failure.
    let _ = command.acknowledgement.try_send(());
    retry_pending_paste_cleanup(&mut keyboard);
    let degraded = outcome.initial_accepted != 4 || !keyboard.pending_paste_cleanup.is_empty();
    drop(keyboard);
    if degraded {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    finish_pending_paste(pending, result);
    if context
        .state
        .post_claim_timeout
        .swap(false, Ordering::AcqRel)
    {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
}

fn publish_initial_paste_acceptance(
    state: &AtomicU8,
    result_slot: &PasteResultSlot,
    keyboard: &mut CallbackKeyboard,
    outcome: injection::PasteInjectionOutcome,
    after_commit: impl FnOnce(),
) -> PasteResult {
    keyboard.pending_paste_cleanup = outcome.pending_cleanup;
    // These atomics are the first operations after retaining the exact cleanup
    // obligation. No cleanup SendInput or blocking work can precede them.
    let result = result_slot.publish(outcome.result);
    state.store(
        if result.submitted {
            PasteCommandState::Committed as u8
        } else {
            PasteCommandState::ResultReady as u8
        },
        Ordering::Release,
    );
    after_commit();
    result
}

fn finish_pending_paste(pending: &mut Option<PendingPaste>, result: PasteResult) {
    let Some(command) = pending.take() else {
        return;
    };
    // SAFETY: this timer ID was returned by SetTimer for the current thread.
    unsafe { KillTimer(null_mut(), command.timer_id) };
    let _ = command.result.publish(result);
    command
        .state
        .store(PasteCommandState::Applied as u8, Ordering::Release);
    let _ = command.acknowledgement.try_send(());
}

fn desktop_identity(
    desktop: windows_sys::Win32::System::StationsAndDesktops::HDESK,
) -> Option<DesktopIdentity> {
    if desktop.is_null() {
        return None;
    }
    let mut identity = DesktopIdentity {
        name: [0; 64],
        len: 0,
    };
    let mut needed = 0_u32;
    let bytes = u32::try_from(identity.name.len() * size_of::<u16>()).ok()?;
    // SAFETY: desktop is a retained handle and the bounded UTF-16 output is
    // owner-local writable storage.
    let read = unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            identity.name.as_mut_ptr().cast(),
            bytes,
            &raw mut needed,
        ) != 0
    };
    if !read || needed < 2 || needed > bytes {
        return None;
    }
    identity.len = u8::try_from(needed / 2 - 1).ok()?;
    Some(identity)
}

fn current_input_desktop() -> Option<DesktopIdentity> {
    // SAFETY: opens a non-inheritable read-only handle to the current input
    // desktop. The handle is closed after extracting its stable object name.
    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return None;
        }
        let identity = desktop_identity(desktop);
        CloseDesktop(desktop);
        identity
    }
}

fn reconcile_native_state(context: &CallbackContext) {
    let input_desktop = current_input_desktop();
    let physical = physical_tracker_from_state(native_physical_key_is_down);
    let modifiers = ModifierTracker::from_state(key_is_down);
    let logical_v_down = key_is_down(VK_V);
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    if keyboard.input_desktop != input_desktop {
        keyboard.input_desktop = input_desktop;
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::CloseAdmission(CancelReason::SecureDesktop),
        );
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return;
    }
    // Native recovery cannot distinguish the synthetic Ctrl emitted by AltGr
    // from an intentionally held Ctrl. Fail closed while both left Ctrl and
    // right Alt are down, even when foreground layout probing is unavailable.
    keyboard.altgr_synthetic_ctrl = modifiers.ctrl.left && modifiers.alt.right;
    let altgr_active = conservative_altgr(&modifiers, keyboard.altgr_synthetic_ctrl);
    let snapshot = PhysicalSnapshot::new(
        physical.held_letter_bits(),
        modifiers.transactional_sides(),
        altgr_active,
    );
    if keyboard.external_held_letters != 0
        || keyboard
            .external_reconcile_after
            .is_some_and(|deadline| Instant::now() < deadline)
        || keyboard.transactional.pending_injected_cleanup().is_some()
        || keyboard.transactional.pending_menu_cleanup().is_some()
        || !keyboard.pending_paste_cleanup.is_empty()
    {
        return;
    }
    if keyboard.transactional.physical_letters() == snapshot.held_letters
        && keyboard.transactional.physical_modifiers() == snapshot.modifiers
        && keyboard.transactional.physical_modifiers() == keyboard.modifiers.transactional_sides()
        && keyboard.altgr_active == snapshot.alt_gr_active
        && keyboard.logical_v_down == logical_v_down
    {
        return;
    }
    keyboard.physical = physical;
    keyboard.modifiers = modifiers;
    keyboard.logical_v_down = logical_v_down;
    keyboard.altgr_active = altgr_active;
    let outcome = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    if outcome.is_some_and(|outcome| outcome.shutdown == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

const fn low_level_hook_module() -> windows_sys::Win32::Foundation::HMODULE {
    // WH_KEYBOARD_LL runs in the installing process and this callback is linked
    // into the owner executable rather than an injectable DLL. Passing the EXE
    // image as hMod can produce a non-null hook that never receives global
    // callbacks; NULL is the documented in-process low-level-hook identity.
    null_mut()
}

fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<u32, PlatformError>>,
    startup_state: Arc<AtomicU8>,
    owner_commands: Receiver<OwnerCommand>,
    paste_commands: Receiver<PasteCommand>,
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

    // Bind this newly created, window-free owner thread to the interactive
    // input desktop before creating its queue or installing any hook. This is
    // explicit rather than relying on desktop inheritance across the
    // service-mediated CreateProcessAsUser launch.
    let thread_id = unsafe { GetCurrentThreadId() };
    let (replay_sender, replay_receiver) = bounded(1);
    context.replay_sender = Some(replay_sender);
    let replay_accepted = Arc::clone(&context.replay_accepted);
    let replay_markers = context.injection_markers;
    let replay_worker = match thread::Builder::new()
        .name("talking-quill-win-replay".into())
        .spawn(move || {
            while let Ok(work) = replay_receiver.recv() {
                let accepted = match work {
                    ReplayWork::Replay {
                        batch,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::inject_replay(replay_markers, batch)),
                    ReplayWork::NeutralizeMenu {
                        modifiers,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::neutralize_menu(replay_markers, modifiers)),
                };
                replay_accepted.store(
                    accepted.map_or(1, |accepted| {
                        u64::try_from(accepted)
                            .unwrap_or(u64::MAX)
                            .saturating_add(2)
                    }),
                    Ordering::Release,
                );
                // Completion is durable in replay_accepted. The owner timer
                // polls it, so a failed wake cannot strand replay authority.
                let _ = post_owner_message_once(thread_id, WM_OWNER_REPLAY);
            }
        }) {
        Ok(worker) => worker,
        Err(_) => {
            CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
    };
    let previous_desktop = unsafe { GetThreadDesktop(thread_id) };
    let input_desktop = unsafe {
        OpenInputDesktop(
            0,
            0,
            DESKTOP_READOBJECTS | DESKTOP_HOOKCONTROL | DESKTOP_SWITCHDESKTOP,
        )
    };
    if previous_desktop.is_null()
        || input_desktop.is_null()
        || unsafe { SetThreadDesktop(input_desktop) } == 0
    {
        if !input_desktop.is_null() {
            unsafe { CloseDesktop(input_desktop) };
        }
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }
    let _thread_desktop = HookThreadDesktop {
        input: input_desktop,
        previous: previous_desktop,
    };

    let mut message = MSG::default();
    // SAFETY: this no-remove peek creates the owner queue before hook
    // installation, so low-level callbacks always have a live message loop.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: the callback has the system ABI and remains live until same-thread
    // unhooking. WH_KEYBOARD_LL executes in this owner process, so no injectable
    // callback DLL/module handle is involved.
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook),
            low_level_hook_module(),
            0,
        )
    };
    if hook.is_null() {
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        let category = match error {
            windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED => "access_denied",
            _ => "native_unavailable",
        };
        eprintln!("keyboard-owner hook install unavailable: {category}");
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
        let _ = ready.send(Err(PlatformError::HookUnavailable));
        return;
    }
    context.observability.record_hook_installed();
    // Focus/foreground/caret epochs make A->B->A transitions observable even
    // when the point-in-time HWND tuple returns to its original values.
    let win_events = unsafe {
        [
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_FOCUS,
                EVENT_OBJECT_FOCUS,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
        ]
    };
    let target_change_evidence_ready = win_events.iter().all(|hook| !hook.is_null());
    context
        .state
        .target_change_evidence_ready
        .store(target_change_evidence_ready, Ordering::Release);
    // These hooks are optional for activation. Paste checks the readiness bit
    // above and fails closed unless the complete target-change evidence set is
    // present; partial hooks only conservatively invalidate epochs.
    let mut installation = OwnerHookInstallation {
        hook,
        win_events,
        timer: 0,
    };
    let owner_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Low-level callbacks are delivered on this message-loop thread. Seed all
        // tracked physical state after installation and before readiness so a key
        // already held cannot begin an activation or session sequence.
        let physical = physical_tracker_from_state(native_physical_key_is_down);
        let modifiers = ModifierTracker::from_state(key_is_down);
        let keyboard = match context.keyboard.get_mut() {
            Ok(keyboard) => keyboard,
            Err(poisoned) => {
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                poisoned.into_inner()
            }
        };
        keyboard.physical = physical;
        keyboard.modifiers = modifiers;
        // Bind candidate admission to the exact desktop handle assigned to
        // this windowless hook thread. Reopening the input desktop here can
        // race a switch and previously made a valid owner start targetless.
        keyboard.input_desktop = desktop_identity(input_desktop);
        keyboard.logical_v_down = key_is_down(VK_V);
        keyboard.altgr_active =
            conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
        keyboard.modifiers_fenced = keyboard.modifiers.mask() != ModifierMask::default();
        keyboard.transactional = TransactionEngine::with_physical_snapshot(
            CompiledActivationConfig::default(),
            PhysicalSnapshot::new(
                keyboard.physical.held_letter_bits(),
                keyboard.modifiers.transactional_sides(),
                keyboard.altgr_active,
            ),
        );

        if !claim_startup(&startup_state) {
            context.gate.close();
            return;
        }
        // SAFETY: null HWND creates a periodic current-thread recovery timer.
        let snapshot_timer = unsafe { SetTimer(null_mut(), 0, NATIVE_SNAPSHOT_INTERVAL_MS, None) };
        if snapshot_timer == 0 {
            context.gate.close();
            let _ = ready.send(Err(PlatformError::HookUnavailable));
            return;
        }
        installation.timer = snapshot_timer;
        let mut startup_ready = Some(ready);
        let mut pending_paste = None;
        loop {
            // SAFETY: null HWND selects this hook owner's complete thread queue.
            let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            if message.message == WM_OWNER_COMMAND {
                process_owner_commands(&context, &owner_commands);
            } else if message.message == WM_OWNER_PASTE {
                process_paste_commands(&context, &paste_commands, &mut pending_paste);
                poll_pending_paste(&context, &mut pending_paste);
            } else if message.message == WM_OWNER_REPLAY {
                process_deferred_callback_replay(&context);
            } else if message.message == WM_TIMER {
                if message.wParam == snapshot_timer {
                    process_deferred_callback_replay(&context);
                    process_owner_commands(&context, &owner_commands);
                    process_paste_commands(&context, &paste_commands, &mut pending_paste);
                    reconcile_native_state(&context);
                    if let Some(ready) = startup_ready.take() {
                        context.observability.record_pump_alive();
                        context.state.hook_status.store(
                            hook_status_to_u8(HookStatus::InstalledUnobserved),
                            Ordering::Release,
                        );
                        if ready.send(Ok(thread_id)).is_err()
                            || context.state.stopping.load(Ordering::Acquire)
                        {
                            context.gate.close();
                            break;
                        }
                    }
                }
                poll_pending_paste(&context, &mut pending_paste);
            } else {
                // SAFETY: `message` was initialized by GetMessageW.
                unsafe {
                    TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
            }
        }

        if let Some(ready) = startup_ready.take() {
            let _ = ready.send(Err(PlatformError::HookUnavailable));
        }
        context.gate.close();
        if pending_paste.is_some() {
            finish_pending_paste(&mut pending_paste, failed_paste(PasteFailure::Unavailable));
        }
        let shutdown_deadline = context
            .state
            .shutdown_deadline
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |installed| *installed)
            .unwrap_or_else(|| Instant::now() + SHUTDOWN_DRAIN_TIMEOUT);
        let mut needs_drain = false;
        if let Some(mut keyboard) = lock_keyboard_recovering(&context) {
            let authority_ready = recover_transaction_authority(&context, &mut keyboard);
            if authority_ready {
                if keyboard.transactional.pending_menu_cleanup().is_some()
                    || keyboard.transactional.pending_injected_cleanup().is_some()
                {
                    let _ =
                        begin_transaction_control(&context, &mut keyboard, Control::RetryCleanup);
                }
                retry_pending_paste_cleanup(&mut keyboard);
                let _ = begin_transaction_control(&context, &mut keyboard, Control::Shutdown);
                keyboard.shutdown_requested = true;
            }
            needs_drain = !transaction_obligations_drained(&keyboard);
        }
        if needs_drain {
            match drain_owned_transaction(&context, &mut message, shutdown_deadline) {
                ShutdownDrainOutcome::Drained => {}
                ShutdownDrainOutcome::TerminalIncomplete => {
                    context.observability.record_shutdown_ownership_deadline();
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context
                        .terminal
                        .trigger(TerminalReason::InputInjectionUnavailable);
                }
            }
        }
        while let Ok(command) = paste_commands.try_recv() {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
        }
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
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
    }));
    if owner_result.is_err() {
        context.gate.close();
        context.state.stopping.store(true, Ordering::Release);
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
    drop(installation);
    drop(context.replay_sender.take());
    let replay_deadline = Instant::now() + Duration::from_millis(500);
    while !replay_worker.is_finished() && Instant::now() < replay_deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if replay_worker.is_finished() {
        let _ = replay_worker.join();
    } else {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
    }
}

fn retry_pending_paste_cleanup(keyboard: &mut CallbackKeyboard) {
    if keyboard.pending_paste_cleanup.is_empty() {
        return;
    }
    let physical_ctrl_down = keyboard.modifiers.ctrl.is_down();
    let physical_v_down = keyboard.logical_v_down;
    let (_, remaining) = injection::retry_paste_cleanup(
        keyboard.pending_paste_cleanup,
        physical_ctrl_down,
        physical_v_down,
    );
    keyboard.pending_paste_cleanup = remaining;
}

fn transaction_obligations_drained(keyboard: &CallbackKeyboard) -> bool {
    keyboard.transactional.journal_len() == 0
        && keyboard.transactional.owned_letters() == 0
        && keyboard.transactional.pending_injected_cleanup().is_none()
        && keyboard.transactional.pending_menu_cleanup().is_none()
        && keyboard.pending_paste_cleanup.is_empty()
        && keyboard.deferred_callback_replay.is_none()
        && keyboard.transaction_authority.is_none()
        && !keyboard.session_escape_native_owned
        && keyboard.captured_enter_source.is_none()
}

fn publish_pending_native_work(context: &CallbackContext) {
    let pending = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| !transaction_obligations_drained(&keyboard));
    context
        .state
        .pending_native_work
        .store(pending, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownDrainOutcome {
    Drained,
    TerminalIncomplete,
}

fn drain_owned_transaction(
    context: &CallbackContext,
    message: &mut MSG,
    deadline: Instant,
) -> ShutdownDrainOutcome {
    // Retained replay/menu/paste cleanup suffixes continue through their
    // accepted-count authorities. Native downs are never exposed at shutdown:
    // an owned physical up must be observed before ownership can retire.
    let mut retry = 0_usize;
    loop {
        // Poll the durable worker result independently of its best-effort wake.
        // This also resolves replay if shutdown began before the normal timer.
        process_deferred_callback_replay(context);
        if let Some(mut keyboard) = lock_keyboard_recovering(context) {
            let authority_ready = recover_transaction_authority(context, &mut keyboard);
            if authority_ready {
                if keyboard.transactional.pending_menu_cleanup().is_some()
                    || keyboard.transactional.pending_injected_cleanup().is_some()
                {
                    let _ =
                        begin_transaction_control(context, &mut keyboard, Control::RetryCleanup);
                }
                retry_pending_paste_cleanup(&mut keyboard);
            }
            if transaction_obligations_drained(&keyboard) {
                return ShutdownDrainOutcome::Drained;
            }
            let now = Instant::now();
            if now >= deadline {
                return ShutdownDrainOutcome::TerminalIncomplete;
            }
        } else if Instant::now() >= deadline {
            return ShutdownDrainOutcome::TerminalIncomplete;
        }
        pump_owner_messages(context, message, deadline);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            continue;
        }
        let delay =
            OWNER_WAKE_RETRY_DELAYS[retry.min(OWNER_WAKE_RETRY_DELAYS.len() - 1)].min(remaining);
        retry = retry.saturating_add(1);
        thread::sleep(delay);
    }
}

fn pump_owner_messages(context: &CallbackContext, message: &mut MSG, deadline: Instant) {
    // SAFETY: message is owner-local writable storage. Bound each pass so a
    // producer that continuously posts messages cannot extend shutdown.
    let mut remaining = 64_usize;
    while remaining != 0
        && Instant::now() < deadline
        && unsafe { PeekMessageW(message, null_mut(), 0, 0, PM_REMOVE) } != 0
    {
        remaining -= 1;
        if message.message == WM_QUIT {
            continue;
        }
        if message.message == WM_OWNER_REPLAY {
            process_deferred_callback_replay(context);
            continue;
        }
        // SAFETY: PeekMessageW initialized this non-quit message.
        unsafe {
            TranslateMessage(message);
            DispatchMessageW(message);
        }
    }
}

fn handle_callback_panic(context: &CallbackContext) {
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    // TerminalSignal closes callback admission synchronously. The poisoned
    // keyboard contents and transaction authority remain intact for owner-side
    // recovery and exact-up drain. No terminal path exposes an owned down.
    context.terminal.trigger(TerminalReason::CallbackPanicked);
}

const fn callback_may_process_source(suppression_enabled: bool, source: InputSource) -> bool {
    suppression_enabled || !source.is_physical()
}

unsafe extern "system" fn keyboard_hook(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    // Callback-stack-local authority: nested helper SendInput callbacks cannot
    // overwrite the outer physical edge's panic disposition.
    let callback_disposition = Cell::new(CallbackDisposition::Pass);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
        context.observability.record_hc_action_callback();
        // SAFETY: HC_ACTION defines l_param as a valid KBDLLHOOKSTRUCT pointer
        // for this callback's duration.
        let native = unsafe { &*(l_param as *const KBDLLHOOKSTRUCT) };
        let phase = match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => KeyPhase::Down,
            WM_KEYUP | WM_SYSKEYUP => KeyPhase::Up,
            _ => return None,
        };
        let source =
            injection::classify(context.injection_markers, native.flags, native.dwExtraInfo);
        if source.is_physical() {
            context.observability.record_physical_callback();
        }
        let extended = native.flags & LLKHF_EXTENDED != 0;
        // Process-lifetime defense in depth: ordinary/package builds bypass
        // every reducer and notification path for physical input. The original
        // hook record is forwarded unchanged below.
        if !callback_may_process_source(context.suppression_enabled, source) {
            if source.is_physical() {
                context.observability.record_physical_callback_filtered();
            }
            return Some(false);
        }
        if matches!(
            source,
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
        ) {
            // Nested SendInput callbacks occur while the outer transaction owns
            // the keyboard mutex. Return before both reducer processing and the
            // pending-work snapshot below; attempting that snapshot here would
            // self-deadlock the hook and trigger LowLevelHooksTimeout removal.
            return Some(false);
        }
        let captured = process_transactional_hook_record_at(
            context,
            TransactionalHookRecord {
                virtual_key: native.vkCode as u16,
                scan_code: native.scanCode,
                extended,
                platform_flags: native.flags,
                phase,
                source,
            },
            HookObservation {
                observed_at_ms: context.owner_epoch.elapsed().as_millis() as u64,
                // LowLevelKeyboardProc runs before Windows updates asynchronous
                // key state. Callback-time GetAsyncKeyState is diagnostic only;
                // ordered LL edges are the transaction authority for every
                // physical-equivalent source. The owner timer repairs missed
                // native transitions outside the callback.
                native_modifiers: None,
            },
            &callback_disposition,
        );
        publish_pending_native_work(context);
        Some(captured)
    }));

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
                handle_callback_panic(context);
                if callback_disposition.get() == CallbackDisposition::Capture {
                    return 1;
                }
            }
            // Untouched input fails open. A turn that already retained the
            // current edge keeps it suppressed while its authority recovers.
            unsafe { CallNextHookEx(null_mut(), code, w_param, l_param) }
        }
    }
}

fn process_transactional_hook_record_at(
    context: &CallbackContext,
    record: TransactionalHookRecord,
    observation: HookObservation,
    callback_disposition: &Cell<CallbackDisposition>,
) -> bool {
    let TransactionalHookRecord {
        virtual_key,
        scan_code,
        extended,
        platform_flags,
        phase,
        source,
    } = record;
    callback_disposition.set(CallbackDisposition::Pass);
    // Helper-owned traffic is a strict bypass. In particular a SendInput call
    // made while the callback mutex is held can never recursively poison it.
    if matches!(
        source,
        InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy
    ) {
        return false;
    }

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return false;
    };
    if source == InputSource::External {
        keyboard.external_reconcile_after = Some(Instant::now() + Duration::from_millis(500));
    }
    if !recover_transaction_authority(context, &mut keyboard) {
        let identity = map_key_identity(virtual_key, scan_code, extended);
        let menu_neutralization_pending =
            keyboard
                .deferred_callback_replay
                .as_ref()
                .is_some_and(|pending| {
                    matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization)
                });
        let suppress_menu_release = menu_neutralization_pending
            && source != InputSource::External
            && phase == KeyPhase::Up
            && menu_modifier_release_slot(identity).is_some();
        let suppress_owned_letter = menu_neutralization_pending
            && matches!(identity, KeyIdentity::Letter(key)
                if keyboard.transactional.owned_letters() & (1_u32 << key.index()) != 0);
        track_deferred_replay_race(&mut keyboard, virtual_key, scan_code, extended, phase);
        if suppress_menu_release && let Some(slot) = menu_modifier_release_slot(identity) {
            keyboard.deferred_menu_releases[slot] = Some(NativeKey {
                virtual_key,
                scan_code,
                extended,
                platform_flags: u64::from(platform_flags),
            });
        }
        if suppress_menu_release || suppress_owned_letter {
            callback_disposition.set(CallbackDisposition::Capture);
            return true;
        }
        return false;
    }
    let identity = map_key_identity(virtual_key, scan_code, extended);
    if source == InputSource::External
        && let KeyIdentity::Letter(key) = identity
    {
        let bit = 1_u32 << key.index();
        match phase {
            KeyPhase::Down => keyboard.external_held_letters |= bit,
            KeyPhase::Up => keyboard.external_held_letters &= !bit,
        }
    }
    let native = NativeKey {
        virtual_key,
        scan_code,
        extended,
        // Shared replay storage is lossless u64; widening preserves the exact
        // Windows u32 LL-hook flags without changing injection behavior.
        platform_flags: u64::from(platform_flags),
    };

    // GetAsyncKeyState can lag a serialized external SendInput edge. The callback
    // edge is authoritative during that bounded publication window; periodic
    // reconciliation runs after the deadline.
    if source != InputSource::External
        && !matches!(identity, KeyIdentity::Modifier(_))
        && let Some(native_modifiers) = observation.native_modifiers
        && native_modifiers.transactional_sides() != keyboard.modifiers.transactional_sides()
    {
        keyboard.modifiers = native_modifiers;
        keyboard.altgr_active =
            conservative_altgr(&native_modifiers, keyboard.altgr_synthetic_ctrl);
        let snapshot = PhysicalSnapshot::new(
            keyboard.physical.held_letter_bits(),
            keyboard.modifiers.transactional_sides(),
            keyboard.altgr_active,
        );
        let outcome =
            begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
        if outcome.is_some_and(|outcome| outcome.shutdown == ShutdownState::Terminal)
            && !context.terminal.is_triggered()
        {
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
        }
    }

    let repeat = match identity {
        KeyIdentity::Modifier(side) => {
            let repeat =
                phase == KeyPhase::Down && keyboard.modifiers.transactional_sides().contains(side);
            keyboard
                .modifiers
                .observe(virtual_key, scan_code, extended, phase);
            let modifiers = keyboard.modifiers.mask();
            keyboard.reducer.observe_modifiers(modifiers);
            repeat
        }
        KeyIdentity::Letter(key) => {
            keyboard
                .physical
                .observe(PhysicalKey::Letter(key), None, phase)
        }
        KeyIdentity::Escape => keyboard.physical.observe(PhysicalKey::Escape, None, phase),
        KeyIdentity::Enter => {
            keyboard
                .physical
                .observe(PhysicalKey::Enter, enter_source(scan_code, extended), phase)
        }
        KeyIdentity::Other(_) => false,
    };
    if virtual_key == VK_V {
        keyboard.logical_v_down = phase == KeyPhase::Down;
    }
    let left_control = matches!(identity, KeyIdentity::Modifier(ModifierSide::LeftCtrl));
    let right_alt = matches!(identity, KeyIdentity::Modifier(ModifierSide::RightAlt));
    if left_control && phase == KeyPhase::Up {
        keyboard.altgr_synthetic_ctrl = false;
    }
    if right_alt {
        keyboard.altgr_synthetic_ctrl = phase == KeyPhase::Down && keyboard.modifiers.ctrl.left;
        keyboard.altgr_active = phase == KeyPhase::Down
            && conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
    } else if let Some(native_modifiers) = observation.native_modifiers {
        keyboard.altgr_active =
            conservative_altgr(&native_modifiers, keyboard.altgr_synthetic_ctrl);
    }
    let snapshot = PhysicalSnapshot::new(
        keyboard.physical.held_letter_bits(),
        keyboard.modifiers.transactional_sides(),
        keyboard.altgr_active,
    );
    let physical_phase = match (phase, repeat) {
        (KeyPhase::Down, true) => PhysicalPhase::Repeat,
        (KeyPhase::Down, false) => PhysicalPhase::Down,
        (KeyPhase::Up, _) => PhysicalPhase::Up,
    };

    // Disabled Windows configurations retain a faithful non-owning shadow of
    // the compiled production matcher. A proof is emitted only after an exact
    // match and matching trigger release in one opaque observation generation.
    if !keyboard.activation.enabled {
        let observation_config = keyboard.transactional.config();
        if let Some(generation) = keyboard.registered_observation.observe(
            &context.observability,
            observation_config,
            snapshot,
            identity,
            physical_phase,
        ) {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::PhysicalObserved),
                Ordering::Release,
            );
            if context
                .outbound
                .try_send(NativeEvent::RegisteredObservation { generation })
                .is_ok()
            {
                context.observability.record_callback_channel_accepted();
            } else {
                context.observability.record_callback_channel_rejected();
            }
        }
    } else {
        keyboard.registered_observation.reset();
    }

    if keyboard.transactional.journal_len() != 0 && !validate_candidate_target(&mut keyboard) {
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::Cancel(CancelReason::TargetChanged),
        );
    }

    let starts_candidate = keyboard
        .transactional
        .event_starts_candidate(identity, physical_phase);
    if starts_candidate {
        context.observability.record_registered_candidate_callback();
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::PhysicalObserved),
            Ordering::Release,
        );
    }
    let candidate_preflight = starts_candidate
        .then(|| capture_coherent_candidate_target(keyboard.input_desktop))
        .flatten();
    if let Some((target, desktop, epoch)) = candidate_preflight {
        keyboard.candidate_target = Some(target);
        keyboard.candidate_desktop = Some(desktop);
        keyboard.candidate_target_epoch = Some(epoch);
        keyboard.candidate_target_changed = false;
    }
    let event = NormalizedEvent {
        key: identity,
        phase: physical_phase,
        source,
        native,
        observed_at_ms: observation.observed_at_ms,
        config_revision: keyboard.transactional.config().revision(),
        gate: transaction_gate(context),
        snapshot,
    };
    let turn = if starts_candidate && candidate_preflight.is_none() {
        keyboard
            .transactional
            .clone()
            .pass_uncapturable_event(event)
    } else {
        keyboard
            .transactional
            .clone()
            .begin(EngineInput::Event(event))
    };
    let record = CompletedHookRecord {
        identity,
        virtual_key,
        phase,
        repeat,
        enter_source: enter_source(scan_code, extended),
        physical: true,
    };
    let completion =
        drive_transaction_turn(context, &mut keyboard, turn, Some(callback_disposition));
    if let Some(completion) = completion {
        return complete_transaction_event(context, &mut keyboard, completion, record);
    }
    let Some(pending) = keyboard.deferred_callback_replay.as_mut() else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return callback_disposition.get() == CallbackDisposition::Capture;
    };
    pending.record = Some(record);
    if matches!(pending.outcome, DeferredEffectOutcome::MenuNeutralization)
        && phase == KeyPhase::Up
        && let Some(slot) = menu_modifier_release_slot(identity)
    {
        if source == InputSource::External {
            // The external sender owns this injected release. Pass the original
            // edge so balance never depends on injecting into another process.
            callback_disposition.set(CallbackDisposition::Pass);
        } else {
            // A physical release was hidden until activation delivery committed.
            // Repost it after neutralization so the foreground remains balanced.
            keyboard.deferred_menu_releases[slot] = Some(native);
        }
    }
    // The replay worker is the sole completion-message publisher. Posting here
    // could consume an empty result before SendInput completes.
    callback_disposition.get() == CallbackDisposition::Capture
}

fn menu_modifier_release_still_needed(keyboard: &CallbackKeyboard, slot: usize) -> bool {
    match slot {
        0 => !keyboard.modifiers.alt.left,
        1 => !keyboard.modifiers.alt.right,
        2 => !keyboard.modifiers.meta.left,
        3 => !keyboard.modifiers.meta.right,
        _ => false,
    }
}

const fn menu_modifier_release_slot(identity: KeyIdentity) -> Option<usize> {
    match identity {
        KeyIdentity::Modifier(ModifierSide::LeftAlt) => Some(0),
        KeyIdentity::Modifier(ModifierSide::RightAlt) => Some(1),
        KeyIdentity::Modifier(ModifierSide::LeftMeta) => Some(2),
        KeyIdentity::Modifier(ModifierSide::RightMeta) => Some(3),
        _ => None,
    }
}

fn track_deferred_replay_race(
    keyboard: &mut CallbackKeyboard,
    virtual_key: u16,
    scan_code: u32,
    extended: bool,
    phase: KeyPhase,
) {
    keyboard.deferred_replay_raced = true;
    let identity = map_key_identity(virtual_key, scan_code, extended);
    match identity {
        KeyIdentity::Modifier(_) => {
            keyboard
                .modifiers
                .observe(virtual_key, scan_code, extended, phase);
        }
        KeyIdentity::Letter(key) => {
            keyboard
                .physical
                .observe(PhysicalKey::Letter(key), None, phase);
        }
        KeyIdentity::Escape => {
            keyboard.physical.observe(PhysicalKey::Escape, None, phase);
        }
        KeyIdentity::Enter => {
            keyboard
                .physical
                .observe(PhysicalKey::Enter, enter_source(scan_code, extended), phase);
        }
        KeyIdentity::Other(_) => {}
    }
    if virtual_key == VK_V {
        keyboard.logical_v_down = phase == KeyPhase::Down;
    }
    if matches!(identity, KeyIdentity::Modifier(ModifierSide::LeftCtrl)) && phase == KeyPhase::Up {
        keyboard.altgr_synthetic_ctrl = false;
    }
    if matches!(identity, KeyIdentity::Modifier(ModifierSide::RightAlt)) {
        keyboard.altgr_synthetic_ctrl = phase == KeyPhase::Down && keyboard.modifiers.ctrl.left;
        keyboard.altgr_active = phase == KeyPhase::Down
            && conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
    }
}

fn transaction_gate(context: &CallbackContext) -> GateState {
    let initialized = context.state.protocol_initialized.load(Ordering::Acquire);
    if context.terminal.is_triggered()
        || context.state.stopping.load(Ordering::Acquire)
        || (initialized && !context.gate.is_open())
    {
        GateState::Closed
    } else {
        GateState::Open
    }
}

fn complete_transaction_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    completion: Completion,
    record: CompletedHookRecord,
) -> bool {
    let CompletedHookRecord {
        identity,
        virtual_key,
        phase,
        repeat,
        enter_source,
        physical,
    } = record;
    let Completion::Event(outcome) = completion else {
        unreachable!("an event turn completes as event")
    };
    if outcome.terminal
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.terminal.is_triggered()
    {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    if physical && phase == KeyPhase::Up && outcome.disposition == EventDisposition::PassCurrent {
        if virtual_key == VK_V {
            keyboard.pending_paste_cleanup =
                keyboard.pending_paste_cleanup.without_virtual_key(VK_V);
        }
        if matches!(
            identity,
            KeyIdentity::Modifier(ModifierSide::LeftCtrl | ModifierSide::RightCtrl)
        ) && !keyboard.modifiers.ctrl.is_down()
        {
            keyboard.pending_paste_cleanup = keyboard
                .pending_paste_cleanup
                .without_virtual_key(VK_CONTROL);
        }
    }
    if keyboard.shutdown_requested && transaction_obligations_drained(keyboard) {
        // SAFETY: called on the keyboard owner thread after the final owned up.
        unsafe { PostQuitMessage(0) };
    }
    if outcome.disposition == EventDisposition::CaptureCurrent {
        return true;
    }
    match identity {
        KeyIdentity::Escape => {
            process_session_event(context, keyboard, PhysicalKey::Escape, phase, repeat, None)
        }
        KeyIdentity::Enter => process_session_event(
            context,
            keyboard,
            PhysicalKey::Enter,
            phase,
            repeat,
            enter_source,
        ),
        KeyIdentity::Letter(_) | KeyIdentity::Modifier(_) | KeyIdentity::Other(_) => false,
    }
}

fn process_session_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    key: PhysicalKey,
    phase: KeyPhase,
    repeat: bool,
    enter_source: Option<EnterSource>,
) -> bool {
    if key == PhysicalKey::Enter
        && keyboard
            .captured_enter_source
            .is_some_and(|captured| Some(captured) != enter_source)
    {
        return false;
    }
    let accepting = context.gate.is_open();
    if !accepting && !keyboard.reducer.is_capturing(key) {
        return false;
    }
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let plan = keyboard.reducer.plan_bindings_at(
        KeyInput {
            key,
            phase,
            modifiers: keyboard.modifiers.mask(),
            repeat,
            injected: false,
        },
        ActivationBindings::default(),
        false,
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
        0,
    );
    let planned_event = plan.event();
    let delivered = planned_event.is_none()
        || (accepting
            && deliver_callback_event(
                &context.outbound,
                &context.terminal,
                planned_event.expect("event presence checked above"),
            ));
    let swallowed = keyboard.reducer.apply(plan, delivered);
    if delivered && swallowed && phase == KeyPhase::Down && !repeat {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = true,
            PhysicalKey::Enter => keyboard.captured_enter_source = enter_source,
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
    }
    if phase == KeyPhase::Up {
        match key {
            PhysicalKey::Escape if keyboard.session_escape_native_owned => {
                keyboard.session_escape_native_owned = false;
            }
            PhysicalKey::Enter if keyboard.captured_enter_source == enter_source => {
                keyboard.captured_enter_source = None;
            }
            PhysicalKey::Letter(_)
            | PhysicalKey::Enter
            | PhysicalKey::Escape
            | PhysicalKey::Other => {}
        }
    }
    swallowed
}

fn map_key_identity(virtual_key: u16, scan_code: u32, extended: bool) -> KeyIdentity {
    if let Some(side) = modifier_side(virtual_key, scan_code, extended) {
        return KeyIdentity::Modifier(side);
    }
    match map_scan_code(scan_code, extended) {
        PhysicalKey::Letter(key) => KeyIdentity::Letter(key),
        PhysicalKey::Escape => KeyIdentity::Escape,
        PhysicalKey::Enter => KeyIdentity::Enter,
        PhysicalKey::Other if (0x41..=0x5A).contains(&virtual_key) => {
            // SendInput/UIAutomation may provide a virtual-key record without
            // KEYEVENTF_SCANCODE. The low-level hook then has no physical
            // position to normalize, so use the canonical A-Z virtual key.
            KeyIdentity::Letter(
                ActivationKey::from_index((virtual_key - 0x41) as u8)
                    .expect("validated A-Z virtual key has an activation index"),
            )
        }
        PhysicalKey::Other => KeyIdentity::Other(virtual_key),
    }
}

const fn modifier_side(virtual_key: u16, scan_code: u32, extended: bool) -> Option<ModifierSide> {
    match virtual_key {
        VK_LCONTROL => Some(ModifierSide::LeftCtrl),
        VK_RCONTROL => Some(ModifierSide::RightCtrl),
        VK_CONTROL if scan_code == 0x1D && extended => Some(ModifierSide::RightCtrl),
        VK_CONTROL => Some(ModifierSide::LeftCtrl),
        VK_LMENU => Some(ModifierSide::LeftAlt),
        VK_RMENU => Some(ModifierSide::RightAlt),
        VK_MENU if scan_code == 0x38 && extended => Some(ModifierSide::RightAlt),
        VK_MENU => Some(ModifierSide::LeftAlt),
        VK_LSHIFT => Some(ModifierSide::LeftShift),
        VK_RSHIFT => Some(ModifierSide::RightShift),
        VK_SHIFT if scan_code == 0x36 => Some(ModifierSide::RightShift),
        VK_SHIFT => Some(ModifierSide::LeftShift),
        VK_LWIN => Some(ModifierSide::LeftMeta),
        VK_RWIN => Some(ModifierSide::RightMeta),
        _ => None,
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

#[cfg(test)]
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

    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return false;
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
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let plan = keyboard.reducer.plan_bindings_at(
        input,
        activation.bindings,
        accepting
            && activation.enabled
            && keyboard.activation_fenced_letters == 0
            && !keyboard.modifiers_fenced
            && !suppress_activation,
        if accepting {
            capture_mode
        } else {
            SessionCaptureMode::Off
        },
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
    if let Some(KeyboardEvent::SessionKey {
        key: talking_quill_keyboard_core::SessionKey::Enter,
        phase: talking_quill_keyboard_core::EventPhase::Down,
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

fn foreground_keyboard_layout() -> windows_sys::Win32::UI::Input::KeyboardAndMouse::HKL {
    // SAFETY: all calls take scalar values or a null optional process pointer.
    let foreground_thread = unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            0
        } else {
            GetWindowThreadProcessId(window, null_mut())
        }
    };
    // SAFETY: zero requests the current thread layout.
    unsafe { GetKeyboardLayout(foreground_thread) }
}

fn conservative_altgr(modifiers: &ModifierTracker, synthetic_ctrl: bool) -> bool {
    conservative_altgr_for_layout(modifiers, synthetic_ctrl, foreground_layout_uses_altgr())
}

const fn conservative_altgr_for_layout(
    modifiers: &ModifierTracker,
    synthetic_ctrl: bool,
    layout_uses_altgr: bool,
) -> bool {
    modifiers.alt.right && (synthetic_ctrl || layout_uses_altgr)
}

fn foreground_layout_uses_altgr() -> bool {
    const ALTGR_PROBES: [u16; 9] = [
        b'@' as u16,
        b'{' as u16,
        b'[' as u16,
        b']' as u16,
        b'}' as u16,
        b'\\' as u16,
        b'~' as u16,
        b'|' as u16,
        0x20AC, // Euro sign
    ];
    let layout = foreground_keyboard_layout();
    ALTGR_PROBES.iter().copied().any(|character| {
        // SAFETY: character and the current foreground HKL are scalar values.
        let mapped = unsafe { VkKeyScanExW(character, layout) };
        if mapped == -1 {
            return false;
        }
        let modifiers = (mapped as u16 >> 8) as u8;
        modifiers & 0b110 == 0b110
    })
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

#[cfg(all(test, feature = "windows-native-test-input"))]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use talking_quill_keyboard_core::{
        ActivationBinding, ActivationContext, ActivationGeneration, EventPhase, ProfileId,
        SessionKey, Shortcut, ShortcutModifiers,
    };

    fn shared_state_for_test() -> SharedState {
        SharedState::new()
    }

    #[test]
    fn auxiliary_audio_fault_cannot_close_keyboard_capture() {
        let keyboard_gate = Arc::new(CallbackGate::new());
        keyboard_gate.open();
        let (keyboard_sender, _keyboard_events) = bounded(1);
        let keyboard_terminal = TerminalSignal::new(Arc::clone(&keyboard_gate), keyboard_sender);
        let (audio_gate, audio_terminal) = isolated_audio_terminal();

        audio_terminal.trigger(TerminalReason::AudioDeviceMonitorUnavailable);

        assert!(audio_terminal.is_triggered());
        assert!(!audio_gate.is_open());
        assert!(!keyboard_terminal.is_triggered());
        assert!(keyboard_gate.is_open());
    }

    #[test]
    fn paste_target_evidence_starts_closed_until_all_native_hooks_install() {
        let state = SharedState::new();
        assert!(!state.target_change_evidence_ready.load(Ordering::Acquire));
    }

    #[test]
    fn paste_result_slot_is_the_only_irreversible_commit_authority() {
        let success = PasteResultSlot::new();
        assert!(success.result().is_none());
        assert!(
            success
                .publish(PasteResult {
                    submitted: true,
                    reason: None,
                })
                .submitted,
        );
        assert!(
            success
                .publish(failed_paste(PasteFailure::Unavailable))
                .submitted,
            "a later disconnect/failure cannot revoke accepted SendInput",
        );

        let failure = PasteResultSlot::new();
        assert!(
            !failure
                .publish(failed_paste(PasteFailure::OsRejected))
                .submitted
        );
        assert!(
            !failure
                .publish(PasteResult {
                    submitted: true,
                    reason: None,
                })
                .submitted,
            "state alone cannot fabricate submitted:true after rejection",
        );
    }

    #[test]
    fn caller_timeout_wins_the_final_waiting_to_injecting_race() {
        let state = AtomicU8::new(PasteCommandState::Waiting as u8);
        assert_eq!(cancel_paste_command(&state), PasteCommandState::Cancelled);
        assert!(!claim_paste_injection(&state));
        assert_eq!(paste_command_state(&state), PasteCommandState::Cancelled);
    }

    #[test]
    fn post_cas_completion_wait_is_bounded_and_reports_indeterminate() {
        let (_sender, response) = bounded::<()>(1);
        let result = PasteResultSlot::new();
        let started = Instant::now();
        assert_eq!(
            wait_for_claimed_paste_completion(&response, &result, Duration::from_millis(5)),
            failed_paste(PasteFailure::Indeterminate)
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn final_injection_claim_wins_the_cancellation_race_authoritatively() {
        let state = AtomicU8::new(PasteCommandState::Waiting as u8);
        assert!(claim_paste_injection(&state));
        assert_eq!(cancel_paste_command(&state), PasteCommandState::Injecting);
        assert_eq!(paste_command_state(&state), PasteCommandState::Injecting);
    }

    #[test]
    fn exact_send_input_rejection_leaves_injecting_before_cleanup() {
        let state = AtomicU8::new(PasteCommandState::Injecting as u8);
        let result = PasteResultSlot::new();
        let mut keyboard = CallbackKeyboard::default();
        let published = publish_initial_paste_acceptance(
            &state,
            &result,
            &mut keyboard,
            injection::test_paste_initial_outcome(
                injection::InjectionMarkers::generate().unwrap(),
                0,
            ),
            || {},
        );
        assert!(!published.submitted);
        assert_eq!(paste_command_state(&state), PasteCommandState::ResultReady);
        assert!(keyboard.pending_paste_cleanup.is_empty());
    }

    #[test]
    fn paste_commit_is_published_before_cleanup_pause() {
        let state = AtomicU8::new(PasteCommandState::Injecting as u8);
        let result = PasteResultSlot::new();
        let mut keyboard = CallbackKeyboard::default();
        let mut paused = false;

        let published = publish_initial_paste_acceptance(
            &state,
            &result,
            &mut keyboard,
            injection::test_paste_initial_outcome(
                injection::InjectionMarkers::generate().unwrap(),
                2,
            ),
            || {
                paused = true;
                assert!(result.result().is_some_and(|result| result.submitted));
                assert_eq!(paste_command_state(&state), PasteCommandState::Committed);
            },
        );

        assert!(paused);
        assert!(published.submitted);
        assert!(!keyboard.pending_paste_cleanup.is_empty());
        assert!(
            result
                .publish(failed_paste(PasteFailure::Unavailable))
                .submitted,
            "cleanup failure after the pause cannot revoke commitment",
        );
    }

    #[test]
    fn session_native_ownership_participates_in_shutdown_drain() {
        let mut keyboard = CallbackKeyboard {
            session_escape_native_owned: true,
            ..CallbackKeyboard::default()
        };
        assert!(!transaction_obligations_drained(&keyboard));
        keyboard.session_escape_native_owned = false;
        keyboard.captured_enter_source = Some(EnterSource::Numpad);
        assert!(!transaction_obligations_drained(&keyboard));
        keyboard.captured_enter_source = None;
        assert!(transaction_obligations_drained(&keyboard));
    }

    #[test]
    fn shutdown_never_retires_session_ownership_without_the_exact_up() {
        let (context, _outbound, _terminal) = test_context(2);
        let mut keyboard = context.keyboard.lock().unwrap();
        keyboard.session_escape_native_owned = true;
        assert!(!transaction_obligations_drained(&keyboard));

        let _ = process_session_event(
            &context,
            &mut keyboard,
            PhysicalKey::Escape,
            KeyPhase::Up,
            false,
            None,
        );
        assert!(transaction_obligations_drained(&keyboard));
    }

    fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
        Shortcut::new(modifiers, keys).unwrap()
    }

    fn activation_context() -> ActivationContext {
        ActivationContext::target_unavailable(ActivationGeneration::FIRST)
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
        Receiver<NativeEvent>,
        Receiver<TerminalReason>,
    ) {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound, outbound_rx) = bounded(outbound_capacity);
        let context = CallbackContext {
            state: Arc::new(shared_state_for_test()),
            keyboard: Mutex::new(CallbackKeyboard {
                activation: ActivationConfig {
                    enabled: true,
                    bindings: full_bindings(),
                },
                input_desktop: current_input_desktop(),
                ..CallbackKeyboard::default()
            }),
            owner_epoch: Instant::now(),
            suppression_enabled: true,
            outbound,
            gate,
            terminal,
            observability: Arc::new(TransactionObservability::new()),
            injection_markers: injection::InjectionMarkers::generate().unwrap(),
            replay_sender: None,
            replay_accepted: Arc::new(AtomicU64::new(0)),
        };
        (context, outbound_rx, terminal_rx)
    }

    #[test]
    fn callback_racing_deferred_replay_passes_without_replacing_authority() {
        let (context, _outbound, _terminal) = test_context(2);
        let mut keyboard = context.keyboard.lock().unwrap();
        let before_journal_len = keyboard.transactional.journal_len();
        keyboard.transaction_authority = Some(TransactionAuthority::AwaitingDeferredReplay);
        drop(keyboard);

        let disposition = Cell::new(CallbackDisposition::Capture);
        let captured = process_transactional_hook_record_at(
            &context,
            TransactionalHookRecord {
                virtual_key: 0x41,
                scan_code: 0x1e,
                extended: false,
                platform_flags: 0,
                phase: KeyPhase::Down,
                source: InputSource::Physical,
            },
            HookObservation {
                observed_at_ms: 1,
                native_modifiers: None,
            },
            &disposition,
        );

        let keyboard = context.keyboard.lock().unwrap();
        assert!(!captured);
        assert_eq!(disposition.get(), CallbackDisposition::Pass);
        assert!(matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::AwaitingDeferredReplay)
        ));
        assert_eq!(keyboard.transactional.journal_len(), before_journal_len);
    }

    #[test]
    fn closed_process_gate_bypasses_every_physical_callback_source() {
        assert!(!callback_may_process_source(false, InputSource::Physical));
        assert!(!callback_may_process_source(
            false,
            InputSource::test_physical()
        ));
        assert!(!callback_may_process_source(false, InputSource::External));
        assert!(callback_may_process_source(true, InputSource::Physical));
    }

    #[test]
    fn paste_deadline_distinguishes_modifier_wait_from_other_native_work() {
        assert_eq!(
            paste_deadline_reason(false),
            PasteFailure::ConflictingModifiers
        );
        assert_eq!(paste_deadline_reason(true), PasteFailure::Unavailable);
    }

    #[test]
    fn stale_paste_context_counts_target_fallback_without_exposing_evidence() {
        let (context, _outbound, _terminal) = test_context(8);
        let (sender, receiver) = bounded(1);
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, _acknowledged) = bounded(1);
        sender
            .send(PasteCommand {
                context: activation_context(),
                expected_clipboard_sha256: ClipboardTextHash::from_bytes([0; 32]),
                injection_deadline: Instant::now() + Duration::from_secs(1),
                state,
                result: Arc::clone(&result),
                acknowledgement,
            })
            .unwrap();
        let mut pending = None;

        process_paste_commands(&context, &receiver, &mut pending);

        assert_eq!(
            result.result(),
            Some(failed_paste(PasteFailure::Unavailable))
        );
        assert_eq!(
            context
                .observability
                .snapshot()
                .native_paste
                .target_validation_fallbacks,
            1
        );
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

    fn transactional_record(
        context: &CallbackContext,
        virtual_key: u16,
        scan_code: u32,
        extended: bool,
        phase: KeyPhase,
        source: InputSource,
        observed_at_ms: u64,
    ) -> bool {
        let disposition = Cell::new(CallbackDisposition::Pass);
        transactional_record_with_disposition(
            context,
            virtual_key,
            scan_code,
            extended,
            phase,
            source,
            observed_at_ms,
            &disposition,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn transactional_record_with_disposition(
        context: &CallbackContext,
        virtual_key: u16,
        scan_code: u32,
        extended: bool,
        phase: KeyPhase,
        source: InputSource,
        observed_at_ms: u64,
        disposition: &Cell<CallbackDisposition>,
    ) -> bool {
        process_transactional_hook_record_at(
            context,
            TransactionalHookRecord {
                virtual_key,
                scan_code,
                extended,
                platform_flags: 0,
                phase,
                source,
            },
            HookObservation {
                observed_at_ms,
                native_modifiers: None,
            },
            disposition,
        )
    }

    fn shadow_letter(context: &CallbackContext, key: ActivationKey, phase: KeyPhase) -> bool {
        transactional_record(
            context,
            u16::from(b'A') + u16::from(key.index()),
            LETTER_SCAN_CODES[usize::from(key.index())],
            false,
            phase,
            InputSource::Physical,
            0,
        )
    }

    fn shadow_modifier(context: &CallbackContext, virtual_key: u16, phase: KeyPhase) -> bool {
        let (scan_code, extended) = match virtual_key {
            VK_LSHIFT => (0x2A, false),
            VK_LMENU => (0x38, false),
            _ => (0, false),
        };
        transactional_record(
            context,
            virtual_key,
            scan_code,
            extended,
            phase,
            InputSource::Physical,
            0,
        )
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

    fn receive_event(receiver: &Receiver<NativeEvent>) -> KeyboardEvent {
        match receiver.recv_timeout(Duration::from_millis(50)).unwrap() {
            NativeEvent::Keyboard(event) => event,
            other => panic!("unexpected outbound: {other:?}"),
        }
    }

    fn apply_mutation(context: &CallbackContext, mutation: OwnerMutation) {
        let (command_tx, command_rx) = bounded(1);
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        command_tx
            .send(OwnerCommand {
                mutation,
                state: Arc::clone(&state),
                acknowledgement,
            })
            .unwrap();
        process_owner_commands(context, &command_rx);
        assert_eq!(owner_command_state(&state), OwnerCommandState::Applied);
        assert!(response.recv().unwrap().is_ok());
    }

    fn apply_config(context: &CallbackContext, activation: ActivationConfig) {
        apply_mutation(context, OwnerMutation::configure(activation));
    }

    #[test]
    fn gateway_eof_retains_external_candidate_until_exact_up_drain() {
        let (context, _outbound, terminal) = test_context(4);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Down,
            InputSource::External,
            1,
        ));
        assert!(transactional_record(
            &context,
            u16::from(b'X'),
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InputSource::External,
            2,
        ));
        publish_pending_native_work(&context);
        assert!(context.state.pending_native_work.load(Ordering::Acquire));

        apply_mutation(&context, OwnerMutation::close_admission());
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.journal_len(), 1);
            assert_eq!(
                keyboard.transactional.owned_letters(),
                1 << ActivationKey::X.index()
            );
        }

        apply_mutation(&context, OwnerMutation::cancel_candidate());
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.journal_len(), 0);
            assert_eq!(
                keyboard.transactional.owned_letters(),
                1 << ActivationKey::X.index()
            );
            assert_eq!(keyboard.transactional.metrics().replayed, 0);
        }
        assert!(context.state.pending_native_work.load(Ordering::Acquire));
        assert_ne!(
            context.keyboard.lock().unwrap().external_held_letters
                & (1 << ActivationKey::X.index()),
            0,
        );
        let release_disposition = Cell::new(CallbackDisposition::Pass);
        assert!(process_transactional_hook_record_at(
            &context,
            TransactionalHookRecord {
                virtual_key: u16::from(b'X'),
                scan_code: LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                extended: false,
                platform_flags: 0,
                phase: KeyPhase::Up,
                source: InputSource::External,
            },
            HookObservation {
                observed_at_ms: 3,
                // Simulate GetAsyncKeyState lagging the serialized external Alt
                // edge. The callback release must remain authoritative.
                native_modifiers: Some(ModifierTracker::default()),
            },
            &release_disposition,
        ));
        assert_eq!(release_disposition.get(), CallbackDisposition::Capture);
        assert_eq!(context.keyboard.lock().unwrap().external_held_letters, 0);
        publish_pending_native_work(&context);
        assert!(!context.state.pending_native_work.load(Ordering::Acquire));
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Up,
            InputSource::External,
            4,
        ));
        assert!(terminal.try_recv().is_err());
    }

    #[test]
    fn gateway_eof_target_change_retains_external_candidate_until_exact_up() {
        let (context, _outbound, terminal) = test_context(4);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Down,
            InputSource::External,
            1,
        ));
        assert!(transactional_record(
            &context,
            u16::from(b'X'),
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InputSource::External,
            2,
        ));
        context.keyboard.lock().unwrap().candidate_target_changed = true;
        apply_mutation(&context, OwnerMutation::close_admission());
        apply_mutation(&context, OwnerMutation::cancel_candidate());
        publish_pending_native_work(&context);
        assert!(context.state.pending_native_work.load(Ordering::Acquire));
        assert!(transactional_record(
            &context,
            u16::from(b'X'),
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Up,
            InputSource::External,
            3,
        ));
        publish_pending_native_work(&context);
        assert!(!context.state.pending_native_work.load(Ordering::Acquire));
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Up,
            InputSource::External,
            4,
        ));
        assert!(terminal.try_recv().is_err());
    }

    #[test]
    fn registered_observation_callback_is_exact_release_only_and_always_passes_through() {
        let (context, outbound, terminal) = test_context(4);
        apply_config(
            &context,
            ActivationConfig {
                enabled: false,
                bindings: full_bindings(),
            },
        );

        assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
        assert!(
            outbound.try_recv().is_err(),
            "candidate or match emitted proof"
        );
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        let observed = context.observability.snapshot().registered_input;
        assert_eq!(
            (
                observed.registered_candidate_callbacks,
                observed.registered_match_callbacks,
                observed.registered_release_callbacks,
                observed.callback_channel_accepted,
            ),
            (1, 1, 1, 1),
        );
        assert!(matches!(
            outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
            NativeEvent::RegisteredObservation { generation: 1 }
        ));
        assert!(outbound.try_recv().is_err());
        assert!(terminal.try_recv().is_err());

        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.journal_len(), 0);
        assert!(keyboard.dispatcher.active.is_none());
        assert!(keyboard.candidate_target.is_none());
        assert!(keyboard.pending_paste_cleanup.is_empty());
        drop(keyboard);
        let counters = context.observability.snapshot().registered_input;
        assert_eq!(counters.registered_candidate_callbacks, 1);
        assert_eq!(counters.registered_match_callbacks, 1);
        assert_eq!(counters.registered_release_callbacks, 1);
        assert_eq!(counters.callback_channel_accepted, 1);
        assert_eq!(counters.callback_channel_rejected, 0);
    }

    #[test]
    fn registered_observation_callback_rejection_never_succeeds_or_activates() {
        let (context, outbound, _terminal) = test_context(0);
        apply_config(
            &context,
            ActivationConfig {
                enabled: false,
                bindings: full_bindings(),
            },
        );
        assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        assert!(outbound.try_recv().is_err());
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.journal_len(), 0);
        assert!(keyboard.dispatcher.active.is_none());
        assert!(keyboard.candidate_target.is_none());
        drop(keyboard);
        let counters = context.observability.snapshot().registered_input;
        assert_eq!(counters.callback_channel_accepted, 0);
        assert_eq!(counters.callback_channel_rejected, 1);
    }

    #[test]
    fn registered_observation_handles_repeats_and_cancels_modifier_changes_or_wrong_releases() {
        let (context, outbound, _terminal) = test_context(8);
        apply_config(
            &context,
            ActivationConfig {
                enabled: false,
                bindings: full_bindings(),
            },
        );
        assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        assert!(outbound.try_recv().is_err());
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));

        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Down));
        assert!(!shadow_modifier(&context, VK_LSHIFT, KeyPhase::Up));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
        assert!(outbound.try_recv().is_err());

        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        assert!(matches!(
            outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
            NativeEvent::RegisteredObservation { generation: 3 }
        ));
    }

    #[test]
    fn registered_observation_preserves_shared_prefix_candidate_sets() {
        let shared = ActivationBindings::new(&[
            ActivationBinding::new(
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
            ),
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
        ])
        .unwrap();
        let (context, outbound, _terminal) = test_context(4);
        apply_config(
            &context,
            ActivationConfig {
                enabled: false,
                bindings: shared,
            },
        );
        assert!(!shadow_modifier(&context, VK_LMENU, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Up));
        assert!(matches!(
            outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
            NativeEvent::RegisteredObservation { generation: 1 }
        ));
        assert!(!shadow_letter(&context, ActivationKey::X, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Down));
        assert!(!shadow_letter(&context, ActivationKey::P, KeyPhase::Up));
        assert!(matches!(
            outbound.recv_timeout(Duration::from_millis(50)).unwrap(),
            NativeEvent::RegisteredObservation { generation: 2 }
        ));
    }

    #[test]
    fn low_level_hook_uses_no_injectable_module() {
        assert!(low_level_hook_module().is_null());
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
            OwnerMutation::set_session_capture(SessionCaptureMode::Recording),
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
        assert_eq!(
            map_key_identity(0x58, 0, false),
            KeyIdentity::Letter(ActivationKey::X)
        );
        assert_eq!(map_key_identity(0x30, 0, false), KeyIdentity::Other(0x30));
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
    fn deferred_menu_release_is_skipped_after_same_side_is_repressed() {
        let (context, _outbound, _terminal) = test_context(1);
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(menu_modifier_release_still_needed(&keyboard, 0));
        keyboard
            .modifiers
            .observe(VK_LMENU, 0x38, false, KeyPhase::Down);
        assert!(!menu_modifier_release_still_needed(&keyboard, 0));
        keyboard
            .modifiers
            .observe(VK_LMENU, 0x38, false, KeyPhase::Up);
        assert!(menu_modifier_release_still_needed(&keyboard, 0));
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
    fn altgr_layout_keeps_right_alt_suppressed_without_a_visible_ctrl_edge() {
        let mut modifiers = ModifierTracker::default();
        modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Down);
        assert!(conservative_altgr_for_layout(&modifiers, false, true));
        assert!(!conservative_altgr_for_layout(&modifiers, false, false));
        modifiers.observe(VK_MENU, 0x38, true, KeyPhase::Up);
        assert!(!conservative_altgr_for_layout(&modifiers, false, true));
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
            KeyboardEvent::Activation {
                binding,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding,
                context: activation_context(),
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
                KeyboardEvent::Activation {
                    binding: expected_binding,
                    context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: one_key,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: full_bindings().iter().next().unwrap(),
                context: activation_context(),
                phase: EventPhase::Down,
            }
        );
        // Activation delivery alone must not globally capture Enter/Escape.
        // Electron explicitly enables that capture only after accepting and
        // visibly starting the session.
        assert_eq!(
            SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire)),
            SessionCaptureMode::Off,
        );
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
            KeyboardEvent::Activation {
                binding: full_bindings().iter().next().unwrap(),
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: expected,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: expected,
                context: activation_context(),
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
                context
                    .state
                    .session_capture_mode
                    .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);

                assert!(enter(&context, first, KeyPhase::Down));
                assert_eq!(
                    receive_event(&outbound),
                    KeyboardEvent::SessionKey {
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
                    KeyboardEvent::SessionKey {
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
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Down,
            },
        );
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Down));

        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);
        apply_config(&context, ActivationConfig::default());
        assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
        assert!(enter(&context, EnterSource::Numpad, KeyPhase::Up));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: SessionKey::Enter,
                phase: EventPhase::Up,
            },
        );
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn cancel_only_captures_escape_but_passes_enter_and_balances_after_off() {
        let (context, outbound, _terminal) = test_context(4);
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::CancelOnly.as_u8(), Ordering::Release);

        assert!(!enter(&context, EnterSource::Main, KeyPhase::Down));
        assert!(!enter(&context, EnterSource::Main, KeyPhase::Up));
        assert!(record(
            &context,
            VK_ESCAPE,
            PhysicalKey::Escape,
            KeyPhase::Down,
        ));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Down,
            },
        );

        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        assert!(record(
            &context,
            VK_ESCAPE,
            PhysicalKey::Escape,
            KeyPhase::Up,
        ));
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: SessionKey::Escape,
                phase: EventPhase::Up,
            },
        );
    }

    #[test]
    fn real_transactional_hook_path_captures_ctrl_shift_activation_and_balances_up() {
        let (context, outbound, _terminal) = test_context(8);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );

        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert!(!transactional_record(
            &context,
            VK_LSHIFT,
            0x2A,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));
        assert!(transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Down,
            InputSource::Physical,
            3,
        ));
        let KeyboardEvent::Activation {
            binding,
            context: activation_context,
            phase: EventPhase::Down,
        } = receive_event(&outbound)
        else {
            panic!("expected transactional activation down")
        };
        assert_eq!(binding.profile_id(), ProfileId::GENERAL);
        assert_eq!(
            activation_context.activation_generation(),
            ActivationGeneration::FIRST
        );
        assert!(transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Up,
            InputSource::Physical,
            9,
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                context,
                phase: EventPhase::Up,
                ..
            } if context == activation_context
        ));
        assert!(!transactional_record(
            &context,
            VK_LSHIFT,
            0x2A,
            false,
            KeyPhase::Up,
            InputSource::Physical,
            10,
        ));
        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Up,
            InputSource::Physical,
            11,
        ));
        let keyboard = context.keyboard.lock().unwrap();
        assert_eq!(keyboard.transactional.owned_letters(), 0);
        assert_eq!(
            keyboard.transactional.physical_modifiers(),
            TransactionalModifierSides::default()
        );
    }

    #[test]
    fn reentrant_helper_injection_cannot_overwrite_outer_panic_disposition() {
        let (context, outbound, _terminal) = test_context(8);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = keyboard
                .transactional
                .clone()
                .with_menu_neutralization_policy(
                talking_quill_keyboard_core::transactional::MenuNeutralizationPolicy::NotRequired,
            );
        }
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert!(transactional_record(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));

        let outer = Cell::new(CallbackDisposition::Pass);
        arm_transaction_panic(TestTransactionPanicPoint::AfterEffect);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transactional_record_with_disposition(
                &context,
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                false,
                KeyPhase::Down,
                InputSource::Physical,
                3,
                &outer,
            )
        }));
        assert!(panicked.is_err());
        assert_eq!(outer.get(), CallbackDisposition::Capture);
        handle_callback_panic(&context);

        let mut keyboard = lock_keyboard_recovering(&context).expect("recover poison");
        assert!(matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::Resume { .. })
        ));
        assert!(recover_transaction_authority(&context, &mut keyboard));
        assert!(keyboard.transaction_authority.is_none());
        drop(keyboard);
        assert!(matches!(
            outbound.try_recv(),
            Ok(NativeEvent::Keyboard(KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }))
        ));
        assert!(
            outbound.try_recv().is_err(),
            "panic recovery cannot redeliver the accepted activation"
        );
    }

    #[test]
    fn panic_after_engine_turn_retains_snapshot_and_closes_admission() {
        let (context, outbound, _terminal) = test_context(8);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert!(!transactional_record(
            &context,
            VK_LSHIFT,
            0x2A,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));
        let before = context.keyboard.lock().unwrap().transactional.clone();
        arm_transaction_panic(TestTransactionPanicPoint::AfterTurn);
        let disposition = Cell::new(CallbackDisposition::Pass);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transactional_record_with_disposition(
                &context,
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                false,
                KeyPhase::Down,
                InputSource::Physical,
                3,
                &disposition,
            )
        }));
        assert!(panicked.is_err());
        handle_callback_panic(&context);
        assert!(!context.gate.is_open());
        assert_eq!(disposition.get(), CallbackDisposition::Capture);
        let keyboard = match context.keyboard.lock() {
            Err(poisoned) => poisoned.into_inner(),
            Ok(_) => panic!("panic seam must poison the authoritative keyboard lock"),
        };
        assert_eq!(
            keyboard.transactional, before,
            "the live engine was never taken"
        );
        assert!(matches!(
            keyboard.transaction_authority,
            Some(TransactionAuthority::Turn(_))
        ));
        drop(keyboard);
        assert!(transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Up,
            InputSource::Physical,
            4,
        ));
        let keyboard = lock_keyboard_recovering(&context).expect("recover terminal drain");
        assert!(keyboard.transaction_authority.is_none());
        assert_eq!(keyboard.transactional.owned_letters(), 0);
        assert!(
            outbound.try_recv().is_err(),
            "closed admission cannot execute the unsubmitted activation"
        );
    }

    #[test]
    fn panic_recovery_processes_current_owned_escape_and_enter_ups() {
        for (virtual_key, scan_code, session_key) in [
            (VK_ESCAPE, 0x01, SessionKey::Escape),
            (VK_RETURN, 0x1C, SessionKey::Enter),
        ] {
            let (context, outbound, _terminal) = test_context(8);
            context
                .state
                .session_capture_mode
                .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
            assert!(transactional_record(
                &context,
                virtual_key,
                scan_code,
                false,
                KeyPhase::Down,
                InputSource::Physical,
                1,
            ));
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: session_key,
                    phase: EventPhase::Down,
                }
            );

            arm_transaction_panic(TestTransactionPanicPoint::AfterTurn);
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transactional_record(
                    &context,
                    0x41,
                    LETTER_SCAN_CODES[usize::from(ActivationKey::A.index())],
                    false,
                    KeyPhase::Down,
                    InputSource::Physical,
                    2,
                )
            }));
            assert!(panicked.is_err());
            handle_callback_panic(&context);

            assert!(transactional_record(
                &context,
                virtual_key,
                scan_code,
                false,
                KeyPhase::Up,
                InputSource::Physical,
                3,
            ));
            assert!(
                outbound.try_recv().is_err(),
                "terminal drain clears ownership without publishing a new session event"
            );
            let keyboard = lock_keyboard_recovering(&context).expect("recovered terminal owner");
            assert!(!keyboard.session_escape_native_owned);
            assert!(keyboard.captured_enter_source.is_none());
        }
    }

    #[test]
    fn panic_after_effect_recovers_exact_outcome_without_redelivery() {
        let (context, outbound, _terminal) = test_context(8);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert!(!transactional_record(
            &context,
            VK_LSHIFT,
            0x2A,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));
        arm_transaction_panic(TestTransactionPanicPoint::AfterEffect);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transactional_record(
                &context,
                0x50,
                LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                false,
                KeyPhase::Down,
                InputSource::Physical,
                3,
            )
        }));
        assert!(panicked.is_err());
        handle_callback_panic(&context);
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
        {
            let mut keyboard = lock_keyboard_recovering(&context).expect("recover poison");
            assert!(matches!(
                keyboard.transaction_authority,
                Some(TransactionAuthority::Resume { .. })
            ));
            assert!(recover_transaction_authority(&context, &mut keyboard));
            assert!(keyboard.transaction_authority.is_none());
            assert_ne!(keyboard.transactional, TransactionEngine::default());
        }
        assert!(
            outbound.try_recv().is_err(),
            "effect was not delivered twice"
        );
    }

    #[test]
    fn external_alt_x_p_is_input_equivalent_and_activates_once() {
        let (mut context, outbound, _terminal) = test_context(4);
        let (replay_sender, replay_receiver) = bounded(1);
        context.replay_sender = Some(replay_sender);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Down,
            InputSource::External,
            1,
        ));
        assert!(transactional_record(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Down,
            InputSource::External,
            2,
        ));
        assert!(transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Down,
            InputSource::External,
            3,
        ));
        assert!(matches!(
            replay_receiver.try_recv(),
            Ok(ReplayWork::NeutralizeMenu { .. })
        ));
        context.replay_accepted.store(4, Ordering::Release);
        process_deferred_callback_replay(&context);
        assert!(transactional_record(
            &context,
            0x50,
            LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
            false,
            KeyPhase::Up,
            InputSource::External,
            4,
        ));
        assert!(transactional_record(
            &context,
            0x58,
            LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
            false,
            KeyPhase::Up,
            InputSource::External,
            5,
        ));
        assert!(!transactional_record(
            &context,
            VK_LMENU,
            0x38,
            false,
            KeyPhase::Up,
            InputSource::External,
            6,
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Up,
                ..
            }
        ));
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn hardware_shaped_physical_syskey_alt_x_uses_ordered_hook_edges() {
        let (mut context, outbound, _terminal) = test_context(4);
        let (replay_sender, replay_receiver) = bounded(1);
        context.replay_sender = Some(replay_sender);
        let alt = ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        };
        let bindings = ActivationBindings::new(&[
            ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
            ActivationBinding::new(
                ProfileId::PROMPT,
                shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
            ),
        ])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings,
            },
        );
        let process = |virtual_key, scan_code, phase, flags| {
            let disposition = Cell::new(CallbackDisposition::Pass);
            process_transactional_hook_record_at(
                &context,
                TransactionalHookRecord {
                    virtual_key,
                    scan_code,
                    extended: false,
                    platform_flags: flags,
                    phase,
                    source: InputSource::Physical,
                },
                HookObservation {
                    observed_at_ms: 1,
                    // This is the production physical callback contract: the
                    // asynchronous state that can lag Alt is diagnostic only.
                    native_modifiers: None,
                },
                &disposition,
            )
        };
        const ALT_CONTEXT_FLAG: u32 = 0x20;
        // Exact physical shape captured on the affected machine. Alt-up won
        // the release race by 16 ms, before X-up. The shorter Alt+X binding
        // must still commit, suppress X-up, and leave the owner ready for the
        // following Alt+X+P attempt.
        assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
        assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
        assert!(process(VK_LMENU, 0x38, KeyPhase::Up, 0x80));
        assert!(matches!(
            replay_receiver.try_recv(),
            Ok(ReplayWork::NeutralizeMenu { .. })
        ));
        assert!(process(0x58, 0x2d, KeyPhase::Up, 0x80));
        context.replay_accepted.store(4, Ordering::Release);
        process_deferred_callback_replay(&context);
        assert!(outbound.try_iter().any(|event| matches!(
            event,
            NativeEvent::Keyboard(KeyboardEvent::ActivationComplete { .. })
        )));
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert!(keyboard.transactional.admission_open());
            assert_eq!(keyboard.transactional.owned_letters(), 0);
        }

        assert!(!process(VK_LMENU, 0x38, KeyPhase::Down, ALT_CONTEXT_FLAG));
        assert!(process(0x58, 0x2d, KeyPhase::Down, ALT_CONTEXT_FLAG));
        assert!(process(0x50, 0x19, KeyPhase::Down, ALT_CONTEXT_FLAG));
    }

    #[test]
    fn physical_and_external_alt_x_share_matcher_suppression_and_delivery() {
        for source in [InputSource::Physical, InputSource::External] {
            let (mut context, outbound, _terminal) = test_context(4);
            let (replay_sender, replay_receiver) = bounded(1);
            context.replay_sender = Some(replay_sender);
            apply_config(
                &context,
                ActivationConfig {
                    enabled: true,
                    bindings: full_bindings(),
                },
            );
            let records = [
                (VK_LMENU, 0x38, KeyPhase::Down, false),
                (
                    0x58,
                    LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                    KeyPhase::Down,
                    true,
                ),
                (
                    0x50,
                    LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                    KeyPhase::Down,
                    true,
                ),
                (
                    0x50,
                    LETTER_SCAN_CODES[usize::from(ActivationKey::P.index())],
                    KeyPhase::Up,
                    true,
                ),
                (
                    0x58,
                    LETTER_SCAN_CODES[usize::from(ActivationKey::X.index())],
                    KeyPhase::Up,
                    true,
                ),
                (VK_LMENU, 0x38, KeyPhase::Up, false),
            ];
            for (at, (virtual_key, scan_code, phase, expected_capture)) in
                records.into_iter().enumerate()
            {
                assert_eq!(
                    transactional_record(
                        &context,
                        virtual_key,
                        scan_code,
                        false,
                        phase,
                        source,
                        u64::try_from(at + 1).unwrap(),
                    ),
                    expected_capture,
                    "source={source:?} edge={at}",
                );
                if at == 2 {
                    assert!(matches!(
                        replay_receiver.try_recv(),
                        Ok(ReplayWork::NeutralizeMenu { .. })
                    ));
                    context.replay_accepted.store(4, Ordering::Release);
                    process_deferred_callback_replay(&context);
                }
            }
            assert!(matches!(
                receive_event(&outbound),
                KeyboardEvent::Activation {
                    phase: EventPhase::Down,
                    ..
                }
            ));
            assert!(matches!(
                receive_event(&outbound),
                KeyboardEvent::Activation {
                    phase: EventPhase::Up,
                    ..
                }
            ));
            assert!(outbound.try_recv().is_err());
        }
    }

    #[test]
    fn unavailable_replay_worker_publishes_a_consumable_suppression_result() {
        let (context, _outbound, terminal) = test_context(4);
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: full_bindings(),
            },
        );
        assert!(!transactional_record(
            &context,
            VK_MENU,
            0,
            false,
            KeyPhase::Down,
            InputSource::External,
            1,
        ));
        assert!(transactional_record(
            &context,
            0x58,
            0,
            false,
            KeyPhase::Down,
            InputSource::External,
            2,
        ));
        assert!(transactional_record(
            &context,
            0x5a,
            0,
            false,
            KeyPhase::Down,
            InputSource::External,
            3,
        ));
        assert_eq!(context.replay_accepted.load(Ordering::Acquire), 1);
        process_deferred_callback_replay(&context);
        let keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.deferred_callback_replay.is_none());
        assert!(keyboard.transaction_authority.is_none());
        assert!(terminal.try_recv().is_ok());
    }

    #[test]
    fn exact_virtual_key_sendinput_sequence_matches_serialized_alt_x_profiles() {
        let (mut context, outbound, _terminal) = test_context(16);
        let (replay_sender, replay_receiver) = bounded(1);
        context.replay_sender = Some(replay_sender);
        let alt = ShortcutModifiers {
            ctrl: false,
            alt: true,
            shift: false,
            meta: false,
        };
        let bindings = ActivationBindings::new(&[
            ActivationBinding::new(ProfileId::GENERAL, shortcut(alt, &[ActivationKey::X])),
            ActivationBinding::new(
                ProfileId::PROMPT,
                shortcut(alt, &[ActivationKey::X, ActivationKey::P]),
            ),
        ])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings,
            },
        );

        let mut at = 1;
        let mut input = |vk, phase| {
            let captured =
                transactional_record(&context, vk, 0, false, phase, InputSource::External, at);
            at += 1;
            captured
        };
        // Exact serialized virtual-key edges emitted by the installed
        // PowerShell validator: unmatched Alt+X+Z, Alt+X, Alt+X+P, then U.
        for (index, (vk, phase, expected)) in [
            (VK_MENU, KeyPhase::Down, false),
            (0x58, KeyPhase::Down, true),
            (0x5A, KeyPhase::Down, true),
            (0x5A, KeyPhase::Up, false),
            (0x58, KeyPhase::Up, false),
            (VK_MENU, KeyPhase::Up, false),
            (VK_MENU, KeyPhase::Down, false),
            (0x58, KeyPhase::Down, true),
            (0x58, KeyPhase::Up, true),
            (VK_MENU, KeyPhase::Up, false),
            (VK_MENU, KeyPhase::Down, false),
            (0x58, KeyPhase::Down, true),
            (0x50, KeyPhase::Down, true),
            (0x50, KeyPhase::Up, true),
            (0x58, KeyPhase::Up, true),
            (VK_MENU, KeyPhase::Up, false),
            (0x55, KeyPhase::Down, false),
            (0x55, KeyPhase::Up, false),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                input(vk, phase),
                expected,
                "event={index} vk={vk:#x} phase={phase:?}"
            );
            if index == 5 {
                // Unit tests have no replay worker or owner message pump.
                // Publish the exact accepted count at a deterministic boundary
                // after proving racing physical edges preserve its authority.
                let work = replay_receiver.try_recv().unwrap();
                process_deferred_callback_replay(&context);
                {
                    let keyboard = context.keyboard.lock().unwrap();
                    assert!(keyboard.deferred_callback_replay.is_some());
                    assert!(matches!(
                        keyboard.transaction_authority,
                        Some(TransactionAuthority::AwaitingDeferredReplay)
                    ));
                }
                let ReplayWork::Replay { batch, .. } = work else {
                    panic!("unmatched sequence must defer replay");
                };
                context.replay_accepted.store(
                    u64::try_from(batch.len()).unwrap().saturating_add(2),
                    Ordering::Release,
                );
                process_deferred_callback_replay(&context);
                let mut keyboard = context.keyboard.lock().unwrap();
                let outcome = begin_transaction_control(
                    &context,
                    &mut keyboard,
                    Control::Reconcile(PhysicalSnapshot::default()),
                );
                assert!(outcome.is_some_and(|outcome| outcome.applied));
                assert!(keyboard.transactional.config().enabled());
                assert!(keyboard.transactional.admission_open());
                assert_eq!(keyboard.transactional.physical_letters(), 0);
                assert_eq!(keyboard.transactional.fenced_letters(), 0);
                assert_eq!(keyboard.transactional.physical_modifiers().bits(), 0);
                assert_eq!(keyboard.transactional.fenced_modifiers().bits(), 0);
            }
            if index == 9 || index == 12 {
                assert!(matches!(
                    replay_receiver.try_recv(),
                    Ok(ReplayWork::NeutralizeMenu { .. })
                ));
                context.replay_accepted.store(4, Ordering::Release);
                process_deferred_callback_replay(&context);
            }
        }
        let events: Vec<_> = outbound.try_iter().collect();
        let downs = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    NativeEvent::Keyboard(KeyboardEvent::Activation {
                        phase: EventPhase::Down,
                        ..
                    })
                )
            })
            .count();
        let ups = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    NativeEvent::Keyboard(KeyboardEvent::Activation {
                        phase: EventPhase::Up,
                        ..
                    })
                )
            })
            .count();
        assert_eq!(downs, ups, "activation notifications remain balanced");
        assert!(downs >= 1);
        let observed = context.observability.snapshot();
        assert_eq!(observed.registered_input.registered_candidate_callbacks, 3);
        assert_eq!(observed.transactions.committed, 2);
        assert_eq!(observed.transactions.replayed, 1);
        assert!(!context.terminal.is_triggered());
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .transactional
                .admission_open()
        );
    }

    #[test]
    fn generic_and_sided_alt_normalize_with_physical_or_virtual_key_letters() {
        for (alt_vk, alt_scan, alt_extended) in [
            (VK_MENU, 0, false),
            (VK_LMENU, 0x38, false),
            (VK_RMENU, 0x38, true),
        ] {
            let side = modifier_side(alt_vk, alt_scan, alt_extended).unwrap();
            assert_eq!(
                side,
                if alt_extended {
                    ModifierSide::RightAlt
                } else {
                    ModifierSide::LeftAlt
                }
            );
        }
        for (vk, scan, expected) in [
            (0x58, 0, ActivationKey::X),
            (0, 0x2D, ActivationKey::X),
            (0x50, 0, ActivationKey::P),
            (0, 0x19, ActivationKey::P),
        ] {
            assert_eq!(
                map_key_identity(vk, scan, false),
                KeyIdentity::Letter(expected)
            );
        }
    }

    #[test]
    fn external_session_keys_are_input_equivalent_and_preserve_balancing() {
        let (context, outbound, _terminal) = test_context(4);
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        for (virtual_key, scan_code) in [(VK_ESCAPE, 0x01), (VK_RETURN, 0x1C)] {
            for phase in [KeyPhase::Down, KeyPhase::Up] {
                assert!(transactional_record(
                    &context,
                    virtual_key,
                    scan_code,
                    false,
                    phase,
                    InputSource::External,
                    1,
                ));
            }
        }
        assert_eq!(outbound.try_iter().count(), 4);
    }

    #[test]
    fn helper_classes_bypass_and_ctrl_right_alt_fails_closed_as_altgr() {
        let (context, outbound, _terminal) = test_context(8);
        let ctrl_alt = ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            shortcut(
                ShortcutModifiers {
                    ctrl: true,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::P],
            ),
        )])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: ctrl_alt,
            },
        );
        let before = context.keyboard.lock().unwrap().transactional.clone();
        for source in [
            InputSource::HelperReplay,
            InputSource::HelperPaste,
            InputSource::HelperDummy,
        ] {
            assert!(!transactional_record(
                &context,
                0x50,
                0x19,
                false,
                KeyPhase::Down,
                source,
                1,
            ));
        }
        assert_eq!(context.keyboard.lock().unwrap().transactional, before);

        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));
        assert!(!transactional_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Down,
            InputSource::Physical,
            10,
        ));
        assert!(context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
        assert!(!transactional_record(
            &context,
            0x50,
            0x19,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            11,
        ));
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn deferred_replay_race_tracks_ctrl_right_alt_as_altgr() {
        let mut keyboard = CallbackKeyboard::default();
        track_deferred_replay_race(&mut keyboard, VK_LCONTROL, 0x1D, false, KeyPhase::Down);
        track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Down);
        assert!(keyboard.altgr_synthetic_ctrl);
        assert!(keyboard.altgr_active);
        track_deferred_replay_race(&mut keyboard, 0x50, 0x19, false, KeyPhase::Down);
        assert!(keyboard.altgr_active);
        track_deferred_replay_race(&mut keyboard, VK_RMENU, 0x38, true, KeyPhase::Up);
        assert!(!keyboard.altgr_synthetic_ctrl);
        assert!(!keyboard.altgr_active);
    }

    #[test]
    fn external_ctrl_is_real_input_and_cannot_fake_an_alt_only_binding() {
        let (context, outbound, _terminal) = test_context(4);
        let alt = ActivationBindings::new(&[ActivationBinding::new(
            ProfileId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            shortcut(
                ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[ActivationKey::P],
            ),
        )])
        .unwrap();
        apply_config(
            &context,
            ActivationConfig {
                enabled: true,
                bindings: alt,
            },
        );
        assert!(!transactional_record(
            &context,
            VK_LCONTROL,
            0x1D,
            false,
            KeyPhase::Down,
            InputSource::External,
            0,
        ));
        assert!(!context.keyboard.lock().unwrap().altgr_synthetic_ctrl);
        assert!(!transactional_record(
            &context,
            VK_RMENU,
            0x38,
            true,
            KeyPhase::Down,
            InputSource::Physical,
            1,
        ));
        assert!(!transactional_record(
            &context,
            0x50,
            0x19,
            false,
            KeyPhase::Down,
            InputSource::Physical,
            2,
        ));
        assert!(outbound.try_recv().is_err());
        let keyboard = context.keyboard.lock().unwrap();
        assert!(
            keyboard
                .transactional
                .physical_modifiers()
                .combined()
                .ctrl()
        );
    }

    #[test]
    fn escape_and_enter_capture_remains_paired_and_modifier_independent() {
        let (context, outbound, _terminal) = test_context(8);
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        modifier(&context, VK_LWIN, KeyPhase::Down);
        for (key, session_key) in [
            (PhysicalKey::Escape, SessionKey::Escape),
            (PhysicalKey::Enter, SessionKey::Enter),
        ] {
            assert!(record(&context, 0, key, KeyPhase::Down));
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: session_key,
                    phase: EventPhase::Down,
                }
            );
            assert!(record(&context, 0, key, KeyPhase::Down));
            assert!(record(&context, 0, key, KeyPhase::Up));
            assert_eq!(
                receive_event(&outbound),
                KeyboardEvent::SessionKey {
                    key: session_key,
                    phase: EventPhase::Up,
                }
            );
        }
    }
}
