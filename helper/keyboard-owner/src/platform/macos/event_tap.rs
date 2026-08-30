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

    fn held_letter_bits(&self) -> u32 {
        LETTER_KEY_CODES
            .iter()
            .enumerate()
            .fold(0_u32, |bits, (index, key_code)| {
                bits | if self.is_held(*key_code) {
                    1_u32 << index
                } else {
                    0
                }
            })
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

    #[cfg(test)]
    fn remove(&mut self, key: ActivationKey) {
        self.0 &= !(1_u32 << u32::from(key.index()));
    }

    #[cfg(test)]
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

    const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.0 & 0b0000_0011 != 0,
            self.0 & 0b0000_1100 != 0,
            self.0 & 0b0011_0000 != 0,
            self.0 & 0b1100_0000 != 0,
        )
    }

    const fn sides(self) -> ModifierSides {
        let mut bits = 0_u8;
        if self.0 & 0b0000_0001 != 0 {
            bits |= ModifierSide::LeftCtrl.bit();
        }
        if self.0 & 0b0000_0010 != 0 {
            bits |= ModifierSide::RightCtrl.bit();
        }
        if self.0 & 0b0000_0100 != 0 {
            bits |= ModifierSide::LeftAlt.bit();
        }
        if self.0 & 0b0000_1000 != 0 {
            bits |= ModifierSide::RightAlt.bit();
        }
        if self.0 & 0b0001_0000 != 0 {
            bits |= ModifierSide::LeftShift.bit();
        }
        if self.0 & 0b0010_0000 != 0 {
            bits |= ModifierSide::RightShift.bit();
        }
        if self.0 & 0b0100_0000 != 0 {
            bits |= ModifierSide::LeftMeta.bit();
        }
        if self.0 & 0b1000_0000 != 0 {
            bits |= ModifierSide::RightMeta.bit();
        }
        ModifierSides::from_bits(bits)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveActivationContext {
    binding: ActivationBinding,
    context: ActivationContext,
}

struct ActivationDispatcher {
    next_generation: Option<ActivationGeneration>,
    active: Option<ActiveActivationContext>,
    targets: TargetRegistry,
}

impl ActivationDispatcher {
    fn initialize_process_epoch(&mut self) {
        let _ = self.targets.initialize_process_epoch();
    }

    fn deliver(
        &mut self,
        outbound: &Sender<NativeEvent>,
        terminal: &TerminalSignal,
        notice: ActivationNotice,
        cached_target: Option<TargetHandle>,
    ) -> bool {
        match notice {
            ActivationNotice::Down { binding } => {
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(cached_target) else {
                    return false;
                };
                if deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context,
                        phase: EventPhase::Down,
                    },
                ) {
                    self.active = Some(ActiveActivationContext { binding, context });
                    true
                } else {
                    self.targets.remove(context);
                    false
                }
            }
            ActivationNotice::Up { binding, .. } => {
                let Some(active) = self.active.take() else {
                    return false;
                };
                if active.binding != binding {
                    self.targets.remove(active.context);
                    return false;
                }
                deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    },
                )
            }
            ActivationNotice::Complete { binding, held_ms } => {
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(cached_target) else {
                    return false;
                };
                if deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::ActivationComplete {
                        binding,
                        context,
                        held_ms,
                    },
                ) {
                    true
                } else {
                    self.targets.remove(context);
                    false
                }
            }
        }
    }

    fn take_context(&mut self, target: Option<TargetHandle>) -> Option<ActivationContext> {
        let generation = self.next_generation?;
        self.next_generation = if generation == ActivationGeneration::MAX {
            None
        } else {
            ActivationGeneration::new(generation.get() + 1)
        };
        Some(self.targets.bind_context(generation, target))
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

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy)]
enum ExpectedReplayBatch {
    Replay(ReplayBatch),
    Cleanup(CleanupBatch),
}

impl ExpectedReplayBatch {
    const fn len(self) -> usize {
        match self {
            Self::Replay(batch) => batch.len(),
            Self::Cleanup(batch) => batch.len(),
        }
    }

    fn record(self, index: usize) -> Option<ReplayRecord> {
        match self {
            Self::Replay(batch) => batch.entries().get(index).copied(),
            Self::Cleanup(batch) => batch.entries().get(index).copied(),
        }
    }
}

struct ReplayObservation {
    batch: ExpectedReplayBatch,
    token: injection::OperationToken,
    next: usize,
    deadline: Instant,
}

#[cfg(test)]
struct SubmittedReplayTerminalSuffix {
    suffix: [ReplayRecord; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY],
    suffix_len: usize,
}

#[cfg(test)]
impl SubmittedReplayTerminalSuffix {
    fn capture(observation: &ReplayObservation) -> Self {
        let mut suffix = [ReplayRecord {
            key: KeyIdentity::Other(0),
            native: NativeKey {
                virtual_key: 0,
                scan_code: 0,
                extended: false,
                platform_flags: 0,
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 0,
        }; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY];
        let mut suffix_len = 0;
        for index in observation.next..observation.batch.len() {
            if let Some(record) = observation.batch.record(index) {
                suffix[suffix_len] = record;
                suffix_len += 1;
            }
        }
        Self { suffix, suffix_len }
    }
}

#[derive(Clone)]
struct InflightEffect {
    effect: EffectRequest,
    continuation: Continuation,
    outcome: Option<EffectOutcome>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
enum CurrentEdgeDisposition {
    #[default]
    Pass,
    Owned,
    Replaced,
}

impl CurrentEdgeDisposition {
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Owned,
            2 => Self::Replaced,
            _ => Self::Pass,
        }
    }
}

/// Callback-critical mutex that treats poison as retained authoritative data.
/// Poison is cleared only by callback recovery after every continuation and
/// native obligation has been restored.
struct RecoveringMutex<T>(Mutex<T>);

impl<T> RecoveringMutex<T> {
    const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    #[cfg(test)]
    fn lock(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        match self.0.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }

    fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError<MutexGuard<'_, T>>> {
        match self.0.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
        }
    }

    fn get_mut(&mut self) -> Result<&mut T, PoisonError<&mut T>> {
        match self.0.get_mut() {
            Ok(value) => Ok(value),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }

    fn clear_poison(&self) {
        self.0.clear_poison();
    }

    #[cfg(test)]
    fn is_poisoned(&self) -> bool {
        self.0.is_poisoned()
    }
}

struct CallbackKeyboard {
    // Retained only for independent Escape/Enter capture and legacy adapter
    // tests. Global activation is exclusively transactional in production.
    reducer: KeyboardReducer,
    transactional: TransactionEngine,
    dispatcher: ActivationDispatcher,
    physical: MacPhysicalTracker,
    modifiers: MacModifierTracker,
    modifier_epoch: u64,
    preheld_letters: PreheldLetters,
    activation: ActivationConfig,
    activation_revision_at: u64,
    escape_capture_enabled_at: u64,
    enter_capture_enabled_at: u64,
    captured_enter_key_code: Option<u16>,
    session_escape_native_owned: bool,
    session_enter_native_owned: Option<u16>,
    gap_reconciled_letters: u32,
    gap_reconciled_escape: bool,
    gap_reconciled_enter_key_code: Option<u16>,
    gap_barrier_pending: bool,
    gap_barrier_token: Option<injection::OperationToken>,
    gap_barrier_observed_down: bool,
    replay_observation: Option<ReplayObservation>,
    replay_target_changed: bool,
    replay_visible_downs: u32,
    replay_visible_down_records:
        [Option<ReplayRecord>; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY],
    replay_visible_down_len: usize,
    replay_target_cleanup_pending: bool,
    inflight_effect: Option<InflightEffect>,
    last_native_effect_failed: bool,
    current_edge_disposition: CurrentEdgeDisposition,
    candidate_target_captured: bool,
    candidate_target: Option<ActivationReservation>,
    owned_down_records: [Option<ReplayRecord>; 26],
    shutdown_requested: bool,
    shutdown_deadline: Option<Instant>,
    shutdown_deadline_reported: bool,
}

impl Default for CallbackKeyboard {
    fn default() -> Self {
        Self {
            reducer: KeyboardReducer::default(),
            transactional: TransactionEngine::default()
                .with_menu_neutralization_policy(MenuNeutralizationPolicy::NotRequired),
            dispatcher: ActivationDispatcher::default(),
            physical: MacPhysicalTracker::default(),
            modifiers: MacModifierTracker::default(),
            modifier_epoch: 1,
            preheld_letters: PreheldLetters::default(),
            activation: ActivationConfig::default(),
            activation_revision_at: 0,
            escape_capture_enabled_at: 0,
            enter_capture_enabled_at: 0,
            captured_enter_key_code: None,
            session_escape_native_owned: false,
            session_enter_native_owned: None,
            gap_reconciled_letters: 0,
            gap_reconciled_escape: false,
            gap_reconciled_enter_key_code: None,
            gap_barrier_pending: false,
            gap_barrier_token: None,
            gap_barrier_observed_down: false,
            replay_observation: None,
            replay_target_changed: false,
            replay_visible_downs: 0,
            replay_visible_down_records: [None;
                talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY],
            replay_visible_down_len: 0,
            replay_target_cleanup_pending: false,
            inflight_effect: None,
            last_native_effect_failed: false,
            current_edge_disposition: CurrentEdgeDisposition::Pass,
            candidate_target_captured: false,
            candidate_target: None,
            owned_down_records: [None; 26],
            shutdown_requested: false,
            shutdown_deadline: None,
            shutdown_deadline_reported: false,
        }
    }
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

    #[cfg(test)]
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

    fn handle_gap_tombstone(&mut self, key_code: u16, phase: KeyPhase, is_repeat: bool) -> bool {
        if phase == KeyPhase::Down && is_repeat {
            // Autorepeat records queued before the hidden physical up remain
            // owned. They neither prove a fresh down nor retire the barrier.
            return if let PhysicalKey::Letter(key) = map_key_code(key_code) {
                self.gap_reconciled_letters & (1_u32 << u32::from(key.index())) != 0
            } else {
                (key_code == ESCAPE_KEY_CODE && self.gap_reconciled_escape)
                    || self.gap_reconciled_enter_key_code == Some(key_code)
            };
        }
        let tombstone = if let PhysicalKey::Letter(key) = map_key_code(key_code) {
            let bit = 1_u32 << u32::from(key.index());
            let present = self.gap_reconciled_letters & bit != 0;
            self.gap_reconciled_letters &= !bit;
            present
        } else if key_code == ESCAPE_KEY_CODE && self.gap_reconciled_escape {
            self.gap_reconciled_escape = false;
            true
        } else if self.gap_reconciled_enter_key_code == Some(key_code) {
            self.gap_reconciled_enter_key_code = None;
            true
        } else {
            false
        };
        if tombstone && !self.gap_tombstones_pending() && self.gap_barrier_token.is_none() {
            // Stream order retired the final uncertain source edge before a
            // barrier was submitted. An already-posted exact pair must remain
            // installed and drain in order even after a fresh down.
            self.gap_barrier_pending = false;
        }
        let captured = tombstone && phase == KeyPhase::Up;
        if captured {
            let _ = self.physical.observe(key_code, KeyPhase::Up);
        }
        captured
    }

    fn gap_tombstones_pending(&self) -> bool {
        self.gap_reconciled_letters != 0
            || self.gap_reconciled_escape
            || self.gap_reconciled_enter_key_code.is_some()
    }

    fn strict_drain_recovery_needed(&self) -> bool {
        self.transactional.journal_len() != 0
            || self.transactional.owned_letters() != 0
            || self.transactional.pending_injected_cleanup().is_some()
            || self.transactional.pending_menu_cleanup().is_some()
            || has_session_ownership(self)
            || self.gap_tombstones_pending()
            || self.gap_barrier_pending
            || self.gap_barrier_token.is_some()
            || self.replay_observation.is_some()
            || self.inflight_effect.is_some()
    }

    fn clear_gap_tombstones(&mut self) {
        self.gap_reconciled_letters = 0;
        self.gap_reconciled_escape = false;
        self.gap_reconciled_enter_key_code = None;
        self.gap_barrier_pending = false;
        self.gap_barrier_token = None;
        self.gap_barrier_observed_down = false;
    }

    fn submitted_replay_authority(&self) -> Option<&ReplayObservation> {
        let observation = self.replay_observation.as_ref()?;
        self.inflight_effect
            .as_ref()
            .is_some_and(|inflight| inflight.outcome.is_none())
            .then_some(observation)
    }

    fn has_submitted_replay_authority(&self) -> bool {
        self.submitted_replay_authority().is_some()
    }

    fn begin_replay_observation(
        &mut self,
        batch: ExpectedReplayBatch,
        token: injection::OperationToken,
    ) -> bool {
        if self.replay_observation.is_some() || batch.len() == 0 {
            return false;
        }
        self.replay_observation = Some(ReplayObservation {
            batch,
            token,
            next: 0,
            deadline: Instant::now() + Duration::from_millis(250),
        });
        if matches!(batch, ExpectedReplayBatch::Replay(_)) {
            self.replay_target_changed = false;
            self.replay_visible_downs = 0;
            self.replay_visible_down_records =
                [None; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY];
            self.replay_visible_down_len = 0;
            self.replay_target_cleanup_pending = false;
        }
        true
    }

    fn current_replay_record(&self) -> Option<ReplayRecord> {
        let observation = self.replay_observation.as_ref()?;
        observation.batch.record(observation.next)
    }

    fn replay_disposition_after_target_check(
        &mut self,
        target_is_current: bool,
    ) -> CurrentEdgeDisposition {
        if !target_is_current {
            self.replay_target_changed = true;
        }
        let Some(record) = self.current_replay_record() else {
            return CurrentEdgeDisposition::Owned;
        };
        let letter_bit = match record.key {
            KeyIdentity::Letter(letter) => 1_u32 << u32::from(letter.index()),
            _ => 0,
        };
        let balancing_modifier_up =
            matches!(record.key, KeyIdentity::Modifier(_)) && record.phase == PhysicalPhase::Up;
        let disposition = if !self.replay_target_changed
            || balancing_modifier_up
            || (record.phase == PhysicalPhase::Up && self.replay_visible_downs & letter_bit != 0)
        {
            CurrentEdgeDisposition::Pass
        } else {
            CurrentEdgeDisposition::Owned
        };
        if disposition == CurrentEdgeDisposition::Pass {
            match record.phase {
                PhysicalPhase::Down => {
                    if letter_bit != 0 && self.replay_visible_downs & letter_bit == 0 {
                        self.replay_visible_down_records[self.replay_visible_down_len] =
                            Some(record);
                        self.replay_visible_down_len += 1;
                    }
                    self.replay_visible_downs |= letter_bit;
                }
                PhysicalPhase::Up => {
                    self.replay_visible_downs &= !letter_bit;
                    if let Some(index) = self.replay_visible_down_records
                        [..self.replay_visible_down_len]
                        .iter()
                        .rposition(|visible| {
                            visible.is_some_and(|visible| visible.key == record.key)
                        })
                    {
                        self.replay_visible_down_records[index] = None;
                    }
                }
                PhysicalPhase::Repeat => {}
            }
        }
        disposition
    }

    fn visible_replay_cleanup(&self) -> CleanupBatch {
        CleanupBatch::from_visible_downs(
            &self.replay_visible_down_records[..self.replay_visible_down_len],
        )
    }

    fn classify_replay_event(
        &self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        let Some(observation) = self.replay_observation.as_ref() else {
            return GapBarrierObservation::Forged;
        };
        let Some(record) = observation.batch.record(observation.next) else {
            return GapBarrierObservation::Forged;
        };
        let expected_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if record.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let expected_repeat = record.phase == PhysicalPhase::Repeat;
        if event_type != expected_type
            || key_code != record.native.virtual_key
            || repeat != expected_repeat
            || flags != record.native.platform_flags
        {
            GapBarrierObservation::Forged
        } else if observation.next + 1 == observation.batch.len() {
            GapBarrierObservation::Complete
        } else {
            GapBarrierObservation::Down
        }
    }

    fn observe_replay_event(
        &mut self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        let classified = self.classify_replay_event(event_type, key_code, repeat, flags);
        if classified == GapBarrierObservation::Forged {
            return classified;
        }
        let observation = self
            .replay_observation
            .as_mut()
            .expect("classified replay observation remains installed");
        observation.next += 1;
        if classified == GapBarrierObservation::Complete {
            self.replay_observation = None;
        }
        classified
    }

    fn observe_gap_barrier_event(
        &mut self,
        event_type: u32,
        key_code: u16,
        repeat: bool,
        flags: u64,
    ) -> GapBarrierObservation {
        if !self.gap_barrier_pending || key_code != 127 || repeat || flags != 0 {
            return GapBarrierObservation::Forged;
        }
        match (self.gap_barrier_observed_down, event_type) {
            (false, ffi::K_CG_EVENT_KEY_DOWN) => {
                self.gap_barrier_observed_down = true;
                GapBarrierObservation::Down
            }
            (true, ffi::K_CG_EVENT_KEY_UP) => {
                self.clear_gap_tombstones();
                GapBarrierObservation::Complete
            }
            _ => GapBarrierObservation::Forged,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GapBarrierObservation {
    Down,
    Complete,
    Forged,
}

const RECOVERY_PHYSICAL_SOURCE_SLOTS: usize = 2;
const RECOVERY_EXTERNAL_SOURCE_SLOTS: usize = 8;
const RECOVERY_SOURCE_SLOTS: usize =
    RECOVERY_PHYSICAL_SOURCE_SLOTS + RECOVERY_EXTERNAL_SOURCE_SLOTS;
const OVERFLOW_BALANCE_CAPACITY: usize = RECOVERY_SOURCE_SLOTS * (128 + 32);

struct RecoveryEdgeJournal {
    pending: [injection::DeferredEvent; injection::DEFERRED_EDGE_CAPACITY],
    pending_len: usize,
    tail: [injection::DeferredEvent; injection::DEFERRED_EDGE_CAPACITY],
    tail_len: usize,
    overflow_balances: [injection::DeferredEvent; OVERFLOW_BALANCE_CAPACITY],
    overflow_balance_head: usize,
    overflow_balance_len: usize,
    submitted_len: usize,
    observed: usize,
    token: Option<injection::OperationToken>,
    submission_deadline: Option<Instant>,
    external_collection_deadline: Option<Instant>,
    next_pool_bank: usize,
    overflow: bool,
    external_source_pids: [i64; RECOVERY_EXTERNAL_SOURCE_SLOTS],
    modifier_bits: [u8; RECOVERY_SOURCE_SLOTS],
    modifier_known: [u8; RECOVERY_SOURCE_SLOTS],
    held_keys: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    key_origins: [[u8; 128]; RECOVERY_SOURCE_SLOTS],
    key_generations: [[u32; 128]; RECOVERY_SOURCE_SLOTS],
    owned_release_generation: [u32; 128],
    held_mouse_buttons: [u32; RECOVERY_SOURCE_SLOTS],
    mouse_foreground_origins: [u32; RECOVERY_SOURCE_SLOTS],
    exposed_keys: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    exposed_mouse_buttons: [u32; RECOVERY_SOURCE_SLOTS],
    discard_key_fences: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    discard_mouse_fences: [u32; RECOVERY_SOURCE_SLOTS],
}

impl Default for RecoveryEdgeJournal {
    fn default() -> Self {
        Self {
            pending: [injection::DeferredEvent::EMPTY; injection::DEFERRED_EDGE_CAPACITY],
            pending_len: 0,
            tail: [injection::DeferredEvent::EMPTY; injection::DEFERRED_EDGE_CAPACITY],
            tail_len: 0,
            overflow_balances: [injection::DeferredEvent::EMPTY; OVERFLOW_BALANCE_CAPACITY],
            overflow_balance_head: 0,
            overflow_balance_len: 0,
            submitted_len: 0,
            observed: 0,
            token: None,
            submission_deadline: None,
            external_collection_deadline: None,
            next_pool_bank: 0,
            overflow: false,
            external_source_pids: [0; RECOVERY_EXTERNAL_SOURCE_SLOTS],
            modifier_bits: [0; RECOVERY_SOURCE_SLOTS],
            modifier_known: [0; RECOVERY_SOURCE_SLOTS],
            held_keys: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            key_origins: [[0; 128]; RECOVERY_SOURCE_SLOTS],
            key_generations: [[0; 128]; RECOVERY_SOURCE_SLOTS],
            owned_release_generation: [0; 128],
            held_mouse_buttons: [0; RECOVERY_SOURCE_SLOTS],
            mouse_foreground_origins: [0; RECOVERY_SOURCE_SLOTS],
            exposed_keys: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            exposed_mouse_buttons: [0; RECOVERY_SOURCE_SLOTS],
            discard_key_fences: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            discard_mouse_fences: [0; RECOVERY_SOURCE_SLOTS],
        }
    }
}

impl RecoveryEdgeJournal {
    fn queued_ordering_pending(&self) -> bool {
        self.pending_len != 0
            || self.tail_len != 0
            || self.overflow_balance_len != 0
            || self.token.is_some()
            || self.overflow
    }

    fn physical_fences_pending(&self) -> bool {
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).any(|slot| {
            self.discard_key_fences[slot].iter().any(|fenced| *fenced)
                || self.discard_mouse_fences[slot] != 0
        })
    }

    fn reconcile_physical_fences(
        &mut self,
        mut key_is_down: impl FnMut(u16) -> bool,
        mut button_is_down: impl FnMut(u32) -> bool,
    ) {
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            for key_code in 0..128_u16 {
                if self.discard_key_fences[slot][usize::from(key_code)] && !key_is_down(key_code) {
                    self.discard_key_fences[slot][usize::from(key_code)] = false;
                }
            }
            for button in 0..32_u32 {
                let bit = 1_u32 << button;
                if self.discard_mouse_fences[slot] & bit != 0 && !button_is_down(button) {
                    self.discard_mouse_fences[slot] &= !bit;
                }
            }
        }
    }

    fn has_pending(&self) -> bool {
        self.queued_ordering_pending()
            || self.physical_fences_pending()
            || self
                .owned_release_generation
                .iter()
                .any(|value| *value != 0)
    }

    fn ordering_pending(&self) -> bool {
        self.queued_ordering_pending() || self.physical_fences_pending()
    }

    #[cfg(test)]
    fn hidden_balanced(&self) -> bool {
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).all(|slot| {
            self.key_origins[slot].iter().all(|origin| *origin != 1)
                && self.held_mouse_buttons[slot] & !self.mouse_foreground_origins[slot] == 0
        })
    }

    fn ready_len(&self) -> usize {
        if self.token.is_some() || self.pending_len == 0 {
            return 0;
        }
        let mut hidden_keys = [[false; 128]; RECOVERY_PHYSICAL_SOURCE_SLOTS];
        let mut hidden_mouse = [0_u32; RECOVERY_PHYSICAL_SOURCE_SLOTS];
        let mut ready = 0;
        for (index, edge) in self.pending[..self.pending_len].iter().copied().enumerate() {
            if edge.uncertain_owned {
                break;
            }
            let physical_slot = if edge.source == InputSource::Physical {
                Some(0)
            } else if edge.source.is_test_physical() {
                Some(1)
            } else {
                None
            };
            if edge.hidden_generation
                && let Some(slot) = physical_slot
            {
                if edge.is_mouse() {
                    let Ok(button) = u32::try_from(edge.mouse_button) else {
                        break;
                    };
                    let Some(bit) = 1_u32.checked_shl(button) else {
                        break;
                    };
                    if edge.is_down {
                        hidden_mouse[slot] |= bit;
                    } else {
                        hidden_mouse[slot] &= !bit;
                    }
                } else if !edge.repeat {
                    hidden_keys[slot][usize::from(edge.key_code)] = edge.is_down;
                }
            }
            if hidden_keys.iter().flatten().all(|held| !*held)
                && hidden_mouse.iter().all(|held| *held == 0)
            {
                ready = index + 1;
            }
        }
        ready
    }

    fn external_pid_referenced(&self, source_pid: i64) -> bool {
        self.pending[..self.pending_len]
            .iter()
            .chain(&self.tail[..self.tail_len])
            .any(|edge| edge.source == InputSource::External && edge.source_pid == source_pid)
            || (0..self.overflow_balance_len).any(|offset| {
                let index = (self.overflow_balance_head + offset) % self.overflow_balances.len();
                let edge = self.overflow_balances[index];
                edge.source == InputSource::External && edge.source_pid == source_pid
            })
    }

    fn external_slot_pinned(&self, index: usize) -> bool {
        let slot = RECOVERY_PHYSICAL_SOURCE_SLOTS + index;
        let pid = self.external_source_pids[index];
        pid != 0
            && (self.external_pid_referenced(pid)
                || self.modifier_bits[slot] != 0
                || self.held_keys[slot].iter().any(|held| *held)
                || self.held_mouse_buttons[slot] != 0
                || self.exposed_keys[slot].iter().any(|held| *held)
                || self.exposed_mouse_buttons[slot] != 0
                || self.discard_key_fences[slot].iter().any(|fenced| *fenced)
                || self.discard_mouse_fences[slot] != 0)
    }

    #[allow(unreachable_patterns)]
    fn source_slot(&mut self, source: InputSource, source_pid: i64) -> Option<usize> {
        if source.is_test_physical() {
            return Some(1);
        }
        match source {
            InputSource::Physical => Some(0),
            InputSource::External => {
                if let Some(index) = self
                    .external_source_pids
                    .iter()
                    .position(|pid| *pid == source_pid && *pid != 0)
                {
                    return Some(RECOVERY_PHYSICAL_SOURCE_SLOTS + index);
                }
                let index = self
                    .external_source_pids
                    .iter()
                    .position(|pid| *pid == 0)
                    .or_else(|| {
                        (0..RECOVERY_EXTERNAL_SOURCE_SLOTS)
                            .find(|index| !self.external_slot_pinned(*index))
                    })?;
                let slot = RECOVERY_PHYSICAL_SOURCE_SLOTS + index;
                self.external_source_pids[index] = source_pid;
                self.modifier_bits[slot] = 0;
                self.modifier_known[slot] = 0;
                self.held_keys[slot] = [false; 128];
                self.key_origins[slot] = [0; 128];
                self.key_generations[slot] = [0; 128];
                self.held_mouse_buttons[slot] = 0;
                self.mouse_foreground_origins[slot] = 0;
                self.exposed_keys[slot] = [false; 128];
                self.exposed_mouse_buttons[slot] = 0;
                self.discard_key_fences[slot] = [false; 128];
                self.discard_mouse_fences[slot] = 0;
                Some(slot)
            }
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy => None,
            _ => None,
        }
    }

    #[allow(unreachable_patterns)]
    fn existing_source_slot(&self, source: InputSource, source_pid: i64) -> Option<usize> {
        if source.is_test_physical() {
            return Some(1);
        }
        match source {
            InputSource::Physical => Some(0),
            InputSource::External => self
                .external_source_pids
                .iter()
                .position(|pid| *pid == source_pid && *pid != 0)
                .map(|index| RECOVERY_PHYSICAL_SOURCE_SLOTS + index),
            InputSource::HelperReplay | InputSource::HelperPaste | InputSource::HelperDummy => None,
            _ => None,
        }
    }

    fn seed_modifier_side(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
    ) {
        let Some(side) = modifier_side_for_key_code(key_code) else {
            return;
        };
        let Some(slot) = self.source_slot(source, source_pid) else {
            return;
        };
        let bit = side.bit();
        if self.modifier_known[slot] & bit != 0 {
            return;
        }
        self.modifier_known[slot] |= bit;
        if is_down {
            self.modifier_bits[slot] |= bit;
        }
    }

    fn modifier_side_transition(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        physical_is_down: Option<bool>,
    ) -> Option<(bool, bool)> {
        let side = modifier_side_for_key_code(key_code)?;
        let slot = self.source_slot(source, source_pid)?;
        let bit = side.bit();
        let was_down = self.modifier_bits[slot] & bit != 0;
        let is_down = if source == InputSource::Physical {
            physical_is_down?
        } else {
            // Test-physical and external streams have no authoritative HID
            // state. Their bounded, source-specific side model advances only
            // the exact keycode named by this flagsChanged edge.
            !was_down
        };
        self.modifier_known[slot] |= bit;
        if is_down {
            self.modifier_bits[slot] |= bit;
        } else {
            self.modifier_bits[slot] &= !bit;
        }
        Some((is_down, was_down))
    }

    fn observe_normal_external_modifier(&mut self, source_pid: i64, key_code: u16) -> Option<bool> {
        self.modifier_side_transition(InputSource::External, source_pid, key_code, None)
            .map(|(is_down, _)| is_down)
    }

    fn key_was_down(&self, source: InputSource, source_pid: i64, key_code: u16) -> bool {
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.held_keys[slot][usize::from(key_code)])
    }

    fn key_is_discard_fenced(&self, source: InputSource, source_pid: i64, key_code: u16) -> bool {
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.discard_key_fences[slot][usize::from(key_code)])
    }

    fn mouse_is_discard_fenced(&self, source: InputSource, source_pid: i64, button: u32) -> bool {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return false;
        };
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.discard_mouse_fences[slot] & bit != 0)
    }

    fn consume_key_discard_fence(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
    ) {
        if !is_down && let Some(slot) = self.existing_source_slot(source, source_pid) {
            self.discard_key_fences[slot][usize::from(key_code)] = false;
        }
    }

    fn consume_mouse_discard_fence(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
    ) {
        if is_down {
            return;
        }
        let Some(bit) = 1_u32.checked_shl(button) else {
            return;
        };
        if let Some(slot) = self.existing_source_slot(source, source_pid) {
            self.discard_mouse_fences[slot] &= !bit;
        }
    }

    fn observe_key_phase(
        &mut self,
        source: InputSource,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
        repeat: bool,
        foreground_was_down: bool,
    ) -> Option<(u32, bool, bool)> {
        let index = usize::from(key_code);
        let slot = self.source_slot(source, source_pid)?;
        let generation = &mut self.key_generations[slot][index];
        if *generation == 0 {
            *generation = 1;
        }
        if self.key_origins[slot][index] == 0 && foreground_was_down {
            self.key_origins[slot][index] = 2;
            self.held_keys[slot][index] = true;
        }
        let mut foreground_balance = false;
        let hidden_generation;
        if is_down && !repeat {
            if !self.held_keys[slot][index] {
                *generation = generation.checked_add(1).unwrap_or(u32::MAX);
                self.key_origins[slot][index] = 1;
            }
            self.held_keys[slot][index] = true;
            hidden_generation = self.key_origins[slot][index] == 1;
        } else if repeat {
            if !self.held_keys[slot][index] {
                self.key_origins[slot][index] = 2;
            }
            self.held_keys[slot][index] = true;
            hidden_generation = self.key_origins[slot][index] == 1;
        } else {
            hidden_generation = self.key_origins[slot][index] == 1;
            foreground_balance = self.key_origins[slot][index] == 2
                || (self.key_origins[slot][index] == 0 && foreground_was_down);
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
        Some((*generation, foreground_balance, hidden_generation))
    }

    fn observe_mouse_phase(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
        foreground_was_down: bool,
    ) -> Option<(bool, bool)> {
        let bit = 1_u32.checked_shl(button)?;
        let slot = self.source_slot(source, source_pid)?;
        if foreground_was_down && self.held_mouse_buttons[slot] & bit == 0 {
            self.held_mouse_buttons[slot] |= bit;
            self.mouse_foreground_origins[slot] |= bit;
        }
        let foreground_balance;
        let hidden_generation;
        if is_down {
            if self.held_mouse_buttons[slot] & bit == 0 {
                self.mouse_foreground_origins[slot] &= !bit;
            }
            self.held_mouse_buttons[slot] |= bit;
            foreground_balance = false;
            hidden_generation = self.mouse_foreground_origins[slot] & bit == 0;
        } else {
            hidden_generation = self.held_mouse_buttons[slot] & bit != 0
                && self.mouse_foreground_origins[slot] & bit == 0;
            foreground_balance = self.mouse_foreground_origins[slot] & bit != 0
                || (self.held_mouse_buttons[slot] & bit == 0 && foreground_was_down);
            self.held_mouse_buttons[slot] &= !bit;
            self.mouse_foreground_origins[slot] &= !bit;
        }
        Some((foreground_balance, hidden_generation))
    }

    fn normal_external_key_transition(
        &mut self,
        source_pid: i64,
        key_code: u16,
        is_down: bool,
        repeat: bool,
    ) {
        let Some(slot) = self.source_slot(InputSource::External, source_pid) else {
            return;
        };
        let index = usize::from(key_code);
        if is_down {
            if !repeat && !self.held_keys[slot][index] {
                self.key_generations[slot][index] =
                    self.key_generations[slot][index].saturating_add(1).max(1);
            }
            self.held_keys[slot][index] = true;
            self.key_origins[slot][index] = 2;
        } else {
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
    }

    fn normal_mouse_transition(
        &mut self,
        source: InputSource,
        source_pid: i64,
        button: u32,
        is_down: bool,
    ) {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return;
        };
        let Some(slot) = self.source_slot(source, source_pid) else {
            return;
        };
        if is_down {
            self.held_mouse_buttons[slot] |= bit;
            self.mouse_foreground_origins[slot] |= bit;
        } else {
            self.held_mouse_buttons[slot] &= !bit;
            self.mouse_foreground_origins[slot] &= !bit;
        }
    }

    fn foreground_mouse_was_down(&self, source: InputSource, source_pid: i64, button: u32) -> bool {
        let Some(bit) = 1_u32.checked_shl(button) else {
            return false;
        };
        self.existing_source_slot(source, source_pid)
            .is_some_and(|slot| self.held_mouse_buttons[slot] & bit != 0)
    }

    fn record_owned_release_at(&mut self, key_code: u16, generation: u32) {
        let index = usize::from(key_code);
        let generation = generation.max(1);
        self.owned_release_generation[index] = generation;
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            self.key_generations[slot][index] = self.key_generations[slot][index].max(generation);
            self.held_keys[slot][index] = false;
            self.key_origins[slot][index] = 0;
        }
    }

    fn record_owned_release(&mut self, key_code: u16) {
        let index = usize::from(key_code);
        let generation = self.key_generations[0][index]
            .max(self.key_generations[1][index])
            .max(1);
        self.record_owned_release_at(key_code, generation);
    }

    fn owned_release_recorded(&self, key_code: u16) -> bool {
        self.owned_release_generation[usize::from(key_code)] != 0
    }

    fn force_old_generation_released(&self, key_code: u16) -> bool {
        let index = usize::from(key_code);
        let released = self.owned_release_generation[index];
        if released == 0 {
            return false;
        }
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            if self.held_keys[slot][index] && self.key_generations[slot][index] > released {
                return false;
            }
        }
        true
    }

    fn physical_newer_generation_held(&self, key_code: u16) -> bool {
        let index = usize::from(key_code);
        let released = self.owned_release_generation[index];
        (0..RECOVERY_PHYSICAL_SOURCE_SLOTS).any(|slot| {
            self.held_keys[slot][index]
                && (released == 0 || self.key_generations[slot][index] > released)
        })
    }

    fn released_letter_bits(&self) -> u32 {
        LETTER_KEY_CODES
            .iter()
            .enumerate()
            .fold(0_u32, |bits, (index, key_code)| {
                bits | if self.owned_release_recorded(*key_code) {
                    1_u32 << index
                } else {
                    0
                }
            })
    }

    fn clear_owned_releases(&mut self) {
        self.owned_release_generation = [0; 128];
    }

    fn resolve_uncertain_ownership(&mut self, keyboard: &mut CallbackKeyboard) {
        if self.submitted_len != 0 {
            return;
        }
        let mut retained = 0;
        for index in 0..self.pending_len {
            let mut edge = self.pending[index];
            let physical = edge.source.is_physical();
            let transactional_owned = physical
                && matches!(map_key_code(edge.key_code), PhysicalKey::Letter(letter)
                    if keyboard.transactional.owned_letters()
                        & (1_u32 << u32::from(letter.index())) != 0);
            let session_owned = physical
                && match map_key_code(edge.key_code) {
                    PhysicalKey::Escape => keyboard.session_escape_native_owned,
                    PhysicalKey::Enter => {
                        keyboard.session_enter_native_owned == Some(edge.key_code)
                    }
                    PhysicalKey::Letter(_) | PhysicalKey::Other => false,
                };
            let belongs_to_old_transaction = transactional_owned
                && !(self.owned_release_recorded(edge.key_code)
                    && edge.generation > self.owned_release_generation[usize::from(edge.key_code)]);
            if edge.uncertain_owned && (belongs_to_old_transaction || session_owned) {
                if edge.event_type == ffi::K_CG_EVENT_KEY_UP {
                    if belongs_to_old_transaction {
                        self.record_owned_release_at(edge.key_code, edge.generation);
                    }
                    if session_owned {
                        match map_key_code(edge.key_code) {
                            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
                            PhysicalKey::Enter => {
                                keyboard.session_enter_native_owned = None;
                                keyboard.captured_enter_key_code = None;
                            }
                            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
                        }
                    }
                }
                continue;
            }
            edge.uncertain_owned = false;
            self.pending[retained] = edge;
            retained += 1;
        }
        for entry in &mut self.pending[retained..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = retained;
        let _ = keyboard.reducer.reconcile_hidden_session_releases(
            keyboard.session_escape_native_owned,
            keyboard.session_enter_native_owned.is_some(),
        );
    }

    fn retain_overflow_edge(&mut self, edge: injection::DeferredEvent) {
        if (edge.source != InputSource::External && !edge.foreground_balance)
            || self.overflow_balance_len == self.overflow_balances.len()
        {
            return;
        }
        let index =
            (self.overflow_balance_head + self.overflow_balance_len) % self.overflow_balances.len();
        self.overflow_balances[index] = edge;
        self.overflow_balance_len += 1;
    }

    fn finalize_discarded_hidden_phases(&mut self) {
        for slot in 0..RECOVERY_PHYSICAL_SOURCE_SLOTS {
            for key in 0..128 {
                if self.key_origins[slot][key] == 1 && self.held_keys[slot][key] {
                    self.discard_key_fences[slot][key] = true;
                    self.held_keys[slot][key] = false;
                    self.key_origins[slot][key] = 0;
                }
            }
            let hidden_mouse = self.held_mouse_buttons[slot] & !self.mouse_foreground_origins[slot];
            self.discard_mouse_fences[slot] |= hidden_mouse;
            self.held_mouse_buttons[slot] &= !hidden_mouse;
        }
    }

    fn enter_overflow(&mut self) {
        if self.overflow {
            return;
        }
        self.overflow = true;
        if self.token.is_none() {
            for index in 0..self.pending_len {
                self.retain_overflow_edge(self.pending[index]);
                self.pending[index] = injection::DeferredEvent::EMPTY;
            }
            self.pending_len = 0;
        }
        for index in 0..self.tail_len {
            self.retain_overflow_edge(self.tail[index]);
            self.tail[index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len = 0;
        self.finalize_discarded_hidden_phases();
    }

    fn observe_foreground_exposure(&mut self, edge: injection::DeferredEvent) {
        let Some(slot) = self.source_slot(edge.source, edge.source_pid) else {
            return;
        };
        if edge.is_mouse() {
            let Ok(button) = u32::try_from(edge.mouse_button) else {
                return;
            };
            let Some(bit) = 1_u32.checked_shl(button) else {
                return;
            };
            if edge.is_down {
                self.exposed_mouse_buttons[slot] |= bit;
            } else {
                self.exposed_mouse_buttons[slot] &= !bit;
            }
        } else {
            self.exposed_keys[slot][usize::from(edge.key_code)] = edge.is_down;
        }
    }

    fn retain_abort_balance(&mut self, mut edge: injection::DeferredEvent) {
        let Some(slot) = self.existing_source_slot(edge.source, edge.source_pid) else {
            self.retain_overflow_edge(edge);
            return;
        };
        if edge.is_mouse() {
            let Ok(button) = u32::try_from(edge.mouse_button) else {
                return;
            };
            let Some(bit) = 1_u32.checked_shl(button) else {
                return;
            };
            if !edge.is_down && self.exposed_mouse_buttons[slot] & bit != 0 {
                edge.foreground_balance = true;
                self.exposed_mouse_buttons[slot] &= !bit;
            }
        } else if !edge.is_down && self.exposed_keys[slot][usize::from(edge.key_code)] {
            edge.foreground_balance = true;
            self.exposed_keys[slot][usize::from(edge.key_code)] = false;
        }
        self.retain_overflow_edge(edge);
    }

    fn abort_submission_to_overflow(&mut self) {
        self.overflow = true;
        for index in self.observed..self.submitted_len {
            self.retain_abort_balance(self.pending[index]);
        }
        for entry in &mut self.pending[..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = 0;
        self.submitted_len = 0;
        self.observed = 0;
        self.token = None;
        self.submission_deadline = None;
        for index in 0..self.tail_len {
            self.retain_overflow_edge(self.tail[index]);
            self.tail[index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len = 0;
        self.refresh_external_collection_deadline();
    }

    fn refresh_external_collection_deadline(&mut self) {
        let has_external = self.pending[..self.pending_len]
            .iter()
            .chain(&self.tail[..self.tail_len])
            .any(|edge| edge.source == InputSource::External)
            || (0..self.overflow_balance_len).any(|offset| {
                let index = (self.overflow_balance_head + offset) % self.overflow_balances.len();
                self.overflow_balances[index].source == InputSource::External
            });
        if has_external {
            self.external_collection_deadline
                .get_or_insert_with(|| Instant::now() + EXTERNAL_DEFERRED_COLLECTION_TIMEOUT);
        } else {
            self.external_collection_deadline = None;
        }
    }

    fn expire_external_collection(&mut self, now: Instant) -> bool {
        if self.token.is_some()
            || !self
                .external_collection_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            return false;
        }
        self.enter_overflow();
        self.materialize_overflow_batch();
        true
    }

    fn append(&mut self, edge: injection::DeferredEvent) -> bool {
        if edge.source == InputSource::External && self.external_collection_deadline.is_none() {
            self.external_collection_deadline =
                Some(Instant::now() + EXTERNAL_DEFERRED_COLLECTION_TIMEOUT);
        }
        if self.overflow {
            self.retain_overflow_edge(edge);
            self.materialize_overflow_batch();
            return false;
        }
        let (entries, len) = if self.token.is_some() {
            (&mut self.tail, &mut self.tail_len)
        } else {
            (&mut self.pending, &mut self.pending_len)
        };
        if *len == entries.len() {
            self.enter_overflow();
            self.retain_overflow_edge(edge);
            self.materialize_overflow_batch();
            return false;
        }
        entries[*len] = edge;
        *len += 1;
        true
    }

    fn materialize_overflow_batch(&mut self) {
        if !self.overflow || self.token.is_some() || self.pending_len != 0 {
            return;
        }
        self.finalize_discarded_hidden_phases();
        let count = self
            .overflow_balance_len
            .min(injection::DEFERRED_EDGE_CAPACITY);
        for index in 0..count {
            let balance_index = (self.overflow_balance_head + index) % self.overflow_balances.len();
            self.pending[index] = self.overflow_balances[balance_index];
            self.overflow_balances[balance_index] = injection::DeferredEvent::EMPTY;
        }
        self.overflow_balance_head =
            (self.overflow_balance_head + count) % self.overflow_balances.len();
        self.overflow_balance_len -= count;
        self.pending_len = count;
        if count == 0 {
            self.overflow = false;
        }
        self.refresh_external_collection_deadline();
    }

    fn ready_slice(&self) -> Option<&[injection::DeferredEvent]> {
        let ready = self.ready_len();
        (ready != 0).then_some(&self.pending[..ready])
    }

    fn begin_submission(
        &mut self,
        token: injection::OperationToken,
        bank: usize,
        count: usize,
    ) -> bool {
        if self.token.is_some() || count == 0 || count > self.pending_len {
            return false;
        }
        let suffix = self.pending_len - count;
        if self.tail_len + suffix > self.tail.len() {
            return false;
        }
        for index in 0..suffix {
            self.tail[self.tail_len + index] = self.pending[count + index];
            self.pending[count + index] = injection::DeferredEvent::EMPTY;
        }
        self.tail_len += suffix;
        self.pending_len = count;
        self.submitted_len = count;
        self.observed = 0;
        self.token = Some(token);
        self.next_pool_bank = (bank + 1) % injection::DEFERRED_POOL_BANKS;
        self.submission_deadline = Some(Instant::now() + Duration::from_millis(250));
        self.refresh_external_collection_deadline();
        true
    }

    fn expected(&self) -> Option<injection::DeferredEvent> {
        self.token
            .and_then(|_| self.pending.get(self.observed).copied())
            .filter(|_| self.observed < self.submitted_len)
    }

    fn advance_observation(&mut self) -> GapBarrierObservation {
        if self.token.is_none() || self.observed >= self.submitted_len {
            return GapBarrierObservation::Forged;
        }
        self.observed += 1;
        if self.observed != self.submitted_len {
            return GapBarrierObservation::Down;
        }
        for entry in &mut self.pending[..self.pending_len] {
            *entry = injection::DeferredEvent::EMPTY;
        }
        self.pending_len = 0;
        self.submitted_len = 0;
        self.observed = 0;
        self.token = None;
        self.submission_deadline = None;
        if self.overflow {
            self.materialize_overflow_batch();
        } else {
            for index in 0..self.tail_len {
                self.pending[index] = self.tail[index];
                self.tail[index] = injection::DeferredEvent::EMPTY;
            }
            self.pending_len = self.tail_len;
            self.tail_len = 0;
        }
        self.refresh_external_collection_deadline();
        GapBarrierObservation::Complete
    }

    fn settle_overflow(&mut self) {
        self.materialize_overflow_batch();
    }
}
fn classify_exact_pair(
    state: u8,
    event_type: u32,
    key_code: u16,
    expected_key_code: u16,
    repeat: bool,
    flags: u64,
    expected_flags: u64,
) -> GapBarrierObservation {
    if key_code != expected_key_code || repeat || flags != expected_flags {
        return GapBarrierObservation::Forged;
    }
    match (state, event_type) {
        (0, ffi::K_CG_EVENT_KEY_DOWN) => GapBarrierObservation::Down,
        (1, ffi::K_CG_EVENT_KEY_UP) => GapBarrierObservation::Complete,
        _ => GapBarrierObservation::Forged,
    }
}

fn observe_exact_pair(
    state: &mut u8,
    event_type: u32,
    key_code: u16,
    expected_key_code: u16,
    repeat: bool,
    flags: u64,
    expected_flags: u64,
) -> GapBarrierObservation {
    let observation = classify_exact_pair(
        *state,
        event_type,
        key_code,
        expected_key_code,
        repeat,
        flags,
        expected_flags,
    );
    match observation {
        GapBarrierObservation::Down => *state = 1,
        GapBarrierObservation::Complete => *state = 2,
        GapBarrierObservation::Forged => {}
    }
    observation
}

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

const fn event_tap_options(suppression_enabled: bool) -> u32 {
    if suppression_enabled {
        ffi::K_CG_EVENT_TAP_OPTION_DEFAULT
    } else {
        ffi::K_CG_EVENT_TAP_OPTION_LISTEN_ONLY
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start_hook(
    state: Arc<SharedState>,
    owner_commands: Receiver<OwnerCommand>,
    paste_commands: Receiver<PasteCommand>,
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
    suppression_enabled: bool,
) -> Result<(JoinHandle<()>, Receiver<()>), PlatformError> {
    let stop_state = Arc::clone(&state);
    let stop_gate = Arc::clone(&gate);
    let context = CallbackContext {
        state,
        suppression_enabled,
        keyboard: RecoveringMutex::new(CallbackKeyboard::default()),
        pending_activation: RecoveringMutex::new(None),
        native_events: RecoveringMutex::new(None),
        injection_identity: None,
        test_physical_seam_enabled: cfg!(feature = "transactional-shortcuts-dev")
            && std::env::var_os("TALKING_QUILL_MACOS_TEST_PHYSICAL_SEAM").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        callback_proxy: AtomicPtr::new(null_mut()),
        current_edge_disposition: AtomicU8::new(CurrentEdgeDisposition::Pass as u8),
        owner_commands,
        paste_commands,
        pending_paste: RecoveringMutex::new(None),
        recovery_edges: RecoveringMutex::new(RecoveryEdgeJournal::default()),
        target_cache: None,
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_tap_disable_request: std::env::var_os("TALKING_QUILL_MACOS_TEST_TAP_DISABLE_REQUEST")
            .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_paused: std::env::var_os(
            "TALKING_QUILL_MACOS_TEST_PASTE_BARRIER_PAUSED",
        )
        .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_release: std::env::var_os(
            "TALKING_QUILL_MACOS_TEST_PASTE_BARRIER_RELEASE",
        )
        .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_split_active: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_down_observed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_pause_announced: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        forced_activation_reservation: None,
        outbound,
        gate,
        terminal,
        observability,
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
        Ok(Ok(())) => Ok((thread, owner_completion)),
        Ok(Err(error)) => {
            stop_gate.close();
            stop_state.quiescing.store(true, Ordering::Release);
            stop_state.stopping.store(true, Ordering::Release);
            if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) && thread.is_finished()
            {
                let _ = thread.join();
            } else {
                drop(thread);
            }
            Err(error)
        }
        Err(_) => {
            stop_gate.close();
            stop_state.quiescing.store(true, Ordering::Release);
            stop_state.stopping.store(true, Ordering::Release);
            if cancel_startup(&startup_state) == StartupState::Running {
                let _ = ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT);
            }
            request_stop(&stop_state);
            if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) && thread.is_finished()
            {
                let _ = thread.join();
            } else {
                drop(thread);
            }
            Err(PlatformError::ThreadStopped)
        }
    }
}

pub(super) fn request_stop(state: &Arc<SharedState>) -> bool {
    state.quiescing.store(true, Ordering::Release);
    state.stopping.store(true, Ordering::Release);
    state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);

    let source = state.owner_command_source.load(Ordering::Acquire);
    let timer = state.owner_wake_timer.load(Ordering::Acquire);
    let run_loop = state.owner_run_loop.load(Ordering::Acquire);
    // SAFETY: all three objects are permanent owner-startup resources,
    // published before Ready and cleared only after pending native work drains.
    // Signalling and wake-up are thread-safe and require no lock/allocation.
    unsafe {
        if !source.is_null() {
            ffi::CFRunLoopSourceSignal(source);
        }
        if !timer.is_null() {
            ffi::CFRunLoopTimerSetNextFireDate(
                timer,
                ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
            );
        }
        if !run_loop.is_null() {
            ffi::CFRunLoopWakeUp(run_loop);
        }
    }
    !run_loop.is_null() && (!source.is_null() || !timer.is_null())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerReleaseStep {
    StopAx,
    DisableTap,
    RemoveTapSource,
    RemoveCommandSource,
    RemoveTimer,
    InvalidateCommandSource,
    InvalidateTimer,
    InvalidateTap,
    DropTargetCache,
    ReleaseTimer,
    ReleaseCommandSource,
    ReleaseTapSource,
    ReleaseTap,
    DropNativePool,
}

const OWNER_RELEASE_ORDER: [OwnerReleaseStep; 14] = [
    OwnerReleaseStep::StopAx,
    OwnerReleaseStep::DisableTap,
    OwnerReleaseStep::RemoveTapSource,
    OwnerReleaseStep::RemoveCommandSource,
    OwnerReleaseStep::RemoveTimer,
    OwnerReleaseStep::InvalidateCommandSource,
    OwnerReleaseStep::InvalidateTimer,
    OwnerReleaseStep::InvalidateTap,
    OwnerReleaseStep::DropTargetCache,
    OwnerReleaseStep::ReleaseTimer,
    OwnerReleaseStep::ReleaseCommandSource,
    OwnerReleaseStep::ReleaseTapSource,
    OwnerReleaseStep::ReleaseTap,
    OwnerReleaseStep::DropNativePool,
];

struct OwnerNativeResourceGuard {
    context: *mut CallbackContext,
    run_loop: ffi::CFRunLoopRef,
    tap: ffi::CFMachPortRef,
    tap_source: ffi::CFRunLoopSourceRef,
    command_source: ffi::CFRunLoopSourceRef,
    maintenance_timer: ffi::CFRunLoopTimerRef,
}

impl Drop for OwnerNativeResourceGuard {
    fn drop(&mut self) {
        // SAFETY: this guard is created and dropped on the run-loop owner. Its
        // context outlives the guard, and the tested order drives the actual
        // operations, keeping callback refcon/pool alive through invalidation.
        let context = unsafe { &mut *self.context };
        context.gate.close();
        context.state.event_tap.store(null_mut(), Ordering::Release);
        context
            .state
            .maintenance_timer
            .store(null_mut(), Ordering::Release);
        for step in OWNER_RELEASE_ORDER {
            match step {
                OwnerReleaseStep::StopAx => {
                    if let Some(cache) = context.target_cache.as_ref() {
                        cache.request_stop();
                    }
                }
                OwnerReleaseStep::DisableTap => unsafe {
                    ffi::CGEventTapEnable(self.tap, false);
                },
                OwnerReleaseStep::RemoveTapSource => unsafe {
                    ffi::CFRunLoopRemoveSource(
                        self.run_loop,
                        self.tap_source,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::RemoveCommandSource => unsafe {
                    ffi::CFRunLoopRemoveSource(
                        self.run_loop,
                        self.command_source,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::RemoveTimer => unsafe {
                    ffi::CFRunLoopRemoveTimer(
                        self.run_loop,
                        self.maintenance_timer,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::InvalidateCommandSource => unsafe {
                    ffi::CFRunLoopSourceInvalidate(self.command_source);
                },
                OwnerReleaseStep::InvalidateTimer => unsafe {
                    ffi::CFRunLoopTimerInvalidate(self.maintenance_timer);
                },
                OwnerReleaseStep::InvalidateTap => unsafe {
                    ffi::CFMachPortInvalidate(self.tap);
                },
                OwnerReleaseStep::DropTargetCache => drop(context.target_cache.take()),
                OwnerReleaseStep::ReleaseTimer => unsafe {
                    ffi::CFRelease(self.maintenance_timer.cast_const());
                },
                OwnerReleaseStep::ReleaseCommandSource => unsafe {
                    ffi::CFRelease(self.command_source.cast_const());
                },
                OwnerReleaseStep::ReleaseTapSource => unsafe {
                    ffi::CFRelease(self.tap_source.cast_const());
                },
                OwnerReleaseStep::ReleaseTap => unsafe {
                    ffi::CFRelease(self.tap.cast_const());
                },
                OwnerReleaseStep::DropNativePool => {
                    let native_events = context
                        .native_events
                        .get_mut()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    drop(native_events.take());
                }
            }
        }
        context
            .state
            .hook_status
            .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    }
}

fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<(), PlatformError>>,
    startup_state: Arc<AtomicU8>,
    owner_completion: Sender<()>,
) {
    // Declared first so cleanup completion is published only after all owner
    // resources and callback context have been dropped.
    let _owner_completion = OwnerCompletion(owner_completion);
    let mut context = Box::new(context);
    let injection_identity = match injection::InjectionIdentity::new() {
        Ok(identity) => identity,
        Err(error) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(error));
            return;
        }
    };
    context.injection_identity = Some(injection_identity);
    let native_events = match injection::NativeEventPool::new(injection_identity) {
        Ok(pool) => pool,
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(PlatformError::NativeFailure));
            return;
        }
    };
    match context.native_events.get_mut() {
        Ok(slot) => *slot = Some(native_events),
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(PlatformError::NativeFailure));
            return;
        }
    }
    let callback_context = (&raw mut *context).cast::<c_void>();
    let mask = (1_u64 << ffi::K_CG_EVENT_LEFT_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_LEFT_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_RIGHT_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_OTHER_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_OTHER_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_KEY_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_KEY_UP)
        | (1_u64 << ffi::K_CG_EVENT_FLAGS_CHANGED);
    // SAFETY: callback context remains boxed until every source and tap has
    // been removed, invalidated, and released below.
    let tap = unsafe {
        ffi::CGEventTapCreate(
            ffi::K_CG_SESSION_EVENT_TAP,
            ffi::K_CG_HEAD_INSERT_EVENT_TAP,
            event_tap_options(context.suppression_enabled),
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
        let _ = ready.send(Ok(()));
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
        let _ = ready.send(Ok(()));
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
        // SAFETY: tap sources and taps are owned and no run loop references them.
        unsafe {
            ffi::CFRelease(tap_source.cast_const());
        }
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }

    let mut timer_context = ffi::CFRunLoopTimerContext {
        version: 0,
        info: callback_context,
        retain: None,
        release: None,
        copy_description: None,
    };
    // SAFETY: Core Foundation copies the context and retains no ownership of
    // the boxed callback pointer. The first fire is parked until paste or
    // shutdown work explicitly arms it.
    let maintenance_timer = unsafe {
        ffi::CFRunLoopTimerCreate(
            null(),
            ffi::CFAbsoluteTimeGetCurrent() + PARKED_TIMER_SECONDS,
            MAINTENANCE_INTERVAL_SECONDS,
            0,
            0,
            Some(maintenance_timer_callback),
            &raw mut timer_context,
        )
    };
    if maintenance_timer.is_null() {
        // SAFETY: neither source is installed in a run loop yet.
        unsafe {
            ffi::CFRunLoopSourceInvalidate(command_source);
            ffi::CFRelease(command_source.cast_const());
            ffi::CFRelease(tap_source.cast_const());
        }
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }
    context
        .state
        .maintenance_timer
        .store(maintenance_timer, Ordering::Release);

    // SAFETY: called on the future run-loop owner thread.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    // SAFETY: all objects are valid and remain alive through CFRunLoopRun.
    unsafe {
        ffi::CFRunLoopAddSource(run_loop, tap_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopAddSource(run_loop, command_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopAddTimer(run_loop, maintenance_timer, ffi::kCFRunLoopCommonModes);
    }
    context.target_cache = TargetCache::start().ok();
    if let Ok(keyboard) = context.keyboard.get_mut() {
        keyboard.seed_from_state(native_key_is_down);
        keyboard.transactional = TransactionEngine::with_physical_snapshot(
            CompiledActivationConfig::default(),
            physical_snapshot(keyboard),
        )
        .with_menu_neutralization_policy(MenuNeutralizationPolicy::NotRequired);
        keyboard.dispatcher.initialize_process_epoch();
    } else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
    }
    // Retain one permanent wake reference to each startup resource. The owner
    // may invalidate its operational references after drain, but request_stop
    // can always signal valid CF objects without a lock/use-after-free race.
    unsafe {
        ffi::CFRetain(run_loop.cast_const());
        ffi::CFRetain(command_source.cast_const());
        ffi::CFRetain(maintenance_timer.cast_const());
    }
    context
        .state
        .owner_run_loop
        .store(run_loop, Ordering::Release);
    context
        .state
        .owner_command_source
        .store(command_source, Ordering::Release);
    context
        .state
        .owner_wake_timer
        .store(maintenance_timer, Ordering::Release);
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_tap_disable_request.is_some() {
        unsafe {
            ffi::CFRunLoopTimerSetNextFireDate(
                maintenance_timer,
                ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
            );
        }
    }
    let _resources = OwnerNativeResourceGuard {
        context: (&raw mut *context),
        run_loop,
        tap,
        tap_source,
        command_source,
        maintenance_timer,
    };
    // The AX cache worker and process epoch are initialized before the tap can
    // invoke the latency-sensitive callback. Catch every owner-loop unwind
    // while the RAII guard still owns a valid callback refcon and resources.
    let owner_lifecycle = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe {
            ffi::CGEventTapEnable(tap, true);
        };
        if claim_startup(&startup_state) {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::InstalledUnobserved),
                Ordering::Release,
            );
            if ready.send(Ok(())).is_ok() && !context.state.stopping.load(Ordering::Acquire) {
                unsafe { ffi::CFRunLoopRun() };
            } else {
                context.gate.close();
            }
        } else {
            context.gate.close();
        }

        while pending_native_work(&context) {
            arm_maintenance_timer(&context);
            unsafe { ffi::CFRunLoopRun() };
        }
    }));
    if owner_lifecycle.is_err() {
        enter_owner_lifecycle_recovery(&context);
        // A second owner/recovery unwind is contained and retried using the
        // permanent timer/source. Resources stay owned until drain proves safe.
        while pending_native_work(&context) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Recheck after every attempt. A failed attempt may have a
                // submitted effect in flight; it must not fall through into a
                // second control/shutdown turn.
                let recovered = attempt_pending_recovery(&context);
                if recovered {
                    begin_owner_shutdown(&context);
                }
                arm_maintenance_timer(&context);
                unsafe { ffi::CFRunLoopRun() };
            }));
        }
    }

    context.gate.close();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    while let Ok(command) = context.owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    let _ = resolve_pending_activation(&context, PendingActivationResolution::FailDelivery);
    cancel_pending_pastes(&context);
    while let Ok(command) = context.paste_commands.try_recv() {
        let _ = cancel_paste_command(&command.state);
        let _ = command
            .result
            .publish(failed_paste(PasteFailure::Unavailable));
        let _ = command.acknowledgement.try_send(());
    }
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // `_resources` drops here before `context`, enforcing ordered native
    // teardown on clean return and every contained owner unwind.
}

fn enter_owner_lifecycle_recovery(context: &CallbackContext) {
    if callback_recovery_requires_deferred_mode(context) {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.terminal.trigger(TerminalReason::CallbackPanicked);
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
    run_owner_callback(info, |context| {
        if context.state.stopping.load(Ordering::Acquire) || context.terminal.is_triggered() {
            let _ = resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context
                .state
                .session_capture_mode
                .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
            cancel_pending_pastes(context);
            begin_owner_shutdown(context);
        } else {
            let _ =
                resolve_pending_activation(context, PendingActivationResolution::ForceTargetless);
            process_owner_commands(context);
            process_paste_commands(context);
        }
    });
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn service_test_tap_disable_request(context: &CallbackContext) {
    let Some(path) = context.test_tap_disable_request.as_ref() else {
        return;
    };
    if !path.try_exists().unwrap_or(false) {
        return;
    }
    let _ = std::fs::remove_file(path);
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if tap.is_null() {
        context
            .terminal
            .trigger(TerminalReason::EventTapTimeoutRecoveryFailed);
        return;
    }
    // SAFETY: owner test control runs on the tap's run loop. It genuinely
    // disables the live tap, verifies disabled state, then routes the exact
    // production disabled-event callback using the retained refcon.
    let disabled = unsafe {
        ffi::CGEventTapEnable(tap, false);
        !ffi::CGEventTapIsEnabled(tap)
    };
    let _ = unsafe {
        event_tap_callback(
            null_mut(),
            ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT,
            null_mut(),
            (context as *const CallbackContext).cast_mut().cast(),
        )
    };
    let reenabled = unsafe { ffi::CGEventTapIsEnabled(tap) };
    super::record_macos_test_tap_disable(disabled, reenabled);
    if !disabled || !reenabled {
        context
            .terminal
            .trigger(TerminalReason::EventTapTimeoutRecoveryFailed);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn service_test_paste_barrier_pause(context: &CallbackContext) {
    if !context
        .test_paste_barrier_split_active
        .load(Ordering::Acquire)
        || !context
            .test_paste_barrier_down_observed
            .load(Ordering::Acquire)
    {
        return;
    }
    let (Some(paused), Some(release)) = (
        context.test_paste_barrier_paused.as_ref(),
        context.test_paste_barrier_release.as_ref(),
    ) else {
        return;
    };
    if !context
        .test_paste_barrier_pause_announced
        .swap(true, Ordering::AcqRel)
        && std::fs::write(paused, b"authenticated barrier down observed\n").is_err()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return;
    }
    if !release.try_exists().unwrap_or(false) {
        arm_maintenance_timer(context);
        return;
    }
    let _ = std::fs::remove_file(release);
    let posted = context.native_events.try_lock().is_ok_and(|mut events| {
        let Some(pool) = events.as_mut() else {
            return false;
        };
        injection::post_prepared_paste_barrier_up(pool);
        true
    });
    if posted {
        context
            .test_paste_barrier_split_active
            .store(false, Ordering::Release);
    } else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

fn deferred_native_ordering_drained(keyboard: &CallbackKeyboard) -> bool {
    keyboard.transactional.journal_len() == 0
        && keyboard.transactional.owned_letters() == 0
        && keyboard.transactional.pending_injected_cleanup().is_none()
        && keyboard.transactional.pending_menu_cleanup().is_none()
        && !has_session_ownership(keyboard)
        && !keyboard.gap_barrier_pending
        && keyboard.gap_barrier_token.is_none()
        && keyboard.replay_observation.is_none()
        && keyboard.inflight_effect.is_none()
}

fn try_clear_recovery_deferred_mode(context: &CallbackContext) -> bool {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        return false;
    }
    let journal_drained = context
        .recovery_edges
        .try_lock()
        .is_ok_and(|journal| !journal.ordering_pending());
    let native_drained = context
        .keyboard
        .try_lock()
        .is_ok_and(|keyboard| deferred_native_ordering_drained(&keyboard));
    let paste_drained = context
        .pending_paste
        .try_lock()
        .is_ok_and(|pending| pending.is_none());
    if journal_drained && native_drained && paste_drained {
        context
            .state
            .recovery_deferred_mode
            .store(false, Ordering::Release);
        true
    } else {
        false
    }
}

fn submit_deferred_edges_if_ready(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    if let Ok(mut keyboard) = context.keyboard.try_lock()
        && let Ok(mut journal) = context.recovery_edges.try_lock()
        && journal.submitted_len == 0
    {
        journal.resolve_uncertain_ownership(&mut keyboard);
        let _ = journal.expire_external_collection(Instant::now());
    }
    let deferred_ready = context.recovery_edges.try_lock().is_ok_and(|mut journal| {
        journal.settle_overflow();
        journal.ready_slice().is_some()
    });
    if !deferred_ready {
        if !try_clear_recovery_deferred_mode(context) {
            arm_maintenance_timer(context);
        }
        return;
    }
    let native_ordering_drained = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
        if !deferred_native_ordering_drained(&keyboard) {
            return false;
        }
        // Deferred physical originals updated the fixed native tracker but
        // intentionally bypassed matcher/admission. Reconcile that final
        // physical generation only after retained replay/cleanup observations
        // drained and before any deferred foreground repost.
        let snapshot = physical_snapshot(&keyboard);
        begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot)).applied
    });
    let paste_drained = context
        .pending_paste
        .try_lock()
        .is_ok_and(|pending| pending.is_none());
    if !native_ordering_drained || !paste_drained {
        arm_maintenance_timer(context);
        return;
    }

    let mut journal = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return;
        }
    };
    journal.settle_overflow();
    let bank = journal.next_pool_bank;
    let Some(edges) = journal.ready_slice() else {
        return;
    };
    let count = edges.len();
    let mut native_events = match context.native_events.try_lock() {
        Ok(events) => events,
        Err(_) => {
            drop(journal);
            arm_maintenance_timer(context);
            return;
        }
    };
    let Some(pool) = native_events.as_mut() else {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    };
    let Some(prepared) = injection::prepare_deferred_events(pool, bank, edges) else {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    };
    if !journal.begin_submission(prepared.token(), bank, count) {
        drop(native_events);
        drop(journal);
        fail_recovery_edge_journal(context);
        return;
    }
    drop(journal);
    prepared.post(pool);
    drop(native_events);
    arm_maintenance_timer(context);
}

unsafe extern "C" fn maintenance_timer_callback(_timer: ffi::CFRunLoopTimerRef, info: *mut c_void) {
    run_owner_callback(info, |context| {
        #[cfg(feature = "transactional-shortcuts-dev")]
        {
            service_test_tap_disable_request(context);
            service_test_paste_barrier_pause(context);
        }
        submit_deferred_edges_if_ready(context);
        monitor_owned_native_state(context);
        if context.terminal.is_triggered() {
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context
                .state
                .session_capture_mode
                .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        }
        if context.state.stopping.load(Ordering::Acquire) {
            let _ = resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
            cancel_pending_pastes(context);
        } else {
            let _ = resolve_pending_activation(context, PendingActivationResolution::Poll);
            poll_pending_paste(context);
        }
        poll_shutdown_drain(context);
        park_maintenance_timer_if_idle(context);
    });
}

fn run_owner_callback(info: *mut c_void, callback: impl FnOnce(&CallbackContext)) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if info.is_null() {
            return;
        }
        // SAFETY: info is the boxed callback context retained through source cleanup.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        if context.state.recovery_pending.load(Ordering::Acquire)
            && !attempt_pending_recovery(context)
        {
            return;
        }
        callback(context);
    }));
    if result.is_err() && !info.is_null() {
        // SAFETY: context remains alive through source invalidation. Never stop
        // the run loop directly: the retained authoritative engine must first
        // replay a candidate or enter strict owned-up drain.
        let context = unsafe { &*info.cast::<CallbackContext>() };
        recover_owner_unwind(context);
    }
}

/// Attempts recovery once and rechecks the release-published gate. `false`
/// means callers may only rearm permanent wake resources or classify drain
/// edges; no owner control, effect, admission, or ordinary callback is safe.
fn attempt_pending_recovery(context: &CallbackContext) -> bool {
    let _ = recover_unwind(context, false);
    let recovered = !context.state.recovery_pending.load(Ordering::Acquire);
    if !recovered {
        arm_maintenance_timer(context);
    }
    recovered
}

fn set_current_edge_disposition(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    disposition: CurrentEdgeDisposition,
) {
    keyboard.current_edge_disposition = disposition;
    context
        .current_edge_disposition
        .store(disposition as u8, Ordering::Release);
}

fn set_atomic_current_edge_disposition(
    context: &CallbackContext,
    disposition: CurrentEdgeDisposition,
) {
    context
        .current_edge_disposition
        .store(disposition as u8, Ordering::Release);
}

fn atomic_current_edge_disposition(context: &CallbackContext) -> CurrentEdgeDisposition {
    CurrentEdgeDisposition::from_u8(context.current_edge_disposition.load(Ordering::Acquire))
}

enum DriveCompletion {
    Complete(Completion),
    ActivationDeferred,
    NativeObservationPending,
    Failed,
}

fn drive_transaction_turn(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    mut turn: Turn,
    mut activation_reservation: Option<ActivationReservation>,
) -> DriveCompletion {
    keyboard.last_native_effect_failed = false;
    let mut injection_failed = false;
    for _ in 0..=MAX_EFFECTS_PER_TURN {
        match turn {
            Turn::Complete { engine, completion } => {
                context.observability.publish(engine.metrics());
                keyboard.transactional = engine;
                keyboard.inflight_effect = None;
                if keyboard.transactional.owned_letters() == 0
                    && keyboard.transactional.journal_len() == 0
                {
                    keyboard.candidate_target_captured = false;
                    keyboard.candidate_target = None;
                    keyboard.owned_down_records = [None; 26];
                }
                if let Completion::Event(outcome) = completion {
                    set_current_edge_disposition(
                        context,
                        keyboard,
                        if outcome.disposition == EventDisposition::CaptureCurrent {
                            CurrentEdgeDisposition::Owned
                        } else {
                            CurrentEdgeDisposition::Pass
                        },
                    );
                }
                keyboard.last_native_effect_failed = injection_failed;
                if injection_failed && !context.terminal.is_triggered() {
                    context.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    context
                        .terminal
                        .trigger(TerminalReason::InputInjectionUnavailable);
                }
                return DriveCompletion::Complete(completion);
            }
            Turn::NeedEffect {
                effect,
                continuation,
            } => {
                keyboard.inflight_effect = Some(InflightEffect {
                    effect,
                    continuation: continuation.clone(),
                    outcome: None,
                });
                let outcome = match effect {
                    EffectRequest::NeutralizeMenu(_) => {
                        // The macOS engine is constructed with NotRequired and
                        // must never report fictional native dummy acceptance.
                        injection_failed = true;
                        EffectOutcome::Neutralized { accepted: 0 }
                    }
                    EffectRequest::CleanupMenuNeutralization(_) => {
                        injection_failed = true;
                        EffectOutcome::MenuCleanupAccepted { accepted: 0 }
                    }
                    EffectRequest::DeliverActivation(notice) => {
                        let retained_reservation =
                            activation_reservation.take().or(keyboard.candidate_target);
                        if !matches!(notice, ActivationNotice::Up { .. })
                            && let Some(reservation) = retained_reservation
                            && let Some(cache) = context.target_cache.as_ref()
                            && let Some(validation_request) =
                                cache.request_activation_validation(&reservation)
                            && let Ok(mut pending) = context.pending_activation.try_lock()
                            && pending.is_none()
                        {
                            *pending = Some(PendingActivation {
                                continuation: continuation.clone(),
                                notice,
                                reservation,
                                validation_request,
                                resolved_delivery: None,
                                deadline: Instant::now() + ACTIVATION_TARGET_TIMEOUT,
                            });
                            drop(pending);
                            keyboard.inflight_effect = None;
                            arm_maintenance_timer(context);
                            #[cfg(test)]
                            if PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.replace(false)) {
                                panic!("induced panic after deferred activation install");
                            }
                            return DriveCompletion::ActivationDeferred;
                        }
                        EffectOutcome::ActivationDelivered(keyboard.dispatcher.deliver(
                            &context.outbound,
                            &context.terminal,
                            notice,
                            None,
                        ))
                    }
                    EffectRequest::Replay(batch) => {
                        #[cfg(test)]
                        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(count.get() + 1));
                        let target_is_current = !keyboard.candidate_target_captured
                            || keyboard.candidate_target.is_some_and(|reservation| {
                                context
                                    .target_cache
                                    .as_ref()
                                    .is_some_and(|cache| cache.reservation_is_current(&reservation))
                            });
                        if !target_is_current {
                            let outcome = EffectOutcome::ReplaySuppressedTargetChanged;
                            if let Some(inflight) = keyboard.inflight_effect.as_mut() {
                                inflight.outcome = Some(outcome);
                            }
                            turn = continuation.resume(outcome);
                            continue;
                        }
                        let submission = if keyboard.replay_observation.is_none() {
                            context.native_events.try_lock().ok().and_then(|mut pool| {
                                let pool = pool.as_mut()?;
                                let prepared = injection::prepare_replay(pool, batch)?;
                                let submission = prepared.submission();
                                let token = submission.token?;
                                if !keyboard.begin_replay_observation(
                                    ExpectedReplayBatch::Replay(batch),
                                    token,
                                ) {
                                    return None;
                                }
                                // Primary-callback replay always enters the HID
                                // stream. Reducer finalization waits for every
                                // exact authenticated tap observation; proxy
                                // submission alone is never treated as replay.
                                prepared.post(pool, None);
                                Some(submission)
                            })
                        } else {
                            None
                        }
                        .unwrap_or(injection::Submission {
                            count: 0,
                            token: None,
                        });
                        if submission.count == batch.len() {
                            #[cfg(feature = "transactional-shortcuts-dev")]
                            if context.test_physical_seam_enabled {
                                super::record_test_replay_submission();
                            }
                            arm_maintenance_timer(context);
                            return DriveCompletion::NativeObservationPending;
                        }
                        injection_failed = true;
                        EffectOutcome::ReplaySubmitted { submitted: 0 }
                    }
                    EffectRequest::CleanupInjected(batch) => {
                        #[cfg(test)]
                        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(count.get() + 1));
                        // Cleanup contains balancing ups only. Once a replay
                        // down was visible, its release must not be stranded by
                        // a later focus change.
                        let submission = if keyboard.replay_observation.is_none() {
                            context.native_events.try_lock().ok().and_then(|mut pool| {
                                let pool = pool.as_mut()?;
                                let prepared = injection::prepare_cleanup(pool, batch)?;
                                let submission = prepared.submission();
                                let token = submission.token?;
                                if !keyboard.begin_replay_observation(
                                    ExpectedReplayBatch::Cleanup(batch),
                                    token,
                                ) {
                                    return None;
                                }
                                prepared.post(pool, None);
                                Some(submission)
                            })
                        } else {
                            None
                        }
                        .unwrap_or(injection::Submission {
                            count: 0,
                            token: None,
                        });
                        if submission.count == batch.len() {
                            arm_maintenance_timer(context);
                            return DriveCompletion::NativeObservationPending;
                        }
                        injection_failed = true;
                        EffectOutcome::CleanupSubmitted { submitted: 0 }
                    }
                };
                if let Some(inflight) = keyboard.inflight_effect.as_mut() {
                    inflight.outcome = Some(outcome);
                }
                #[cfg(test)]
                if PANIC_AFTER_EFFECT_OUTCOME.with(|flag| flag.replace(false)) {
                    panic!("induced panic after native effect outcome");
                }
                turn = continuation.resume(outcome);
            }
        }
    }
    // A malformed effect chain is terminal, but panicking here would discard
    // its continuation-owned engine. The caller's authoritative snapshot stays
    // installed and owner recovery replays/drains it.
    keyboard.last_native_effect_failed = true;
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.terminal.trigger(TerminalReason::ReducerPoisoned);
    DriveCompletion::Failed
}

fn observed_native_effect_outcome(effect: EffectRequest) -> Option<EffectOutcome> {
    match effect {
        EffectRequest::Replay(batch) => Some(EffectOutcome::ReplaySubmitted {
            submitted: batch.len(),
        }),
        EffectRequest::CleanupInjected(batch) => Some(EffectOutcome::CleanupSubmitted {
            submitted: batch.len(),
        }),
        _ => None,
    }
}

fn mark_observed_native_effect(keyboard: &mut CallbackKeyboard) -> bool {
    let Some(inflight) = keyboard.inflight_effect.as_mut() else {
        return false;
    };
    let Some(outcome) = observed_native_effect_outcome(inflight.effect) else {
        return false;
    };
    inflight.outcome = Some(outcome);
    true
}

fn resume_observed_native_effect(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    let Some(inflight) = keyboard.inflight_effect.take() else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return DriveCompletion::Failed;
    };
    let Some(outcome) = observed_native_effect_outcome(inflight.effect) else {
        keyboard.inflight_effect = Some(inflight);
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    };
    drive_transaction_turn(
        context,
        keyboard,
        inflight.continuation.resume(outcome),
        None,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetChangedReplayRecovery {
    AwaitObservation,
    ReconcileVisibleDowns,
    FinishAfterCleanup,
}

fn target_changed_replay_recovery(
    keyboard: &CallbackKeyboard,
    inflight: &InflightEffect,
) -> Option<TargetChangedReplayRecovery> {
    (inflight.outcome.is_none()
        && matches!(inflight.effect, EffectRequest::Replay(_))
        && keyboard.replay_target_changed)
        .then(|| {
            if keyboard.replay_observation.is_some() {
                TargetChangedReplayRecovery::AwaitObservation
            } else if keyboard.replay_target_cleanup_pending {
                TargetChangedReplayRecovery::FinishAfterCleanup
            } else {
                TargetChangedReplayRecovery::ReconcileVisibleDowns
            }
        })
}

fn reconcile_target_changed_replay(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    let cleanup = keyboard.visible_replay_cleanup();
    if cleanup.is_empty() {
        return finish_target_changed_replay(context, keyboard);
    }
    let submission = context.native_events.try_lock().ok().and_then(|mut pool| {
        let pool = pool.as_mut()?;
        let prepared = injection::prepare_cleanup(pool, cleanup)?;
        let submission = prepared.submission();
        let token = submission.token?;
        if submission.count != cleanup.len()
            || !keyboard.begin_replay_observation(ExpectedReplayBatch::Cleanup(cleanup), token)
        {
            return None;
        }
        keyboard.replay_target_cleanup_pending = true;
        prepared.post(pool, None);
        Some(submission)
    });
    if submission.is_some_and(|submitted| submitted.count == cleanup.len()) {
        arm_maintenance_timer(context);
        DriveCompletion::NativeObservationPending
    } else {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        DriveCompletion::Failed
    }
}

fn finish_target_changed_replay(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
) -> DriveCompletion {
    keyboard.replay_target_cleanup_pending = false;
    let Some(inflight) = keyboard.inflight_effect.take() else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    };
    if !matches!(inflight.effect, EffectRequest::Replay(_)) {
        keyboard.inflight_effect = Some(inflight);
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return DriveCompletion::Failed;
    }
    drive_transaction_turn(
        context,
        keyboard,
        inflight
            .continuation
            .resume(EffectOutcome::ReplaySuppressedTargetChanged),
        None,
    )
}

const fn failed_effect_outcome(effect: EffectRequest) -> EffectOutcome {
    match effect {
        EffectRequest::NeutralizeMenu(_) => EffectOutcome::Neutralized { accepted: 0 },
        EffectRequest::CleanupMenuNeutralization(_) => {
            EffectOutcome::MenuCleanupAccepted { accepted: 0 }
        }
        EffectRequest::DeliverActivation(_) => EffectOutcome::ActivationDelivered(false),
        EffectRequest::Replay(_) => EffectOutcome::ReplaySubmitted { submitted: 0 },
        EffectRequest::CleanupInjected(_) => EffectOutcome::CleanupSubmitted { submitted: 0 },
    }
}

fn callback_recovery_requires_deferred_mode(context: &CallbackContext) -> bool {
    let journal_ordering = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.ordering_pending());
    let keyboard_ordering = context.keyboard.try_lock().map_or(true, |keyboard| {
        keyboard.transactional.journal_len() != 0
            || keyboard.transactional.owned_letters() != 0
            || keyboard.inflight_effect.is_some()
            || keyboard.replay_observation.is_some()
            || keyboard.transactional.pending_injected_cleanup().is_some()
            || keyboard.transactional.pending_menu_cleanup().is_some()
            || has_session_ownership(&keyboard)
            || keyboard.gap_barrier_pending
            || keyboard.gap_barrier_token.is_some()
    });
    let paste_ordering = context
        .pending_paste
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    journal_ordering || keyboard_ordering || paste_ordering
}

fn recover_callback_unwind(context: &CallbackContext) -> CurrentEdgeDisposition {
    recover_unwind(context, callback_recovery_requires_deferred_mode(context))
}

fn recover_owner_unwind(context: &CallbackContext) -> CurrentEdgeDisposition {
    recover_unwind(context, callback_recovery_requires_deferred_mode(context))
}

fn recover_unwind(context: &CallbackContext, enable_deferred_mode: bool) -> CurrentEdgeDisposition {
    if enable_deferred_mode {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    context
        .state
        .recovery_pending
        .store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.terminal.trigger(TerminalReason::CallbackPanicked);

    // Recovery is itself an FFI-boundary operation. A second unwind must never
    // escape the callback; leave poison set and retain drain ownership so the
    // owner timer can retry instead of pretending teardown is safe.
    let recovered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Preflight every callback-critical lock before mutating a continuation
        // or control state. Owner/event callbacks are serialized on this run
        // loop, so a successful preflight makes a later WouldBlock impossible
        // without re-entrancy; failed preflight leaves all submitted outcomes
        // and continuations untouched for the permanent-wake retry.
        drop(context.keyboard.try_lock().ok()?);
        drop(context.pending_activation.try_lock().ok()?);
        drop(context.pending_paste.try_lock().ok()?);
        drop(context.recovery_edges.try_lock().ok()?);
        drop(context.native_events.try_lock().ok()?);

        let mut recovered_inflight = false;
        let mut preserve_current_cleanup_pass = false;
        let mut keyboard = context.keyboard.try_lock().ok()?;
        {
            let mut recovery_edges = context.recovery_edges.try_lock().ok()?;
            recovery_edges.resolve_uncertain_ownership(&mut keyboard);
        }
        if let Some(inflight) = keyboard.inflight_effect.take() {
            #[cfg(test)]
            if PANIC_ONCE_DURING_RECOVERY.with(|flag| flag.replace(false)) {
                keyboard.inflight_effect = Some(inflight);
                panic!("induced second panic during recovery");
            }
            if let Some(target_recovery) = target_changed_replay_recovery(&keyboard, &inflight) {
                keyboard.inflight_effect = Some(inflight);
                let completion = match target_recovery {
                    TargetChangedReplayRecovery::AwaitObservation => None,
                    TargetChangedReplayRecovery::ReconcileVisibleDowns => {
                        Some(reconcile_target_changed_replay(context, &mut keyboard))
                    }
                    TargetChangedReplayRecovery::FinishAfterCleanup => {
                        preserve_current_cleanup_pass = atomic_current_edge_disposition(context)
                            == CurrentEdgeDisposition::Pass;
                        Some(finish_target_changed_replay(context, &mut keyboard))
                    }
                };
                if let Some(completion) = completion {
                    match completion {
                        DriveCompletion::Complete(_) => recovered_inflight = true,
                        DriveCompletion::NativeObservationPending => {}
                        DriveCompletion::ActivationDeferred | DriveCompletion::Failed => {
                            return None;
                        }
                    }
                }
            } else if inflight.outcome.is_none() && keyboard.replay_observation.is_some() {
                // HID submission is not completion. Preserve the continuation
                // untouched until the exact observation cursor reaches its end.
                keyboard.inflight_effect = Some(inflight);
            } else {
                let outcome = inflight
                    .outcome
                    .unwrap_or_else(|| failed_effect_outcome(inflight.effect));
                let turn = inflight.continuation.resume(outcome);
                let _ = drive_transaction_turn(context, &mut keyboard, turn, None);
                recovered_inflight = true;
            }
        }
        drop(keyboard);
        if recovered_inflight {
            let mut pending = context.pending_activation.try_lock().ok()?;
            if pending
                .as_ref()
                .is_some_and(|pending| pending.resolved_delivery.is_some())
            {
                let _ = pending.take();
            }
        }
        let awaiting_exact_native_observation = context
            .keyboard
            .try_lock()
            .ok()?
            .has_submitted_replay_authority();
        if !awaiting_exact_native_observation
            && !resolve_pending_activation(context, PendingActivationResolution::FailDelivery)
        {
            return None;
        }
        let mut keyboard = context.keyboard.try_lock().ok()?;
        if !awaiting_exact_native_observation {
            let _ = begin_transaction_control(
                context,
                &mut keyboard,
                Control::CloseAdmission(CancelReason::EffectProtocolViolation),
            );
            if !reconcile_recovery_locked(context, &mut keyboard) {
                return None;
            }
        }
        if preserve_current_cleanup_pass {
            // The exact cleanup up was authenticated and made visible before
            // the unwind. Terminal replay completion must not retroactively
            // suppress that already-authoritative current edge.
            set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Pass);
        }
        drop(keyboard);

        // Revalidate every lock after recovery effects commit. No evidence or
        // pooled event is discarded here, and poison remains set if a nested
        // callback somehow made a lock unavailable.
        drop(context.pending_activation.try_lock().ok()?);
        drop(context.pending_paste.try_lock().ok()?);
        drop(context.recovery_edges.try_lock().ok()?);
        drop(context.native_events.try_lock().ok()?);
        keep_strict_drain_tap_enabled(context);
        arm_maintenance_timer(context);
        Some(())
    }))
    .ok()
    .flatten()
    .is_some();

    if recovered {
        context.keyboard.clear_poison();
        context.pending_activation.clear_poison();
        context.pending_paste.clear_poison();
        context.recovery_edges.clear_poison();
        context.native_events.clear_poison();
        context
            .state
            .recovery_pending
            .store(false, Ordering::Release);
        let _ = try_clear_recovery_deferred_mode(context);
    } else {
        // Permanent source/timer resources make this bounded wake safe even if
        // a nested callback currently holds one lock.
        arm_maintenance_timer(context);
    }
    atomic_current_edge_disposition(context)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingActivationResolution {
    Poll,
    ForceTargetless,
    FailDelivery,
}

fn resolve_pending_activation(
    context: &CallbackContext,
    resolution: PendingActivationResolution,
) -> bool {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => return false,
    };
    let mut pending_slot = match context.pending_activation.try_lock() {
        Ok(pending) => pending,
        Err(_) => return false,
    };
    let Some(pending) = pending_slot.as_mut() else {
        return true;
    };

    let mut validated_evidence = None;
    let ready = pending.resolved_delivery.is_some()
        || if resolution == PendingActivationResolution::FailDelivery {
            true
        } else if let Some(cache) = context.target_cache.as_ref()
            && let Some(response) = pending.validation_request.try_response()
        {
            let ticket = pending.validation_request.ticket();
            if let Some((after, epoch, boundary_epoch, _range_epoch, worker_confirmed)) = response
                .into_current_handle(
                    ticket,
                    cache.current_epoch(),
                    cache.current_boundary_epoch(),
                    cache.current_selected_range_epoch(),
                )
                && worker_confirmed
                && epoch == cache.current_epoch()
                && boundary_epoch == cache.current_boundary_epoch()
                && cache.reservation_is_current(&pending.reservation)
            {
                validated_evidence = Some(after);
            }
            true
        } else {
            resolution == PendingActivationResolution::ForceTargetless
                || Instant::now() >= pending.deadline
        };
    if !ready {
        return true;
    }

    let delivered = if let Some(delivered) = pending.resolved_delivery {
        delivered
    } else if resolution == PendingActivationResolution::FailDelivery {
        false
    } else {
        // A response can race a notification between channel receipt and this
        // final owner-side check. Such a race keeps activation functional but
        // deliberately strips its paste target.
        let evidence = context
            .target_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.current_epoch() == pending.validation_request.ticket().start_epoch()
                    && cache.current_boundary_epoch()
                        == pending.validation_request.ticket().start_boundary_epoch()
            })
            .then_some(validated_evidence)
            .flatten();
        keyboard.dispatcher.deliver(
            &context.outbound,
            &context.terminal,
            pending.notice,
            evidence,
        )
    };
    pending.resolved_delivery = Some(delivered);
    let continuation = pending.continuation.clone();
    // Transfer continuation authority before resuming it. A failed delivery may
    // install an observed HID replay; the activation slot must never remain a
    // second resumable owner of that same continuation.
    let _resolved = pending_slot
        .take()
        .expect("pending activation remains installed until authority transfer");
    drop(pending_slot);
    let completion = drive_transaction_turn(
        context,
        &mut keyboard,
        continuation.resume(EffectOutcome::ActivationDelivered(delivered)),
        None,
    );
    let DriveCompletion::Complete(Completion::Event(outcome)) = completion else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
        return false;
    };
    if outcome.terminal && context.gate.is_open() && !context.terminal.is_triggered() {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    true
}

fn begin_transaction_snapshot(engine: &TransactionEngine, input: EngineInput) -> Turn {
    engine.clone().begin(input)
}

fn begin_transaction_control(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    control: Control,
) -> talking_quill_keyboard_core::transactional::ControlOutcome {
    if keyboard.has_submitted_replay_authority() {
        // A successfully HID-submitted replay is immutable native authority.
        // No lifecycle/control request may replace its continuation, begin a
        // successor turn, or synthesize a failed outcome while any exact edge
        // remains unobserved. Admission/terminal atomics are owned by callers;
        // only exact observation may retire it. Terminal teardown stays incomplete.
        return talking_quill_keyboard_core::transactional::ControlOutcome {
            applied: false,
            cancellation: None,
            shutdown: keyboard.transactional.shutdown_state(),
        };
    }
    // Begin from an allocation-free snapshot. The authoritative engine stays
    // installed until a complete turn replaces it, so an unexpected unwind
    // can never leave Default in place or lose captured ownership.
    let turn = begin_transaction_snapshot(&keyboard.transactional, EngineInput::Control(control));
    let completion = drive_transaction_turn(context, keyboard, turn, None);
    match completion {
        DriveCompletion::Complete(Completion::Control(outcome)) => outcome,
        DriveCompletion::NativeObservationPending => {
            talking_quill_keyboard_core::transactional::ControlOutcome {
                applied: false,
                cancellation: None,
                shutdown: keyboard.transactional.shutdown_state(),
            }
        }
        DriveCompletion::Complete(Completion::Event(_)) | DriveCompletion::ActivationDeferred => {
            talking_quill_keyboard_core::transactional::ControlOutcome {
                applied: false,
                cancellation: Some(CancelReason::EffectProtocolViolation),
                shutdown: keyboard.transactional.shutdown_state(),
            }
        }
        DriveCompletion::Failed => talking_quill_keyboard_core::transactional::ControlOutcome {
            applied: false,
            cancellation: Some(CancelReason::EffectProtocolViolation),
            shutdown: keyboard.transactional.shutdown_state(),
        },
    }
}

fn process_owner_commands(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
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
            Ok(mut keyboard) => match command.mutation.kind {
                OwnerMutationKind::Configure => {
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
                        let outcome = begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(compiled),
                        );
                        if !outcome.applied {
                            return false;
                        }
                        // Keep the legacy reducer fenced solely for independent
                        // Escape/Enter ownership and existing adapter tests.
                        keyboard.reducer.fence_activation_revision();
                        keyboard.fence_current_letters();
                        keyboard.merge_current_state_as_preheld(native_key_is_down);
                        keyboard.activation_revision_at = event_timestamp_now();
                        keyboard.activation = command.mutation.activation;
                        true
                    })
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
                    true
                }
                OwnerMutationKind::SuspendNativeInput => {
                    let disabled = keyboard
                        .transactional
                        .config()
                        .revision()
                        .checked_next()
                        .and_then(|revision| {
                            CompiledActivationConfig::compile(
                                revision,
                                false,
                                keyboard.transactional.config().bindings(),
                            )
                            .ok()
                        });
                    if disabled.is_none_or(|disabled| {
                        !begin_transaction_control(
                            context,
                            &mut keyboard,
                            Control::ReplaceConfig(disabled),
                        )
                        .applied
                    }) {
                        false
                    } else {
                        keyboard.activation.enabled = false;
                        context
                            .state
                            .session_capture_mode
                            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
                        deliver_balancing_events(context, &mut keyboard.reducer);
                        keyboard.fence_current_letters();
                        true
                    }
                }
            },
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

fn process_paste_commands(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    while let Ok(command) = context.paste_commands.try_recv() {
        if !paste_before_deadline(command.deadline, Instant::now())
            || !paste_admission_open(context)
        {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        if command
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
        if !paste_admission_open(context) {
            let _ = cancel_paste_command(&command.state);
            let _ = command
                .result
                .publish(failed_paste(PasteFailure::Unavailable));
            let _ = command.acknowledgement.try_send(());
            continue;
        }
        let evidence = if let Ok(mut keyboard) = context.keyboard.try_lock() {
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
        let mut pending = match context.pending_paste.try_lock() {
            Ok(pending) => pending,
            Err(_) => {
                let _ = command
                    .result
                    .publish(failed_paste(PasteFailure::Unavailable));
                command
                    .state
                    .store(PasteCommandState::Applied as u8, Ordering::Release);
                let _ = command.acknowledgement.try_send(());
                continue;
            }
        };
        if pending.is_some() {
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
            acknowledgement: command.acknowledgement,
            evidence,
            expected_clipboard_sha256: command.expected_clipboard_sha256,
            validation_request: None,
            validated_target_epoch: None,
            validated_target_boundary_epoch: None,
            validated_selected_range_epoch: None,
            insertion_request: None,
            neutral_modifier_epoch: None,
            neutral_barrier_state: 0,
            neutral_barrier_token: None,
            deadline: command.deadline,
            injection_cutoff: paste_injection_cutoff(command.deadline),
            modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
        });
        drop(pending);
        poll_pending_paste(context);
        if context
            .pending_paste
            .try_lock()
            .is_ok_and(|pending| pending.is_some())
        {
            arm_maintenance_timer(context);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PastePostChecks {
    admission_open: bool,
    before_deadline: bool,
    before_injection_cutoff: bool,
    modifiers_neutral: bool,
    permissions_granted: bool,
    secure_input_inactive: bool,
    target_valid: bool,
}

const fn modifier_wait_timed_out(reason: PasteFailure, before_injection_cutoff: bool) -> bool {
    matches!(reason, PasteFailure::ConflictingModifiers) && !before_injection_cutoff
}

fn paste_check_failure(state: &AtomicU8, checks: PastePostChecks) -> Option<PasteFailure> {
    if paste_command_state(state) != PasteCommandState::Waiting || !checks.admission_open {
        return Some(PasteFailure::Unavailable);
    }
    if !checks.before_deadline || !checks.before_injection_cutoff {
        return Some(if checks.modifiers_neutral {
            PasteFailure::Unavailable
        } else {
            PasteFailure::ConflictingModifiers
        });
    }
    if !checks.modifiers_neutral {
        return Some(PasteFailure::ConflictingModifiers);
    }
    if !checks.permissions_granted {
        return Some(PasteFailure::PermissionDenied);
    }
    if !checks.secure_input_inactive {
        return Some(PasteFailure::SecureInput);
    }
    if !checks.target_valid {
        return Some(PasteFailure::Unavailable);
    }
    None
}

fn claim_paste_injection(state: &AtomicU8, checks: PastePostChecks) -> Result<(), PasteFailure> {
    if let Some(reason) = paste_check_failure(state, checks) {
        return Err(reason);
    }
    state
        .compare_exchange(
            PasteCommandState::Waiting as u8,
            PasteCommandState::Injecting as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map(|_| ())
        .map_err(|_| PasteFailure::Unavailable)
}

#[cfg(any(test, feature = "transactional-shortcuts-dev"))]
const fn paste_modifier_barrier_valid(
    expected_epoch: Option<u64>,
    current_epoch: u64,
    logical_neutral: bool,
    hid_neutral: bool,
    barrier_complete: bool,
) -> bool {
    barrier_complete
        && logical_neutral
        && hid_neutral
        && matches!(expected_epoch, Some(expected) if expected == current_epoch)
}

fn paste_admission_open(context: &CallbackContext) -> bool {
    context.gate.is_open()
        && !context.terminal.is_triggered()
        && !context.state.stopping.load(Ordering::Acquire)
        && !context.state.quiescing.load(Ordering::Acquire)
}

fn current_paste_checks(
    context: &CallbackContext,
    command: &PendingPaste,
    target_valid: bool,
) -> PastePostChecks {
    let now = Instant::now();
    let permissions = permission_snapshot();
    PastePostChecks {
        admission_open: paste_admission_open(context),
        before_deadline: paste_before_deadline(command.deadline, now),
        before_injection_cutoff: paste_before_deadline(command.injection_cutoff, now),
        modifiers_neutral: native_modifiers_neutral()
            && context
                .keyboard
                .try_lock()
                .is_ok_and(|keyboard| keyboard.modifiers.sides().bits() == 0),
        permissions_granted: permissions.accessibility == PermissionState::Granted
            && permissions.input_monitoring == PermissionState::Granted
            && permissions.event_post == PermissionState::Granted,
        secure_input_inactive: !secure_input_active(),
        target_valid,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerValidation {
    Pending,
    Valid {
        epoch: u64,
        boundary_epoch: u64,
        selected_range_epoch: u64,
    },
    Invalid,
}

fn poll_worker_target_validation(
    context: &CallbackContext,
    command: &mut PendingPaste,
) -> WorkerValidation {
    let Some(cache) = context.target_cache.as_ref() else {
        return WorkerValidation::Invalid;
    };
    if command.validation_request.is_none() {
        let Some(request) = cache.request_target_validation(command.evidence) else {
            return WorkerValidation::Invalid;
        };
        command.validation_request = Some(request);
        return WorkerValidation::Pending;
    }
    let request = command
        .validation_request
        .as_mut()
        .expect("validation request installed above");
    let ticket = request.ticket();
    let Some(response) = request.try_response() else {
        return WorkerValidation::Pending;
    };
    command.validation_request = None;
    let Some((current, epoch, boundary_epoch, selected_range_epoch, _worker_confirmed)) = response
        .into_current_handle(
            ticket,
            cache.current_epoch(),
            cache.current_boundary_epoch(),
            cache.current_selected_range_epoch(),
        )
    else {
        return WorkerValidation::Invalid;
    };
    if command.evidence == current {
        WorkerValidation::Valid {
            epoch,
            boundary_epoch,
            selected_range_epoch,
        }
    } else {
        WorkerValidation::Invalid
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertionOwnerAction {
    Wait,
    CompleteSuccess,
    CompleteFailure(PasteFailure),
    CompleteTargetFallback,
    CancelPending,
    TerminalAmbiguous,
}

fn insertion_owner_action(status: InsertionStatus, deadline_reached: bool) -> InsertionOwnerAction {
    match status {
        InsertionStatus::Succeeded => InsertionOwnerAction::CompleteSuccess,
        InsertionStatus::Failed(reason) => InsertionOwnerAction::CompleteFailure(reason),
        InsertionStatus::TargetInvalid => InsertionOwnerAction::CompleteTargetFallback,
        InsertionStatus::Ambiguous => InsertionOwnerAction::TerminalAmbiguous,
        InsertionStatus::Claimed if deadline_reached => InsertionOwnerAction::TerminalAmbiguous,
        InsertionStatus::Pending if deadline_reached => InsertionOwnerAction::CancelPending,
        InsertionStatus::Pending | InsertionStatus::Claimed => InsertionOwnerAction::Wait,
    }
}

fn close_ambiguous_insertion_admission(context: &CallbackContext) {
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::InputInjectionUnavailable);
    arm_maintenance_timer(context);
}

fn poll_pending_paste(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    let mut pending = match context.pending_paste.try_lock() {
        Ok(pending) => pending,
        Err(_) => return,
    };
    let Some(command) = pending.as_mut() else {
        return;
    };
    if paste_command_state(&command.state) == PasteCommandState::Injecting
        && command.insertion_request.is_some()
    {
        let status = command
            .insertion_request
            .as_ref()
            .map(InsertionRequest::status)
            .unwrap_or(InsertionStatus::Ambiguous);
        match insertion_owner_action(status, Instant::now() >= command.deadline) {
            InsertionOwnerAction::CompleteSuccess => {
                command
                    .state
                    .store(PasteCommandState::Committed as u8, Ordering::Release);
                finish_pending_paste(
                    &mut pending,
                    PasteResult {
                        submitted: true,
                        reason: None,
                    },
                );
            }
            InsertionOwnerAction::CompleteFailure(reason) => {
                finish_pending_paste(&mut pending, failed_paste(reason));
            }
            InsertionOwnerAction::CompleteTargetFallback => {
                context.observability.record_target_validation_fallback();
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            }
            InsertionOwnerAction::TerminalAmbiguous => {
                close_ambiguous_insertion_admission(context);
            }
            InsertionOwnerAction::CancelPending => {
                let cancelled = command
                    .insertion_request
                    .as_ref()
                    .is_some_and(InsertionRequest::cancel);
                if cancelled {
                    finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                } else {
                    // A failed cancellation can only mean that the worker won
                    // claim/completion. Retain the request and let the next
                    // owner turn consume exact completion or go terminal.
                    arm_maintenance_timer(context);
                }
            }
            InsertionOwnerAction::Wait => arm_maintenance_timer(context),
        }
        return;
    }
    if command.neutral_modifier_epoch.is_some() && command.neutral_barrier_state < 2 {
        if Instant::now() >= command.deadline {
            // Modifiers were already neutral when the barrier was posted. A
            // missing observation is an event-post/tap failure, not a modifier
            // wait timeout.
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
        }
        return;
    }
    let checks = current_paste_checks(context, command, true);
    if checks.modifiers_neutral {
        command.modifier_wait.finish();
    }
    if let Some(reason) = paste_check_failure(&command.state, checks) {
        if reason == PasteFailure::ConflictingModifiers
            && paste_before_deadline(command.injection_cutoff, Instant::now())
        {
            // Physical modifiers may still be unwinding from the activation
            // chord. Wait under the same absolute deadline; do not post a
            // barrier or paste until both logical and HID state are neutral.
            command.modifier_wait.start();
            return;
        }
        if modifier_wait_timed_out(
            reason,
            paste_before_deadline(command.injection_cutoff, Instant::now()),
        ) {
            context.observability.record_modifier_timeout();
        }
        finish_pending_paste(&mut pending, failed_paste(reason));
        return;
    }

    // AX validation is the last asynchronous operation. Its exact workspace
    // and event-boundary epochs are frozen before the authenticated modifier
    // barrier is posted; no AX request is allowed after that barrier.
    if command.validated_target_epoch.is_none() {
        match poll_worker_target_validation(context, command) {
            WorkerValidation::Pending => return,
            WorkerValidation::Invalid => {
                context.observability.record_target_validation_fallback();
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
            WorkerValidation::Valid {
                epoch,
                boundary_epoch,
                selected_range_epoch,
            } => {
                command.validated_target_epoch = Some(epoch);
                command.validated_target_boundary_epoch = Some(boundary_epoch);
                command.validated_selected_range_epoch = Some(selected_range_epoch);
            }
        }
    }

    if command.neutral_barrier_token.is_none() {
        let checks = current_paste_checks(context, command, true);
        if checks.modifiers_neutral {
            command.modifier_wait.finish();
        }
        if let Some(reason) = paste_check_failure(&command.state, checks) {
            if reason == PasteFailure::ConflictingModifiers
                && paste_before_deadline(command.injection_cutoff, Instant::now())
            {
                command.modifier_wait.start();
                return;
            }
            if modifier_wait_timed_out(
                reason,
                paste_before_deadline(command.injection_cutoff, Instant::now()),
            ) {
                context.observability.record_modifier_timeout();
            }
            finish_pending_paste(&mut pending, failed_paste(reason));
            return;
        }
        let Some(cache) = context.target_cache.as_ref() else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        };
        let (Some(epoch), Some(boundary_epoch), Some(selected_range_epoch)) = (
            command.validated_target_epoch,
            command.validated_target_boundary_epoch,
            command.validated_selected_range_epoch,
        ) else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        };
        command.insertion_request = cache.prepare_insertion(
            command.evidence,
            epoch,
            boundary_epoch,
            selected_range_epoch,
            command.expected_clipboard_sha256,
            command.injection_cutoff,
        );
        if command.insertion_request.is_none() {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        }
        let mut native_events = match context.native_events.try_lock() {
            Ok(pool) if pool.is_some() => pool,
            _ => {
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
        };
        let pool = native_events
            .as_mut()
            .expect("native event pool checked above");
        let modifier_epoch = context
            .keyboard
            .try_lock()
            .ok()
            .filter(|keyboard| keyboard.modifiers.sides().bits() == 0)
            .map(|keyboard| keyboard.modifier_epoch);
        let Some(modifier_epoch) = modifier_epoch else {
            finish_pending_paste(
                &mut pending,
                failed_paste(PasteFailure::ConflictingModifiers),
            );
            return;
        };
        let Some(barrier) = injection::prepare_paste_barrier(pool) else {
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::OsRejected));
            return;
        };
        command.neutral_modifier_epoch = Some(modifier_epoch);
        command.neutral_barrier_state = 0;
        command.neutral_barrier_token = Some(barrier.token());
        // Install the exact token before CGEventPost and release the callback
        // lock so the down cannot be lost to owner-side contention.
        drop(pending);
        #[cfg(feature = "transactional-shortcuts-dev")]
        let split_barrier = context.test_paste_barrier_paused.is_some()
            && context.test_paste_barrier_release.is_some();
        #[cfg(feature = "transactional-shortcuts-dev")]
        if split_barrier {
            context
                .test_paste_barrier_down_observed
                .store(false, Ordering::Release);
            context
                .test_paste_barrier_pause_announced
                .store(false, Ordering::Release);
            context
                .test_paste_barrier_split_active
                .store(true, Ordering::Release);
            barrier.post_down(pool);
        } else {
            barrier.post(pool);
        }
        #[cfg(not(feature = "transactional-shortcuts-dev"))]
        barrier.post(pool);
        drop(native_events);
        // Require the ordered neutral barrier and another independently
        // refreshed target tuple before posting.
        return;
    }

    // The barrier callback submits only the preallocated target-specific AX
    // work. Owner polling waits for that worker result; no global Command+V is
    // posted for this path.
    arm_maintenance_timer(context);
}

fn publish_pending_paste_result(command: &PendingPaste, result: PasteResult) -> PasteResult {
    let authoritative = command.result.publish(result);
    let _ = command.acknowledgement.try_send(());
    authoritative
}

fn finish_pending_paste(pending: &mut Option<PendingPaste>, result: PasteResult) {
    let Some(command) = pending.as_ref() else {
        return;
    };
    let _ = publish_pending_paste_result(command, result);
    // Release publication precedes Applied so no caller can observe completion
    // without the fixed authoritative result slot.
    command
        .state
        .store(PasteCommandState::Applied as u8, Ordering::Release);
    let _ = pending.take();
}

fn cancel_pending_pastes(context: &CallbackContext) {
    let Ok(mut pending) = context.pending_paste.try_lock() else {
        return;
    };
    let Some(command) = pending.as_mut() else {
        return;
    };
    let barrier_unobserved =
        command.neutral_barrier_token.is_some() && command.neutral_barrier_state < 2;
    let insertion_status = command
        .insertion_request
        .as_ref()
        .map(InsertionRequest::status);
    match insertion_status {
        Some(InsertionStatus::Claimed | InsertionStatus::Ambiguous) => {
            close_ambiguous_insertion_admission(context);
            return;
        }
        Some(InsertionStatus::Pending) if Instant::now() < command.deadline => {
            arm_maintenance_timer(context);
            return;
        }
        Some(InsertionStatus::Pending) => {
            if command
                .insertion_request
                .as_ref()
                .is_some_and(InsertionRequest::cancel)
            {
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
                return;
            }
            close_ambiguous_insertion_admission(context);
            return;
        }
        Some(InsertionStatus::Succeeded) => {
            command
                .state
                .store(PasteCommandState::Committed as u8, Ordering::Release);
            finish_pending_paste(
                &mut pending,
                PasteResult {
                    submitted: true,
                    reason: None,
                },
            );
            return;
        }
        Some(InsertionStatus::Failed(reason)) => {
            finish_pending_paste(&mut pending, failed_paste(reason));
            return;
        }
        Some(InsertionStatus::TargetInvalid) => {
            context.observability.record_target_validation_fallback();
            finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
            return;
        }
        None => {}
    }
    if barrier_unobserved && Instant::now() < command.deadline {
        let _ = cancel_paste_command(&command.state);
        arm_maintenance_timer(context);
        return;
    }
    if barrier_unobserved {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    finish_pending_paste(&mut pending, failed_paste(PasteFailure::Unavailable));
}

fn keyboard_has_pending_native_work(keyboard: &CallbackKeyboard) -> bool {
    keyboard.strict_drain_recovery_needed()
}

/// Single conservative authority for owner-loop lifetime. A busy state lock is
/// pending work: teardown is forbidden until an owner turn can prove otherwise.
fn pending_native_work(context: &CallbackContext) -> bool {
    let keyboard_pending = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| keyboard_has_pending_native_work(&keyboard));
    let activation_pending = context
        .pending_activation
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    let paste_pending = context
        .pending_paste
        .try_lock()
        .map_or(true, |pending| pending.is_some());
    let deferred_pending = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.has_pending());
    let pending = context.state.recovery_pending.load(Ordering::Acquire)
        || context.state.recovery_deferred_mode.load(Ordering::Acquire)
        || keyboard_pending
        || activation_pending
        || paste_pending
        || deferred_pending
        || !context.owner_commands.is_empty()
        || !context.paste_commands.is_empty();
    context
        .state
        .pending_native_work
        .store(pending, Ordering::Release);
    pending
}

fn stop_owner_run_loop_if_drained(context: &CallbackContext) -> bool {
    if pending_native_work(context) {
        arm_maintenance_timer(context);
        false
    } else {
        // SAFETY: this is called only on the owner run loop. The authoritative
        // predicate proved that no retained/submitted native work can outlive
        // the tap or startup-allocated event pool.
        #[cfg(feature = "transactional-shortcuts-dev")]
        super::mark_test_semantic_drain_complete();
        unsafe { ffi::CFRunLoopStop(ffi::CFRunLoopGetCurrent()) };
        true
    }
}

fn begin_owner_shutdown(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    // Keep the AX target worker alive through candidate replay and terminal
    // drain. Owner teardown stops it only after native authority is drained.
    let _ = resolve_pending_activation(context, PendingActivationResolution::FailDelivery);
    cancel_pending_pastes(context);
    while let Ok(command) = context.owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    while let Ok(command) = context.paste_commands.try_recv() {
        let _ = cancel_paste_command(&command.state);
        let _ = command
            .result
            .publish(failed_paste(PasteFailure::Unavailable));
        let _ = command.acknowledgement.try_send(());
    }
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            poisoned.into_inner()
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            arm_maintenance_timer(context);
            return;
        }
    };
    if !keyboard.shutdown_requested {
        // Install the one-shot control authority before executing a turn. If
        // native submission or continuation recovery unwinds, the poisoned
        // authoritative state prevents a second Shutdown control/post.
        keyboard.shutdown_requested = true;
        keyboard.shutdown_deadline = context
            .state
            .shutdown_deadline
            .lock()
            .map_or_else(|poisoned| *poisoned.into_inner(), |installed| *installed)
            .or_else(|| Some(Instant::now() + SHUTDOWN_DRAIN_TIMEOUT));
        keyboard.shutdown_deadline_reported = false;
        #[cfg(test)]
        TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.set(count.get() + 1));
        let _ = begin_transaction_control(context, &mut keyboard, Control::Shutdown);
    }
    drop(keyboard);
    let _ = stop_owner_run_loop_if_drained(context);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownDrainAction {
    Continue,
    ReportUnresponsive,
    Stop,
}

fn shutdown_drain_action(
    has_ownership: bool,
    deadline: Option<Instant>,
    deadline_reported: bool,
    now: Instant,
) -> ShutdownDrainAction {
    if !has_ownership {
        ShutdownDrainAction::Stop
    } else if !deadline_reported && deadline.is_some_and(|deadline| now >= deadline) {
        ShutdownDrainAction::ReportUnresponsive
    } else {
        ShutdownDrainAction::Continue
    }
}

fn terminal_shutdown_incomplete(context: &CallbackContext) {
    context.observability.record_shutdown_ownership_deadline();
    // An in-process owner cannot hand off a still-held suppressed down within a
    // bounded deadline without exposing the down or a later unmatched up.
    // Terminal teardown therefore posts no keyboard event and retires no
    // ownership. Production cannot reach this state because its process gate
    // prevents every native suppression facility from opening.
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if !tap.is_null() {
        unsafe { ffi::CGEventTapEnable(tap, false) };
    }
    if let Ok(mut pending) = context.pending_paste.try_lock()
        && let Some(command) = pending.as_ref()
    {
        let acceptance_ambiguous = command.insertion_request.as_ref().is_some_and(|request| {
            matches!(
                request.status(),
                InsertionStatus::Claimed | InsertionStatus::Ambiguous
            )
        });
        if !acceptance_ambiguous {
            let _ = publish_pending_paste_result(command, failed_paste(PasteFailure::Unavailable));
            command
                .state
                .store(PasteCommandState::Applied as u8, Ordering::Release);
        }
        // Terminal teardown may release the RPC waiter, but an
        // acceptance-ambiguous operation deliberately has no ordinary result.
        let _ = pending.take();
    }
    // Do not repost deferred keyboard or mouse input here. The absolute
    // deadline is an explicit incomplete semantic drain, not authority to
    // inject into whichever target is now foreground or to clear ownership.
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::OwnerThreadUnresponsive);
    unsafe { ffi::CFRunLoopStop(ffi::CFRunLoopGetCurrent()) };
}

fn poll_shutdown_drain(context: &CallbackContext) {
    if context.state.recovery_pending.load(Ordering::Acquire) {
        arm_maintenance_timer(context);
        return;
    }
    if !context.state.stopping.load(Ordering::Acquire) {
        return;
    }
    let keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            poisoned.into_inner()
        }
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if !keyboard.shutdown_requested {
        drop(keyboard);
        begin_owner_shutdown(context);
        return;
    }
    let now = Instant::now();
    let deadline = keyboard.shutdown_deadline;
    let deadline_reported = keyboard.shutdown_deadline_reported;
    drop(keyboard);
    let has_ownership = pending_native_work(context);
    match shutdown_drain_action(has_ownership, deadline, deadline_reported, now) {
        ShutdownDrainAction::Stop => {
            let _ = stop_owner_run_loop_if_drained(context);
        }
        ShutdownDrainAction::ReportUnresponsive => {
            if let Ok(mut keyboard) = context.keyboard.try_lock() {
                keyboard.shutdown_deadline_reported = true;
            }
            terminal_shutdown_incomplete(context);
        }
        ShutdownDrainAction::Continue => {}
    }
}

fn arm_maintenance_timer(context: &CallbackContext) {
    let _ = pending_native_work(context);
    let timer = context.state.maintenance_timer.load(Ordering::Acquire);
    if timer.is_null() {
        return;
    }
    // SAFETY: the owner timer is retained until run-loop cleanup.
    unsafe {
        ffi::CFRunLoopTimerSetNextFireDate(
            timer,
            ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
        );
    }
}

fn park_maintenance_timer_if_idle(context: &CallbackContext) {
    if pending_native_work(context) {
        return;
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_tap_disable_request.is_some() {
        let timer = context.state.maintenance_timer.load(Ordering::Acquire);
        if !timer.is_null() {
            unsafe {
                ffi::CFRunLoopTimerSetNextFireDate(
                    timer,
                    ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
                );
            }
        }
        return;
    }
    let timer = context.state.maintenance_timer.load(Ordering::Acquire);
    if !timer.is_null() {
        // SAFETY: the timer is retained until owner cleanup.
        unsafe {
            ffi::CFRunLoopTimerSetNextFireDate(
                timer,
                ffi::CFAbsoluteTimeGetCurrent() + PARKED_TIMER_SECONDS,
            );
        }
    }
}

const fn owned_native_transition_is_terminal(
    ownership_pending: bool,
    secure_input: bool,
    permissions_available: bool,
) -> bool {
    ownership_pending && (secure_input || !permissions_available)
}

fn insertion_has_irreversible_authority(status: InsertionStatus) -> bool {
    matches!(
        status,
        InsertionStatus::Claimed | InsertionStatus::Ambiguous
    )
}

fn irreversible_native_ownership(context: &CallbackContext) -> bool {
    let keyboard_owned = context
        .keyboard
        .try_lock()
        .map_or(true, |keyboard| keyboard_has_pending_native_work(&keyboard));
    let deferred_owned = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.has_pending());
    let paste_owned = context.pending_paste.try_lock().map_or(true, |pending| {
        pending.as_ref().is_some_and(|command| {
            (command.neutral_barrier_token.is_some() && command.neutral_barrier_state < 2)
                || command
                    .insertion_request
                    .as_ref()
                    .is_some_and(|request| insertion_has_irreversible_authority(request.status()))
        })
    });
    context.state.recovery_pending.load(Ordering::Acquire)
        || context.state.recovery_deferred_mode.load(Ordering::Acquire)
        || keyboard_owned
        || deferred_owned
        || paste_owned
}

fn native_permissions_available() -> bool {
    let granted = permissions_allow_native_input(permission_snapshot());
    #[cfg(feature = "transactional-shortcuts-dev")]
    {
        granted && !super::macos_test_permission_loss_active()
    }
    #[cfg(not(feature = "transactional-shortcuts-dev"))]
    {
        granted
    }
}

fn close_owned_native_admission(
    context: &CallbackContext,
    reason: TerminalReason,
    cancellation: CancelReason,
) {
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context.terminal.trigger(reason);
    if let Ok(mut keyboard) = context.keyboard.try_lock()
        && !keyboard.has_submitted_replay_authority()
    {
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::CloseAdmission(cancellation),
        );
    }
}

fn monitor_owned_native_state(context: &CallbackContext) {
    let replay_observation_timed_out = context.keyboard.try_lock().map_or(true, |keyboard| {
        keyboard
            .replay_observation
            .as_ref()
            .is_some_and(|observation| Instant::now() >= observation.deadline)
    });
    let deferred_observation_timed_out =
        context.recovery_edges.try_lock().map_or(true, |journal| {
            journal
                .submission_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        });
    if deferred_observation_timed_out && let Ok(mut journal) = context.recovery_edges.try_lock() {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    // A fully observed barrier followed by Pending AX work is still safely
    // cancellable clipboard-only. Only actual native/replay ownership or an
    // explicit AX claim makes a Secure Input/permission transition terminal.
    let ownership_pending = irreversible_native_ownership(context);
    let secure = secure_input_active();
    let permissions_available = native_permissions_available();
    if !replay_observation_timed_out
        && !deferred_observation_timed_out
        && !owned_native_transition_is_terminal(ownership_pending, secure, permissions_available)
    {
        return;
    }
    close_owned_native_admission(
        context,
        if replay_observation_timed_out || deferred_observation_timed_out {
            TerminalReason::InputInjectionUnavailable
        } else if secure {
            TerminalReason::EventTapDisabledByUserInput
        } else {
            TerminalReason::InputInjectionUnavailable
        },
        CancelReason::SecureDesktop,
    );
    keep_strict_drain_tap_enabled(context);
}

fn has_session_ownership(keyboard: &CallbackKeyboard) -> bool {
    keyboard.session_escape_native_owned || keyboard.session_enter_native_owned.is_some()
}

fn native_modifiers_neutral() -> bool {
    MODIFIER_KEY_CODES
        .into_iter()
        .all(|key_code| !native_key_is_down(key_code))
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

fn reconcile_hidden_session_native_ownership(
    keyboard: &mut CallbackKeyboard,
    escape_is_down: bool,
    enter_is_down: bool,
) -> (bool, bool, Option<u16>) {
    let captured_enter = keyboard.session_enter_native_owned;
    let escape_cleared = keyboard.session_escape_native_owned && !escape_is_down;
    let enter_cleared = keyboard.session_enter_native_owned.is_some() && !enter_is_down;
    let _ = keyboard
        .reducer
        .reconcile_hidden_session_releases(escape_is_down, enter_is_down);
    if escape_cleared {
        keyboard.session_escape_native_owned = false;
    }
    if enter_cleared {
        keyboard.session_enter_native_owned = None;
        keyboard.captured_enter_key_code = None;
    }
    (escape_cleared, enter_cleared, captured_enter)
}

fn reconcile_strict_drain_after_gap(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if keyboard.has_submitted_replay_authority() {
        // The disabled tap is re-enabled by the caller so the original HID
        // suffix can continue to reach this exact observation cursor. A gap
        // snapshot/control turn here would overwrite immutable authority.
        arm_maintenance_timer(context);
        return;
    }
    let owned_before = keyboard.transactional.owned_letters();
    let escape_owned = keyboard.session_escape_native_owned;
    let enter_owned = keyboard.session_enter_native_owned.is_some();
    let cleanup_pending = keyboard.transactional.pending_injected_cleanup().is_some()
        || keyboard.transactional.pending_menu_cleanup().is_some();
    let physical_fences_pending = context
        .recovery_edges
        .try_lock()
        .map_or(true, |journal| journal.physical_fences_pending());
    if owned_before == 0
        && !escape_owned
        && !enter_owned
        && !cleanup_pending
        && !physical_fences_pending
    {
        return;
    }

    // A disabled-tap notification is the proof of a callback gap. While this
    // owner callback is linearized, rebuild native state and let the shared
    // engine intersect committed ownership with authoritative held keys.
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(&keyboard);
    let _ = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    let cleared_letters = owned_before & !snapshot.held_letters;

    let escape_is_down = native_key_is_down(ESCAPE_KEY_CODE);
    let captured_enter = keyboard.session_enter_native_owned;
    let enter_is_down = captured_enter.is_some_and(native_key_is_down);
    let (escape_cleared, enter_cleared, _) =
        reconcile_hidden_session_native_ownership(&mut keyboard, escape_is_down, enter_is_down);

    if cleared_letters != 0 || escape_cleared || enter_cleared || physical_fences_pending {
        keyboard.gap_reconciled_letters |= cleared_letters;
        keyboard.gap_reconciled_escape |= escape_cleared;
        if enter_cleared {
            keyboard.gap_reconciled_enter_key_code = captured_enter;
        }
        keyboard.gap_barrier_pending = true;
        keyboard.gap_barrier_observed_down = false;
    }
    // Reconciliation may leave a retained helper-injected release obligation.
    // Retry it on the owner before deciding that shutdown is drain-complete.
    let _ = begin_transaction_control(context, &mut keyboard, Control::RetryCleanup);
}

fn keep_strict_drain_tap_enabled(context: &CallbackContext) {
    let ownership_pending = pending_native_work(context);
    if !ownership_pending {
        return;
    }
    reconcile_strict_drain_after_gap(context);
    let tap = context.state.event_tap.load(Ordering::Acquire);
    if tap.is_null() {
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
        return;
    }
    // SAFETY: reconciliation runs while the tap is disabled. Re-enable only
    // after the owner snapshot and tombstones are installed.
    let enabled = unsafe {
        ffi::CGEventTapEnable(tap, true);
        ffi::CGEventTapIsEnabled(tap)
    };
    if !enabled {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::OwnerThreadUnresponsive);
        return;
    }
    let barrier_needed = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
        if keyboard.gap_barrier_pending && keyboard.gap_barrier_token.is_none() {
            keyboard.gap_barrier_observed_down = false;
            true
        } else {
            false
        }
    });
    if barrier_needed {
        let Some((mut native_events, barrier)) =
            context
                .native_events
                .try_lock()
                .ok()
                .and_then(|mut events| {
                    let barrier = injection::prepare_gap_barrier(events.as_mut()?)?;
                    Some((events, barrier))
                })
        else {
            // No timer fallback may discard tombstones; retain drain and retry.
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return;
        };
        let installed = context.keyboard.try_lock().is_ok_and(|mut keyboard| {
            if keyboard.gap_barrier_pending && keyboard.gap_barrier_token.is_none() {
                keyboard.gap_barrier_token = Some(barrier.token());
                true
            } else {
                false
            }
        });
        if installed {
            barrier.post(
                native_events
                    .as_mut()
                    .expect("native event pool prepared the gap barrier"),
            );
        }
    }
}

fn complete_gap_barrier(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => return,
    };
    keyboard.clear_gap_tombstones();
    if let Ok(mut journal) = context.recovery_edges.try_lock() {
        journal.reconcile_physical_fences(native_key_is_down, native_mouse_button_is_down);
    }
    let shutdown_may_drain =
        context.state.stopping.load(Ordering::Acquire) && keyboard.shutdown_requested;
    drop(keyboard);
    let _ = try_clear_recovery_deferred_mode(context);
    if shutdown_may_drain {
        let _ = stop_owner_run_loop_if_drained(context);
    }
}

fn invalidate_target_cache(context: &CallbackContext) {
    if let Some(cache) = &context.target_cache {
        cache.invalidate_boundary();
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
        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            let _ = begin_transaction_control(
                context,
                &mut keyboard,
                Control::CloseAdmission(CancelReason::SecureDesktop),
            );
            deliver_balancing_events(context, &mut keyboard.reducer);
        }
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context.terminal.trigger(reason);
    }
    decision
}

fn handle_user_input_tap_disable(context: &CallbackContext) {
    let submitted_replay = context
        .keyboard
        .try_lock()
        .is_ok_and(|keyboard| keyboard.has_submitted_replay_authority());
    if !submitted_replay {
        let _ = resolve_pending_activation(
            context,
            if context.state.stopping.load(Ordering::Acquire) || context.terminal.is_triggered() {
                PendingActivationResolution::FailDelivery
            } else {
                PendingActivationResolution::ForceTargetless
            },
        );
    }
    invalidate_target_cache(context);
    if !context.state.quiescing.load(Ordering::Acquire) {
        let _ = apply_tap_recovery(context, TapRecoveryEvent::DisabledByUserInput);
    }
    // User-input disable is a callback gap. If ownership survives, re-enable
    // only the strict drain tap after HID reconciliation.
    keep_strict_drain_tap_enabled(context);
}

fn resynchronize_after_gap(context: &CallbackContext) {
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return;
        }
    };
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(&keyboard);
    let outcome = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

struct CallbackProxyGuard<'a>(&'a AtomicPtr<c_void>);

impl Drop for CallbackProxyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(null_mut(), Ordering::Release);
    }
}

fn recovery_drain_disposition(
    context: &CallbackContext,
    disposition: CurrentEdgeDisposition,
) -> CurrentEdgeDisposition {
    set_atomic_current_edge_disposition(context, disposition);
    disposition
}

fn recovery_test_physical_source(context: &CallbackContext, marker: i64) -> bool {
    #[cfg(test)]
    if marker == TEST_RECOVERY_PHYSICAL_MARKER {
        return true;
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled && injection::is_test_physical_marker(marker) {
        return true;
    }
    let _ = (context.test_physical_seam_enabled, marker);
    false
}

const fn is_mouse_event_type(event_type: u32) -> bool {
    matches!(
        event_type,
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
            | ffi::K_CG_EVENT_LEFT_MOUSE_UP
            | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
            | ffi::K_CG_EVENT_RIGHT_MOUSE_UP
            | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
            | ffi::K_CG_EVENT_OTHER_MOUSE_UP
    )
}

const fn mouse_event_is_down(event_type: u32) -> bool {
    matches!(
        event_type,
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
            | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
            | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
    )
}

fn recovery_ordering_exists(context: &CallbackContext) -> bool {
    // Release-published before recovery-only suppression and retained through
    // every exact tail rollover. A normal transactional candidate alone is
    // never authority to defer foreground input.
    context.state.recovery_deferred_mode.load(Ordering::Acquire)
}

fn fail_recovery_edge_journal(context: &CallbackContext) {
    context
        .state
        .recovery_deferred_mode
        .store(true, Ordering::Release);
    context.gate.close();
    context.state.quiescing.store(true, Ordering::Release);
    context.state.stopping.store(true, Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    context
        .terminal
        .trigger(TerminalReason::InputInjectionUnavailable);
    arm_maintenance_timer(context);
}

fn deferred_mouse_button(event_type: u32, event: ffi::CGEventRef) -> Option<u32> {
    let reported =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER) };
    u32::try_from(reported).ok().or(match event_type {
        ffi::K_CG_EVENT_LEFT_MOUSE_DOWN | ffi::K_CG_EVENT_LEFT_MOUSE_UP => Some(0),
        ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN | ffi::K_CG_EVENT_RIGHT_MOUSE_UP => Some(1),
        ffi::K_CG_EVENT_OTHER_MOUSE_DOWN | ffi::K_CG_EVENT_OTHER_MOUSE_UP => Some(2),
        _ => None,
    })
}

fn deferred_edge_matches_observed(
    expected: injection::DeferredEvent,
    event_type: u32,
    event: ffi::CGEventRef,
) -> bool {
    if event_type != expected.event_type || unsafe { ffi::CGEventGetFlags(event) } != expected.flags
    {
        return false;
    }
    if expected.is_mouse() {
        let location = unsafe { ffi::CGEventGetLocation(event) };
        let button =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER) };
        let click =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE) };
        let number =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_NUMBER) };
        let pressure =
            unsafe { ffi::CGEventGetDoubleValueField(event, ffi::K_CG_MOUSE_EVENT_PRESSURE) };
        let delta_x =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_X) };
        let delta_y =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_Y) };
        let instant_mouser = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
        };
        let subtype =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_SUBTYPE) };
        location == expected.location
            && button == expected.mouse_button
            && click == expected.mouse_click_state
            && number == expected.mouse_number
            && pressure == expected.mouse_pressure
            && delta_x == expected.mouse_delta_x
            && delta_y == expected.mouse_delta_y
            && instant_mouser == expected.mouse_instant_mouser
            && subtype == expected.mouse_subtype
    } else {
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        let keyboard_type = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        };
        key_code == i64::from(expected.key_code)
            && repeat == expected.repeat
            && keyboard_type == expected.keyboard_type
    }
}

fn observe_deferred_repost(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> Option<CurrentEdgeDisposition> {
    if event.is_null() {
        return None;
    }
    let identity = context.injection_identity?;
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let mut journal = context.recovery_edges.try_lock().ok()?;
    if !injection::token_matches(identity, journal.token, marker, source_pid) {
        return None;
    }
    let Some(expected) = journal.expected() else {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        drop(journal);
        fail_recovery_edge_journal(context);
        return Some(CurrentEdgeDisposition::Owned);
    };
    if !deferred_edge_matches_observed(expected, event_type, event) {
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        drop(journal);
        fail_recovery_edge_journal(context);
        return Some(CurrentEdgeDisposition::Owned);
    }
    // The original was suppressed. Publish Pass before advancing the exact
    // tagged replacement so unwind cannot lose a foreground-visible edge.
    set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Pass);
    journal.observe_foreground_exposure(expected);
    let observation = journal.advance_observation();
    drop(journal);
    if observation == GapBarrierObservation::Complete {
        let _ = try_clear_recovery_deferred_mode(context);
    }
    arm_maintenance_timer(context);
    Some(CurrentEdgeDisposition::Pass)
}

fn defer_callback_edge(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    source: InputSource,
    source_pid: i64,
    original_marker: i64,
    uncertain_owned: bool,
) -> CurrentEdgeDisposition {
    if event.is_null() {
        fail_recovery_edge_journal(context);
        return CurrentEdgeDisposition::Owned;
    }
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let timestamp = unsafe { ffi::CGEventGetTimestamp(event) };
    let mut journal = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        }
    };
    let mut edge = injection::DeferredEvent {
        event_type,
        flags,
        original_timestamp: timestamp,
        source,
        source_pid,
        original_marker,
        uncertain_owned,
        ..injection::DeferredEvent::EMPTY
    };

    if is_mouse_event_type(event_type) {
        let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
        else {
            journal.enter_overflow();
            journal.settle_overflow();
            drop(journal);
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        };
        let foreground_was_down = journal.foreground_mouse_was_down(source, source_pid, button);
        let phase = journal.observe_mouse_phase(
            source,
            source_pid,
            button,
            mouse_event_is_down(event_type),
            foreground_was_down,
        );
        let (foreground_balance, hidden_generation) = match phase {
            Some(phase) => phase,
            None if source == InputSource::External => (false, false),
            None => {
                journal.enter_overflow();
                journal.settle_overflow();
                drop(journal);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        edge.is_down = mouse_event_is_down(event_type);
        edge.foreground_balance = foreground_balance;
        edge.hidden_generation = hidden_generation;
        edge.location = unsafe { ffi::CGEventGetLocation(event) };
        edge.mouse_number =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_NUMBER) };
        edge.mouse_click_state =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE) };
        edge.mouse_pressure =
            unsafe { ffi::CGEventGetDoubleValueField(event, ffi::K_CG_MOUSE_EVENT_PRESSURE) };
        edge.mouse_button = i64::from(button);
        edge.mouse_delta_x =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_X) };
        edge.mouse_delta_y =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_Y) };
        edge.mouse_instant_mouser = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
        };
        edge.mouse_subtype =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_SUBTYPE) };
    } else {
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let Ok(key_code) = u16::try_from(key_code)
            .ok()
            .filter(|key| *key < 128)
            .ok_or(())
        else {
            journal.enter_overflow();
            journal.settle_overflow();
            drop(journal);
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        };
        let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        edge.keyboard_type = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        };

        let tracked_was_down = if source.is_physical() {
            context.keyboard.try_lock().ok().map(|keyboard| {
                if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
                    modifier_side_for_key_code(key_code)
                        .is_some_and(|side| keyboard.modifiers.sides().contains(side))
                } else {
                    keyboard.physical.is_held(key_code)
                }
            })
        } else {
            Some(journal.key_was_down(source, source_pid, key_code))
        };
        if source.is_physical()
            && event_type == ffi::K_CG_EVENT_FLAGS_CHANGED
            && let Some(was_down) = tracked_was_down
        {
            journal.seed_modifier_side(source, source_pid, key_code, was_down);
        }
        let mut foreground_was_down = tracked_was_down.unwrap_or(false);
        let mut source_model_available = true;
        let is_down = if event_type == ffi::K_CG_EVENT_KEY_UP {
            false
        } else if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
            let physical_hid =
                (source == InputSource::Physical).then(|| native_key_is_down(key_code));
            match journal.modifier_side_transition(source, source_pid, key_code, physical_hid) {
                Some((transition, side_was_down)) => {
                    foreground_was_down = side_was_down;
                    transition
                }
                None if source == InputSource::External => {
                    // External flagsChanged is replayed from its exact scalar
                    // event shape; phase modeling is not required for ordering.
                    source_model_available = false;
                    false
                }
                None => {
                    journal.enter_overflow();
                    journal.settle_overflow();
                    drop(journal);
                    fail_recovery_edge_journal(context);
                    return CurrentEdgeDisposition::Owned;
                }
            }
        } else {
            true
        };
        if is_down && !repeat {
            // A nonrepeat down is a new deferred generation. The recovery
            // classifier may already have updated the native tracker for this
            // exact edge; that post-edge state is not a foreground baseline.
            foreground_was_down = false;
        }
        let modeled_phase = source_model_available.then(|| {
            journal.observe_key_phase(
                source,
                source_pid,
                key_code,
                is_down,
                repeat,
                foreground_was_down,
            )
        });
        let (generation, foreground_balance, hidden_generation) = match modeled_phase.flatten() {
            Some(phase) => phase,
            None if source == InputSource::External => (0, false, false),
            None => {
                journal.enter_overflow();
                journal.settle_overflow();
                drop(journal);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        edge.key_code = key_code;
        edge.repeat = repeat;
        edge.is_down = is_down;
        edge.generation = generation;
        edge.foreground_balance = foreground_balance;
        edge.hidden_generation = hidden_generation;
        if source.is_physical()
            && let Ok(mut keyboard) = context.keyboard.try_lock()
        {
            if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
                let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
            } else {
                let phase = if is_down {
                    KeyPhase::Down
                } else {
                    KeyPhase::Up
                };
                let _ = keyboard.physical.observe(key_code, phase);
            }
        }
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled && injection::is_test_physical_marker(original_marker)
        {
            super::observe_macos_test_physical_key(key_code, is_down);
        }
    }

    let appended = journal.append(edge);
    let overflow = journal.overflow;
    journal.settle_overflow();
    drop(journal);
    if !appended || overflow {
        // Capacity loss is terminal for admission, but only concrete balancing
        // suffixes and already-submitted exact observations retain lifetime.
        fail_recovery_edge_journal(context);
    } else {
        arm_maintenance_timer(context);
    }
    CurrentEdgeDisposition::Owned
}
fn observe_normal_mouse_transition(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> bool {
    if !is_mouse_event_type(event_type) || event.is_null() {
        return true;
    }
    let Some(identity) = context.injection_identity else {
        return false;
    };
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let test_physical = recovery_test_physical_source(context, marker);
    if source_pid == identity.source_pid && !test_physical {
        return true;
    }
    let source = if test_physical {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
    else {
        return false;
    };
    let Ok(mut journal) = context.recovery_edges.try_lock() else {
        return false;
    };
    journal.normal_mouse_transition(source, source_pid, button, mouse_event_is_down(event_type));
    true
}

fn discard_overflow_fenced_nonhelper(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    source: InputSource,
    source_pid: i64,
) -> bool {
    if event.is_null() {
        return false;
    }
    let Ok(mut journal) = context.recovery_edges.try_lock() else {
        return false;
    };
    if is_mouse_event_type(event_type) {
        let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
        else {
            return false;
        };
        if !journal.mouse_is_discard_fenced(source, source_pid, button) {
            return false;
        }
        journal.consume_mouse_discard_fence(
            source,
            source_pid,
            button,
            mouse_event_is_down(event_type),
        );
        drop(journal);
        let _ = try_clear_recovery_deferred_mode(context);
        return true;
    }
    if !matches!(
        event_type,
        ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
    ) {
        return false;
    }
    let key_code =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let Ok(key_code) = u16::try_from(key_code)
        .ok()
        .filter(|key| *key < 128)
        .ok_or(())
    else {
        return false;
    };
    if !journal.key_is_discard_fenced(source, source_pid, key_code) {
        return false;
    }
    let is_down = if event_type == ffi::K_CG_EVENT_KEY_UP {
        false
    } else if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        let physical_hid = (source == InputSource::Physical).then(|| native_key_is_down(key_code));
        journal
            .modifier_side_transition(source, source_pid, key_code, physical_hid)
            .is_some_and(|(is_down, _)| is_down)
    } else {
        true
    };
    journal.consume_key_discard_fence(source, source_pid, key_code, is_down);
    drop(journal);
    if source.is_physical()
        && let Ok(mut keyboard) = context.keyboard.try_lock()
    {
        if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
            let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
        } else {
            let _ = keyboard.physical.observe(
                key_code,
                if is_down {
                    KeyPhase::Down
                } else {
                    KeyPhase::Up
                },
            );
        }
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled
        && recovery_test_physical_source(context, unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA)
        })
    {
        super::observe_macos_test_physical_key(key_code, is_down);
    }
    let _ = try_clear_recovery_deferred_mode(context);
    true
}

fn defer_nonhelper_if_ordered(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> Option<CurrentEdgeDisposition> {
    if event.is_null()
        || (!is_mouse_event_type(event_type)
            && !matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
            ))
    {
        return None;
    }
    let identity = context.injection_identity?;
    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let test_physical = recovery_test_physical_source(context, marker);
    if source_pid == identity.source_pid && !test_physical {
        return None;
    }
    let source = if test_physical {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    if discard_overflow_fenced_nonhelper(context, event_type, event, source, source_pid) {
        return Some(CurrentEdgeDisposition::Owned);
    }
    if !recovery_ordering_exists(context) {
        return None;
    }
    Some(defer_callback_edge(
        context, event_type, event, source, source_pid, marker, false,
    ))
}

/// Allocation-free, nonblocking classifier used only while callback recovery
/// remains unresolved. It advances already-installed exact drain observations
/// and retires exact native ownership, but never invokes matcher/admission,
/// submits an effect, or begins a control turn.
fn classify_recovery_drain_event(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
) -> CurrentEdgeDisposition {
    if callback_recovery_requires_deferred_mode(context) {
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
    }
    // Suppress by default. Every Pass below is based on exact operation state or
    // a source/shape that cannot belong to retained helper/native ownership.
    set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Owned);

    if matches!(
        event_type,
        ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT | ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
    ) {
        let tap = context.state.event_tap.load(Ordering::Acquire);
        if !tap.is_null() {
            // Keep the drain tap alive without reconciliation/control while
            // poison recovery is unresolved.
            unsafe { ffi::CGEventTapEnable(tap, true) };
        }
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }
    if event.is_null() {
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    if is_mouse_event_type(event_type) {
        let marker =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        if context.injection_identity.is_some_and(|identity| {
            source_pid == identity.source_pid && !recovery_test_physical_source(context, marker)
        }) {
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        }
        if let Some(disposition) = defer_nonhelper_if_ordered(context, event_type, event) {
            return recovery_drain_disposition(context, disposition);
        }
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }

    if !matches!(
        event_type,
        ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
    ) {
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }

    let marker =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA) };
    let source_pid =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID) };
    let key_code_raw =
        unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
    let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
        && unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
        };
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let Some(identity) = context.injection_identity else {
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    };
    let test_physical_source = recovery_test_physical_source(context, marker);

    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled && marker == injection::TEST_PERMISSION_LOSS_MARKER {
        // Test controls are ordinary work and cannot run during recovery.
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    if source_pid == identity.source_pid && !test_physical_source {
        let Ok(key_code) = u16::try_from(key_code_raw) else {
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        };

        // The target-specific AX path retains only the neutral barrier token.
        if let Ok(mut pending) = context.pending_paste.try_lock()
            && let Some(command) = pending.as_mut()
            && injection::token_matches(identity, command.neutral_barrier_token, marker, source_pid)
        {
            let observation = observe_exact_pair(
                &mut command.neutral_barrier_state,
                event_type,
                key_code,
                127,
                repeat,
                flags,
                0,
            );
            #[cfg(feature = "transactional-shortcuts-dev")]
            if observation != GapBarrierObservation::Forged
                && context.test_physical_seam_enabled
                && let Some(token) = command.neutral_barrier_token
            {
                super::record_test_marker_acknowledgement(
                    super::MacosTestOperationClass::PasteBarrier,
                    token,
                );
            }
            let _ = observation;
            arm_maintenance_timer(context);
            return CurrentEdgeDisposition::Owned;
        }

        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            let gap_token = keyboard.gap_barrier_token;
            if injection::token_matches(identity, gap_token, marker, source_pid) {
                let observation =
                    keyboard.observe_gap_barrier_event(event_type, key_code, repeat, flags);
                #[cfg(feature = "transactional-shortcuts-dev")]
                if observation != GapBarrierObservation::Forged
                    && context.test_physical_seam_enabled
                    && let Some(token) = gap_token
                {
                    super::record_test_marker_acknowledgement(
                        super::MacosTestOperationClass::GapBarrier,
                        token,
                    );
                }
                if observation == GapBarrierObservation::Complete
                    && let Ok(mut journal) = context.recovery_edges.try_lock()
                {
                    journal
                        .reconcile_physical_fences(native_key_is_down, native_mouse_button_is_down);
                }
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
                arm_maintenance_timer(context);
                return CurrentEdgeDisposition::Owned;
            }

            let replay = keyboard.replay_observation.as_ref().map(|observation| {
                (
                    observation.token,
                    matches!(observation.batch, ExpectedReplayBatch::Cleanup(_)),
                )
            });
            if injection::token_matches(
                identity,
                replay.map(|(token, _)| token),
                marker,
                source_pid,
            ) {
                let observation =
                    keyboard.classify_replay_event(event_type, key_code, repeat, flags);
                if observation != GapBarrierObservation::Forged {
                    let cleanup = replay.is_some_and(|(_, cleanup)| cleanup);
                    let target_is_current = !keyboard.candidate_target_captured
                        || keyboard.candidate_target.is_some_and(|reservation| {
                            context
                                .target_cache
                                .as_ref()
                                .is_some_and(|cache| cache.reservation_is_current(&reservation))
                        });
                    let disposition = keyboard
                        .replay_disposition_after_target_check(cleanup || target_is_current);
                    // Publish the exact foreground disposition before advancing
                    // the authenticated cursor. A target change suppresses the
                    // entire remaining suffix instead of exposing it elsewhere.
                    set_current_edge_disposition(context, &mut keyboard, disposition);
                    let advanced =
                        keyboard.observe_replay_event(event_type, key_code, repeat, flags);
                    debug_assert_eq!(advanced, observation);
                    if advanced == GapBarrierObservation::Complete {
                        let completion = if keyboard.replay_target_cleanup_pending && cleanup {
                            Some(finish_target_changed_replay(context, &mut keyboard))
                        } else if keyboard.replay_target_changed && !cleanup {
                            Some(reconcile_target_changed_replay(context, &mut keyboard))
                        } else {
                            (!mark_observed_native_effect(&mut keyboard))
                                .then_some(DriveCompletion::Failed)
                        };
                        if completion.is_some_and(|completion| {
                            !matches!(
                                completion,
                                DriveCompletion::Complete(_)
                                    | DriveCompletion::NativeObservationPending
                            )
                        }) {
                            context.terminal.trigger(TerminalReason::ReducerPoisoned);
                        }
                    }
                    #[cfg(feature = "transactional-shortcuts-dev")]
                    if context.test_physical_seam_enabled
                        && let Some((token, cleanup)) = replay
                    {
                        super::record_test_marker_acknowledgement(
                            if cleanup {
                                super::MacosTestOperationClass::Cleanup
                            } else {
                                super::MacosTestOperationClass::Replay
                            },
                            token,
                        );
                    }
                    arm_maintenance_timer(context);
                    return disposition;
                }
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
            }
        }

        // Own-process events are never unrelated. Busy/poison-unresolved state,
        // stale generations, and malformed exact shapes all remain suppressed.
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    let source = if test_physical_source {
        InputSource::test_physical()
    } else {
        injection::unmarked_source(identity, source_pid)
    };
    if !source.is_physical() {
        if recovery_ordering_exists(context) {
            return defer_callback_edge(
                context, event_type, event, source, source_pid, marker, false,
            );
        }
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    }
    let Ok(key_code) = u16::try_from(key_code_raw) else {
        arm_maintenance_timer(context);
        return recovery_drain_disposition(context, CurrentEdgeDisposition::Pass);
    };
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            // Preserve the complete scalar edge in the independent journal.
            // Recovery resolves whether it belonged to old native ownership
            // before any deferred batch can be submitted.
            return defer_callback_edge(
                context, event_type, event, source, source_pid, marker, true,
            );
        }
    };
    keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;

    if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        // `defer_callback_edge` owns side-specific HID/tracker/source-model
        // advancement. Never infer one side from the aggregate family flag.
        drop(keyboard);
        return defer_callback_edge(
            context, event_type, event, source, source_pid, marker, false,
        );
    }

    let phase = if event_type == ffi::K_CG_EVENT_KEY_UP {
        KeyPhase::Up
    } else {
        KeyPhase::Down
    };
    if keyboard.handle_gap_tombstone(key_code, phase, repeat) {
        drop(keyboard);
        arm_maintenance_timer(context);
        return CurrentEdgeDisposition::Owned;
    }

    let key = map_key_code(key_code);
    let transactional_owned = match key {
        PhysicalKey::Letter(letter) => {
            keyboard.transactional.owned_letters() & (1_u32 << u32::from(letter.index())) != 0
        }
        PhysicalKey::Escape | PhysicalKey::Enter | PhysicalKey::Other => false,
    };
    let session_owned = match key {
        PhysicalKey::Escape => keyboard.session_escape_native_owned,
        PhysicalKey::Enter => keyboard.session_enter_native_owned == Some(key_code),
        PhysicalKey::Letter(_) | PhysicalKey::Other => false,
    };
    let ordering_sensitive =
        keyboard.transactional.journal_len() != 0 || keyboard.inflight_effect.is_some();

    let mut fresh_owned_generation = false;
    if transactional_owned {
        let mut journal = match context.recovery_edges.try_lock() {
            Ok(journal) => journal,
            Err(_) => {
                drop(keyboard);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        if phase == KeyPhase::Up && !journal.physical_newer_generation_held(key_code) {
            journal.record_owned_release(key_code);
        } else if journal.owned_release_recorded(key_code) {
            fresh_owned_generation = (phase == KeyPhase::Down && !repeat)
                || journal.physical_newer_generation_held(key_code);
        }
    }
    if phase == KeyPhase::Up && session_owned {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = None;
                keyboard.captured_enter_key_code = None;
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
        let escape_still_down = keyboard.session_escape_native_owned;
        let enter_still_down = keyboard.session_enter_native_owned.is_some();
        let _ = keyboard
            .reducer
            .reconcile_hidden_session_releases(escape_still_down, enter_still_down);
    }

    // A nonrepeat down retires a gap tombstone and starts a new generation.
    // Once an old owned up was observed, that fresh generation and all of its
    // repeats/up are deferred rather than confused with stale ownership.
    let deferred_ordering = ordering_sensitive
        || context
            .recovery_edges
            .try_lock()
            .map_or(true, |journal| journal.ordering_pending());
    let owned_old_edge = (transactional_owned && !fresh_owned_generation) || session_owned;
    if !owned_old_edge && (deferred_ordering || fresh_owned_generation) {
        drop(keyboard);
        return defer_callback_edge(
            context, event_type, event, source, source_pid, marker, false,
        );
    }
    let _ = keyboard.physical.observe(key_code, phase);
    let disposition = if owned_old_edge {
        CurrentEdgeDisposition::Owned
    } else {
        CurrentEdgeDisposition::Pass
    };
    keyboard.current_edge_disposition = disposition;
    drop(keyboard);
    arm_maintenance_timer(context);
    recovery_drain_disposition(context, disposition)
}

fn repost_current_mouse_after_replay(proxy: ffi::CGEventTapProxy, event: ffi::CGEventRef) -> bool {
    #[cfg(test)]
    if proxy.is_null() {
        return true;
    }
    // SAFETY: production callers provide the live callback proxy and the
    // current event remains valid for the complete callback invocation.
    unsafe { ffi::CGEventTapPostEvent(proxy, event) };
    true
}

unsafe extern "C" fn event_tap_callback(
    proxy: ffi::CGEventTapProxy,
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
        // A closed process-lifetime gate is a passive, listen-only observer.
        // Bypass all reducer, ownership, recovery, and notification paths even
        // if a future configuration bug attempts to arm native capture.
        if !context.suppression_enabled {
            return false;
        }
        context.callback_proxy.store(proxy, Ordering::Release);
        let _proxy_guard = CallbackProxyGuard(&context.callback_proxy);
        if let Some(disposition) = observe_deferred_repost(context, event_type, event) {
            return disposition != CurrentEdgeDisposition::Pass;
        }
        if context.state.recovery_pending.load(Ordering::Acquire)
            && !attempt_pending_recovery(context)
        {
            return classify_recovery_drain_event(context, event_type, event)
                != CurrentEdgeDisposition::Pass;
        }
        if let Some(disposition) = defer_nonhelper_if_ordered(context, event_type, event) {
            set_atomic_current_edge_disposition(context, disposition);
            return disposition != CurrentEdgeDisposition::Pass;
        }
        let mouse_down = matches!(
            event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
                | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
                | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
        );
        if !mouse_down && !observe_normal_mouse_transition(context, event_type, event) {
            recover_callback_unwind(context);
            return true;
        }
        set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Pass);
        if let Ok(mut keyboard) = context.keyboard.try_lock() {
            keyboard.current_edge_disposition = CurrentEdgeDisposition::Pass;
        }
        if mouse_down {
            let _ =
                resolve_pending_activation(context, PendingActivationResolution::ForceTargetless);
            invalidate_target_cache(context);
            let mut keyboard = match context.keyboard.try_lock() {
                Ok(keyboard) => keyboard,
                Err(_) => {
                    recover_callback_unwind(context);
                    return true;
                }
            };
            if keyboard.transactional.journal_len() == 0 {
                drop(keyboard);
                if !observe_normal_mouse_transition(context, event_type, event) {
                    recover_callback_unwind(context);
                    return true;
                }
                return false;
            }
            let outcome = begin_transaction_control(
                context,
                &mut keyboard,
                Control::Cancel(CancelReason::InvalidContinuation),
            );
            if !outcome.applied || keyboard.last_native_effect_failed {
                let native_failed = keyboard.last_native_effect_failed
                    || outcome.cancellation == Some(CancelReason::EffectProtocolViolation);
                drop(keyboard);
                let marker = unsafe {
                    ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA)
                };
                let source_pid = unsafe {
                    ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
                };
                let source = if recovery_test_physical_source(context, marker) {
                    InputSource::test_physical()
                } else {
                    context
                        .injection_identity
                        .map_or(InputSource::Physical, |identity| {
                            injection::unmarked_source(identity, source_pid)
                        })
                };
                if native_failed {
                    fail_recovery_edge_journal(context);
                } else {
                    context
                        .state
                        .recovery_deferred_mode
                        .store(true, Ordering::Release);
                }
                set_atomic_current_edge_disposition(context, CurrentEdgeDisposition::Owned);
                let _ = defer_callback_edge(
                    context, event_type, event, source, source_pid, marker, false,
                );
                return true;
            }
            set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Replaced);
            drop(keyboard);
            // Replay was inserted through this same proxy first. Repost the
            // suppressed mouse event only afterwards, preserving replay ->
            // focus-change ordering without copying or allocating in callback.
            if repost_current_mouse_after_replay(proxy, event) {
                #[cfg(feature = "transactional-shortcuts-dev")]
                if context.test_physical_seam_enabled {
                    super::record_test_mouse_repost();
                }
                if !observe_normal_mouse_transition(context, event_type, event) {
                    recover_callback_unwind(context);
                }
            }
            return true;
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            handle_user_input_tap_disable(context);
            return false;
        }
        if event_type == ffi::K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
            let submitted_replay = context
                .keyboard
                .try_lock()
                .is_ok_and(|keyboard| keyboard.has_submitted_replay_authority());
            if !submitted_replay {
                let _ = resolve_pending_activation(
                    context,
                    if context.state.stopping.load(Ordering::Acquire)
                        || context.terminal.is_triggered()
                    {
                        PendingActivationResolution::FailDelivery
                    } else {
                        PendingActivationResolution::ForceTargetless
                    },
                );
            }
            invalidate_target_cache(context);
            let ownership_pending = pending_native_work(context);
            if ownership_pending {
                close_owned_native_admission(
                    context,
                    TerminalReason::EventTapTimeoutRecoveryFailed,
                    CancelReason::SecureDesktop,
                );
                keep_strict_drain_tap_enabled(context);
                return false;
            }
            if context.state.quiescing.load(Ordering::Acquire) {
                keep_strict_drain_tap_enabled(context);
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
            } else {
                keep_strict_drain_tap_enabled(context);
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
        let Some(injection_identity) = context.injection_identity else {
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        };
        let test_physical_source = recovery_test_physical_source(context, marker);
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled && marker == injection::TEST_PERMISSION_LOSS_MARKER {
            super::set_macos_test_permission_loss(true);
            monitor_owned_native_state(context);
            return true;
        }
        if source_pid == injection_identity.source_pid && !test_physical_source {
            context
                .current_edge_disposition
                .store(CurrentEdgeDisposition::Owned as u8, Ordering::Release);
            if let Ok(mut keyboard) = context.keyboard.try_lock() {
                keyboard.current_edge_disposition = CurrentEdgeDisposition::Owned;
            }
        }
        let gap_token = context
            .keyboard
            .try_lock()
            .ok()
            .and_then(|keyboard| keyboard.gap_barrier_token);
        if injection::token_matches(injection_identity, gap_token, marker, source_pid) {
            let key_code = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE)
            };
            let repeat = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
            let flags = unsafe { ffi::CGEventGetFlags(event) };
            let observation = u16::try_from(key_code)
                .ok()
                .and_then(|key_code| {
                    context.keyboard.try_lock().ok().map(|mut keyboard| {
                        keyboard.observe_gap_barrier_event(event_type, key_code, repeat, flags)
                    })
                })
                .unwrap_or(GapBarrierObservation::Forged);
            if observation == GapBarrierObservation::Complete {
                complete_gap_barrier(context);
            }
            if observation != GapBarrierObservation::Forged {
                #[cfg(feature = "transactional-shortcuts-dev")]
                if context.test_physical_seam_enabled
                    && let Some(token) = gap_token
                {
                    super::record_test_marker_acknowledgement(
                        super::MacosTestOperationClass::GapBarrier,
                        token,
                    );
                }
                return true;
            }
            // A correct nonce/PID with the wrong shape/order is not an
            // acknowledgement and must not mutate recovery state.
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        }
        let paste_barrier_token = context.pending_paste.try_lock().ok().and_then(|pending| {
            pending
                .as_ref()
                .and_then(|command| command.neutral_barrier_token)
        });
        if injection::token_matches(injection_identity, paste_barrier_token, marker, source_pid) {
            let key_code = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE)
            };
            let repeat = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
            let flags = unsafe { ffi::CGEventGetFlags(event) };
            let mut pending = match context.pending_paste.try_lock() {
                Ok(pending) => pending,
                Err(_) => {
                    context
                        .terminal
                        .trigger(TerminalReason::InputInjectionUnavailable);
                    return true;
                }
            };
            let observation = pending
                .as_mut()
                .and_then(|command| {
                    Some(observe_exact_pair(
                        &mut command.neutral_barrier_state,
                        event_type,
                        u16::try_from(key_code).ok()?,
                        127,
                        repeat,
                        flags,
                        0,
                    ))
                })
                .unwrap_or(GapBarrierObservation::Forged);
            #[cfg(feature = "transactional-shortcuts-dev")]
            if observation != GapBarrierObservation::Forged
                && context.test_physical_seam_enabled
                && let Some(token) = paste_barrier_token
            {
                super::record_test_marker_acknowledgement(
                    super::MacosTestOperationClass::PasteBarrier,
                    token,
                );
            }
            #[cfg(feature = "transactional-shortcuts-dev")]
            if observation == GapBarrierObservation::Down
                && context
                    .test_paste_barrier_split_active
                    .load(Ordering::Acquire)
            {
                // Callback work remains one release-store. Owner maintenance
                // performs all file I/O before releasing the prebuilt up.
                context
                    .test_paste_barrier_down_observed
                    .store(true, Ordering::Release);
                arm_maintenance_timer(context);
            }
            if observation == GapBarrierObservation::Complete {
                let post_result = (|| -> Result<(), PasteFailure> {
                    let command = pending.as_mut().ok_or(PasteFailure::Unavailable)?;
                    let validated_epoch = command
                        .validated_target_epoch
                        .ok_or(PasteFailure::Unavailable)?;
                    let validated_boundary_epoch = command
                        .validated_target_boundary_epoch
                        .ok_or(PasteFailure::Unavailable)?;
                    let validated_selected_range_epoch = command
                        .validated_selected_range_epoch
                        .ok_or(PasteFailure::Unavailable)?;
                    let cache = context
                        .target_cache
                        .as_ref()
                        .ok_or(PasteFailure::Unavailable)?;
                    if !cache.handle_is_current(
                        command.evidence,
                        validated_epoch,
                        validated_boundary_epoch,
                        validated_selected_range_epoch,
                    ) {
                        context.observability.record_target_validation_fallback();
                        return Err(PasteFailure::Unavailable);
                    }
                    let expected_modifier_epoch = command
                        .neutral_modifier_epoch
                        .ok_or(PasteFailure::ConflictingModifiers)?;
                    if !context.keyboard.try_lock().is_ok_and(|keyboard| {
                        keyboard.modifier_epoch == expected_modifier_epoch
                            && keyboard.modifiers.sides().bits() == 0
                    }) || !native_modifiers_neutral()
                    {
                        return Err(PasteFailure::ConflictingModifiers);
                    }
                    let _gate_lease = context
                        .gate
                        .try_acquire_delivery()
                        .ok_or(PasteFailure::Unavailable)?;
                    let now = Instant::now();
                    claim_paste_injection(
                        &command.state,
                        PastePostChecks {
                            admission_open: paste_admission_open(context),
                            before_deadline: paste_before_deadline(command.deadline, now),
                            before_injection_cutoff: paste_before_deadline(
                                command.injection_cutoff,
                                now,
                            ),
                            modifiers_neutral: true,
                            // Owner validation immediately before posting the
                            // barrier proved TCC grants. The callback performs
                            // no AX/permission messaging.
                            permissions_granted: true,
                            secure_input_inactive: !secure_input_active(),
                            target_valid: true,
                        },
                    )?;
                    let insertion = command
                        .insertion_request
                        .as_mut()
                        .ok_or(PasteFailure::Unavailable)?;
                    if cache.submit_insertion(insertion) {
                        Ok(())
                    } else {
                        Err(PasteFailure::Unavailable)
                    }
                })();
                if let Err(reason) = post_result {
                    finish_pending_paste(&mut pending, failed_paste(reason));
                }
                arm_maintenance_timer(context);
            } else if observation == GapBarrierObservation::Forged {
                finish_pending_paste(&mut pending, failed_paste(PasteFailure::OsRejected));
                context
                    .terminal
                    .trigger(TerminalReason::InputInjectionUnavailable);
            }
            return true;
        }
        let replay_operation = context.keyboard.try_lock().ok().and_then(|keyboard| {
            keyboard.replay_observation.as_ref().map(|work| {
                (
                    work.token,
                    matches!(work.batch, ExpectedReplayBatch::Cleanup(_)),
                )
            })
        });
        let replay_token = replay_operation.map(|(token, _)| token);
        if injection::token_matches(injection_identity, replay_token, marker, source_pid) {
            let key_code = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE)
            };
            let repeat = unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
            let flags = unsafe { ffi::CGEventGetFlags(event) };
            if !injection::replay_shape_is_valid(event_type, key_code, repeat) {
                context
                    .terminal
                    .trigger(TerminalReason::InputInjectionUnavailable);
                return true;
            }
            let (observation, replay_disposition) = u16::try_from(key_code)
                .ok()
                .and_then(|key_code| {
                    context.keyboard.try_lock().ok().map(|mut keyboard| {
                        let classified =
                            keyboard.classify_replay_event(event_type, key_code, repeat, flags);
                        if classified == GapBarrierObservation::Forged {
                            return (classified, CurrentEdgeDisposition::Owned);
                        }
                        let cleanup = replay_operation.is_some_and(|(_, cleanup)| cleanup);
                        let target_is_current = !keyboard.candidate_target_captured
                            || keyboard.candidate_target.is_some_and(|reservation| {
                                context
                                    .target_cache
                                    .as_ref()
                                    .is_some_and(|cache| cache.reservation_is_current(&reservation))
                            });
                        let disposition = keyboard
                            .replay_disposition_after_target_check(cleanup || target_is_current);
                        set_current_edge_disposition(context, &mut keyboard, disposition);
                        let observation =
                            keyboard.observe_replay_event(event_type, key_code, repeat, flags);
                        #[cfg(test)]
                        if PANIC_AFTER_REPLAY_RECOGNITION.with(|flag| flag.replace(false)) {
                            panic!("induced panic after replay recognition");
                        }
                        if observation == GapBarrierObservation::Complete {
                            let completion = if keyboard.replay_target_cleanup_pending && cleanup {
                                finish_target_changed_replay(context, &mut keyboard)
                            } else if keyboard.replay_target_changed && !cleanup {
                                reconcile_target_changed_replay(context, &mut keyboard)
                            } else {
                                resume_observed_native_effect(context, &mut keyboard)
                            };
                            if !matches!(
                                completion,
                                DriveCompletion::Complete(_)
                                    | DriveCompletion::NativeObservationPending
                            ) {
                                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                            }
                            set_current_edge_disposition(context, &mut keyboard, disposition);
                        }
                        (observation, disposition)
                    })
                })
                .unwrap_or((GapBarrierObservation::Forged, CurrentEdgeDisposition::Owned));
            if observation == GapBarrierObservation::Forged {
                context
                    .terminal
                    .trigger(TerminalReason::InputInjectionUnavailable);
                return true;
            }
            invalidate_target_cache(context);
            #[cfg(feature = "transactional-shortcuts-dev")]
            if observation != GapBarrierObservation::Forged
                && context.test_physical_seam_enabled
                && let Some((token, cleanup)) = replay_operation
            {
                super::record_test_marker_acknowledgement(
                    if cleanup {
                        super::MacosTestOperationClass::Cleanup
                    } else {
                        super::MacosTestOperationClass::Replay
                    },
                    token,
                );
            }
            if observation == GapBarrierObservation::Complete {
                let _ = try_clear_recovery_deferred_mode(context);
                if context.state.stopping.load(Ordering::Acquire) {
                    let _ = stop_owner_run_loop_if_drained(context);
                }
            }
            return replay_disposition != CurrentEdgeDisposition::Pass;
        }
        if source_pid == injection_identity.source_pid && !test_physical_source {
            // A delayed token from a completed operation, or any own-process
            // event that does not exactly match the currently installed
            // generation and shape, is never accepted as an acknowledgement.
            context
                .terminal
                .trigger(TerminalReason::InputInjectionUnavailable);
            return true;
        }
        let source = if test_physical_source {
            InputSource::Physical
        } else {
            injection::unmarked_source(injection_identity, source_pid)
        };
        if !resolve_pending_activation(context, PendingActivationResolution::ForceTargetless) {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return true;
        }
        // SAFETY: keycode, timestamp, flags and autorepeat are defined for the
        // keyboard event types included in this tap.
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let Ok(key_code) = u16::try_from(key_code) else {
            return false;
        };
        let flags = unsafe { ffi::CGEventGetFlags(event) };
        let event_timestamp = unsafe { ffi::CGEventGetTimestamp(event) };
        let native_repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        if source == InputSource::External
            && matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
            )
            && let Ok(mut journal) = context.recovery_edges.try_lock()
        {
            journal.normal_external_key_transition(
                source_pid,
                key_code,
                event_type == ffi::K_CG_EVENT_KEY_DOWN,
                native_repeat,
            );
        }
        let candidate_start = source.is_physical()
            && matches!(
                event_type,
                ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
            )
            && context.keyboard.try_lock().is_ok_and(|keyboard| {
                keyboard.transactional.event_starts_candidate(
                    transactional_key_identity(key_code),
                    if event_type == ffi::K_CG_EVENT_KEY_UP {
                        PhysicalPhase::Up
                    } else if native_repeat {
                        PhysicalPhase::Repeat
                    } else {
                        PhysicalPhase::Down
                    },
                )
            });
        let activation_reservation = candidate_start
            .then(|| {
                context
                    .target_cache
                    .as_ref()
                    .and_then(TargetCache::reserve_activation)
            })
            .flatten();
        #[cfg(test)]
        let activation_reservation =
            activation_reservation.or(context.forced_activation_reservation);
        // Reserve only at a possible activation boundary, then establish the
        // exact event epoch before any validation request can be queued.
        invalidate_target_cache(context);
        apply_tap_recovery(context, TapRecoveryEvent::Activity);
        process_transactional_event(
            context,
            CallbackEvent {
                event_ref: event,
                marker,
                event_type,
                key_code,
                native_repeat,
                flags,
                event_timestamp,
                source,
                source_pid,
                test_physical_source,
            },
            activation_reservation,
        )
    });

    match handled {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            if !user_info.is_null() {
                // SAFETY: context remains alive until owner-thread tap cleanup.
                let context = unsafe { &*user_info.cast::<CallbackContext>() };
                return match recover_callback_unwind(context) {
                    CurrentEdgeDisposition::Pass => event,
                    CurrentEdgeDisposition::Owned | CurrentEdgeDisposition::Replaced => null_mut(),
                };
            }
            event
        }
    }
}

#[derive(Clone, Copy)]
struct CallbackEvent {
    event_ref: ffi::CGEventRef,
    marker: i64,
    event_type: u32,
    key_code: u16,
    native_repeat: bool,
    flags: u64,
    event_timestamp: u64,
    source: InputSource,
    source_pid: i64,
    test_physical_source: bool,
}

fn process_transactional_event(
    context: &CallbackContext,
    event: CallbackEvent,
    activation_reservation: Option<ActivationReservation>,
) -> bool {
    let CallbackEvent {
        event_ref,
        marker,
        event_type,
        key_code,
        native_repeat,
        flags,
        event_timestamp,
        source,
        source_pid,
        test_physical_source,
    } = event;
    let mut keyboard = match context.keyboard.try_lock() {
        Ok(keyboard) => keyboard,
        Err(_) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            return false;
        }
    };

    if source.is_physical()
        && matches!(
            event_type,
            ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP
        )
    {
        let gap_phase = if event_type == ffi::K_CG_EVENT_KEY_UP {
            KeyPhase::Up
        } else {
            KeyPhase::Down
        };
        if keyboard.handle_gap_tombstone(key_code, gap_phase, native_repeat) {
            // A native snapshot proved this edge belongs to the pre-gap held
            // key. Capture queued repeats without mutating native state, and
            // capture the delayed old up while keeping the tracker released.
            // Only a nonrepeat fresh down retires and continues normally.
            return true;
        }
    }

    let candidate_start = source.is_physical()
        && keyboard.transactional.event_starts_candidate(
            transactional_key_identity(key_code),
            if event_type == ffi::K_CG_EVENT_KEY_UP {
                PhysicalPhase::Up
            } else if native_repeat {
                PhysicalPhase::Repeat
            } else {
                PhysicalPhase::Down
            },
        );
    if candidate_start && let Some(reservation) = activation_reservation {
        keyboard.candidate_target_captured = true;
        keyboard.candidate_target = Some(reservation);
        keyboard.owned_down_records = [None; 26];
        if let KeyIdentity::Letter(letter) = transactional_key_identity(key_code) {
            keyboard.owned_down_records[usize::from(letter.index())] = Some(ReplayRecord {
                key: KeyIdentity::Letter(letter),
                native: NativeKey {
                    virtual_key: key_code,
                    scan_code: u32::from(key_code),
                    extended: false,
                    platform_flags: flags,
                },
                phase: PhysicalPhase::Down,
                observed_at_ms: event_timestamp / 1_000_000,
            });
        }
    }

    let secure_transition = source.is_physical() && secure_input_active();
    let event_modifiers = modifier_mask_from_flags(flags);
    let (key, phase, session_phase) = if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
        if let Some(side) = modifier_side_for_key_code(key_code) {
            let modeled_is_down = if source.is_physical() {
                let was_down = keyboard.modifiers.sides().contains(side);
                Some(if test_physical_source {
                    !was_down
                } else {
                    native_key_is_down(key_code)
                })
            } else {
                context
                    .recovery_edges
                    .try_lock()
                    .ok()
                    .and_then(|mut journal| {
                        journal.observe_normal_external_modifier(source_pid, key_code)
                    })
            };
            if let Some(is_down) = modeled_is_down {
                if source.is_physical() {
                    let was_down = keyboard.modifiers.sides().contains(side);
                    if was_down == is_down && !secure_transition {
                        reconcile_locked(context, &mut keyboard);
                        return false;
                    }
                    let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
                    let Some(next_epoch) = keyboard.modifier_epoch.checked_add(1) else {
                        recover_callback_unwind(context);
                        return true;
                    };
                    keyboard.modifier_epoch = next_epoch;
                    if keyboard.modifiers.mask() != event_modifiers {
                        reconcile_locked(context, &mut keyboard);
                        return false;
                    }
                    keyboard.reducer.observe_modifiers(event_modifiers);
                }
                #[cfg(feature = "transactional-shortcuts-dev")]
                if test_physical_source {
                    super::observe_macos_test_physical_key(key_code, is_down);
                }
                (
                    KeyIdentity::Modifier(side),
                    if is_down {
                        PhysicalPhase::Down
                    } else {
                        PhysicalPhase::Up
                    },
                    None,
                )
            } else {
                // Every bounded external-source slot is pinned. Preserve the
                // event as an external cancellation boundary without inventing
                // a side transition from aggregate family flags.
                (KeyIdentity::Other(key_code), PhysicalPhase::Down, None)
            }
        } else {
            // Caps Lock and other flags-only keys are invalid continuations but
            // do not alter the side-specific modifier snapshot.
            (KeyIdentity::Other(key_code), PhysicalPhase::Down, None)
        }
    } else {
        let key_phase = match event_type {
            ffi::K_CG_EVENT_KEY_DOWN => KeyPhase::Down,
            ffi::K_CG_EVENT_KEY_UP => KeyPhase::Up,
            _ => return false,
        };
        let key = transactional_key_identity(key_code);
        #[cfg(feature = "transactional-shortcuts-dev")]
        if test_physical_source {
            super::observe_macos_test_physical_key(
                key_code,
                event_type == ffi::K_CG_EVENT_KEY_DOWN,
            );
        }
        if source.is_physical() {
            let was_held = keyboard.physical.is_held(key_code);
            let discontinuity = match key_phase {
                KeyPhase::Down => was_held != native_repeat,
                KeyPhase::Up => !was_held,
            };
            let relevant = keyboard.transactional.config().enabled()
                || keyboard.transactional.owned_letters() != 0
                || keyboard.transactional.journal_len() != 0
                || SessionCaptureMode::from_u8(
                    context.state.session_capture_mode.load(Ordering::Acquire),
                ) != SessionCaptureMode::Off;
            let native_mismatch = !test_physical_source
                && relevant
                && !keyboard
                    .physical
                    .native_state_is_consistent_except(key_code, native_key_is_down);
            if !secure_transition
                && (discontinuity
                    || native_mismatch
                    || event_modifiers != keyboard.modifiers.mask())
            {
                reconcile_locked(context, &mut keyboard);
                return false;
            }
        }
        let repeat = source.is_physical()
            && key_phase == KeyPhase::Down
            && keyboard.physical.observe(key_code, key_phase);
        if source.is_physical() && key_phase == KeyPhase::Up {
            let _ = keyboard.physical.observe(key_code, key_phase);
        }
        let repeat = repeat || native_repeat;
        (
            key,
            if repeat {
                PhysicalPhase::Repeat
            } else {
                key_phase.into()
            },
            Some((key_phase, repeat)),
        )
    };

    let permission_transition = source.is_physical()
        && matches!(key, KeyIdentity::Letter(_))
        && phase == PhysicalPhase::Down
        && keyboard.transactional.config().enabled()
        && !native_permissions_available();
    let native_transition = secure_transition || permission_transition;

    let snapshot = physical_snapshot(&keyboard);
    let gate = if context.gate.is_open() && !native_transition {
        GateState::Open
    } else {
        GateState::Closed
    };
    let current_revision = keyboard.transactional.config().revision();
    let config_revision = if source.is_physical()
        && keyboard.activation_revision_at != 0
        && event_timestamp <= keyboard.activation_revision_at
    {
        ConfigRevision::new(current_revision.get().saturating_sub(1))
    } else {
        current_revision
    };
    let event = NormalizedEvent {
        key,
        phase,
        source,
        native: NativeKey {
            virtual_key: key_code,
            scan_code: u32::from(key_code),
            extended: false,
            platform_flags: flags,
        },
        observed_at_ms: event_timestamp / 1_000_000,
        config_revision,
        gate,
        snapshot,
    };
    // Preserve the authoritative engine across every effect. Only a completed
    // turn commits the cloned successor; callback unwind retains ownership.
    let turn = if candidate_start && activation_reservation.is_none() {
        keyboard
            .transactional
            .clone()
            .pass_uncapturable_event(event)
    } else {
        begin_transaction_snapshot(&keyboard.transactional, EngineInput::Event(event))
    };
    if turn.event_disposition_hint() == Some(EventDisposition::CaptureCurrent) {
        set_current_edge_disposition(context, &mut keyboard, CurrentEdgeDisposition::Owned);
        if source.is_physical()
            && phase == PhysicalPhase::Down
            && let KeyIdentity::Letter(letter) = key
        {
            keyboard.owned_down_records[usize::from(letter.index())] = Some(ReplayRecord {
                key,
                native: event.native,
                phase: PhysicalPhase::Down,
                observed_at_ms: event.observed_at_ms,
            });
        }
    }
    let completion = drive_transaction_turn(context, &mut keyboard, turn, activation_reservation);
    let outcome = match completion {
        DriveCompletion::Complete(Completion::Event(outcome)) => outcome,
        DriveCompletion::NativeObservationPending => {
            // No later physical/external callback may overtake the HID replay.
            // Physical current is already the reserved final replay record;
            // external current is copied into the fixed deferred scalar queue.
            drop(keyboard);
            context
                .state
                .recovery_deferred_mode
                .store(true, Ordering::Release);
            if !source.is_physical() && !event_ref.is_null() {
                let _ = defer_callback_edge(
                    context, event_type, event_ref, source, source_pid, marker, false,
                );
            }
            return true;
        }
        _ => {
            // Activation delivery and physical replay retain the current edge
            // through an installed continuation/replay record.
            return true;
        }
    };
    if native_transition {
        // Existing Escape/Enter ownership may still consume this exact up, but
        // no fresh session down may be admitted during the transition.
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    }
    let transaction_captured = outcome.disposition == EventDisposition::CaptureCurrent;
    if source.is_physical()
        && transaction_captured
        && let KeyIdentity::Letter(letter) = event.key
    {
        let slot = usize::from(letter.index());
        match event.phase {
            PhysicalPhase::Down => {
                keyboard.owned_down_records[slot] = Some(ReplayRecord {
                    key: event.key,
                    native: event.native,
                    phase: PhysicalPhase::Down,
                    observed_at_ms: event.observed_at_ms,
                });
            }
            PhysicalPhase::Up => keyboard.owned_down_records[slot] = None,
            PhysicalPhase::Repeat => {}
        }
    }
    set_current_edge_disposition(
        context,
        &mut keyboard,
        if transaction_captured {
            CurrentEdgeDisposition::Owned
        } else {
            CurrentEdgeDisposition::Pass
        },
    );
    let session_captured = !transaction_captured
        && source.is_physical()
        && session_phase.is_some_and(|(key_phase, repeat)| {
            process_session_event(
                context,
                &mut keyboard,
                key_code,
                key_phase,
                repeat,
                event_timestamp,
            )
        });
    if native_transition {
        finish_native_transition(context, &mut keyboard);
    } else if outcome.terminal && context.gate.is_open() && !context.terminal.is_triggered() {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }

    if keyboard.transactional.journal_len() != 0
        || keyboard.transactional.owned_letters() != 0
        || has_session_ownership(&keyboard)
    {
        arm_maintenance_timer(context);
    }

    let captured = transaction_captured || session_captured;
    set_current_edge_disposition(
        context,
        &mut keyboard,
        if captured {
            CurrentEdgeDisposition::Owned
        } else {
            CurrentEdgeDisposition::Pass
        },
    );
    let shutdown_may_drain =
        context.state.stopping.load(Ordering::Acquire) && keyboard.shutdown_requested;
    drop(keyboard);
    if shutdown_may_drain {
        let _ = stop_owner_run_loop_if_drained(context);
    }
    captured
}

fn process_session_event(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    key_code: u16,
    phase: KeyPhase,
    repeat: bool,
    event_timestamp: u64,
) -> bool {
    let key = map_key_code(key_code);
    let session_key = match key {
        PhysicalKey::Escape => SessionKey::Escape,
        PhysicalKey::Enter => SessionKey::Enter,
        PhysicalKey::Letter(_) | PhysicalKey::Other => return false,
    };
    if key == PhysicalKey::Enter
        && keyboard
            .session_enter_native_owned
            .is_some_and(|captured| captured != key_code)
    {
        return false;
    }
    let native_owned = match key {
        PhysicalKey::Escape => keyboard.session_escape_native_owned,
        PhysicalKey::Enter => keyboard.session_enter_native_owned == Some(key_code),
        PhysicalKey::Letter(_) | PhysicalKey::Other => false,
    };
    let capture_mode =
        SessionCaptureMode::from_u8(context.state.session_capture_mode.load(Ordering::Acquire));
    let accepting = context.gate.is_open();
    let cutoff = match session_key {
        SessionKey::Escape => keyboard.escape_capture_enabled_at,
        SessionKey::Enter => keyboard.enter_capture_enabled_at,
    };
    let predates_policy = cutoff != 0 && event_timestamp <= cutoff;
    if (!accepting || predates_policy) && !native_owned {
        return false;
    }
    let plan = keyboard.reducer.plan_bindings_at(
        KeyInput {
            key,
            phase,
            modifiers: keyboard.modifiers.mask(),
            repeat,
            injected: false,
        },
        talking_quill_keyboard_core::ActivationBindings::default(),
        false,
        if accepting && !predates_policy {
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
    let swallowed = keyboard.reducer.apply(plan, delivered);
    if delivered && swallowed && phase == KeyPhase::Down && !repeat {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = true,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = Some(key_code);
                keyboard.captured_enter_key_code = Some(key_code);
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
    }
    if native_owned && phase == KeyPhase::Up {
        match key {
            PhysicalKey::Escape => keyboard.session_escape_native_owned = false,
            PhysicalKey::Enter => {
                keyboard.session_enter_native_owned = None;
                keyboard.captured_enter_key_code = None;
            }
            PhysicalKey::Letter(_) | PhysicalKey::Other => {}
        }
        // UI balancing may already have reset reducer protocol state, but the
        // matching native up remains owned and must still be suppressed.
        return true;
    }
    swallowed || native_owned
}

fn reconcile_locked(context: &CallbackContext, keyboard: &mut CallbackKeyboard) {
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    keyboard.seed_from_state(native_key_is_down);
    let snapshot = physical_snapshot(keyboard);
    let outcome = begin_transaction_control(context, keyboard, Control::Reconcile(snapshot));
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

fn reconcile_recovery_locked(context: &CallbackContext, keyboard: &mut CallbackKeyboard) -> bool {
    if keyboard.has_submitted_replay_authority() {
        arm_maintenance_timer(context);
        return true;
    }
    deliver_balancing_events(context, &mut keyboard.reducer);
    let mut recovery_edges = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return false;
        }
    };
    keyboard
        .transactional
        .retire_released_owned_letters(recovery_edges.released_letter_bits());
    keyboard.seed_from_state(|key_code| {
        recovery_edges.physical_newer_generation_held(key_code)
            || (!recovery_edges.force_old_generation_released(key_code)
                && native_key_is_down(key_code))
    });
    let snapshot = physical_snapshot(keyboard);
    let outcome = begin_transaction_control(context, keyboard, Control::Reconcile(snapshot));
    if outcome.applied {
        // Clear only after the reconciled successor commits. A nested unwind
        // retains exact release generations for the next permanent-wake retry.
        recovery_edges.clear_owned_releases();
    }
    drop(recovery_edges);
    if (!outcome.applied || keyboard.transactional.shutdown_state() == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
    outcome.applied
}

fn finish_native_transition(context: &CallbackContext, keyboard: &mut CallbackKeyboard) {
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    deliver_balancing_events(context, &mut keyboard.reducer);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    if !context.terminal.is_triggered() {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

fn physical_snapshot(keyboard: &CallbackKeyboard) -> PhysicalSnapshot {
    PhysicalSnapshot::new(
        keyboard.physical.held_letter_bits(),
        keyboard.modifiers.sides(),
        false,
    )
}

fn transactional_key_identity(key_code: u16) -> KeyIdentity {
    match map_key_code(key_code) {
        PhysicalKey::Letter(key) => KeyIdentity::Letter(key),
        PhysicalKey::Escape => KeyIdentity::Escape,
        PhysicalKey::Enter => KeyIdentity::Enter,
        PhysicalKey::Other => KeyIdentity::Other(key_code),
    }
}

const fn modifier_side_for_key_code(key_code: u16) -> Option<ModifierSide> {
    match key_code {
        LEFT_CONTROL_KEY_CODE => Some(ModifierSide::LeftCtrl),
        RIGHT_CONTROL_KEY_CODE => Some(ModifierSide::RightCtrl),
        LEFT_OPTION_KEY_CODE => Some(ModifierSide::LeftAlt),
        RIGHT_OPTION_KEY_CODE => Some(ModifierSide::RightAlt),
        LEFT_SHIFT_KEY_CODE => Some(ModifierSide::LeftShift),
        RIGHT_SHIFT_KEY_CODE => Some(ModifierSide::RightShift),
        LEFT_COMMAND_KEY_CODE => Some(ModifierSide::LeftMeta),
        RIGHT_COMMAND_KEY_CODE => Some(ModifierSide::RightMeta),
        _ => None,
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn test_modifier_barrier_contract() -> bool {
    paste_modifier_barrier_valid(Some(9), 9, true, true, true)
        && !paste_modifier_barrier_valid(Some(9), 10, true, true, true)
        && !paste_modifier_barrier_valid(Some(9), 9, true, false, true)
        && !paste_modifier_barrier_valid(Some(9), 9, true, true, false)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn test_permission_disable_recovery_contract() -> bool {
    owned_native_transition_is_terminal(true, true, true)
        && owned_native_transition_is_terminal(true, false, false)
        && !owned_native_transition_is_terminal(false, true, false)
        && shutdown_drain_action(
            true,
            Some(Instant::now() - Duration::from_nanos(1)),
            false,
            Instant::now(),
        ) == ShutdownDrainAction::ReportUnresponsive
}

#[cfg(test)]
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
    if let Some(KeyboardEvent::SessionKey {
        key: talking_quill_keyboard_core::SessionKey::Enter,
        phase: talking_quill_keyboard_core::EventPhase::Down,
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

#[cfg(test)]
const fn is_synthetic_event(
    identity: injection::InjectionIdentity,
    _marker: i64,
    source_pid: i64,
) -> bool {
    !matches!(
        injection::unmarked_source(identity, source_pid),
        InputSource::Physical
    )
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

fn native_mouse_button_is_down(button: u32) -> bool {
    // SAFETY: HID-system button state accepts a bounded CGMouseButton and does
    // not allocate, retain, or call Accessibility APIs.
    unsafe { ffi::CGEventSourceButtonState(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, button) }
}

fn native_key_is_down(key_code: u16) -> bool {
    #[cfg(feature = "transactional-shortcuts-dev")]
    if let Some(held) = super::macos_test_physical_key_state(key_code) {
        return held;
    }
    // SAFETY: HID-system key state is the authoritative physical device state
    // and accepts every bounded CGKeyCode. Synthetic/logical session state must
    // not satisfy neutral checks or strict-drain reconciliation.
    unsafe { ffi::CGEventSourceKeyState(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, key_code) }
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

#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
mod tests {
    use super::super::{OwnerMutation, owner_command_state};
    use super::*;
    use talking_quill_keyboard_core::{
        ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, EventPhase,
        ProfileId, Shortcut, ShortcutModifiers,
        transactional::{EventJournal, JOURNAL_CAPACITY},
    };

    trait FnLockPoison {
        fn poison(&self);
    }

    impl<T> FnLockPoison for RecoveringMutex<T> {
        fn poison(&self) {
            let _guard = self.lock().unwrap();
            panic!("induced callback-critical mutex poison");
        }
    }

    fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
        Shortcut::new(modifiers, keys).unwrap()
    }

    fn activation_context() -> ActivationContext {
        ActivationContext::target_unavailable(ActivationGeneration::FIRST)
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
    ) -> (CallbackContext, Receiver<NativeEvent>, Sender<OwnerCommand>) {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound, outbound_rx) = bounded(outbound_capacity);
        let (command_tx, owner_commands) = bounded(4);
        let (_paste_tx, paste_commands) = bounded(1);
        (
            CallbackContext {
                state: Arc::new(SharedState::new()),
                suppression_enabled: true,
                keyboard: RecoveringMutex::new(CallbackKeyboard {
                    activation: ActivationConfig {
                        enabled: true,
                        bindings: bindings(),
                    },
                    ..CallbackKeyboard::default()
                }),
                pending_activation: RecoveringMutex::new(None),
                native_events: RecoveringMutex::new(None),
                injection_identity: Some(injection::InjectionIdentity::for_test(42)),
                test_physical_seam_enabled: false,
                callback_proxy: AtomicPtr::new(null_mut()),
                current_edge_disposition: AtomicU8::new(CurrentEdgeDisposition::Pass as u8),
                owner_commands,
                paste_commands,
                pending_paste: RecoveringMutex::new(None),
                recovery_edges: RecoveringMutex::new(RecoveryEdgeJournal::default()),
                target_cache: None,
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_tap_disable_request: None,
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_paste_barrier_paused: None,
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_paste_barrier_release: None,
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_paste_barrier_split_active: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_paste_barrier_down_observed: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "transactional-shortcuts-dev")]
                test_paste_barrier_pause_announced: std::sync::atomic::AtomicBool::new(false),
                forced_activation_reservation: None,
                outbound,
                gate,
                terminal,
                observability: Arc::new(TransactionObservability::new()),
            },
            outbound_rx,
            command_tx,
        )
    }

    fn test_context() -> (CallbackContext, Receiver<NativeEvent>, Sender<OwnerCommand>) {
        test_context_with_capacity(8)
    }

    #[test]
    fn closed_process_gate_uses_listen_only_tap_and_bypasses_every_callback_path() {
        assert_eq!(
            event_tap_options(false),
            ffi::K_CG_EVENT_TAP_OPTION_LISTEN_ONLY
        );
        assert_eq!(event_tap_options(true), ffi::K_CG_EVENT_TAP_OPTION_DEFAULT);

        let (mut context, outbound, _commands) = test_context();
        context.suppression_enabled = false;
        let event = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let returned = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_KEY_DOWN,
                event,
                (&raw mut context).cast(),
            )
        };
        assert_eq!(returned, event);
        assert_eq!(
            context
                .keyboard
                .lock()
                .unwrap()
                .transactional
                .owned_letters(),
            0
        );
        assert!(outbound.try_recv().is_err());
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    fn engine_with_ctrl_shift_x_candidate() -> TransactionEngine {
        let compiled =
            CompiledActivationConfig::compile(ConfigRevision::new(1), true, bindings()).unwrap();
        let mut engine = TransactionEngine::new(compiled);
        let mut sides = ModifierSides::default();
        for (side, key_code) in [
            (ModifierSide::LeftCtrl, LEFT_CONTROL_KEY_CODE),
            (ModifierSide::LeftShift, LEFT_SHIFT_KEY_CODE),
        ] {
            sides.insert(side);
            let event = NormalizedEvent {
                key: KeyIdentity::Modifier(side),
                phase: PhysicalPhase::Down,
                source: InputSource::test_physical(),
                native: NativeKey {
                    virtual_key: key_code,
                    scan_code: u32::from(key_code),
                    ..NativeKey::default()
                },
                observed_at_ms: 1,
                config_revision: ConfigRevision::new(1),
                gate: GateState::Open,
                snapshot: PhysicalSnapshot::new(0, sides, false),
            };
            let Turn::Complete {
                engine: next,
                completion: Completion::Event(_),
            } = engine.begin(EngineInput::Event(event))
            else {
                panic!("modifier turn must complete");
            };
            engine = next;
        }
        let x_bit = 1_u32 << u32::from(ActivationKey::X.index());
        let x = NormalizedEvent {
            key: KeyIdentity::Letter(ActivationKey::X),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
                scan_code: u32::from(LETTER_KEY_CODES[usize::from(ActivationKey::X.index())]),
                ..NativeKey::default()
            },
            observed_at_ms: 2,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(x_bit, sides, false),
        };
        let Turn::Complete {
            engine,
            completion: Completion::Event(outcome),
        } = engine.begin(EngineInput::Event(x))
        else {
            panic!("ordered prefix must remain a candidate");
        };
        assert_eq!(outcome.disposition, EventDisposition::CaptureCurrent);
        engine
    }

    #[test]
    fn stale_candidate_target_suppresses_config_shutdown_callback_tap_and_permission_replays() {
        let controls = [
            Control::ReplaceConfig(
                CompiledActivationConfig::compile(ConfigRevision::new(2), true, bindings())
                    .unwrap(),
            ),
            Control::Shutdown,
            Control::CloseAdmission(CancelReason::ActivationDeliveryFailed),
            Control::CloseAdmission(CancelReason::SecureDesktop),
            Control::CloseAdmission(CancelReason::GateClosed),
        ];
        for control in controls {
            let (mut context, _outbound, _commands) = test_context();
            let cache = TargetCache::with_open_validation_queue_for_test();
            cache.install_current_handle_for_test(
                super::super::target::target_handle_for_test(8),
                11,
                1,
            );
            context.target_cache = Some(cache);
            {
                let mut keyboard = context.keyboard.lock().unwrap();
                keyboard.transactional = engine_with_ctrl_shift_x_candidate();
                keyboard.candidate_target_captured = true;
                keyboard.candidate_target =
                    Some(super::super::target::activation_reservation_for_test(7, 11));
                let outcome = begin_transaction_control(&context, &mut keyboard, control);
                assert!(!outcome.applied);
                assert_eq!(outcome.cancellation, Some(CancelReason::TargetChanged));
                assert_eq!(outcome.shutdown, ShutdownState::Terminal);
                assert!(keyboard.replay_observation.is_none());
                assert!(keyboard.transactional.owned_letters() != 0);
            }
            let snapshot = context.observability.snapshot();
            assert_eq!(snapshot.replay.attempted, 0);
            assert_eq!(snapshot.transactions.cancellation_reasons.target_changed, 1);
        }
    }

    #[test]
    fn target_change_suppresses_new_replay_downs_but_allows_balancing_letter_and_modifier_ups() {
        let mut journal = EventJournal::new();
        let records = [
            ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::X),
                native: NativeKey {
                    virtual_key: 7,
                    ..NativeKey::default()
                },
                phase: PhysicalPhase::Down,
                observed_at_ms: 1,
            },
            ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::X),
                native: NativeKey {
                    virtual_key: 7,
                    ..NativeKey::default()
                },
                phase: PhysicalPhase::Up,
                observed_at_ms: 2,
            },
            ReplayRecord {
                key: KeyIdentity::Modifier(ModifierSide::LeftShift),
                native: NativeKey {
                    virtual_key: LEFT_SHIFT_KEY_CODE,
                    ..NativeKey::default()
                },
                phase: PhysicalPhase::Up,
                observed_at_ms: 3,
            },
            ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::P),
                native: NativeKey {
                    virtual_key: 35,
                    ..NativeKey::default()
                },
                phase: PhysicalPhase::Down,
                observed_at_ms: 4,
            },
        ];
        for record in records {
            journal.push(record).unwrap();
        }
        let batch = journal.replay_batch().unwrap();
        let mut keyboard = CallbackKeyboard::default();
        assert!(keyboard.begin_replay_observation(
            ExpectedReplayBatch::Replay(batch),
            injection::OperationToken::for_test(1),
        ));
        assert_eq!(
            keyboard.replay_disposition_after_target_check(true),
            CurrentEdgeDisposition::Pass
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 7, false, 0),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.replay_disposition_after_target_check(false),
            CurrentEdgeDisposition::Pass
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 7, false, 0),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.replay_disposition_after_target_check(false),
            CurrentEdgeDisposition::Pass
        );
        assert_eq!(
            keyboard.observe_replay_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_SHIFT_KEY_CODE,
                false,
                0,
            ),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.replay_disposition_after_target_check(false),
            CurrentEdgeDisposition::Owned
        );
        assert!(
            keyboard.visible_replay_cleanup().is_empty(),
            "an already-visible down/up pair needs no terminal cleanup"
        );
    }

    #[test]
    fn target_change_reconciles_the_exact_already_visible_replay_down() {
        let mut journal = EventJournal::new();
        let visible_down = ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::X),
            native: NativeKey {
                virtual_key: 7,
                scan_code: 53,
                extended: true,
                platform_flags: 0x1234,
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 9,
        };
        let second_visible_down = ReplayRecord {
            key: KeyIdentity::Letter(ActivationKey::P),
            native: NativeKey {
                virtual_key: 35,
                scan_code: 35,
                extended: false,
                platform_flags: 0x40,
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 10,
        };
        journal.push(visible_down).unwrap();
        journal.push(second_visible_down).unwrap();
        journal
            .push(ReplayRecord {
                key: KeyIdentity::Letter(ActivationKey::A),
                native: NativeKey {
                    virtual_key: 0,
                    scan_code: 0,
                    extended: false,
                    platform_flags: 0,
                },
                phase: PhysicalPhase::Down,
                observed_at_ms: 11,
            })
            .unwrap();
        let mut keyboard = CallbackKeyboard::default();
        assert!(keyboard.begin_replay_observation(
            ExpectedReplayBatch::Replay(journal.replay_batch().unwrap()),
            injection::OperationToken::for_test(2),
        ));
        assert_eq!(
            keyboard.replay_disposition_after_target_check(true),
            CurrentEdgeDisposition::Pass
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 7, false, 0x1234),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.replay_disposition_after_target_check(true),
            CurrentEdgeDisposition::Pass
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 35, false, 0x40),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.replay_disposition_after_target_check(false),
            CurrentEdgeDisposition::Owned
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 0, false, 0),
            GapBarrierObservation::Complete
        );

        let cleanup = keyboard.visible_replay_cleanup();
        assert_eq!(cleanup.len(), 2);
        assert_eq!(
            cleanup.entries(),
            &[
                ReplayRecord {
                    phase: PhysicalPhase::Up,
                    ..second_visible_down
                },
                ReplayRecord {
                    phase: PhysicalPhase::Up,
                    ..visible_down
                },
            ]
        );

        let Turn::NeedEffect {
            effect,
            continuation,
        } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation supplies replay authority")
        };
        let inflight = InflightEffect {
            effect,
            continuation,
            outcome: None,
        };
        assert_eq!(
            target_changed_replay_recovery(&keyboard, &inflight),
            Some(TargetChangedReplayRecovery::ReconcileVisibleDowns),
            "panic after the final original record must reconcile before drain"
        );
        keyboard.replay_target_cleanup_pending = true;
        assert_eq!(
            target_changed_replay_recovery(&keyboard, &inflight),
            Some(TargetChangedReplayRecovery::FinishAfterCleanup),
            "panic after the final cleanup up must finish without reposting it"
        );
        assert!(keyboard.begin_replay_observation(
            ExpectedReplayBatch::Cleanup(cleanup),
            injection::OperationToken::for_test(3),
        ));
        assert_eq!(
            target_changed_replay_recovery(&keyboard, &inflight),
            Some(TargetChangedReplayRecovery::AwaitObservation),
            "a submitted cleanup remains authoritative until exact observation"
        );
    }

    fn tagged_keyboard_event(
        event_type: u32,
        key_code: u16,
        flags: u64,
        marker: i64,
    ) -> ffi::CGEventRef {
        let event = unsafe {
            ffi::CGEventCreateKeyboardEvent(null(), key_code, event_type != ffi::K_CG_EVENT_KEY_UP)
        };
        assert!(!event.is_null());
        unsafe {
            ffi::CGEventSetType(event, event_type);
            ffi::CGEventSetFlags(event, flags);
            ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
        }
        event
    }

    fn tagged_mouse_event(
        event_type: u32,
        location: ffi::CGPoint,
        button: u32,
        click_state: i64,
        marker: i64,
    ) -> ffi::CGEventRef {
        let event = unsafe { ffi::CGEventCreateMouseEvent(null(), event_type, location, button) };
        assert!(!event.is_null());
        unsafe {
            ffi::CGEventSetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE, click_state);
            ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
        }
        event
    }

    fn install_event_source_identity(context: &mut CallbackContext, event: ffi::CGEventRef) {
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        context.injection_identity = Some(injection::InjectionIdentity::for_test(source_pid));
    }

    fn receive_event(receiver: &Receiver<NativeEvent>) -> KeyboardEvent {
        match receiver.try_recv().unwrap() {
            NativeEvent::Keyboard(event) => event,
            other => panic!("unexpected outbound: {other:?}"),
        }
    }

    fn deliver_test_activation(
        context: &CallbackContext,
        keyboard: &mut CallbackKeyboard,
        notice: ActivationNotice,
    ) -> bool {
        keyboard
            .dispatcher
            .deliver(&context.outbound, &context.terminal, notice, None)
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
    fn owner_resource_release_order_keeps_refcon_and_pool_until_native_invalidation() {
        assert_eq!(
            OWNER_RELEASE_ORDER,
            [
                OwnerReleaseStep::StopAx,
                OwnerReleaseStep::DisableTap,
                OwnerReleaseStep::RemoveTapSource,
                OwnerReleaseStep::RemoveCommandSource,
                OwnerReleaseStep::RemoveTimer,
                OwnerReleaseStep::InvalidateCommandSource,
                OwnerReleaseStep::InvalidateTimer,
                OwnerReleaseStep::InvalidateTap,
                OwnerReleaseStep::DropTargetCache,
                OwnerReleaseStep::ReleaseTimer,
                OwnerReleaseStep::ReleaseCommandSource,
                OwnerReleaseStep::ReleaseTapSource,
                OwnerReleaseStep::ReleaseTap,
                OwnerReleaseStep::DropNativePool,
            ]
        );
    }

    #[test]
    fn owner_callback_panic_is_contained_and_recovery_commits_before_return() {
        let (context, _outbound, _commands) = test_context();
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| panic!("induced owner callback panic"),
        );
        assert_eq!(
            context.terminal.reason(),
            Some(TerminalReason::CallbackPanicked)
        );
        assert!(!context.state.recovery_pending.load(Ordering::Acquire));
        assert!(!context.keyboard.is_poisoned());
        assert!(!pending_native_work(&context));
    }

    #[test]
    fn secure_input_or_permission_gap_is_terminal_only_while_ownership_exists() {
        assert!(!owned_native_transition_is_terminal(false, true, true));
        assert!(!owned_native_transition_is_terminal(false, false, false));
        assert!(owned_native_transition_is_terminal(true, true, true));
        assert!(owned_native_transition_is_terminal(true, false, false));
        assert!(!owned_native_transition_is_terminal(true, false, true));
    }

    #[test]
    fn insertion_safety_transition_is_clipboard_only_before_claim() {
        assert!(!insertion_has_irreversible_authority(
            InsertionStatus::Pending
        ));
        assert!(!insertion_has_irreversible_authority(
            InsertionStatus::Failed(PasteFailure::SecureInput)
        ));
        assert!(insertion_has_irreversible_authority(
            InsertionStatus::Claimed
        ));
        assert!(insertion_has_irreversible_authority(
            InsertionStatus::Ambiguous
        ));
    }

    #[test]
    fn insertion_claim_timeout_is_terminal_while_exact_results_are_publishable() {
        assert_eq!(
            insertion_owner_action(InsertionStatus::Claimed, false),
            InsertionOwnerAction::Wait
        );
        assert_eq!(
            insertion_owner_action(InsertionStatus::Claimed, true),
            InsertionOwnerAction::TerminalAmbiguous
        );
        assert_eq!(
            insertion_owner_action(InsertionStatus::Ambiguous, false),
            InsertionOwnerAction::TerminalAmbiguous
        );
        assert_eq!(
            insertion_owner_action(InsertionStatus::Succeeded, true),
            InsertionOwnerAction::CompleteSuccess
        );
        assert_eq!(
            insertion_owner_action(InsertionStatus::Failed(PasteFailure::OsRejected), true,),
            InsertionOwnerAction::CompleteFailure(PasteFailure::OsRejected)
        );
        assert_eq!(
            insertion_owner_action(InsertionStatus::Pending, true),
            InsertionOwnerAction::CancelPending
        );
    }

    #[test]
    fn transaction_snapshot_survives_an_induced_effect_executor_unwind() {
        let keyboard = CallbackKeyboard::default();
        let authoritative = keyboard.transactional.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let turn = begin_transaction_snapshot(
                &keyboard.transactional,
                EngineInput::Control(Control::RetryCleanup),
            );
            assert!(matches!(turn, Turn::Complete { .. }));
            panic!("injected effect executor panic");
        }));
        assert!(result.is_err());
        assert_eq!(keyboard.transactional, authoritative);
    }

    #[test]
    fn unwind_passes_only_an_unowned_current_edge() {
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Pass);
        }
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Pass
        );
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Replaced);
        }
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Replaced
        );
    }

    #[test]
    fn callback_recovery_restores_and_clears_every_critical_poisoned_lock() {
        let (context, _outbound, _commands) = test_context();
        for poison in [
            &context.pending_activation as &dyn FnLockPoison,
            &context.pending_paste as &dyn FnLockPoison,
            &context.recovery_edges as &dyn FnLockPoison,
            &context.native_events as &dyn FnLockPoison,
        ] {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poison.poison()));
        }
        assert!(context.pending_activation.is_poisoned());
        assert!(context.pending_paste.is_poisoned());
        assert!(context.recovery_edges.is_poisoned());
        assert!(context.native_events.is_poisoned());
        let _ = recover_callback_unwind(&context);
        assert!(!context.pending_activation.is_poisoned());
        assert!(!context.pending_paste.is_poisoned());
        assert!(!context.recovery_edges.is_poisoned());
        assert!(!context.native_events.is_poisoned());
        assert!(!pending_native_work(&context));
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    #[test]
    fn actual_event_tap_callback_recovers_poisoned_deferred_trigger_lock() {
        let (mut context, _outbound, _commands) = test_context();
        context.test_physical_seam_enabled = true;
        context.injection_identity =
            Some(injection::InjectionIdentity::for_test(i64::from(unsafe {
                ffi::getpid()
            })));
        context.target_cache = Some(TargetCache::with_open_validation_queue_for_test());
        context.forced_activation_reservation =
            Some(super::super::target::activation_reservation_for_test(7, 11));
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            let _ = keyboard
                .physical
                .observe(LEFT_CONTROL_KEY_CODE, KeyPhase::Down);
            let _ = keyboard
                .physical
                .observe(LEFT_SHIFT_KEY_CODE, KeyPhase::Down);
            let _ = keyboard.physical.observe(
                LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
                KeyPhase::Down,
            );
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
        }
        let event = unsafe {
            ffi::CGEventCreateKeyboardEvent(
                null(),
                LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
                true,
            )
        };
        assert!(!event.is_null());
        unsafe {
            ffi::CGEventSetFlags(
                event,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            );
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_EVENT_SOURCE_USER_DATA,
                injection::TEST_PHYSICAL_MARKER,
            );
        }
        PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.set(true));
        let returned = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_KEY_DOWN,
                event,
                (&raw mut context).cast(),
            )
        };
        assert!(returned.is_null(), "owned trigger remains suppressed");
        assert!(!context.keyboard.is_poisoned());
        assert!(context.pending_activation.lock().unwrap().is_none());
        assert!(context.keyboard.lock().unwrap().inflight_effect.is_none());
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    #[test]
    fn deferred_trigger_continuation_remains_installed_when_install_turn_panics() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let mut sides = ModifierSides::default();
        sides.insert(ModifierSide::LeftCtrl);
        sides.insert(ModifierSide::LeftShift);
        let trigger = NormalizedEvent {
            key: KeyIdentity::Letter(ActivationKey::P),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
                ..NativeKey::default()
            },
            observed_at_ms: 3,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(
                (1_u32 << u32::from(ActivationKey::X.index()))
                    | (1_u32 << u32::from(ActivationKey::P.index())),
                sides,
                false,
            ),
        };
        let turn = engine.begin(EngineInput::Event(trigger));
        let (mut context, _outbound, _commands) = test_context();
        context.target_cache = Some(TargetCache::with_open_validation_queue_for_test());
        PANIC_AFTER_DEFERRED_INSTALL.with(|flag| flag.set(true));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut keyboard = context.keyboard.lock().unwrap();
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
            let _ = drive_transaction_turn(
                &context,
                &mut keyboard,
                turn,
                Some(super::super::target::activation_reservation_for_test(7, 11)),
            );
        }));
        assert!(unwind.is_err());
        assert!(context.keyboard.is_poisoned());
        assert!(context.pending_activation.lock().unwrap().is_some());
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Owned
        );
        assert!(!context.keyboard.is_poisoned());
        assert!(context.pending_activation.lock().unwrap().is_none());
    }

    #[test]
    fn deferred_activation_and_submitted_replay_survive_induced_unwind_without_duplicate_effect() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let mut sides = ModifierSides::default();
        sides.insert(ModifierSide::LeftCtrl);
        sides.insert(ModifierSide::LeftShift);
        let held = (1_u32 << u32::from(ActivationKey::X.index()))
            | (1_u32 << u32::from(ActivationKey::P.index()));
        let trigger = NormalizedEvent {
            key: KeyIdentity::Letter(ActivationKey::P),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
                ..NativeKey::default()
            },
            observed_at_ms: 3,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(held, sides, false),
        };
        let Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation,
        } = engine.begin(EngineInput::Event(trigger))
        else {
            panic!("exact ordered trigger must defer activation delivery");
        };
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
        }
        *context.pending_activation.lock().unwrap() = Some(PendingActivation {
            continuation,
            notice,
            reservation: super::super::target::activation_reservation_for_test(7, 11),
            validation_request: super::super::target::validation_request_for_test(3, 11),
            resolved_delivery: None,
            deadline: Instant::now() + Duration::from_secs(1),
        });
        PANIC_AFTER_EFFECT_OUTCOME.with(|flag| flag.set(true));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = resolve_pending_activation(&context, PendingActivationResolution::FailDelivery);
        }));
        assert!(unwind.is_err());
        assert!(context.keyboard.is_poisoned());
        assert!(context.pending_activation.lock().unwrap().is_some());
        assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Owned
        );
        assert!(!context.keyboard.is_poisoned());
        assert!(context.pending_activation.lock().unwrap().is_none());
        assert!(context.keyboard.lock().unwrap().inflight_effect.is_none());
    }

    #[test]
    fn panic_after_exact_replay_recognition_keeps_foreground_event_passed_once() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            ..
        } = engine.begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let first = batch.entries()[0];
        let event_type = if matches!(first.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if first.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let token = injection::OperationToken::for_test(72);
        let event = tagged_keyboard_event(
            event_type,
            first.native.virtual_key,
            first.native.platform_flags,
            token.marker(),
        );
        let (mut context, _outbound, _commands) = test_context();
        install_event_source_identity(&mut context, event);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token,));
        }
        PANIC_AFTER_REPLAY_RECOGNITION.with(|flag| flag.set(true));
        let returned =
            unsafe { event_tap_callback(null_mut(), event_type, event, (&raw mut context).cast()) };
        assert_eq!(
            returned, event,
            "recognized replay must remain foreground-visible"
        );
        assert_eq!(
            atomic_current_edge_disposition(&context),
            CurrentEdgeDisposition::Pass
        );
        let keyboard = context.keyboard.lock().unwrap();
        let observation = keyboard
            .replay_observation
            .as_ref()
            .expect("remaining replay suffix stays installed");
        assert_eq!(observation.next, 1, "recognized edge advances exactly once");
        drop(keyboard);
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    #[test]
    fn panic_after_hid_replay_submission_waits_for_exact_observation_without_reposting() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            continuation,
        } = engine.begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let (context, _outbound, _commands) = test_context();
        let token = injection::OperationToken::for_test(27);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
            assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token,));
            keyboard.inflight_effect = Some(InflightEffect {
                effect: EffectRequest::Replay(batch),
                continuation,
                outcome: None,
            });
        }
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _keyboard = context.keyboard.lock().unwrap();
            panic!("after replay submission while callback lock is held");
        }));
        assert!(unwind.is_err());
        assert!(context.keyboard.is_poisoned());
        PANIC_ONCE_DURING_RECOVERY.with(|flag| flag.set(true));
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Owned
        );
        assert!(context.state.recovery_pending.load(Ordering::Acquire));
        assert!(context.keyboard.is_poisoned());
        // Permanent owner source/timer entry retries recovery before its normal
        // callback body. The injected second panic is one-shot.
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
        assert!(!context.state.recovery_pending.load(Ordering::Acquire));
        assert!(!context.keyboard.is_poisoned());
        let keyboard = context.keyboard.lock().unwrap();
        assert!(
            keyboard.inflight_effect.is_some(),
            "submission alone cannot consume the continuation"
        );
        let observation = keyboard
            .replay_observation
            .as_ref()
            .expect("original submitted operation remains pending observation");
        assert_eq!(observation.token, token);
        assert_eq!(observation.next, 0);
    }

    fn full_replay_batch(current_phase: PhysicalPhase) -> ReplayBatch {
        let mut journal = EventJournal::new();
        let x = LETTER_KEY_CODES[usize::from(ActivationKey::X.index())];
        let y = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
        for index in 0..JOURNAL_CAPACITY {
            let current = index + 1 == JOURNAL_CAPACITY;
            journal
                .push(ReplayRecord {
                    key: if current && current_phase == PhysicalPhase::Down {
                        KeyIdentity::Other(y)
                    } else {
                        KeyIdentity::Letter(ActivationKey::X)
                    },
                    native: NativeKey {
                        virtual_key: if current && current_phase == PhysicalPhase::Down {
                            y
                        } else {
                            x
                        },
                        scan_code: u32::from(if current && current_phase == PhysicalPhase::Down {
                            y
                        } else {
                            x
                        }),
                        extended: false,
                        platform_flags: 0,
                    },
                    phase: if current {
                        current_phase
                    } else if index == 0 {
                        PhysicalPhase::Down
                    } else {
                        PhysicalPhase::Repeat
                    },
                    observed_at_ms: index as u64 + 1,
                })
                .unwrap();
        }
        journal.replay_batch().unwrap()
    }

    fn replay_continuation_for_test() -> Continuation {
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(_),
            continuation,
        } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation provides a replay continuation");
        };
        continuation
    }

    #[test]
    fn full_capacity_submitted_replay_is_immutable_across_every_terminal_interruption() {
        let positions = [0, JOURNAL_CAPACITY / 2, JOURNAL_CAPACITY - 1];
        let disruptions = [
            TerminalReason::InputInjectionUnavailable, // observation timeout
            TerminalReason::EventTapDisabledByUserInput, // Secure Input
            TerminalReason::InputInjectionUnavailable, // permission loss
            TerminalReason::EventTapTimeoutRecoveryFailed, // disabled/timeout tap
        ];
        for current_phase in [PhysicalPhase::Repeat, PhysicalPhase::Down] {
            let batch = full_replay_batch(current_phase);
            for next in positions {
                for (disruption_index, reason) in disruptions.into_iter().enumerate() {
                    let (context, _outbound, _commands) = test_context();
                    let token = injection::OperationToken::for_test(
                        1_000 + disruption_index as u64 * 100 + next as u64,
                    );
                    {
                        let mut keyboard = context.keyboard.lock().unwrap();
                        assert!(
                            keyboard.begin_replay_observation(
                                ExpectedReplayBatch::Replay(batch),
                                token,
                            )
                        );
                        keyboard.inflight_effect = Some(InflightEffect {
                            effect: EffectRequest::Replay(batch),
                            continuation: replay_continuation_for_test(),
                            outcome: None,
                        });
                        for index in 0..next {
                            let record = batch.entries()[index];
                            let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
                                ffi::K_CG_EVENT_FLAGS_CHANGED
                            } else if record.phase == PhysicalPhase::Up {
                                ffi::K_CG_EVENT_KEY_UP
                            } else {
                                ffi::K_CG_EVENT_KEY_DOWN
                            };
                            assert_ne!(
                                keyboard.observe_replay_event(
                                    event_type,
                                    record.native.virtual_key,
                                    record.phase == PhysicalPhase::Repeat,
                                    record.native.platform_flags,
                                ),
                                GapBarrierObservation::Forged,
                            );
                        }
                    }

                    close_owned_native_admission(&context, reason, CancelReason::SecureDesktop);
                    let mut keyboard = context.keyboard.lock().unwrap();
                    TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|attempts| attempts.set(0));
                    let snapshot = physical_snapshot(&keyboard);
                    for control in [
                        Control::CloseAdmission(CancelReason::SecureDesktop),
                        Control::Reconcile(snapshot),
                        Control::RetryCleanup,
                    ] {
                        let outcome = begin_transaction_control(&context, &mut keyboard, control);
                        assert!(!outcome.applied);
                        assert_eq!(outcome.cancellation, None);
                    }
                    assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|value| value.get()), 0);
                    let observation = keyboard
                        .submitted_replay_authority()
                        .expect("submitted replay remains authoritative");
                    assert_eq!(observation.token, token);
                    assert_eq!(observation.next, next);
                    let terminal_suffix = SubmittedReplayTerminalSuffix::capture(observation);
                    assert_eq!(terminal_suffix.suffix_len, JOURNAL_CAPACITY - next);
                    assert_eq!(
                        &terminal_suffix.suffix[..terminal_suffix.suffix_len],
                        &batch.entries()[next..],
                        "terminal teardown preserves the exact unobserved suffix once",
                    );
                    assert_eq!(
                        terminal_suffix
                            .suffix
                            .iter()
                            .take(terminal_suffix.suffix_len)
                            .filter(|record| record.observed_at_ms == JOURNAL_CAPACITY as u64)
                            .count(),
                        1,
                        "reserved current repeat/terminator remains exactly once",
                    );
                }
            }
        }
    }

    #[test]
    fn owner_lifecycle_failed_recovery_never_duplicates_submitted_effect_or_shutdown_control() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            continuation,
        } = engine.begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let (context, _outbound, _commands) = test_context();
        let token = injection::OperationToken::for_test(81);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token));
            keyboard.inflight_effect = Some(InflightEffect {
                effect: EffectRequest::Replay(batch),
                continuation,
                outcome: None,
            });
            // The one-shot authority is installed before the original
            // Shutdown turn submits this replay.
            keyboard.shutdown_requested = true;
            keyboard.shutdown_deadline = Some(Instant::now() + SHUTDOWN_DRAIN_TIMEOUT);
        }
        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
        TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.set(1));
        enter_owner_lifecycle_recovery(&context);

        // Model an unresolved callback-critical resource. Preflight must fail
        // before consuming the submitted outcome, and shutdown is gated even
        // if a future caller invokes it defensively.
        let native_events = context.native_events.try_lock().unwrap();
        assert!(!attempt_pending_recovery(&context));
        begin_owner_shutdown(&context);
        assert!(context.state.recovery_pending.load(Ordering::Acquire));
        assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
        assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 0);
        assert_eq!(TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.get()), 1);
        drop(native_events);

        assert!(attempt_pending_recovery(&context));
        begin_owner_shutdown(&context);
        begin_owner_shutdown(&context);
        assert!(!context.state.recovery_pending.load(Ordering::Acquire));
        assert!(context.keyboard.lock().unwrap().inflight_effect.is_some());
        assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 0);
        assert_eq!(TEST_SHUTDOWN_CONTROL_ATTEMPTS.with(|count| count.get()), 1);
        let keyboard = context.keyboard.lock().unwrap();
        let observation = keyboard
            .replay_observation
            .as_ref()
            .expect("the original submitted replay remains the only observation");
        assert_eq!(observation.token, token);
        assert_eq!(observation.next, 0);
    }

    #[test]
    fn normal_candidate_callbacks_never_enter_recovery_deferred_mode() {
        fn event(
            context: &mut CallbackContext,
            event_type: u32,
            key_code: u16,
            flags: u64,
        ) -> (ffi::CGEventRef, ffi::CGEventRef) {
            let event =
                tagged_keyboard_event(event_type, key_code, flags, TEST_RECOVERY_PHYSICAL_MARKER);
            let returned = unsafe {
                event_tap_callback(
                    null_mut(),
                    event_type,
                    event,
                    (context as *mut CallbackContext).cast(),
                )
            };
            (event, returned)
        }

        let (mut context, outbound, _commands) = test_context();
        for (event_type, key_code, flags) in [
            (
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_CONTROL_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
            ),
            (
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_SHIFT_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            ),
            (
                ffi::K_CG_EVENT_KEY_DOWN,
                LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            ),
            (
                ffi::K_CG_EVENT_KEY_DOWN,
                LETTER_KEY_CODES[usize::from(ActivationKey::P.index())],
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            ),
        ] {
            let (event, returned) = event(&mut context, event_type, key_code, flags);
            if event_type == ffi::K_CG_EVENT_KEY_DOWN {
                assert!(returned.is_null(), "candidate/trigger is reducer-owned");
            }
            unsafe { ffi::CFRelease(event.cast_const()) };
        }
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
        assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));
        assert!(!context.recovery_edges.lock().unwrap().ordering_pending());
    }

    #[test]
    fn replay_failure_preserves_keyboard_current_and_defers_balanced_mouse_pair() {
        fn candidate_context() -> CallbackContext {
            let (context, _outbound, _commands) = test_context();
            {
                let mut keyboard = context.keyboard.lock().unwrap();
                keyboard.transactional = engine_with_ctrl_shift_x_candidate();
                for key_code in [
                    LEFT_CONTROL_KEY_CODE,
                    LEFT_SHIFT_KEY_CODE,
                    LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
                ] {
                    let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
                }
                let _ = keyboard
                    .modifiers
                    .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
                let _ = keyboard
                    .modifiers
                    .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
            }
            context
        }

        let key_flags = ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT;
        for (event_type, key_code, flags) in [
            (ffi::K_CG_EVENT_KEY_DOWN, 100, key_flags),
            (
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_SHIFT_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
            ),
        ] {
            let mut context = candidate_context();
            TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
            let event =
                tagged_keyboard_event(event_type, key_code, flags, TEST_RECOVERY_PHYSICAL_MARKER);
            let returned = unsafe {
                event_tap_callback(null_mut(), event_type, event, (&raw mut context).cast())
            };
            assert_eq!(
                returned, event,
                "unsubmitted terminating original survives exactly once"
            );
            assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
            assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));
            assert!(!context.recovery_edges.lock().unwrap().ordering_pending());
            unsafe { ffi::CFRelease(event.cast_const()) };
        }

        let mut context = candidate_context();
        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
        let mouse = tagged_mouse_event(
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
            ffi::CGPoint { x: 90.0, y: 70.0 },
            0,
            1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let returned = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
                mouse,
                (&raw mut context).cast(),
            )
        };
        assert!(returned.is_null(), "original mouse down is replaced");
        assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
        assert_eq!(
            atomic_current_edge_disposition(&context),
            CurrentEdgeDisposition::Owned
        );
        assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
        {
            let journal = context.recovery_edges.lock().unwrap();
            assert_eq!(journal.pending_len, 1);
            assert_eq!(
                journal.pending[0].event_type,
                ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
            );
            assert!(!journal.pending[0].foreground_balance);
            assert_eq!(journal.ready_len(), 0, "physical down waits for its up");
        }
        let mouse_up = tagged_mouse_event(
            ffi::K_CG_EVENT_LEFT_MOUSE_UP,
            ffi::CGPoint { x: 90.0, y: 70.0 },
            0,
            1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let returned_up = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_LEFT_MOUSE_UP,
                mouse_up,
                (&raw mut context).cast(),
            )
        };
        assert!(returned_up.is_null());
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 2);
        assert_eq!(journal.pending[1].event_type, ffi::K_CG_EVENT_LEFT_MOUSE_UP);
        assert_eq!(journal.ready_len(), 2);
        drop(journal);
        unsafe {
            ffi::CFRelease(mouse_up.cast_const());
            ffi::CFRelease(mouse.cast_const());
        }
    }

    #[test]
    fn side_specific_modifier_models_keep_both_sides_independent_for_every_family() {
        for (left, right) in [
            (LEFT_CONTROL_KEY_CODE, RIGHT_CONTROL_KEY_CODE),
            (LEFT_OPTION_KEY_CODE, RIGHT_OPTION_KEY_CODE),
            (LEFT_SHIFT_KEY_CODE, RIGHT_SHIFT_KEY_CODE),
            (LEFT_COMMAND_KEY_CODE, RIGHT_COMMAND_KEY_CODE),
        ] {
            for (pid, release_order) in [(301, [left, right]), (302, [right, left])] {
                let mut journal = RecoveryEdgeJournal::default();
                assert_eq!(
                    journal
                        .modifier_side_transition(InputSource::External, pid, left, None)
                        .map(|transition| transition.0),
                    Some(true)
                );
                assert_eq!(
                    journal
                        .modifier_side_transition(InputSource::External, pid, right, None)
                        .map(|transition| transition.0),
                    Some(true)
                );
                for key_code in release_order {
                    assert_eq!(
                        journal
                            .modifier_side_transition(InputSource::External, pid, key_code, None)
                            .map(|transition| transition.0),
                        Some(false)
                    );
                }

                let mut physical = RecoveryEdgeJournal::default();
                physical.seed_modifier_side(InputSource::test_physical(), -1, left, false);
                physical.seed_modifier_side(InputSource::test_physical(), -1, right, false);
                assert_eq!(
                    physical
                        .modifier_side_transition(InputSource::test_physical(), -1, left, None)
                        .map(|transition| transition.0),
                    Some(true)
                );
                assert_eq!(
                    physical
                        .modifier_side_transition(InputSource::test_physical(), -1, right, None)
                        .map(|transition| transition.0),
                    Some(true)
                );
                for key_code in release_order {
                    assert_eq!(
                        physical
                            .modifier_side_transition(
                                InputSource::test_physical(),
                                -1,
                                key_code,
                                None,
                            )
                            .map(|transition| transition.0),
                        Some(false)
                    );
                }
            }
        }
    }

    #[test]
    fn ninth_external_pid_still_cancels_candidate_and_recovery_retains_scalar() {
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            for key_code in [
                LEFT_CONTROL_KEY_CODE,
                LEFT_SHIFT_KEY_CODE,
                LETTER_KEY_CODES[usize::from(ActivationKey::X.index())],
            ] {
                let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
            }
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_SHIFT_KEY_CODE, true);
        }
        {
            let mut journal = context.recovery_edges.lock().unwrap();
            for pid in 1..=RECOVERY_EXTERNAL_SOURCE_SLOTS as i64 {
                assert_eq!(
                    journal.observe_normal_external_modifier(pid, LEFT_CONTROL_KEY_CODE),
                    Some(true)
                );
            }
            assert!(journal.source_slot(InputSource::External, 99).is_none());
        }
        TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.set(0));
        let _captured = process_transactional_event(
            &context,
            CallbackEvent {
                event_ref: null_mut(),
                marker: 0,
                event_type: ffi::K_CG_EVENT_FLAGS_CHANGED,
                key_code: LEFT_SHIFT_KEY_CODE,
                native_repeat: false,
                flags: ffi::K_CG_EVENT_FLAG_MASK_CONTROL | ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
                event_timestamp: 10,
                source: InputSource::External,
                source_pid: 99,
                test_physical_source: false,
            },
            None,
        );
        assert_eq!(TEST_EFFECT_SUBMISSION_ATTEMPTS.with(|count| count.get()), 1);
        assert!(!context.state.recovery_deferred_mode.load(Ordering::Acquire));

        let event = tagged_keyboard_event(
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_SHIFT_KEY_CODE,
            ffi::K_CG_EVENT_FLAG_MASK_SHIFT,
            0,
        );
        let disposition = defer_callback_edge(
            &context,
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            event,
            InputSource::External,
            99,
            0,
            false,
        );
        assert_eq!(disposition, CurrentEdgeDisposition::Owned);
        let journal = context.recovery_edges.lock().unwrap();
        assert!(
            journal.pending[..journal.pending_len]
                .iter()
                .any(|edge| edge.source_pid == 99)
        );
        for pid in 1..=RECOVERY_EXTERNAL_SOURCE_SLOTS as i64 {
            assert!(
                journal
                    .existing_source_slot(InputSource::External, pid)
                    .is_some()
            );
        }
        drop(journal);
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    #[test]
    fn external_source_slots_pin_buffer_exposure_and_fence_references() {
        let mut journal = RecoveryEdgeJournal::default();
        for pid in 10..18 {
            assert!(journal.source_slot(InputSource::External, pid).is_some());
        }
        assert!(journal.append(injection::DeferredEvent {
            source: InputSource::External,
            source_pid: 10,
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            ..injection::DeferredEvent::EMPTY
        }));
        assert!(journal.begin_submission(injection::OperationToken::for_test(388), 0, 1));
        assert!(journal.append(injection::DeferredEvent {
            source: InputSource::External,
            source_pid: 13,
            event_type: ffi::K_CG_EVENT_KEY_UP,
            ..injection::DeferredEvent::EMPTY
        }));
        let slot_11 = journal
            .existing_source_slot(InputSource::External, 11)
            .unwrap();
        journal.exposed_keys[slot_11][16] = true;
        let slot_12 = journal
            .existing_source_slot(InputSource::External, 12)
            .unwrap();
        journal.discard_key_fences[slot_12][17] = true;
        assert!(journal.source_slot(InputSource::External, 99).is_some());
        for pid in [10, 11, 12, 13] {
            assert!(
                journal
                    .existing_source_slot(InputSource::External, pid)
                    .is_some()
            );
        }
        assert!(journal.external_pid_referenced(10));
    }

    #[test]
    fn deferred_flags_changed_streams_ignore_aggregate_side_flags() {
        fn assert_stream(source: InputSource, source_pid: i64, marker: i64) {
            let (context, _outbound, _commands) = test_context();
            let events = [
                tagged_keyboard_event(
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    LEFT_CONTROL_KEY_CODE,
                    ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                    marker,
                ),
                tagged_keyboard_event(
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    RIGHT_CONTROL_KEY_CODE,
                    ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                    marker,
                ),
                // Aggregate Control remains set because right Control is held;
                // the left keycode nevertheless identifies a left-side up.
                tagged_keyboard_event(
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    LEFT_CONTROL_KEY_CODE,
                    ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                    marker,
                ),
                tagged_keyboard_event(
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    RIGHT_CONTROL_KEY_CODE,
                    0,
                    marker,
                ),
            ];
            for event in events {
                assert_eq!(
                    defer_callback_edge(
                        &context,
                        ffi::K_CG_EVENT_FLAGS_CHANGED,
                        event,
                        source,
                        source_pid,
                        marker,
                        false,
                    ),
                    CurrentEdgeDisposition::Owned
                );
            }
            let journal = context.recovery_edges.lock().unwrap();
            assert_eq!(journal.pending_len, 4);
            assert_eq!(
                journal.pending[..4]
                    .iter()
                    .map(|edge| edge.is_down)
                    .collect::<Vec<_>>(),
                vec![true, true, false, false]
            );
            assert!(journal.hidden_balanced());
            drop(journal);
            unsafe {
                for event in events {
                    ffi::CFRelease(event.cast_const());
                }
            }
        }

        assert_stream(InputSource::External, 550, 0);
        assert_stream(
            InputSource::test_physical(),
            -1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
    }

    #[test]
    fn unpaired_external_down_submits_without_holding_shutdown_for_an_up() {
        let (context, _outbound, _commands) = test_context();
        context.state.stopping.store(true, Ordering::Release);
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
        {
            let mut journal = context.recovery_edges.lock().unwrap();
            assert!(journal.append(injection::DeferredEvent {
                event_type: ffi::K_CG_EVENT_KEY_DOWN,
                key_code: 16,
                is_down: true,
                source: InputSource::External,
                source_pid: 901,
                hidden_generation: true,
                ..injection::DeferredEvent::EMPTY
            }));
            assert_eq!(journal.ready_len(), 1);
            assert!(journal.external_collection_deadline.is_some());
            assert!(journal.begin_submission(injection::OperationToken::for_test(390), 0, 1));
            let edge = journal.expected().unwrap();
            journal.observe_foreground_exposure(edge);
            assert_eq!(
                journal.advance_observation(),
                GapBarrierObservation::Complete
            );
            assert!(!journal.has_pending());
            assert!(journal.external_collection_deadline.is_none());
        }
        assert!(try_clear_recovery_deferred_mode(&context));
        assert!(!pending_native_work(&context));

        let mut bounded = RecoveryEdgeJournal::default();
        let (generation, foreground_balance, hidden_generation) = bounded
            .observe_key_phase(InputSource::Physical, -1, 7, true, false, false)
            .unwrap();
        assert!(bounded.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 7,
            is_down: true,
            source: InputSource::Physical,
            source_pid: -1,
            generation,
            foreground_balance,
            hidden_generation,
            ..injection::DeferredEvent::EMPTY
        }));
        assert!(bounded.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 8,
            is_down: true,
            source: InputSource::External,
            source_pid: 903,
            ..injection::DeferredEvent::EMPTY
        }));
        bounded.external_collection_deadline = Some(Instant::now());
        assert!(bounded.expire_external_collection(Instant::now()));
        assert_eq!(bounded.pending_len, 1);
        assert_eq!(bounded.pending[0].source_pid, 903);
        assert!(bounded.physical_fences_pending());
        assert_eq!(bounded.ready_len(), 1);
    }

    #[test]
    fn delayed_external_up_joins_next_batch_without_reordering_down() {
        let mut journal = RecoveryEdgeJournal::default();
        assert!(journal.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 16,
            is_down: true,
            source: InputSource::External,
            source_pid: 902,
            ..injection::DeferredEvent::EMPTY
        }));
        assert!(journal.begin_submission(injection::OperationToken::for_test(391), 0, 1));
        assert!(journal.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code: 16,
            source: InputSource::External,
            source_pid: 902,
            ..injection::DeferredEvent::EMPTY
        }));
        assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_DOWN);
        assert_eq!(journal.tail[0].event_type, ffi::K_CG_EVENT_KEY_UP);
        let down = journal.expected().unwrap();
        journal.observe_foreground_exposure(down);
        assert_eq!(
            journal.advance_observation(),
            GapBarrierObservation::Complete
        );
        assert_eq!(journal.pending_len, 1);
        assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
        assert_eq!(journal.ready_len(), 1);
    }

    #[test]
    fn submitted_and_tail_buffers_roll_over_at_maximum_without_reordering() {
        fn edge(index: usize) -> injection::DeferredEvent {
            injection::DeferredEvent {
                event_type: if index.is_multiple_of(2) {
                    ffi::K_CG_EVENT_KEY_DOWN
                } else {
                    ffi::K_CG_EVENT_LEFT_MOUSE_UP
                },
                original_timestamp: index as u64,
                ..injection::DeferredEvent::EMPTY
            }
        }

        let mut journal = RecoveryEdgeJournal::default();
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            assert!(journal.append(edge(index)));
        }
        assert!(journal.begin_submission(
            injection::OperationToken::for_test(401),
            0,
            injection::DEFERRED_EDGE_CAPACITY,
        ));
        assert_eq!(journal.next_pool_bank, 1);
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            assert!(journal.append(edge(injection::DEFERRED_EDGE_CAPACITY + index)));
        }
        assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
        assert_eq!(journal.tail_len, injection::DEFERRED_EDGE_CAPACITY);
        assert!(!journal.overflow);
        for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
            let _ = journal.advance_observation();
        }
        assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
        assert_eq!(journal.tail_len, 0);
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            assert_eq!(
                journal.pending[index].original_timestamp,
                (injection::DEFERRED_EDGE_CAPACITY + index) as u64
            );
        }
        assert!(journal.begin_submission(
            injection::OperationToken::for_test(402),
            1,
            injection::DEFERRED_EDGE_CAPACITY,
        ));
        assert_eq!(journal.next_pool_bank, 0);
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            assert!(journal.append(edge(2 * injection::DEFERRED_EDGE_CAPACITY + index)));
        }
        for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
            let _ = journal.advance_observation();
        }
        assert_eq!(journal.pending[0].original_timestamp, 128);
        assert!(!journal.overflow);
    }

    #[test]
    fn malformed_submitted_suffix_retains_balance_for_already_exposed_down() {
        let mut journal = RecoveryEdgeJournal::default();
        for (event_type, is_down) in [
            (ffi::K_CG_EVENT_KEY_DOWN, true),
            (ffi::K_CG_EVENT_KEY_UP, false),
        ] {
            assert!(journal.append(injection::DeferredEvent {
                event_type,
                key_code: 16,
                is_down,
                source: InputSource::External,
                source_pid: 710,
                ..injection::DeferredEvent::EMPTY
            }));
        }
        assert!(journal.begin_submission(injection::OperationToken::for_test(410), 0, 2));
        let down = journal.expected().unwrap();
        journal.observe_foreground_exposure(down);
        assert_eq!(journal.advance_observation(), GapBarrierObservation::Down);
        journal.abort_submission_to_overflow();
        journal.settle_overflow();
        assert_eq!(journal.pending_len, 1);
        assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
        assert!(journal.pending[0].foreground_balance);
    }

    #[test]
    fn deferred_observation_rejects_every_mutated_lossless_scalar() {
        let keyboard = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
            0x8000_0000_0010_0000,
            0,
        );
        unsafe {
            ffi::CGEventSetIntegerValueField(keyboard, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE, 41);
        }
        let mut expected = injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
            flags: unsafe { ffi::CGEventGetFlags(keyboard) },
            keyboard_type: unsafe {
                ffi::CGEventGetIntegerValueField(keyboard, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
            },
            ..injection::DeferredEvent::EMPTY
        };
        assert!(deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_KEY_DOWN,
            keyboard
        ));
        unsafe { ffi::CGEventSetFlags(keyboard, expected.flags ^ ffi::K_CG_EVENT_FLAG_MASK_SHIFT) };
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_KEY_DOWN,
            keyboard
        ));
        unsafe {
            ffi::CGEventSetFlags(keyboard, expected.flags);
            ffi::CGEventSetIntegerValueField(
                keyboard,
                ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE,
                expected.keyboard_type + 1,
            );
        }
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_KEY_DOWN,
            keyboard
        ));

        let mouse = tagged_mouse_event(
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            ffi::CGPoint { x: 12.5, y: 33.25 },
            2,
            3,
            0,
        );
        unsafe {
            for (field, value) in [
                (ffi::K_CG_MOUSE_EVENT_NUMBER, 4),
                (ffi::K_CG_MOUSE_EVENT_DELTA_X, -7),
                (ffi::K_CG_MOUSE_EVENT_DELTA_Y, 9),
                (ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER, 1),
                (ffi::K_CG_MOUSE_EVENT_SUBTYPE, 2),
            ] {
                ffi::CGEventSetIntegerValueField(mouse, field, value);
            }
            ffi::CGEventSetDoubleValueField(mouse, ffi::K_CG_MOUSE_EVENT_PRESSURE, 0.625);
        }
        expected = injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            flags: unsafe { ffi::CGEventGetFlags(mouse) },
            location: unsafe { ffi::CGEventGetLocation(mouse) },
            mouse_number: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_NUMBER)
            },
            mouse_click_state: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_CLICK_STATE)
            },
            mouse_pressure: unsafe {
                ffi::CGEventGetDoubleValueField(mouse, ffi::K_CG_MOUSE_EVENT_PRESSURE)
            },
            mouse_button: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER)
            },
            mouse_delta_x: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_DELTA_X)
            },
            mouse_delta_y: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_DELTA_Y)
            },
            mouse_instant_mouser: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
            },
            mouse_subtype: unsafe {
                ffi::CGEventGetIntegerValueField(mouse, ffi::K_CG_MOUSE_EVENT_SUBTYPE)
            },
            ..injection::DeferredEvent::EMPTY
        };
        assert!(deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            mouse
        ));
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_UP,
            mouse
        ));
        unsafe { ffi::CGEventSetFlags(mouse, expected.flags ^ ffi::K_CG_EVENT_FLAG_MASK_SHIFT) };
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            mouse
        ));
        unsafe { ffi::CGEventSetFlags(mouse, expected.flags) };
        for (field, original, mutation) in [
            (
                ffi::K_CG_MOUSE_EVENT_NUMBER,
                expected.mouse_number,
                expected.mouse_number + 1,
            ),
            (
                ffi::K_CG_MOUSE_EVENT_CLICK_STATE,
                expected.mouse_click_state,
                expected.mouse_click_state + 1,
            ),
            (
                ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER,
                expected.mouse_button,
                expected.mouse_button + 1,
            ),
            (
                ffi::K_CG_MOUSE_EVENT_DELTA_X,
                expected.mouse_delta_x,
                expected.mouse_delta_x + 1,
            ),
            (
                ffi::K_CG_MOUSE_EVENT_DELTA_Y,
                expected.mouse_delta_y,
                expected.mouse_delta_y + 1,
            ),
            (
                ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER,
                expected.mouse_instant_mouser,
                i64::from(expected.mouse_instant_mouser == 0),
            ),
            (
                ffi::K_CG_MOUSE_EVENT_SUBTYPE,
                expected.mouse_subtype,
                i64::from(expected.mouse_subtype == 0),
            ),
        ] {
            unsafe { ffi::CGEventSetIntegerValueField(mouse, field, mutation) };
            assert!(!deferred_edge_matches_observed(
                expected,
                ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
                mouse
            ));
            unsafe { ffi::CGEventSetIntegerValueField(mouse, field, original) };
        }
        unsafe {
            ffi::CGEventSetDoubleValueField(
                mouse,
                ffi::K_CG_MOUSE_EVENT_PRESSURE,
                if expected.mouse_pressure <= 0.5 {
                    expected.mouse_pressure + 0.25
                } else {
                    expected.mouse_pressure - 0.25
                },
            );
        }
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            mouse
        ));
        unsafe {
            ffi::CGEventSetDoubleValueField(
                mouse,
                ffi::K_CG_MOUSE_EVENT_PRESSURE,
                expected.mouse_pressure,
            );
            ffi::CGEventSetLocation(
                mouse,
                ffi::CGPoint {
                    x: expected.location.x + 1.0,
                    y: expected.location.y,
                },
            );
        }
        assert!(!deferred_edge_matches_observed(
            expected,
            ffi::K_CG_EVENT_OTHER_MOUSE_DOWN,
            mouse
        ));
        unsafe {
            ffi::CFRelease(mouse.cast_const());
            ffi::CFRelease(keyboard.cast_const());
        }
    }

    #[test]
    fn owner_recovery_with_submitted_replay_defers_physical_edges_before_observation() {
        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            ..
        } = engine.begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let token = injection::OperationToken::for_test(89);
        let modifier = tagged_keyboard_event(
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_CONTROL_KEY_CODE,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let key = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())],
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
            assert!(keyboard.begin_replay_observation(ExpectedReplayBatch::Replay(batch), token));
        }
        enter_owner_lifecycle_recovery(&context);
        assert!(context.state.recovery_pending.load(Ordering::Acquire));
        assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
        let native_events = context.native_events.try_lock().unwrap();
        let context_ptr = (&raw const context).cast_mut().cast();
        for (event_type, event) in [
            (ffi::K_CG_EVENT_FLAGS_CHANGED, modifier),
            (ffi::K_CG_EVENT_KEY_DOWN, key),
        ] {
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert!(returned.is_null());
        }
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 2);
        assert_eq!(journal.pending[0].key_code, LEFT_CONTROL_KEY_CODE);
        assert_eq!(
            journal.pending[1].key_code,
            LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())]
        );
        assert!(
            context
                .keyboard
                .lock()
                .unwrap()
                .replay_observation
                .is_some()
        );
        drop(journal);
        drop(native_events);
        unsafe {
            ffi::CFRelease(key.cast_const());
            ffi::CFRelease(modifier.cast_const());
        }
    }

    #[test]
    fn failed_recovery_defers_external_modifier_keyboard_and_balanced_mouse_in_order() {
        let y = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
        let z = LETTER_KEY_CODES[usize::from(ActivationKey::Z.index())];
        let external_flags = 0x8000_0000_0010_0000;
        let external_down = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, z, external_flags, 0);
        let external_up = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, z, external_flags, 0);
        let ctrl_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_FLAGS_CHANGED,
            LEFT_CONTROL_KEY_CODE,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let y_down = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            y,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let y_up =
            tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, y, 0, TEST_RECOVERY_PHYSICAL_MARKER);
        let location = ffi::CGPoint { x: 321.0, y: 222.0 };
        let mouse_down = tagged_mouse_event(
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN,
            location,
            0,
            1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let mouse_up = tagged_mouse_event(
            ffi::K_CG_EVENT_LEFT_MOUSE_UP,
            location,
            0,
            1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let (mut context, _outbound, _commands) = test_context();
        let external_pid = unsafe {
            ffi::CGEventGetIntegerValueField(external_down, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        context.injection_identity = Some(injection::InjectionIdentity::for_test(external_pid + 1));
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            let _ = keyboard
                .modifiers
                .observe_flags_changed(LEFT_CONTROL_KEY_CODE, true);
        }
        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();
        for (event_type, event) in [
            (ffi::K_CG_EVENT_KEY_DOWN, external_down),
            (ffi::K_CG_EVENT_KEY_UP, external_up),
            (ffi::K_CG_EVENT_FLAGS_CHANGED, ctrl_up),
            (ffi::K_CG_EVENT_KEY_DOWN, y_down),
            (ffi::K_CG_EVENT_KEY_UP, y_up),
            (ffi::K_CG_EVENT_LEFT_MOUSE_DOWN, mouse_down),
            (ffi::K_CG_EVENT_LEFT_MOUSE_UP, mouse_up),
        ] {
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert!(returned.is_null(), "ordered original must be suppressed");
        }
        let journal = context.recovery_edges.lock().unwrap();
        assert_eq!(journal.pending_len, 7);
        assert!(journal.hidden_balanced());
        assert_eq!(journal.pending[0].source, InputSource::External);
        assert_eq!(journal.pending[0].key_code, z);
        assert_eq!(journal.pending[0].flags, unsafe {
            ffi::CGEventGetFlags(external_down)
        },);
        assert_eq!(journal.pending[0].keyboard_type, unsafe {
            ffi::CGEventGetIntegerValueField(external_down, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        },);
        assert_eq!(journal.pending[0].original_timestamp, unsafe {
            ffi::CGEventGetTimestamp(external_down)
        },);
        assert_eq!(journal.pending[1].key_code, z);
        assert_eq!(journal.pending[2].key_code, LEFT_CONTROL_KEY_CODE);
        assert_eq!(journal.pending[3].key_code, y);
        assert_eq!(journal.pending[4].key_code, y);
        assert_eq!(
            journal.pending[5].event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
        );
        assert_eq!(journal.pending[6].event_type, ffi::K_CG_EVENT_LEFT_MOUSE_UP);
        assert_eq!(journal.pending[5].location, location);
        assert_eq!(journal.pending[6].location, location);
        drop(journal);
        drop(native_events);
        unsafe {
            for event in [
                mouse_up,
                mouse_down,
                y_up,
                y_down,
                ctrl_up,
                external_up,
                external_down,
            ] {
                ffi::CFRelease(event.cast_const());
            }
        }
    }

    #[test]
    fn failed_recovery_classifier_suppresses_owned_up_then_retry_reconciles_once() {
        let key = ActivationKey::X;
        let key_code = LETTER_KEY_CODES[usize::from(key.index())];
        let event = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let session_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            ESCAPE_KEY_CODE,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let (context, _outbound, _commands) = test_context();
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine_with_ctrl_shift_x_candidate();
            keyboard.session_escape_native_owned = true;
            let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
            let _ = keyboard.physical.observe(ESCAPE_KEY_CODE, KeyPhase::Down);
        }

        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();
        let returned =
            unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, event, context_ptr) };
        assert!(returned.is_null(), "the exact owned up cannot leak");
        let returned_session = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, session_up, context_ptr)
        };
        assert!(
            returned_session.is_null(),
            "the exact session-owned up cannot leak"
        );
        assert!(context.state.recovery_pending.load(Ordering::Acquire));
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_ne!(keyboard.transactional.owned_letters(), 0);
            assert!(!keyboard.session_escape_native_owned);
        }
        assert!(
            context
                .recovery_edges
                .lock()
                .unwrap()
                .owned_release_recorded(key_code)
        );
        drop(native_events);

        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
        assert!(!context.state.recovery_pending.load(Ordering::Acquire));
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.owned_letters(), 0);
        }
        assert!(
            !context
                .recovery_edges
                .lock()
                .unwrap()
                .owned_release_recorded(key_code)
        );
        assert!(!pending_native_work(&context));
        // A later permanent wake sees no retained release and cannot drain it
        // or execute recovery a second time.
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| {},
        );
        assert!(!pending_native_work(&context));
        unsafe {
            ffi::CFRelease(session_up.cast_const());
            ffi::CFRelease(event.cast_const());
        }
    }

    #[test]
    fn owned_up_fresh_generation_replays_after_retained_replay_and_balances_once() {
        let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::X.index())];
        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(batch),
            ..
        } = engine.clone().begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let replay_token = injection::OperationToken::for_test(91);
        let first_replay = batch.entries()[0];
        let first_replay_type = if matches!(first_replay.key, KeyIdentity::Modifier(_)) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if first_replay.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        };
        let first_replay_event = tagged_keyboard_event(
            first_replay_type,
            first_replay.native.virtual_key,
            first_replay.native.platform_flags,
            replay_token.marker(),
        );
        let old_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let fresh_down = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let fresh_repeat = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        unsafe {
            ffi::CGEventSetIntegerValueField(fresh_repeat, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        let fresh_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let (mut context, _outbound, _commands) = test_context();
        install_event_source_identity(&mut context, first_replay_event);
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.transactional = engine;
            let _ = keyboard.physical.observe(key_code, KeyPhase::Down);
            assert!(
                keyboard
                    .begin_replay_observation(ExpectedReplayBatch::Replay(batch), replay_token,)
            );
        }
        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();
        for (event_type, event) in [
            (ffi::K_CG_EVENT_KEY_UP, old_up),
            (ffi::K_CG_EVENT_KEY_DOWN, fresh_down),
            (ffi::K_CG_EVENT_KEY_DOWN, fresh_repeat),
        ] {
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert!(returned.is_null());
        }
        {
            let journal = context.recovery_edges.lock().unwrap();
            assert_eq!(journal.pending_len, 2);
            assert_eq!(journal.pending[0].generation, 2);
            assert_eq!(journal.pending[1].generation, 2);
            assert!(journal.physical_newer_generation_held(key_code));
            assert!(!journal.force_old_generation_released(key_code));
        }
        drop(native_events);
        assert!(attempt_pending_recovery(&context));
        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.transactional.owned_letters(), 0);
            assert!(keyboard.physical.is_held(key_code));
            assert!(!deferred_native_ordering_drained(&keyboard));
        }

        let returned_up = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, fresh_up, context_ptr)
        };
        assert!(returned_up.is_null(), "fresh up is deferred with its down");
        {
            let journal = context.recovery_edges.lock().unwrap();
            assert_eq!(journal.pending_len, 3);
            assert_eq!(journal.pending[2].generation, 2);
            assert!(journal.hidden_balanced());
        }

        for index in 0..batch.len() {
            let record = batch.entries()[index];
            let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
                ffi::K_CG_EVENT_FLAGS_CHANGED
            } else if record.phase == PhysicalPhase::Up {
                ffi::K_CG_EVENT_KEY_UP
            } else {
                ffi::K_CG_EVENT_KEY_DOWN
            };
            let event = if index == 0 {
                first_replay_event
            } else {
                tagged_keyboard_event(
                    event_type,
                    record.native.virtual_key,
                    record.native.platform_flags,
                    replay_token.marker(),
                )
            };
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert_eq!(returned, event, "retained replay is foreground-first");
            unsafe { ffi::CFRelease(event.cast_const()) };
        }
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(deferred_native_ordering_drained(&keyboard));
            let snapshot = physical_snapshot(&keyboard);
            assert!(
                begin_transaction_control(&context, &mut keyboard, Control::Reconcile(snapshot),)
                    .applied
            );
            assert_eq!(keyboard.transactional.physical_letters(), 0);
        }

        let deferred_token = injection::OperationToken::for_test(92);
        assert!(
            context
                .recovery_edges
                .lock()
                .unwrap()
                .begin_submission(deferred_token, 0, 3)
        );
        for (event_type, event, repeat) in [
            (ffi::K_CG_EVENT_KEY_DOWN, fresh_down, false),
            (ffi::K_CG_EVENT_KEY_DOWN, fresh_repeat, true),
            (ffi::K_CG_EVENT_KEY_UP, fresh_up, false),
        ] {
            unsafe {
                ffi::CGEventSetIntegerValueField(
                    event,
                    ffi::K_CG_EVENT_SOURCE_USER_DATA,
                    deferred_token.marker(),
                );
                ffi::CGEventSetIntegerValueField(
                    event,
                    ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT,
                    i64::from(repeat),
                );
            }
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
            assert_eq!(returned, event, "deferred generation passes once");
        }
        assert!(!context.recovery_edges.lock().unwrap().has_pending());
        let duplicate = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, fresh_up, context_ptr)
        };
        assert!(
            duplicate.is_null(),
            "stale deferred generation cannot pass twice"
        );
        unsafe {
            ffi::CFRelease(fresh_up.cast_const());
            ffi::CFRelease(fresh_repeat.cast_const());
            ffi::CFRelease(fresh_down.cast_const());
            ffi::CFRelease(old_up.cast_const());
        }
    }

    #[test]
    fn failed_recovery_classifier_enforces_tombstone_repeat_up_and_fresh_down_rules() {
        let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::P.index())];
        let repeat = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        unsafe {
            ffi::CGEventSetIntegerValueField(repeat, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        let delayed_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let fresh_down = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let (context, _outbound, _commands) = test_context();
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.gap_reconciled_letters = 1_u32 << u32::from(ActivationKey::P.index());
            keyboard.gap_barrier_pending = true;
        }
        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();
        let returned_repeat = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_DOWN, repeat, context_ptr)
        };
        assert!(returned_repeat.is_null());
        assert!(context.keyboard.lock().unwrap().gap_tombstones_pending());

        let returned_up = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, delayed_up, context_ptr)
        };
        assert!(returned_up.is_null());
        assert!(!context.keyboard.lock().unwrap().gap_tombstones_pending());

        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.gap_reconciled_letters = 1_u32 << u32::from(ActivationKey::P.index());
            keyboard.gap_barrier_pending = true;
        }
        let returned_fresh = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_KEY_DOWN,
                fresh_down,
                context_ptr,
            )
        };
        assert_eq!(returned_fresh, fresh_down);
        let keyboard = context.keyboard.lock().unwrap();
        assert!(!keyboard.gap_tombstones_pending());
        assert!(!keyboard.gap_barrier_pending);
        drop(keyboard);
        drop(native_events);
        assert!(attempt_pending_recovery(&context));
        assert!(!pending_native_work(&context));
        unsafe {
            ffi::CFRelease(fresh_down.cast_const());
            ffi::CFRelease(delayed_up.cast_const());
            ffi::CFRelease(repeat.cast_const());
        }
    }

    #[test]
    fn failed_recovery_classifier_advances_barrier_and_commits_only_exact_paste_up() {
        let barrier_token = injection::OperationToken::for_test(82);
        let barrier_down =
            tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, 127, 0, barrier_token.marker());
        let barrier_up =
            tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, 127, 0, barrier_token.marker());
        let (mut barrier_context, _outbound, _commands) = test_context();
        install_event_source_identity(&mut barrier_context, barrier_down);
        barrier_context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        {
            let mut keyboard = barrier_context.keyboard.lock().unwrap();
            keyboard.gap_reconciled_escape = true;
            keyboard.gap_barrier_pending = true;
            keyboard.gap_barrier_token = Some(barrier_token);
        }
        let barrier_context_ptr = (&raw const barrier_context).cast_mut().cast();
        let native_events = barrier_context.native_events.try_lock().unwrap();
        for (event_type, event) in [
            (ffi::K_CG_EVENT_KEY_DOWN, barrier_down),
            (ffi::K_CG_EVENT_KEY_UP, barrier_up),
        ] {
            let returned =
                unsafe { event_tap_callback(null_mut(), event_type, event, barrier_context_ptr) };
            assert!(returned.is_null(), "barrier pair stays helper-owned");
        }
        assert!(!barrier_context.keyboard.lock().unwrap().gap_barrier_pending);
        drop(native_events);
        assert!(attempt_pending_recovery(&barrier_context));
        assert!(!pending_native_work(&barrier_context));

        unsafe {
            ffi::CFRelease(barrier_up.cast_const());
            ffi::CFRelease(barrier_down.cast_const());
        }
    }

    #[test]
    fn failed_recovery_classifier_passes_exact_replay_and_cleanup_once() {
        fn assert_exact_operation(batch: ExpectedReplayBatch, generation: u64) {
            let token = injection::OperationToken::for_test(generation);
            let first = batch.record(0).expect("test operation is nonempty");
            let first_type = if matches!(first.key, KeyIdentity::Modifier(_)) {
                ffi::K_CG_EVENT_FLAGS_CHANGED
            } else if first.phase == PhysicalPhase::Up {
                ffi::K_CG_EVENT_KEY_UP
            } else {
                ffi::K_CG_EVENT_KEY_DOWN
            };
            let first_event = tagged_keyboard_event(
                first_type,
                first.native.virtual_key,
                first.native.platform_flags,
                token.marker(),
            );
            let (mut context, _outbound, _commands) = test_context();
            install_event_source_identity(&mut context, first_event);
            context
                .state
                .recovery_pending
                .store(true, Ordering::Release);
            assert!(
                context
                    .keyboard
                    .lock()
                    .unwrap()
                    .begin_replay_observation(batch, token)
            );
            let context_ptr = (&raw const context).cast_mut().cast();
            let native_events = context.native_events.try_lock().unwrap();

            for index in 0..batch.len() {
                let record = batch.record(index).expect("bounded operation record");
                let event_type = if matches!(record.key, KeyIdentity::Modifier(_)) {
                    ffi::K_CG_EVENT_FLAGS_CHANGED
                } else if record.phase == PhysicalPhase::Up {
                    ffi::K_CG_EVENT_KEY_UP
                } else {
                    ffi::K_CG_EVENT_KEY_DOWN
                };
                let event = if index == 0 {
                    first_event
                } else {
                    tagged_keyboard_event(
                        event_type,
                        record.native.virtual_key,
                        record.native.platform_flags,
                        token.marker(),
                    )
                };
                let returned =
                    unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
                assert_eq!(returned, event, "exact replay/cleanup must pass");
                assert_eq!(
                    atomic_current_edge_disposition(&context),
                    CurrentEdgeDisposition::Pass
                );
                unsafe { ffi::CFRelease(event.cast_const()) };
            }
            assert!(
                context
                    .keyboard
                    .lock()
                    .unwrap()
                    .replay_observation
                    .is_none()
            );
            drop(native_events);
            assert!(attempt_pending_recovery(&context));
            assert!(!pending_native_work(&context));
        }

        let engine = engine_with_ctrl_shift_x_candidate();
        let Turn::NeedEffect {
            effect: EffectRequest::Replay(replay),
            ..
        } = engine.begin(EngineInput::Control(Control::Cancel(
            CancelReason::InvalidContinuation,
        )))
        else {
            panic!("candidate cancellation requires replay");
        };
        let cleanup = replay
            .cleanup_for_accepted(replay.len())
            .expect("full candidate prefix has bounded cleanup");
        assert!(!cleanup.is_empty());
        assert_exact_operation(ExpectedReplayBatch::Replay(replay), 84);
        assert_exact_operation(ExpectedReplayBatch::Cleanup(cleanup), 85);
    }

    #[test]
    fn overflow_keeps_only_foreground_physical_balance_and_owner_can_release() {
        let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
        let mut journal = RecoveryEdgeJournal::default();
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            let edge = injection::DeferredEvent {
                event_type: if index.is_multiple_of(2) {
                    ffi::K_CG_EVENT_KEY_DOWN
                } else {
                    ffi::K_CG_EVENT_KEY_UP
                },
                source: InputSource::Physical,
                source_pid: -1,
                ..injection::DeferredEvent::EMPTY
            };
            assert!(journal.append(edge));
        }
        let (generation, foreground_balance, hidden_generation) = journal
            .observe_key_phase(InputSource::Physical, -1, key_code, false, false, true)
            .unwrap();
        let balance = injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code,
            generation,
            source: InputSource::Physical,
            source_pid: -1,
            foreground_balance,
            hidden_generation,
            ..injection::DeferredEvent::EMPTY
        };
        assert!(!journal.append(balance));
        assert!(journal.overflow);
        assert_eq!(journal.pending_len, 1);
        assert_eq!(journal.pending[0].event_type, ffi::K_CG_EVENT_KEY_UP);
        assert!(journal.pending[0].foreground_balance);
        assert!(journal.hidden_balanced());
        assert!(journal.begin_submission(injection::OperationToken::for_test(499), 0, 1));
        assert_eq!(
            journal.advance_observation(),
            GapBarrierObservation::Complete
        );
        assert!(!journal.overflow);
        assert!(!journal.has_pending());

        let mut external = RecoveryEdgeJournal::default();
        for index in 0..injection::DEFERRED_EDGE_CAPACITY {
            assert!(external.append(injection::DeferredEvent {
                event_type: ffi::K_CG_EVENT_KEY_DOWN,
                key_code,
                is_down: true,
                source: InputSource::External,
                source_pid: 808,
                original_timestamp: index as u64,
                ..injection::DeferredEvent::EMPTY
            }));
        }
        assert!(!external.append(injection::DeferredEvent {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code,
            source: InputSource::External,
            source_pid: 808,
            original_timestamp: injection::DEFERRED_EDGE_CAPACITY as u64,
            ..injection::DeferredEvent::EMPTY
        }));
        assert_eq!(external.pending_len, injection::DEFERRED_EDGE_CAPACITY);
        assert_eq!(external.overflow_balance_len, 1);
        assert!(!external.physical_fences_pending());
        assert!(external.begin_submission(
            injection::OperationToken::for_test(500),
            0,
            injection::DEFERRED_EDGE_CAPACITY,
        ));
        for _ in 0..injection::DEFERRED_EDGE_CAPACITY {
            let _ = external.advance_observation();
        }
        assert_eq!(external.pending_len, 1);
        assert_eq!(external.pending[0].original_timestamp, 64);
    }

    #[test]
    fn physical_fences_clear_only_after_authoritative_gap_state_is_up() {
        let mut journal = RecoveryEdgeJournal::default();
        journal.discard_key_fences[0][16] = true;
        journal.discard_mouse_fences[0] = 1;
        journal.reconcile_physical_fences(|_| true, |_| true);
        assert!(journal.physical_fences_pending());
        journal.reconcile_physical_fences(|_| false, |_| false);
        assert!(!journal.physical_fences_pending());
    }

    #[test]
    fn physical_overflow_fence_retains_owner_until_matching_up() {
        let key_code = LETTER_KEY_CODES[usize::from(ActivationKey::Y.index())];
        let identity_probe = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, key_code, 0, 0);
        let source_pid = unsafe {
            ffi::CGEventGetIntegerValueField(identity_probe, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        unsafe { ffi::CFRelease(identity_probe.cast_const()) };
        let (mut context, _outbound, _commands) = test_context();
        context.injection_identity = Some(injection::InjectionIdentity::for_test(source_pid + 1));
        context.keyboard.lock().unwrap().transactional = engine_with_ctrl_shift_x_candidate();
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
        let context_ptr = (&raw const context).cast_mut().cast();

        for _ in 0..(injection::DEFERRED_EDGE_CAPACITY / 2) {
            for event_type in [ffi::K_CG_EVENT_KEY_DOWN, ffi::K_CG_EVENT_KEY_UP] {
                let event =
                    tagged_keyboard_event(event_type, key_code, 0, TEST_RECOVERY_PHYSICAL_MARKER);
                let returned =
                    unsafe { event_tap_callback(null_mut(), event_type, event, context_ptr) };
                assert!(returned.is_null());
                unsafe { ffi::CFRelease(event.cast_const()) };
            }
        }
        {
            let journal = context.recovery_edges.lock().unwrap();
            assert_eq!(journal.pending_len, injection::DEFERRED_EDGE_CAPACITY);
            assert!(journal.hidden_balanced());
            assert!(journal.token.is_none());
        }

        let overflow_down = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_DOWN,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let overflow_up = tagged_keyboard_event(
            ffi::K_CG_EVENT_KEY_UP,
            key_code,
            0,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let returned_down = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_KEY_DOWN,
                overflow_down,
                context_ptr,
            )
        };
        assert!(returned_down.is_null());
        {
            let journal = context.recovery_edges.lock().unwrap();
            assert!(!journal.overflow);
            assert_eq!(journal.pending_len, 0, "no unexposed half may be replayed");
            assert!(journal.hidden_balanced());
            assert!(journal.key_is_discard_fenced(
                InputSource::test_physical(),
                source_pid,
                key_code,
            ));
            assert!(journal.physical_fences_pending());
            assert!(journal.token.is_none());
        }
        assert_eq!(
            context.terminal.reason(),
            Some(TerminalReason::InputInjectionUnavailable)
        );

        context.keyboard.lock().unwrap().transactional = TransactionEngine::default();
        submit_deferred_edges_if_ready(&context);
        assert!(context.state.recovery_deferred_mode.load(Ordering::Acquire));
        assert!(pending_native_work(&context));

        let returned_up = unsafe {
            event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_UP, overflow_up, context_ptr)
        };
        assert!(
            returned_up.is_null(),
            "discarded down's up is fenced, not exposed"
        );
        let journal = context.recovery_edges.lock().unwrap();
        assert!(!journal.overflow);
        assert_eq!(journal.pending_len, 0);
        assert!(journal.hidden_balanced());
        assert!(journal.token.is_none());
        assert!(
            !journal.key_is_discard_fenced(InputSource::test_physical(), source_pid, key_code,)
        );
        drop(journal);
        assert!(!pending_native_work(&context));

        {
            let mut journal = context.recovery_edges.lock().unwrap();
            journal.discard_mouse_fences[1] = 1;
        }
        context
            .state
            .recovery_deferred_mode
            .store(true, Ordering::Release);
        assert!(pending_native_work(&context));
        let mouse_up = tagged_mouse_event(
            ffi::K_CG_EVENT_LEFT_MOUSE_UP,
            ffi::CGPoint { x: 40.0, y: 50.0 },
            0,
            1,
            TEST_RECOVERY_PHYSICAL_MARKER,
        );
        let returned_mouse = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_LEFT_MOUSE_UP,
                mouse_up,
                context_ptr,
            )
        };
        assert!(returned_mouse.is_null());
        assert!(!pending_native_work(&context));
        unsafe {
            ffi::CFRelease(mouse_up.cast_const());
            ffi::CFRelease(overflow_up.cast_const());
            ffi::CFRelease(overflow_down.cast_const());
        }
    }

    #[test]
    fn failed_recovery_classifier_passes_provably_unrelated_external_event() {
        let event = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_DOWN, 100, 0, 0);
        let (mut context, _outbound, _commands) = test_context();
        let event_pid = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_UNIX_PROCESS_ID)
        };
        context.injection_identity = Some(injection::InjectionIdentity::for_test(event_pid + 1));
        context
            .state
            .recovery_pending
            .store(true, Ordering::Release);
        let context_ptr = (&raw const context).cast_mut().cast();
        let native_events = context.native_events.try_lock().unwrap();
        let returned =
            unsafe { event_tap_callback(null_mut(), ffi::K_CG_EVENT_KEY_DOWN, event, context_ptr) };
        assert_eq!(returned, event);
        assert_eq!(
            atomic_current_edge_disposition(&context),
            CurrentEdgeDisposition::Pass
        );
        assert!(context.state.recovery_pending.load(Ordering::Acquire));
        drop(native_events);
        assert!(attempt_pending_recovery(&context));
        assert!(!pending_native_work(&context));
        unsafe { ffi::CFRelease(event.cast_const()) };
    }

    #[test]
    fn shutdown_deadline_selects_terminal_incomplete_instead_of_success() {
        let now = Instant::now();
        let expired = Some(now - Duration::from_millis(1));
        assert_eq!(
            shutdown_drain_action(true, expired, false, now),
            ShutdownDrainAction::ReportUnresponsive
        );
        assert_eq!(
            shutdown_drain_action(false, expired, false, now),
            ShutdownDrainAction::Stop
        );
    }

    #[test]
    fn shutdown_candidate_retains_semantic_ownership_without_posting_replay() {
        let Turn::Complete {
            engine,
            completion: Completion::Control(outcome),
        } = engine_with_ctrl_shift_x_candidate().begin(EngineInput::Control(Control::Shutdown))
        else {
            panic!("candidate shutdown must complete without an injection effect");
        };
        assert!(matches!(
            outcome.shutdown,
            ShutdownState::Draining { owned_letters } if owned_letters != 0
        ));
        assert_ne!(engine.owned_letters(), 0);
        assert_eq!(engine.journal_len(), 0);

        let (context, _outbound, _commands) = test_context();
        context.state.stopping.store(true, Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.shutdown_requested = true;
            keyboard.transactional = engine;
        }
        assert!(pending_native_work(&context));
        assert_eq!(
            shutdown_drain_action(true, None, false, Instant::now()),
            ShutdownDrainAction::Continue
        );
    }

    #[test]
    fn tombstone_suppresses_all_queued_repeats_then_the_delayed_old_up() {
        let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
        let key_code = LETTER_KEY_CODES[15];
        let mut keyboard = CallbackKeyboard {
            gap_reconciled_letters: p_bit,
            gap_barrier_pending: true,
            ..CallbackKeyboard::default()
        };

        for _ in 0..3 {
            assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
            assert!(keyboard.gap_tombstones_pending());
            assert!(keyboard.gap_barrier_pending);
            assert!(!keyboard.physical.is_held(key_code));
        }
        assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
        assert!(!keyboard.gap_tombstones_pending());
        assert!(!keyboard.gap_barrier_pending);
        assert!(!keyboard.physical.is_held(key_code));
    }

    #[test]
    fn only_nonrepeat_fresh_down_retires_tombstone_and_balances_its_fresh_up() {
        let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
        let key_code = LETTER_KEY_CODES[15];
        let mut keyboard = CallbackKeyboard {
            gap_reconciled_letters: p_bit,
            gap_barrier_pending: true,
            ..CallbackKeyboard::default()
        };
        assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
        // Stream order proves any old queued up precedes this genuine down.
        assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, false));
        assert!(!keyboard.gap_tombstones_pending());
        assert!(!keyboard.gap_barrier_pending);

        // Production processing now admits exactly this fresh pair.
        assert!(!keyboard.physical.observe(key_code, KeyPhase::Down));
        assert!(keyboard.physical.is_held(key_code));
        assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
        assert!(!keyboard.physical.observe(key_code, KeyPhase::Up));
        assert!(!keyboard.physical.is_held(key_code));
    }

    #[test]
    fn barrier_between_repeat_and_later_edge_retires_owned_suppression() {
        let p_bit = 1_u32 << u32::from(ActivationKey::P.index());
        let key_code = LETTER_KEY_CODES[15];
        let mut keyboard = CallbackKeyboard {
            gap_reconciled_letters: p_bit,
            gap_barrier_pending: true,
            ..CallbackKeyboard::default()
        };
        assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
        // The marked barrier down is swallowed but does not prove the older
        // pipeline drained; another queued repeat remains owned.
        assert!(keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
        assert!(keyboard.gap_barrier_pending);
        // Only the marked barrier up completes the proof.
        keyboard.clear_gap_tombstones();
        assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Down, true));
        assert!(!keyboard.handle_gap_tombstone(key_code, KeyPhase::Up, false));
    }

    #[test]
    fn barrier_only_state_keeps_disabled_tap_recovery_admitted() {
        let keyboard = CallbackKeyboard {
            gap_barrier_pending: true,
            ..CallbackKeyboard::default()
        };
        assert!(keyboard.strict_drain_recovery_needed());
    }

    #[test]
    fn ordered_barrier_retires_only_remaining_tombstones() {
        let mut keyboard = CallbackKeyboard {
            gap_reconciled_escape: true,
            gap_reconciled_enter_key_code: Some(KEYPAD_ENTER_KEY_CODE),
            gap_barrier_pending: true,
            ..CallbackKeyboard::default()
        };
        assert!(keyboard.gap_tombstones_pending());
        keyboard.clear_gap_tombstones();
        assert!(!keyboard.gap_tombstones_pending());
        assert!(!keyboard.gap_barrier_pending);
    }

    #[test]
    fn replay_observation_requires_exact_record_shape_flags_and_order() {
        let mut journal = talking_quill_keyboard_core::transactional::EventJournal::new();
        for (phase, flags) in [
            (PhysicalPhase::Down, 0x10_u64),
            (PhysicalPhase::Up, 0x20_u64),
        ] {
            journal
                .push(ReplayRecord {
                    key: KeyIdentity::Letter(ActivationKey::V),
                    native: NativeKey {
                        virtual_key: 9,
                        platform_flags: flags,
                        ..NativeKey::default()
                    },
                    phase,
                    observed_at_ms: 1,
                })
                .unwrap();
        }
        let mut keyboard = CallbackKeyboard::default();
        assert!(keyboard.begin_replay_observation(
            ExpectedReplayBatch::Replay(journal.replay_batch().unwrap()),
            injection::OperationToken::for_test(1),
        ));
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 9, false, 0x10),
            GapBarrierObservation::Forged
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_DOWN, 9, false, 0x10),
            GapBarrierObservation::Down
        );
        assert_eq!(
            keyboard.observe_replay_event(ffi::K_CG_EVENT_KEY_UP, 9, false, 0x20),
            GapBarrierObservation::Complete
        );
        assert!(keyboard.replay_observation.is_none());
    }

    #[test]
    fn delayed_operation_tokens_cannot_advance_replay_gap_barrier_paste_barrier_or_paste() {
        let identity = injection::InjectionIdentity::for_test(42);
        for (old_generation, current_generation) in [(1, 2), (3, 4), (5, 6), (7, 8)] {
            let old = injection::OperationToken::for_test(old_generation);
            let current = injection::OperationToken::for_test(current_generation);
            assert!(!injection::token_matches(
                identity,
                Some(current),
                old.marker(),
                identity.source_pid,
            ));
        }
    }

    #[test]
    fn tagged_pair_requires_exact_down_up_shape_and_order() {
        let mut state = 0;
        assert_eq!(
            observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, false, 0, 0,),
            GapBarrierObservation::Forged
        );
        assert_eq!(state, 0);
        assert_eq!(
            observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_DOWN, 127, 127, false, 0, 0,),
            GapBarrierObservation::Down
        );
        assert_eq!(
            observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, true, 0, 0,),
            GapBarrierObservation::Forged
        );
        assert_eq!(
            observe_exact_pair(&mut state, ffi::K_CG_EVENT_KEY_UP, 127, 127, false, 0, 0,),
            GapBarrierObservation::Complete
        );
    }

    #[test]
    fn paste_modifier_epoch_and_barrier_reject_edges_on_either_side() {
        assert!(paste_modifier_barrier_valid(Some(7), 7, true, true, true));
        assert!(!paste_modifier_barrier_valid(Some(7), 8, true, true, true));
        assert!(!paste_modifier_barrier_valid(Some(7), 7, false, true, true));
        assert!(!paste_modifier_barrier_valid(Some(7), 7, true, false, true));
        assert!(!paste_modifier_barrier_valid(Some(7), 7, true, true, false));
        assert!(!paste_modifier_barrier_valid(None, 7, true, true, true));
    }

    #[test]
    fn production_barrier_callback_rejects_focus_and_modifier_edges_after_validation() {
        fn run(invalidate_focus: bool, mutate_modifier: bool) -> (PasteResult, u64) {
            let barrier =
                injection::OperationToken::for_test(if invalidate_focus { 901 } else { 902 });
            let event = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, 127, 0, barrier.marker());
            let (mut context, _outbound, _commands) = test_context();
            install_event_source_identity(&mut context, event);
            let handle = super::super::target::target_handle_for_test(7);
            let cache = TargetCache::with_open_validation_queue_for_test();
            cache.install_current_handle_for_test(handle, 11, 13);
            let insertion_request = cache.prepare_insertion(
                handle,
                11,
                13,
                1,
                crate::platform::ClipboardTextHash::from_bytes([7; 32]),
                Instant::now() + Duration::from_secs(1),
            );
            context.target_cache = Some(cache);
            let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
            let result = Arc::new(super::super::PasteResultSlot::new());
            let (acknowledgement, _observed) = bounded(1);
            *context.pending_paste.lock().unwrap() = Some(PendingPaste {
                state,
                result: Arc::clone(&result),
                acknowledgement,
                evidence: handle,
                expected_clipboard_sha256: crate::platform::ClipboardTextHash::from_bytes([7; 32]),
                validation_request: None,
                validated_target_epoch: Some(11),
                validated_target_boundary_epoch: Some(13),
                validated_selected_range_epoch: Some(1),
                insertion_request,
                neutral_modifier_epoch: Some(1),
                neutral_barrier_state: 1,
                neutral_barrier_token: Some(barrier),
                deadline: Instant::now() + Duration::from_secs(1),
                injection_cutoff: Instant::now() + Duration::from_secs(1),
                modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
            });
            if mutate_modifier {
                let modifier_edge = tagged_keyboard_event(
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    LEFT_CONTROL_KEY_CODE,
                    ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                    TEST_RECOVERY_PHYSICAL_MARKER,
                );
                let _ = unsafe {
                    event_tap_callback(
                        null_mut(),
                        ffi::K_CG_EVENT_FLAGS_CHANGED,
                        modifier_edge,
                        (&raw mut context).cast(),
                    )
                };
                unsafe { ffi::CFRelease(modifier_edge.cast_const()) };
            }
            if invalidate_focus {
                for event_type in [ffi::K_CG_EVENT_KEY_DOWN, ffi::K_CG_EVENT_KEY_UP] {
                    let focus_edge =
                        tagged_keyboard_event(event_type, 123, 0, TEST_RECOVERY_PHYSICAL_MARKER);
                    let _ = unsafe {
                        event_tap_callback(
                            null_mut(),
                            event_type,
                            focus_edge,
                            (&raw mut context).cast(),
                        )
                    };
                    unsafe { ffi::CFRelease(focus_edge.cast_const()) };
                }
            }
            let returned = unsafe {
                event_tap_callback(
                    null_mut(),
                    ffi::K_CG_EVENT_KEY_UP,
                    event,
                    (&raw mut context).cast(),
                )
            };
            assert!(
                returned.is_null(),
                "authenticated barrier remains helper-owned"
            );
            let authoritative = result
                .result()
                .expect("barrier rejection completes request");
            unsafe { ffi::CFRelease(event.cast_const()) };
            (
                authoritative,
                context
                    .observability
                    .snapshot()
                    .native_paste
                    .target_validation_fallbacks,
            )
        }

        assert_eq!(
            run(true, false),
            (failed_paste(PasteFailure::Unavailable), 1),
            "focus/workspace epoch change before barrier up counts one target fallback"
        );
        assert_eq!(
            run(false, true),
            (failed_paste(PasteFailure::ConflictingModifiers), 0),
            "modifier edge at barrier up is not a target fallback"
        );
    }

    #[test]
    fn paste_final_gate_rejects_every_mutable_safety_transition() {
        let valid = PastePostChecks {
            admission_open: true,
            before_deadline: true,
            before_injection_cutoff: true,
            modifiers_neutral: true,
            permissions_granted: true,
            secure_input_inactive: true,
            target_valid: true,
        };
        let waiting = AtomicU8::new(PasteCommandState::Waiting as u8);
        assert_eq!(paste_check_failure(&waiting, valid), None);
        assert!(!modifier_wait_timed_out(
            PasteFailure::ConflictingModifiers,
            true
        ));
        assert!(modifier_wait_timed_out(
            PasteFailure::ConflictingModifiers,
            false
        ));
        assert!(!modifier_wait_timed_out(PasteFailure::Unavailable, false));

        for (checks, expected) in [
            (
                PastePostChecks {
                    admission_open: false,
                    ..valid
                },
                PasteFailure::Unavailable,
            ),
            (
                PastePostChecks {
                    before_deadline: false,
                    ..valid
                },
                PasteFailure::Unavailable,
            ),
            (
                PastePostChecks {
                    before_injection_cutoff: false,
                    ..valid
                },
                PasteFailure::Unavailable,
            ),
            (
                PastePostChecks {
                    modifiers_neutral: false,
                    ..valid
                },
                PasteFailure::ConflictingModifiers,
            ),
            (
                PastePostChecks {
                    permissions_granted: false,
                    ..valid
                },
                PasteFailure::PermissionDenied,
            ),
            (
                PastePostChecks {
                    secure_input_inactive: false,
                    ..valid
                },
                PasteFailure::SecureInput,
            ),
            (
                PastePostChecks {
                    target_valid: false,
                    ..valid
                },
                PasteFailure::Unavailable,
            ),
        ] {
            assert_eq!(paste_check_failure(&waiting, checks), Some(expected));
        }
    }

    #[test]
    fn cancellation_during_delayed_target_validation_prevents_late_injection() {
        let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
        let posted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (started_tx, started_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let worker_state = Arc::clone(&state);
        let worker_posted = Arc::clone(&posted);
        let validator = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let checks = PastePostChecks {
                admission_open: true,
                before_deadline: true,
                before_injection_cutoff: true,
                modifiers_neutral: true,
                permissions_granted: true,
                secure_input_inactive: true,
                target_valid: true,
            };
            if claim_paste_injection(&worker_state, checks).is_ok() {
                worker_posted.store(true, Ordering::Release);
            }
        });

        started_rx.recv().unwrap();
        assert_eq!(cancel_paste_command(&state), PasteCommandState::Cancelled);
        release_tx.send(()).unwrap();
        validator.join().unwrap();
        assert!(!posted.load(Ordering::Acquire));
        assert_eq!(paste_command_state(&state), PasteCommandState::Cancelled);
    }

    #[test]
    fn gate_or_terminal_transition_during_validation_prevents_post() {
        let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
        let admission_open = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let posted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (started_tx, started_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let worker_state = Arc::clone(&state);
        let worker_admission = Arc::clone(&admission_open);
        let worker_posted = Arc::clone(&posted);
        let validator = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let checks = PastePostChecks {
                admission_open: worker_admission.load(Ordering::Acquire),
                before_deadline: true,
                before_injection_cutoff: true,
                modifiers_neutral: true,
                permissions_granted: true,
                secure_input_inactive: true,
                target_valid: true,
            };
            if claim_paste_injection(&worker_state, checks).is_ok() {
                worker_posted.store(true, Ordering::Release);
            }
        });

        started_rx.recv().unwrap();
        admission_open.store(false, Ordering::Release);
        release_tx.send(()).unwrap();
        validator.join().unwrap();
        assert!(!posted.load(Ordering::Acquire));
        assert_eq!(paste_command_state(&state), PasteCommandState::Waiting);
    }

    #[test]
    fn pending_activation_resolves_before_a_later_physical_event() {
        let (context, outbound, _commands) = test_context();
        let compiled =
            CompiledActivationConfig::compile(ConfigRevision::new(1), true, bindings()).unwrap();
        let mut engine = TransactionEngine::new(compiled);
        let mut sides = ModifierSides::default();
        for (side, key_code) in [
            (ModifierSide::LeftCtrl, LEFT_CONTROL_KEY_CODE),
            (ModifierSide::LeftShift, LEFT_SHIFT_KEY_CODE),
        ] {
            sides.insert(side);
            let event = NormalizedEvent {
                key: KeyIdentity::Modifier(side),
                phase: PhysicalPhase::Down,
                source: InputSource::test_physical(),
                native: NativeKey {
                    virtual_key: key_code,
                    scan_code: u32::from(key_code),
                    extended: false,
                    platform_flags: 0,
                },
                observed_at_ms: 1,
                config_revision: ConfigRevision::new(1),
                gate: GateState::Open,
                snapshot: PhysicalSnapshot::new(0, sides, false),
            };
            let Turn::Complete {
                engine: next,
                completion: Completion::Event(_),
            } = engine.begin(EngineInput::Event(event))
            else {
                panic!("modifier cannot request an effect");
            };
            engine = next;
        }
        let trigger = ActivationKey::P;
        let trigger_bit = 1_u32 << u32::from(trigger.index());
        let down = NormalizedEvent {
            key: KeyIdentity::Letter(trigger),
            phase: PhysicalPhase::Down,
            source: InputSource::test_physical(),
            native: NativeKey {
                virtual_key: LETTER_KEY_CODES[usize::from(trigger.index())],
                scan_code: u32::from(LETTER_KEY_CODES[usize::from(trigger.index())]),
                extended: false,
                platform_flags: 0,
            },
            observed_at_ms: 2,
            config_revision: ConfigRevision::new(1),
            gate: GateState::Open,
            snapshot: PhysicalSnapshot::new(trigger_bit, sides, false),
        };
        let Turn::NeedEffect {
            effect: EffectRequest::DeliverActivation(notice),
            continuation,
        } = engine.begin(EngineInput::Event(down))
        else {
            panic!("exact trigger requests activation delivery");
        };
        *context.pending_activation.lock().unwrap() = Some(PendingActivation {
            continuation,
            notice,
            reservation: super::super::target::activation_reservation_for_test(7, 11),
            validation_request: super::super::target::validation_request_for_test(3, 11),
            resolved_delivery: None,
            deadline: Instant::now() + Duration::from_secs(1),
        });

        // A later source edge never enters a placeholder reducer. If the AX
        // response is not ready, the pending activation first commits
        // targetless, then the later edge is processed by the restored engine.
        assert!(resolve_pending_activation(
            &context,
            PendingActivationResolution::ForceTargetless,
        ));
        assert!(context.pending_activation.lock().unwrap().is_none());
        assert_ne!(
            context
                .keyboard
                .lock()
                .unwrap()
                .transactional
                .owned_letters(),
            0
        );
        let KeyboardEvent::Activation {
            context: activation,
            phase: EventPhase::Down,
            ..
        } = receive_event(&outbound)
        else {
            panic!("expected activation down");
        };
        assert!(activation.target_token().is_none());

        let up = NormalizedEvent {
            phase: PhysicalPhase::Up,
            snapshot: PhysicalSnapshot::new(0, sides, false),
            observed_at_ms: 3,
            ..down
        };
        let mut keyboard = context.keyboard.lock().unwrap();
        let engine = std::mem::take(&mut keyboard.transactional);
        let completion = drive_transaction_turn(
            &context,
            &mut keyboard,
            engine.begin(EngineInput::Event(up)),
            None,
        );
        assert!(matches!(
            completion,
            DriveCompletion::Complete(Completion::Event(
                talking_quill_keyboard_core::transactional::EventOutcome {
                    disposition: EventDisposition::CaptureCurrent,
                    ..
                }
            ))
        ));
        drop(keyboard);
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::Activation {
                phase: EventPhase::Up,
                ..
            }
        ));
    }

    #[test]
    fn activation_event_boundary_without_cached_evidence_is_immediately_targetless() {
        let (context, outbound, _commands) = test_context();
        let binding = bindings().iter().next().unwrap();
        let notice = ActivationNotice::Down { binding };
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(
            keyboard
                .dispatcher
                .deliver(&context.outbound, &context.terminal, notice, None,)
        );
        let KeyboardEvent::Activation {
            context: activation,
            ..
        } = receive_event(&outbound)
        else {
            panic!("expected activation");
        };
        assert_eq!(
            activation.activation_generation(),
            ActivationGeneration::FIRST
        );
        // No cache means no IPC fallback and no later target binding.
        assert!(activation.target_token().is_none());
    }

    #[test]
    fn physical_snapshots_use_hid_system_state_not_logical_session_state() {
        assert_eq!(ffi::K_CG_EVENT_SOURCE_STATE_HID_SYSTEM, 1);
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
    fn all_eight_modifier_keycodes_preserve_side_specific_identity() {
        for (key_code, side) in [
            (LEFT_CONTROL_KEY_CODE, ModifierSide::LeftCtrl),
            (RIGHT_CONTROL_KEY_CODE, ModifierSide::RightCtrl),
            (LEFT_OPTION_KEY_CODE, ModifierSide::LeftAlt),
            (RIGHT_OPTION_KEY_CODE, ModifierSide::RightAlt),
            (LEFT_SHIFT_KEY_CODE, ModifierSide::LeftShift),
            (RIGHT_SHIFT_KEY_CODE, ModifierSide::RightShift),
            (LEFT_COMMAND_KEY_CODE, ModifierSide::LeftMeta),
            (RIGHT_COMMAND_KEY_CODE, ModifierSide::RightMeta),
        ] {
            assert_eq!(modifier_side_for_key_code(key_code), Some(side));
            let mut tracker = MacModifierTracker::default();
            assert!(tracker.observe_flags_changed(key_code, true));
            assert!(tracker.sides().contains(side));
            assert!(tracker.observe_flags_changed(key_code, false));
            assert!(!tracker.sides().contains(side));
        }
        assert_eq!(modifier_side_for_key_code(57), None);
    }

    #[test]
    fn transactional_dispatcher_pairs_context_and_advances_complete_generation() {
        let (context, outbound, _commands) = test_context();
        let binding = bindings().iter().next().unwrap();
        let first_context = {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(deliver_test_activation(
                &context,
                &mut keyboard,
                ActivationNotice::Down { binding },
            ));
            let KeyboardEvent::Activation {
                context: first,
                phase: EventPhase::Down,
                ..
            } = receive_event(&outbound)
            else {
                panic!("expected activation down");
            };
            assert!(deliver_test_activation(
                &context,
                &mut keyboard,
                ActivationNotice::Up {
                    binding,
                    held_ms: 15,
                },
            ));
            first
        };
        let KeyboardEvent::Activation {
            context: up_context,
            phase: EventPhase::Up,
            ..
        } = receive_event(&outbound)
        else {
            panic!("expected activation up");
        };
        assert_eq!(up_context, first_context);
        assert_eq!(
            first_context.activation_generation(),
            ActivationGeneration::FIRST
        );

        {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(deliver_test_activation(
                &context,
                &mut keyboard,
                ActivationNotice::Complete {
                    binding,
                    held_ms: 20,
                },
            ));
        }
        let KeyboardEvent::ActivationComplete {
            context: complete,
            held_ms: 20,
            ..
        } = receive_event(&outbound)
        else {
            panic!("expected complete activation");
        };
        assert_eq!(complete.activation_generation().get(), 2);
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
            KeyboardEvent::Activation {
                phase: EventPhase::Down,
                ..
            }
        ));
    }

    #[test]
    fn ui_balancing_never_discards_native_session_up_ownership() {
        let (context, outbound, _commands) = test_context();
        context
            .state
            .session_capture_mode
            .store(SessionCaptureMode::Recording.as_u8(), Ordering::Release);
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            assert!(process_session_event(
                &context,
                &mut keyboard,
                ESCAPE_KEY_CODE,
                KeyPhase::Down,
                false,
                1,
            ));
            assert!(keyboard.session_escape_native_owned);
            deliver_balancing_events(&context, &mut keyboard.reducer);
            assert!(keyboard.session_escape_native_owned);
        }
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                phase: EventPhase::Down,
                ..
            }
        ));
        assert!(matches!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                phase: EventPhase::Up,
                ..
            }
        ));
        context.gate.close();
        let mut keyboard = context.keyboard.lock().unwrap();
        assert!(process_session_event(
            &context,
            &mut keyboard,
            ESCAPE_KEY_CODE,
            KeyPhase::Up,
            false,
            2,
        ));
        assert!(!keyboard.session_escape_native_owned);
    }

    #[test]
    fn secure_and_permission_transition_balance_only_ui_state() {
        for _path in ["secure-input", "permission"] {
            let (context, _outbound, _commands) = test_context();
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.session_escape_native_owned = true;
            keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
            keyboard.captured_enter_key_code = Some(RETURN_KEY_CODE);
            finish_native_transition(&context, &mut keyboard);
            assert!(keyboard.session_escape_native_owned);
            assert_eq!(keyboard.session_enter_native_owned, Some(RETURN_KEY_CODE));
            assert!(has_session_ownership(&keyboard));
        }
    }

    #[test]
    fn suspend_native_input_retains_native_session_ownership() {
        let (context, _outbound, commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.session_escape_native_owned = true;
            keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
        }
        let state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        commands
            .send(OwnerCommand {
                mutation: OwnerMutation {
                    kind: OwnerMutationKind::SuspendNativeInput,
                    activation: ActivationConfig::default(),
                    session_capture_mode: SessionCaptureMode::Off,
                },
                state,
                acknowledgement,
            })
            .unwrap();
        process_owner_commands(&context);
        assert!(response.recv().unwrap().is_ok());
        assert!(has_session_ownership(&context.keyboard.lock().unwrap()));
    }

    #[test]
    fn callback_panic_keeps_hidden_session_ups_owned_by_the_ordered_barrier() {
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.session_escape_native_owned = true;
            keyboard.session_enter_native_owned = Some(RETURN_KEY_CODE);
            keyboard.captured_enter_key_code = Some(RETURN_KEY_CODE);
            set_current_edge_disposition(&context, &mut keyboard, CurrentEdgeDisposition::Owned);
        }
        assert_eq!(
            recover_callback_unwind(&context),
            CurrentEdgeDisposition::Owned
        );
        let keyboard = context.keyboard.lock().unwrap();
        assert!(!has_session_ownership(&keyboard));
        assert!(keyboard.gap_tombstones_pending());
        assert!(keyboard.gap_barrier_pending);
        assert!(keyboard.strict_drain_recovery_needed());
    }

    #[test]
    fn terminal_tap_disable_recovery_preserves_session_suppression_until_barrier() {
        let (context, _outbound, _commands) = test_context();
        {
            let mut keyboard = context.keyboard.lock().unwrap();
            keyboard.session_escape_native_owned = true;
        }
        let decision = apply_tap_recovery(&context, TapRecoveryEvent::DisabledByUserInput);
        assert!(matches!(decision, TapRecoveryDecision::Terminal(_)));
        assert!(context.keyboard.lock().unwrap().session_escape_native_owned);
        keep_strict_drain_tap_enabled(&context);
        let keyboard = context.keyboard.lock().unwrap();
        assert!(keyboard.gap_reconciled_escape);
        assert!(keyboard.gap_barrier_pending);
    }

    #[test]
    fn hid_hidden_session_release_still_requires_ordered_gap_barrier_retirement() {
        let mut keyboard = CallbackKeyboard {
            session_escape_native_owned: true,
            session_enter_native_owned: Some(RETURN_KEY_CODE),
            captured_enter_key_code: Some(RETURN_KEY_CODE),
            gap_barrier_pending: true,
            gap_barrier_token: Some(injection::OperationToken::for_test(40)),
            ..CallbackKeyboard::default()
        };
        let (escape, enter, captured_enter) =
            reconcile_hidden_session_native_ownership(&mut keyboard, false, false);
        assert!(escape && enter);
        assert_eq!(captured_enter, Some(RETURN_KEY_CODE));
        keyboard.gap_reconciled_escape = true;
        keyboard.gap_reconciled_enter_key_code = captured_enter;
        assert!(keyboard.strict_drain_recovery_needed());
        assert_eq!(
            keyboard.observe_gap_barrier_event(ffi::K_CG_EVENT_KEY_DOWN, 127, false, 0,),
            GapBarrierObservation::Down
        );
        assert!(keyboard.strict_drain_recovery_needed());
        assert_eq!(
            keyboard.observe_gap_barrier_event(ffi::K_CG_EVENT_KEY_UP, 127, false, 0),
            GapBarrierObservation::Complete
        );
        assert!(!keyboard.strict_drain_recovery_needed());
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
            KeyboardEvent::SessionKey {
                key: talking_quill_keyboard_core::SessionKey::Enter,
                phase: EventPhase::Down,
            }
        );
        assert_eq!(
            receive_event(&outbound),
            KeyboardEvent::SessionKey {
                key: talking_quill_keyboard_core::SessionKey::Enter,
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
            KeyboardEvent::SessionKey {
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
            KeyboardEvent::SessionKey {
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
            KeyboardEvent::SessionKey {
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
            KeyboardEvent::SessionKey {
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
        let identity = injection::InjectionIdentity::for_test(42);
        assert!(is_synthetic_event(identity, 101, 42));
        assert!(is_synthetic_event(identity, 0, 42));
        assert!(!is_synthetic_event(identity, 0, 0));

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

        {
            let keyboard = context.keyboard.lock().unwrap();
            assert_eq!(keyboard.activation, updated);
            assert!(keyboard.transactional.config().enabled());
            assert_eq!(keyboard.transactional.config().bindings(), updated.bindings);
            assert_eq!(keyboard.transactional.config().revision().get(), 1);
        }
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
                KeyboardEvent::Activation {
                    binding: expected_binding,
                    context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: one_key,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: accepted,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: accepted,
                context: activation_context(),
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
            KeyboardEvent::Activation {
                binding: bindings().iter().nth(1).unwrap(),
                context: activation_context(),
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
