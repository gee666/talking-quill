//! Coherent reducer reads and ordered native subset reads.
use super::*;

impl TransactionObservability {
    pub(crate) fn snapshot(&self) -> TransactionObservabilitySnapshot {
        loop {
            let before = self.publication.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let reason = |reason: CancelReason| load(&self.cancellation_reasons[reason.index()]);
            let registered_input = self.load_registered_input_after_subsets(|| {}, || {});
            let (modifier_wait_duration_ms_total, modifier_wait_duration_ms_max) =
                self.load_modifier_wait_durations_after_max(|| {});
            let snapshot = TransactionObservabilitySnapshot {
                transactions: TransactionCounters {
                    started: load(&self.started),
                    committed: load(&self.committed),
                    replayed: load(&self.replayed),
                    cancelled: load(&self.cancelled),
                    journal_high_water: load(&self.journal_high_water),
                    cancellation_reasons: CancellationReasonCounters {
                        invalid_continuation: reason(CancelReason::InvalidContinuation),
                        modifier_changed: reason(CancelReason::ModifierChanged),
                        alt_gr: reason(CancelReason::AltGr),
                        journal_overflow: reason(CancelReason::JournalOverflow),
                        configuration_replaced: reason(CancelReason::ConfigurationReplaced),
                        revision_mismatch: reason(CancelReason::RevisionMismatch),
                        gate_closed: reason(CancelReason::GateClosed),
                        shutdown: reason(CancelReason::Shutdown),
                        helper_disconnected: reason(CancelReason::HelperDisconnected),
                        secure_desktop: reason(CancelReason::SecureDesktop),
                        timeout: reason(CancelReason::Timeout),
                        activation_delivery_failed: reason(CancelReason::ActivationDeliveryFailed),
                        neutralization_failed: reason(CancelReason::NeutralizationFailed),
                        replay_failed: reason(CancelReason::ReplayFailed),
                        effect_protocol_violation: reason(CancelReason::EffectProtocolViolation),
                        physical_state_mismatch: reason(CancelReason::PhysicalStateMismatch),
                        target_changed: reason(CancelReason::TargetChanged),
                    },
                },
                replay: EffectOutcomeCounters {
                    attempted: load(&self.replay_attempted),
                    succeeded: load(&self.replay_succeeded),
                    partial: load(&self.replay_partial),
                    failed: load(&self.replay_failed),
                },
                dummy: EffectOutcomeCounters {
                    attempted: load(&self.dummy_attempted),
                    succeeded: load(&self.dummy_succeeded),
                    partial: load(&self.dummy_partial),
                    failed: load(&self.dummy_failed),
                },
                registered_input,
                native_paste: NativePasteCounters {
                    target_validation_fallbacks: load(&self.target_validation_fallbacks),
                    modifier_wait_duration_ms_total,
                    modifier_wait_duration_ms_max,
                    modifier_timeouts: load(&self.modifier_timeouts),
                    shutdown_ownership_deadlines: load(&self.shutdown_ownership_deadlines),
                },
            };
            if self.publication.load(Ordering::Acquire) == before {
                return snapshot;
            }
        }
    }

    pub(super) fn load_registered_input_after_subsets(
        &self,
        after_leaf_subsets: impl FnOnce(),
        after_physical_subset: impl FnOnce(),
    ) -> RegisteredInputCounters {
        // These are the call-order subset relations in this metric group:
        // pump <= hook, filtered <= physical <= HC_ACTION, candidate <=
        // physical, and release <= match. Shared-prefix matching can produce
        // more than one match per candidate. Channel outcomes can occur for
        // both match and release notifications, and adapter dequeue runs on a
        // different thread, so those counters are not subset pairs.
        let pump_alive = load_acquire(&self.pump_alive);
        let physical_callbacks_filtered = load_acquire(&self.physical_callbacks_filtered);
        let registered_candidate_callbacks = load_acquire(&self.registered_candidate_callbacks);
        let registered_release_callbacks = load_acquire(&self.registered_release_callbacks);
        after_leaf_subsets();

        // Physical is itself a published subset of HC_ACTION and the base for
        // filtered and candidate. Load it after both deeper subsets.
        let physical_callbacks = load_acquire(&self.physical_callbacks);
        after_physical_subset();

        RegisteredInputCounters {
            hook_installed: load(&self.hook_installed),
            pump_alive,
            hc_action_callbacks: load(&self.hc_action_callbacks),
            physical_callbacks,
            physical_callbacks_filtered,
            registered_candidate_callbacks,
            registered_match_callbacks: load(&self.registered_match_callbacks),
            registered_release_callbacks,
            callback_channel_accepted: load(&self.callback_channel_accepted),
            callback_channel_rejected: load(&self.callback_channel_rejected),
            adapter_dequeued: load(&self.adapter_dequeued),
        }
    }

    pub(super) fn load_modifier_wait_durations_after_max(
        &self,
        after_max: impl FnOnce(),
    ) -> (u64, u64) {
        // The maximum is the publication edge for this related pair. Reading
        // it first means a concurrent writer can only make the following total
        // newer. Acquire pairs with record_modifier_wait's AcqRel fetch_max,
        // so a maximum already observed always carries its contributing total.
        let maximum = self
            .modifier_wait_duration_ms_max
            .load(Ordering::Acquire)
            .min(MAX_OBSERVABILITY_COUNTER);
        after_max();
        let total = load(&self.modifier_wait_duration_ms_total);
        (total, maximum)
    }
}
