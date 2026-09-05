//! Authoritative keyboard state and poison-preserving callback locks.

use super::*;

#[derive(Clone)]
pub(super) struct InflightEffect {
    pub(super) effect: EffectRequest,
    pub(super) continuation: Continuation,
    pub(super) outcome: Option<EffectOutcome>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum CurrentEdgeDisposition {
    #[default]
    Pass,
    Owned,
    Replaced,
}

impl CurrentEdgeDisposition {
    pub(super) const fn from_u8(value: u8) -> Self {
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
pub(super) struct RecoveringMutex<T>(pub(super) Mutex<T>);

impl<T> RecoveringMutex<T> {
    pub(super) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    #[cfg(test)]
    pub(super) fn lock(&self) -> Result<MutexGuard<'_, T>, PoisonError<MutexGuard<'_, T>>> {
        match self.0.lock() {
            Ok(guard) => Ok(guard),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }

    pub(super) fn try_lock(&self) -> Result<MutexGuard<'_, T>, TryLockError<MutexGuard<'_, T>>> {
        match self.0.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
        }
    }

    pub(super) fn get_mut(&mut self) -> Result<&mut T, PoisonError<&mut T>> {
        match self.0.get_mut() {
            Ok(value) => Ok(value),
            Err(poisoned) => Ok(poisoned.into_inner()),
        }
    }

    pub(super) fn clear_poison(&self) {
        self.0.clear_poison();
    }

    #[cfg(test)]
    pub(super) fn is_poisoned(&self) -> bool {
        self.0.is_poisoned()
    }
}

pub(super) struct CallbackKeyboard {
    // Retained only for independent Escape/Enter capture and legacy adapter
    // tests. Global activation is exclusively transactional in production.
    pub(super) reducer: KeyboardReducer,
    pub(super) transactional: TransactionEngine,
    pub(super) dispatcher: ActivationDispatcher,
    pub(super) physical: MacPhysicalTracker,
    pub(super) modifiers: MacModifierTracker,
    pub(super) modifier_epoch: u64,
    pub(super) preheld_letters: PreheldLetters,
    pub(super) activation: ActivationConfig,
    pub(super) activation_revision_at: u64,
    pub(super) escape_capture_enabled_at: u64,
    pub(super) enter_capture_enabled_at: u64,
    pub(super) captured_enter_key_code: Option<u16>,
    pub(super) session_escape_native_owned: bool,
    pub(super) session_enter_native_owned: Option<u16>,
    pub(super) gap_reconciled_letters: u32,
    pub(super) gap_reconciled_escape: bool,
    pub(super) gap_reconciled_enter_key_code: Option<u16>,
    pub(super) gap_barrier_pending: bool,
    pub(super) gap_barrier_token: Option<injection::OperationToken>,
    pub(super) gap_barrier_observed_down: bool,
    pub(super) replay_observation: Option<ReplayObservation>,
    pub(super) replay_target_changed: bool,
    pub(super) replay_visible_downs: u32,
    pub(super) replay_visible_down_records:
        [Option<ReplayRecord>; talking_quill_keyboard_core::transactional::JOURNAL_CAPACITY],
    pub(super) replay_visible_down_len: usize,
    pub(super) replay_target_cleanup_pending: bool,
    pub(super) inflight_effect: Option<InflightEffect>,
    pub(super) last_native_effect_failed: bool,
    pub(super) current_edge_disposition: CurrentEdgeDisposition,
    pub(super) candidate_target_captured: bool,
    pub(super) candidate_target: Option<ActivationReservation>,
    pub(super) owned_down_records: [Option<ReplayRecord>; 26],
    pub(super) shutdown_requested: bool,
    pub(super) shutdown_deadline: Option<Instant>,
    pub(super) shutdown_deadline_reported: bool,
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
