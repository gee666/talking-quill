//! Deliver balanced activation notices and allocate their target contexts.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::windows::hook) struct ActiveActivationContext {
    pub(in crate::platform::windows::hook) binding: ActivationBinding,
    pub(in crate::platform::windows::hook) context: ActivationContext,
}

#[derive(Debug)]
pub(in crate::platform::windows::hook) struct ActivationDispatcher {
    pub(in crate::platform::windows::hook) next_generation: Option<ActivationGeneration>,
    pub(in crate::platform::windows::hook) active: Option<ActiveActivationContext>,
    pub(in crate::platform::windows::hook) targets: TargetRegistry,
}

impl ActivationDispatcher {
    pub(in crate::platform::windows::hook) fn deliver(
        &mut self,
        outbound: &Sender<NativeEvent>,
        terminal: &TerminalSignal,
        context_observability: &TransactionObservability,
        notice: ActivationNotice,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> bool {
        match notice {
            ActivationNotice::Down { binding } => {
                context_observability.record_registered_match_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: EventPhase::Down,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    self.active = Some(ActiveActivationContext { binding, context });
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
            ActivationNotice::Up { binding, .. } => {
                context_observability.record_registered_release_callback();
                let Some(active) = self.active.take() else {
                    return false;
                };
                if active.binding != binding {
                    self.targets.remove(active.context);
                    return false;
                }
                let delivered = deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    },
                );
                if delivered {
                    context_observability.record_callback_channel_accepted();
                } else {
                    context_observability.record_callback_channel_rejected();
                }
                delivered
            }
            ActivationNotice::Complete { binding, held_ms } => {
                context_observability.record_registered_match_callback();
                context_observability.record_registered_release_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::ActivationComplete {
                    binding,
                    context,
                    held_ms,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
        }
    }

    pub(in crate::platform::windows::hook) fn take_context(
        &mut self,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> Option<ActivationContext> {
        let generation = self.next_generation?;
        self.next_generation = if generation == ActivationGeneration::MAX {
            None
        } else {
            ActivationGeneration::new(generation.get() + 1)
        };
        Some(self.targets.capture_context(
            generation,
            candidate_target.and_then(CandidateTargetEvidence::paste_evidence),
        ))
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
