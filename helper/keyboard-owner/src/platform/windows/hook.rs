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

mod lifecycle;
use lifecycle::*;

mod callback_state;
use callback_state::*;

mod owner_protocol;
use owner_protocol::*;

mod paste_protocol;
use paste_protocol::*;

mod tracking;
use tracking::*;

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
