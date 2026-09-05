use super::OwnerGatewayBackend;
use crate::gateway::TransactionObservabilitySnapshot;
use std::sync::atomic::Ordering;
use talking_quill_owner_protocol::schema as wire;

pub(super) fn owner_observability_from_wire(
    value: &wire::OwnerCounters,
) -> crate::gateway::OwnerObservabilitySnapshot {
    crate::gateway::OwnerObservabilitySnapshot {
        starts: value.starts.get(),
        clean_exits: value.clean_exits.get(),
        abnormal_exits: value.abnormal_exits.get(),
        singleton_collisions: value.singleton_collisions.get(),
        auth_attempts: value.auth_attempts.get(),
        auth_failures: crate::gateway::OwnerAuthFailureCounters {
            cross_user: value.auth_failures.cross_user.get(),
            wrong_session: value.auth_failures.wrong_session.get(),
            code_identity: value.auth_failures.code_identity.get(),
            mac: value.auth_failures.mac.get(),
            protocol: value.auth_failures.protocol.get(),
        },
        lease_acquired: value.lease_acquired.get(),
        lease_renewed: value.lease_renewed.get(),
        lease_expired: value.lease_expired.get(),
        lease_disconnected: value.lease_disconnected.get(),
        lease_released_neutral: value.lease_released_neutral.get(),
        lease_released_draining: value.lease_released_draining.get(),
        drain_duration_ms_total: value.drain_duration_ms_total.get(),
        drain_duration_ms_max: value.drain_duration_ms_max.get(),
        maintenance_postponed: value.maintenance_postponed.get(),
        handoff_succeeded: value.handoff_succeeded.get(),
        handoff_failed: value.handoff_failed.get(),
        degraded: value.degraded.get(),
        hook_recoveries: value.hook_recoveries.get(),
    }
}

fn observability_from_wire(value: &wire::ObservabilityResult) -> TransactionObservabilitySnapshot {
    let registered = value.registered_input.as_ref();
    TransactionObservabilitySnapshot {
        registered_input: crate::gateway::RegisteredInputCounters {
            hook_installed: registered.map_or(0, |value| value.hook_installed.get()),
            pump_alive: registered.map_or(0, |value| value.pump_alive.get()),
            hc_action_callbacks: registered.map_or(0, |value| value.hc_action_callbacks.get()),
            physical_callbacks: registered.map_or(0, |value| value.physical_callbacks.get()),
            physical_callbacks_filtered: registered
                .map_or(0, |value| value.physical_callbacks_filtered.get()),
            registered_candidate_callbacks: registered
                .map_or(0, |value| value.registered_candidate_callbacks.get()),
            registered_match_callbacks: registered
                .map_or(0, |value| value.registered_match_callbacks.get()),
            registered_release_callbacks: registered
                .map_or(0, |value| value.registered_release_callbacks.get()),
            callback_channel_accepted: registered
                .map_or(0, |value| value.callback_channel_accepted.get()),
            callback_channel_rejected: registered
                .map_or(0, |value| value.callback_channel_rejected.get()),
            adapter_dequeued: registered.map_or(0, |value| value.adapter_dequeued.get()),
            owner_admitted: registered.map_or(0, |value| value.owner_admitted.get()),
            owner_flushed: registered.map_or(0, |value| value.owner_flushed.get()),
            owner_rejected: registered.map_or(0, |value| value.owner_rejected.get()),
            gateway_received: 0,
            v10_notification_accepted: 0,
            electron_received: 0,
            observation_accepted: 0,
        },
        transactions: crate::gateway::TransactionCounters {
            started: value.transactions.started.get(),
            committed: value.transactions.committed.get(),
            replayed: value.transactions.replayed.get(),
            cancelled: value.transactions.cancelled.get(),
            journal_high_water: value.transactions.journal_high_water.get(),
            cancellation_reasons: crate::gateway::CancellationReasonCounters {
                invalid_continuation: value
                    .transactions
                    .cancellation_reasons
                    .invalid_continuation
                    .get(),
                modifier_changed: value
                    .transactions
                    .cancellation_reasons
                    .modifier_changed
                    .get(),
                alt_gr: value.transactions.cancellation_reasons.alt_gr.get(),
                journal_overflow: value
                    .transactions
                    .cancellation_reasons
                    .journal_overflow
                    .get(),
                configuration_replaced: value
                    .transactions
                    .cancellation_reasons
                    .configuration_replaced
                    .get(),
                revision_mismatch: value
                    .transactions
                    .cancellation_reasons
                    .revision_mismatch
                    .get(),
                gate_closed: value.transactions.cancellation_reasons.gate_closed.get(),
                shutdown: value.transactions.cancellation_reasons.shutdown.get(),
                helper_disconnected: value
                    .transactions
                    .cancellation_reasons
                    .helper_disconnected
                    .get(),
                secure_desktop: value.transactions.cancellation_reasons.secure_desktop.get(),
                timeout: value.transactions.cancellation_reasons.timeout.get(),
                activation_delivery_failed: value
                    .transactions
                    .cancellation_reasons
                    .activation_delivery_failed
                    .get(),
                neutralization_failed: value
                    .transactions
                    .cancellation_reasons
                    .neutralization_failed
                    .get(),
                replay_failed: value.transactions.cancellation_reasons.replay_failed.get(),
                effect_protocol_violation: value
                    .transactions
                    .cancellation_reasons
                    .effect_protocol_violation
                    .get(),
                physical_state_mismatch: value
                    .transactions
                    .cancellation_reasons
                    .physical_state_mismatch
                    .get(),
                target_changed: value.transactions.cancellation_reasons.target_changed.get(),
            },
        },
        replay: crate::gateway::EffectOutcomeCounters {
            attempted: value.replay.attempted.get(),
            succeeded: value.replay.succeeded.get(),
            partial: value.replay.partial.get(),
            failed: value.replay.failed.get(),
        },
        dummy: crate::gateway::EffectOutcomeCounters {
            attempted: value.dummy.attempted.get(),
            succeeded: value.dummy.succeeded.get(),
            partial: value.dummy.partial.get(),
            failed: value.dummy.failed.get(),
        },
        native_paste: crate::gateway::NativePasteCounters {
            target_validation_fallbacks: value.native_paste.target_validation_fallbacks.get(),
            modifier_wait_duration_ms_total: value
                .native_paste
                .modifier_wait_duration_ms_total
                .get(),
            modifier_wait_duration_ms_max: value.native_paste.modifier_wait_duration_ms_max.get(),
            modifier_timeouts: value.native_paste.modifier_timeouts.get(),
            shutdown_ownership_deadlines: value.native_paste.shutdown_ownership_deadlines.get(),
        },
    }
}

impl OwnerGatewayBackend {
    pub(super) fn native_observability(
        &self,
        value: &wire::ObservabilityResult,
    ) -> TransactionObservabilitySnapshot {
        let mut result = observability_from_wire(value);
        result.registered_input.gateway_received =
            self.event_counters.received.load(Ordering::Relaxed);
        result.registered_input.v10_notification_accepted =
            self.event_counters.v10_accepted.load(Ordering::Relaxed);
        result
    }
}
