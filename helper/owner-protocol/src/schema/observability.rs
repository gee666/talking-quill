//! Privacy-safe bounded aggregate response schemas.
use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AuthFailures {
    pub cross_user: Counter,
    pub wrong_session: Counter,
    pub code_identity: Counter,
    pub mac: Counter,
    pub protocol: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OwnerCounters {
    pub starts: Counter,
    pub clean_exits: Counter,
    pub abnormal_exits: Counter,
    pub singleton_collisions: Counter,
    pub auth_attempts: Counter,
    pub auth_failures: AuthFailures,
    pub lease_acquired: Counter,
    pub lease_renewed: Counter,
    pub lease_expired: Counter,
    pub lease_disconnected: Counter,
    pub lease_released_neutral: Counter,
    pub lease_released_draining: Counter,
    pub drain_duration_ms_total: Counter,
    pub drain_duration_ms_max: Counter,
    pub maintenance_postponed: Counter,
    pub handoff_succeeded: Counter,
    pub handoff_failed: Counter,
    pub degraded: Counter,
    pub hook_recoveries: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CancellationReasons {
    pub invalid_continuation: Counter,
    pub modifier_changed: Counter,
    pub alt_gr: Counter,
    pub journal_overflow: Counter,
    pub configuration_replaced: Counter,
    pub revision_mismatch: Counter,
    pub gate_closed: Counter,
    pub shutdown: Counter,
    pub helper_disconnected: Counter,
    pub secure_desktop: Counter,
    pub timeout: Counter,
    pub activation_delivery_failed: Counter,
    pub neutralization_failed: Counter,
    pub replay_failed: Counter,
    pub effect_protocol_violation: Counter,
    pub physical_state_mismatch: Counter,
    pub target_changed: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TransactionCounters {
    pub started: Counter,
    pub committed: Counter,
    pub replayed: Counter,
    pub cancelled: Counter,
    pub journal_high_water: Counter,
    pub cancellation_reasons: CancellationReasons,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectCounters {
    pub attempted: Counter,
    pub succeeded: Counter,
    pub partial: Counter,
    pub failed: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RegisteredInputCounters {
    pub hook_installed: Counter,
    pub pump_alive: Counter,
    pub hc_action_callbacks: Counter,
    pub physical_callbacks: Counter,
    pub physical_callbacks_filtered: Counter,
    pub registered_candidate_callbacks: Counter,
    pub registered_match_callbacks: Counter,
    pub registered_release_callbacks: Counter,
    pub callback_channel_accepted: Counter,
    pub callback_channel_rejected: Counter,
    pub adapter_dequeued: Counter,
    pub owner_admitted: Counter,
    pub owner_flushed: Counter,
    pub owner_rejected: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NativePasteCounters {
    pub target_validation_fallbacks: Counter,
    pub modifier_wait_duration_ms_total: Counter,
    pub modifier_wait_duration_ms_max: Counter,
    pub modifier_timeouts: Counter,
    pub shutdown_ownership_deadlines: Counter,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityResult {
    pub owner: OwnerCounters,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "registeredInput"
    )]
    pub registered_input: Option<RegisteredInputCounters>,
    pub transactions: TransactionCounters,
    pub replay: EffectCounters,
    pub dummy: EffectCounters,
    #[serde(rename = "nativePaste")]
    pub native_paste: NativePasteCounters,
}
