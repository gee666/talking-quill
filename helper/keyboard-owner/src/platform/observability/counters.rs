//! Fixed aggregate-only JSON diagnostic schema.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionObservabilitySnapshot {
    pub transactions: TransactionCounters,
    pub replay: EffectOutcomeCounters,
    pub dummy: EffectOutcomeCounters,
    pub registered_input: RegisteredInputCounters,
    pub native_paste: NativePasteCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredInputCounters {
    pub hook_installed: u64,
    pub pump_alive: u64,
    pub hc_action_callbacks: u64,
    pub physical_callbacks: u64,
    pub physical_callbacks_filtered: u64,
    pub registered_candidate_callbacks: u64,
    pub registered_match_callbacks: u64,
    pub registered_release_callbacks: u64,
    pub callback_channel_accepted: u64,
    pub callback_channel_rejected: u64,
    pub adapter_dequeued: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePasteCounters {
    pub target_validation_fallbacks: u64,
    pub modifier_wait_duration_ms_total: u64,
    pub modifier_wait_duration_ms_max: u64,
    pub modifier_timeouts: u64,
    pub shutdown_ownership_deadlines: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionCounters {
    pub started: u64,
    pub committed: u64,
    pub replayed: u64,
    pub cancelled: u64,
    pub journal_high_water: u64,
    pub cancellation_reasons: CancellationReasonCounters,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectOutcomeCounters {
    pub attempted: u64,
    pub succeeded: u64,
    pub partial: u64,
    pub failed: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancellationReasonCounters {
    pub invalid_continuation: u64,
    pub modifier_changed: u64,
    pub alt_gr: u64,
    pub journal_overflow: u64,
    pub configuration_replaced: u64,
    pub revision_mismatch: u64,
    pub gate_closed: u64,
    pub shutdown: u64,
    pub helper_disconnected: u64,
    pub secure_desktop: u64,
    pub timeout: u64,
    pub activation_delivery_failed: u64,
    pub neutralization_failed: u64,
    pub replay_failed: u64,
    pub effect_protocol_violation: u64,
    pub physical_state_mismatch: u64,
    pub target_changed: u64,
}
