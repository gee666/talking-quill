//! Shared owner state and callback-local keyboard state.
use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ActivationConfig {
    pub(super) enabled: bool,
    pub(super) bindings: ActivationBindings,
}

pub(super) struct SharedState {
    pub(super) session_capture_mode: AtomicU8,
    pub(super) hook_status: AtomicU8,
    pub(super) protocol_initialized: AtomicBool,
    pub(super) stopping: AtomicBool,
    pub(super) post_claim_timeout: AtomicBool,
    pub(super) pending_native_work: AtomicBool,
    pub(super) target_change_evidence_ready: AtomicBool,
    pub(super) shutdown_deadline: Mutex<Option<Instant>>,
}

impl SharedState {
    pub(super) fn new() -> Self {
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

#[derive(Default)]
pub(super) struct CallbackKeyboard {
    // Retained only for independent Escape/Enter capture and compatibility
    // tests. Global activation is exclusively transactional on Windows.
    pub(super) reducer: KeyboardReducer,
    pub(super) transactional: TransactionEngine,
    pub(super) transaction_authority: Option<TransactionAuthority>,
    pub(super) deferred_callback_replay: Option<DeferredCallbackReplay>,
    pub(super) deferred_replay_raced: bool,
    pub(super) deferred_menu_releases: [Option<NativeKey>; 4],
    pub(super) deferred_observed_at_ms: u64,
    pub(super) dispatcher: ActivationDispatcher,
    pub(super) physical: WindowsPhysicalTracker,
    pub(super) modifiers: ModifierTracker,
    pub(super) activation_fenced_letters: u32,
    pub(super) activation: ActivationConfig,
    pub(super) captured_enter_source: Option<EnterSource>,
    pub(super) session_escape_native_owned: bool,
    pub(super) altgr_active: bool,
    pub(super) altgr_synthetic_ctrl: bool,
    pub(super) modifiers_fenced: bool,
    pub(super) pending_paste_cleanup: injection::PasteCleanup,
    pub(super) candidate_target: Option<CandidateTargetEvidence>,
    pub(super) candidate_desktop: Option<DesktopIdentity>,
    pub(super) candidate_target_epoch: Option<u64>,
    pub(super) candidate_target_changed: bool,
    pub(super) input_desktop: Option<DesktopIdentity>,
    pub(super) logical_v_down: bool,
    pub(super) shutdown_requested: bool,
    pub(super) external_reconcile_after: Option<Instant>,
    pub(super) external_held_letters: u32,
    pub(super) registered_observation: RegisteredObservationShadow,
}

pub(super) struct CallbackContext {
    pub(super) state: Arc<SharedState>,
    pub(super) keyboard: Mutex<CallbackKeyboard>,
    pub(super) owner_epoch: Instant,
    pub(super) suppression_enabled: bool,
    pub(super) outbound: Sender<NativeEvent>,
    pub(super) gate: Arc<CallbackGate>,
    pub(super) terminal: Arc<TerminalSignal>,
    pub(super) observability: Arc<TransactionObservability>,
    pub(super) injection_markers: injection::InjectionMarkers,
    pub(super) replay_sender: Option<Sender<ReplayWork>>,
    pub(super) replay_accepted: Arc<AtomicU64>,
}

pub(super) fn lock_keyboard_recovering(
    context: &CallbackContext,
) -> Option<MutexGuard<'_, CallbackKeyboard>> {
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
