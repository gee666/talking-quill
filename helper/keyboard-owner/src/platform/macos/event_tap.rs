//! Event-tap owner facade. The boxed `CallbackContext` remains owned by the
//! hook run loop, and every child module borrows that same authoritative state.
//!
//! `startup`, `owner_thread`, and `resources` own the handshake and CF lifetime.
//! `callback` routes authenticated observations before ordinary input reaches
//! `normalization`, `transactional_event`, and `session`. `effects` retains each
//! continuation until native observation or activation validation completes.
//!
//! Paste modules keep target validation on the AX worker and submit insertion
//! only after the exact neutral barrier. `unwind`, `gap_recovery`, and the
//! recovery classifier preserve native obligations when ordinary admission is
//! closed. `journal_*` implements bounded deferred ordering without replacing
//! the submitted replay cursor. `shutdown` is the conservative lifetime gate.
//!
//! Tests are grouped by protocol contract under `tests/`. Only the existing
//! start/stop and development contract entry points are exposed to macos.

#![cfg_attr(all(test, feature = "local-unsigned-owner"), allow(dead_code))]

use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, TryLockError,
        atomic::{AtomicPtr, AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, bounded};

use super::{
    ActivationConfig, OWNER_COMPLETION_TIMEOUT, OwnerCommand, OwnerCommandState, OwnerCompletion,
    OwnerMutationKind, PasteCommand, PasteCommandState, PendingPaste, SharedState, StartupState,
    accessibility::permission_snapshot,
    cancel_owner_command, cancel_paste_command, cancel_startup, claim_owner_command, claim_startup,
    failed_paste, ffi, injection, owner_completed, paste_before_deadline, paste_command_state,
    paste_injection_cutoff, secure_input_active,
    target::{
        ActivationReservation, InsertionRequest, InsertionStatus, TargetCache, TargetHandle,
        TargetRegistry, ValidationRequest,
    },
};
use crate::{
    platform::NativeEvent,
    platform::{
        CallbackGate, HookStatus, ModifierNeutralWait, PasteFailure, PasteResult, PermissionState,
        PlatformError, TapRecoveryDecision, TapRecoveryEvent, TapRecoveryPolicy, TerminalReason,
        TerminalSignal, TransactionObservability, deliver_callback_event, hook_status_to_u8,
        permissions_allow_native_input,
    },
};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, ActivationKey, EventPhase,
    KeyInput, KeyPhase, KeyboardEvent, KeyboardReducer, ModifierMask, PhysicalKey,
    SessionCaptureMode, SessionKey,
    transactional::{
        ActivationNotice, CancelReason, CleanupBatch, CompiledActivationConfig, Completion,
        ConfigRevision, Continuation, Control, EffectOutcome, EffectRequest, EngineInput,
        EventDisposition, GateState, InputSource, KeyIdentity, MAX_EFFECTS_PER_TURN,
        MenuNeutralizationPolicy, ModifierSide, ModifierSides, NativeKey, NormalizedEvent,
        PhysicalPhase, PhysicalSnapshot, ReplayBatch, ReplayRecord, ShutdownState,
        TransactionEngine, Turn,
    },
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
const MAINTENANCE_INTERVAL_SECONDS: f64 = 0.010;
const PARKED_TIMER_SECONDS: f64 = 31_536_000.0;
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_millis(1_500);
const ACTIVATION_TARGET_TIMEOUT: Duration = Duration::from_millis(25);
const EXTERNAL_DEFERRED_COLLECTION_TIMEOUT: Duration = Duration::from_millis(250);

#[cfg(test)]
std::thread_local! {
    static PANIC_AFTER_DEFERRED_INSTALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_AFTER_EFFECT_OUTCOME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_AFTER_REPLAY_RECOGNITION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_AFTER_PASTE_UP_OBSERVATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_ONCE_DURING_RECOVERY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_EFFECT_SUBMISSION_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_SHUTDOWN_CONTROL_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
const TEST_RECOVERY_PHYSICAL_MARKER: i64 = 0x5451_5245_434f_5652;

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

struct PendingActivation {
    continuation: Continuation,
    notice: ActivationNotice,
    reservation: ActivationReservation,
    validation_request: ValidationRequest,
    resolved_delivery: Option<bool>,
    deadline: Instant,
}

struct CallbackContext {
    state: Arc<SharedState>,
    suppression_enabled: bool,
    keyboard: RecoveringMutex<CallbackKeyboard>,
    pending_activation: RecoveringMutex<Option<PendingActivation>>,
    native_events: RecoveringMutex<Option<injection::NativeEventPool>>,
    injection_identity: Option<injection::InjectionIdentity>,
    test_physical_seam_enabled: bool,
    callback_proxy: AtomicPtr<c_void>,
    current_edge_disposition: AtomicU8,
    owner_commands: Receiver<OwnerCommand>,
    paste_commands: Receiver<PasteCommand>,
    pending_paste: RecoveringMutex<Option<PendingPaste>>,
    recovery_edges: RecoveringMutex<RecoveryEdgeJournal>,
    target_cache: Option<TargetCache>,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_tap_disable_request: Option<std::path::PathBuf>,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_paste_barrier_paused: Option<std::path::PathBuf>,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_paste_barrier_release: Option<std::path::PathBuf>,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_paste_barrier_split_active: std::sync::atomic::AtomicBool,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_paste_barrier_down_observed: std::sync::atomic::AtomicBool,
    #[cfg(feature = "transactional-shortcuts-dev")]
    test_paste_barrier_pause_announced: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    forced_activation_reservation: Option<ActivationReservation>,
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
}

// All state below is owned by the tap run loop. Modules borrow the same
// context; extraction adds no queues, threads, locks, or native submissions.
mod physical;
use physical::*;
mod activation;
use activation::*;
mod observation;
use observation::*;
mod state;
use state::*;
mod journal;
mod keyboard_replay;
mod keyboard_tracking;
use journal::*;
mod journal_ordering;
mod journal_overflow;
mod journal_ownership;
mod journal_phases;
mod journal_sources;
mod journal_submission;
mod startup;
use startup::event_tap_options;
pub(super) use startup::{request_stop, start_hook};
mod resources;
use resources::*;
mod owner_thread;
use owner_thread::*;
mod owner_callbacks;
use owner_callbacks::*;
#[cfg(feature = "transactional-shortcuts-dev")]
mod dev_seams;
#[cfg(feature = "transactional-shortcuts-dev")]
use dev_seams::*;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) use dev_seams::{
    test_modifier_barrier_contract, test_permission_disable_recovery_contract,
};
mod deferred_submission;
use deferred_submission::*;
mod disposition;
use disposition::*;
mod effects;
use effects::*;
mod replay_completion;
use replay_completion::*;
mod unwind;
use unwind::*;
mod activation_validation;
use activation_validation::*;
mod commands;
use commands::*;
mod paste_admission;
use paste_admission::*;
mod paste_validation;
use paste_validation::*;
mod paste_poll;
use paste_poll::*;
mod paste_completion;
use paste_completion::*;
mod shutdown;
use shutdown::*;
mod permissions;
use permissions::*;
mod session;
use session::*;
mod gap_recovery;
use gap_recovery::*;
mod tap_recovery;
use tap_recovery::*;
mod deferred_observation;
use deferred_observation::*;
mod deferred_capture;
use deferred_capture::*;
mod deferred_fences;
use deferred_fences::*;
mod recovery_classifier;
use recovery_classifier::*;
mod callback;
use callback::*;
mod transactional_event;
use transactional_event::*;
#[cfg(test)]
mod legacy_test_adapter;
#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
use legacy_test_adapter::*;

#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
mod tests;

mod gap_observation;
use gap_observation::*;

mod paste_observation;
use paste_observation::*;

mod replay_observation;
use replay_observation::*;

mod mouse_callback;
use mouse_callback::*;

mod normalization;
use normalization::*;
