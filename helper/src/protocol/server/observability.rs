use super::Server;
use crate::gateway::{
    EffectOutcomeCounters, GatewayBackend, MAX_OBSERVABILITY_COUNTER, PasteFailure, PasteResult,
    TransactionCounters,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeyboardCaptureCounters {
    runtime_rollback_active: bool,
    development_disabled: bool,
    activation_enable_requests_blocked: u64,
    session_capture_requests_blocked: u64,
    shutdown_ownership_deadlines: u64,
    terminal_disablements: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasteFailureCounters {
    permission_denied: u64,
    secure_input: u64,
    conflicting_modifiers: u64,
    os_rejected: u64,
    unavailable: u64,
    indeterminate: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PasteCounters {
    attempted: u64,
    submitted: u64,
    target_validation_fallback: u64,
    native_wait_duration_ms_total: u64,
    native_wait_duration_ms_max: u64,
    modifier_timeouts: u64,
    failures: PasteFailureCounters,
}

impl PasteCounters {
    pub(super) fn record(&mut self, result: PasteResult) {
        increment_counter(&mut self.attempted);
        if result.submitted {
            increment_counter(&mut self.submitted);
            return;
        }
        match result.reason.unwrap_or(PasteFailure::Unavailable) {
            PasteFailure::PermissionDenied => {
                increment_counter(&mut self.failures.permission_denied)
            }
            PasteFailure::SecureInput => increment_counter(&mut self.failures.secure_input),
            PasteFailure::ConflictingModifiers => {
                increment_counter(&mut self.failures.conflicting_modifiers)
            }
            PasteFailure::OsRejected => increment_counter(&mut self.failures.os_rejected),
            PasteFailure::Unavailable => increment_counter(&mut self.failures.unavailable),
            PasteFailure::Indeterminate => increment_counter(&mut self.failures.indeterminate),
        }
    }

    fn merge_native(
        &mut self,
        target_fallbacks: u64,
        modifier_wait_duration_ms_total: u64,
        modifier_wait_duration_ms_max: u64,
        modifier_timeouts: u64,
    ) {
        self.target_validation_fallback = self
            .target_validation_fallback
            .saturating_add(target_fallbacks)
            .min(MAX_OBSERVABILITY_COUNTER);
        self.native_wait_duration_ms_total = self
            .native_wait_duration_ms_total
            .saturating_add(modifier_wait_duration_ms_total)
            .min(MAX_OBSERVABILITY_COUNTER);
        self.native_wait_duration_ms_max = self
            .native_wait_duration_ms_max
            .max(modifier_wait_duration_ms_max);
        self.modifier_timeouts = self
            .modifier_timeouts
            .saturating_add(modifier_timeouts)
            .min(MAX_OBSERVABILITY_COUNTER);
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeObservabilityResult {
    keyboard_capture: KeyboardCaptureCounters,
    keyboard_owner: crate::gateway::KeyboardOwnerSnapshot,
    owner: crate::gateway::OwnerObservabilitySnapshot,
    registered_input: crate::gateway::RegisteredInputCounters,
    transactions: TransactionCounters,
    replay: EffectOutcomeCounters,
    dummy: EffectOutcomeCounters,
    paste: PasteCounters,
}

pub(super) fn increment_counter(counter: &mut u64) {
    *counter = counter.saturating_add(1).min(MAX_OBSERVABILITY_COUNTER);
}

impl<P: GatewayBackend> Server<P> {
    pub(super) fn runtime_observability_result(&self) -> RuntimeObservabilityResult {
        let owner_snapshot = self.platform.runtime_owner_observability();
        let native = owner_snapshot.native;
        let mut paste = self.paste_counters;
        paste.merge_native(
            native.native_paste.target_validation_fallbacks,
            native.native_paste.modifier_wait_duration_ms_total,
            native.native_paste.modifier_wait_duration_ms_max,
            native.native_paste.modifier_timeouts,
        );
        RuntimeObservabilityResult {
            keyboard_owner: self.platform.keyboard_owner(),
            owner: owner_snapshot.owner,
            registered_input: native.registered_input,
            keyboard_capture: KeyboardCaptureCounters {
                runtime_rollback_active: self.activation_capture_gate.runtime_rollback_active(),
                development_disabled: self.activation_capture_gate.development_disabled(),
                activation_enable_requests_blocked: self.activation_enable_requests_blocked,
                session_capture_requests_blocked: self.session_capture_requests_blocked,
                shutdown_ownership_deadlines: native.native_paste.shutdown_ownership_deadlines,
                terminal_disablements: u64::from(self.terminal.is_triggered()),
            },
            transactions: native.transactions,
            replay: native.replay,
            dummy: native.dummy,
            paste,
        }
    }

    /// Collects one fixed aggregate after native teardown and every admitted
    /// callback delivery is quiescent. If native owner completion could not be
    /// proved, no final snapshot is returned because its counters could still
    /// change on a detached thread.
    #[doc(hidden)]
    pub fn take_terminal_observability(&mut self) -> Option<Value> {
        self.quiesce_platform();
        if self.terminal_observability_collected || !self.terminal_observability_authoritative {
            return None;
        }
        self.terminal_observability_collected = true;
        serde_json::to_value(self.runtime_observability_result()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_wait_uses_native_platform_measurements_and_saturates() {
        let mut counters = PasteCounters::default();
        counters.record(PasteResult {
            submitted: true,
            reason: None,
        });
        counters.record(PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        });
        counters.merge_native(0, 35, 25, 0);
        assert_eq!(counters.attempted, 2);
        assert_eq!(counters.submitted, 1);
        assert_eq!(counters.native_wait_duration_ms_total, 35);
        assert_eq!(counters.native_wait_duration_ms_max, 25);

        counters.merge_native(0, MAX_OBSERVABILITY_COUNTER, MAX_OBSERVABILITY_COUNTER, 0);
        assert_eq!(
            counters.native_wait_duration_ms_total,
            MAX_OBSERVABILITY_COUNTER
        );
        assert_eq!(
            counters.native_wait_duration_ms_max,
            MAX_OBSERVABILITY_COUNTER
        );
    }
}
